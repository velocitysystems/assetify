//! The engine: assetify's own [`Provider`] implementation.
//!
//! Per requested asset, single-flighted per `(id, format_major)`:
//!
//! 1. **Validate** the id and file names — they become filesystem
//!    paths, so traversal and reserved shapes are rejected before any
//!    path is built.
//! 2. **Poison** the previously served revision when the request
//!    carries a rejection echo.
//! 3. **Ensure a revision**: ask the resolver where bytes live; serve
//!    the named revision from cache when present; otherwise fetch
//!    every file into staging (hashing as it streams), verify every
//!    digest, and atomically place the whole set. On any resolution
//!    or acquisition failure, fall back to the newest serviceable
//!    revision already on disk.
//! 4. **Serve**: locate each requested file by unique name and open
//!    the backing its declared [`AccessKind`] deserves.
//!
//! A missing asset is a degraded capability, never an error: every
//! failure lands as [`AssetResponse::Unavailable`] with a reason, and
//! the next request retries.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

#[cfg(not(feature = "mmap"))]
use crate::access::FileRandom;
#[cfg(feature = "mmap")]
use crate::access::MmapRandom;
use crate::contract::access::{AccessKind, AssetPath, FileAccess};
use crate::contract::delivery::{AssetResponse, PreparedAsset, PreparedFile};
use crate::contract::provider::Provider;
use crate::contract::request::AssetRequest;
use crate::error::AssetifyError;
use crate::source::{AssetSource, Locator, SourceResolver, local};
use crate::store::{Store, layout};

/// Key of one acquisition flight: an asset within a lane.
type Slot = (String, u32);

/// The engine: a cache root, an optional resolver, and the
/// [`Provider`] implementation over them.
///
/// Construct with [`Assetify::builder`]. Without a resolver the
/// engine runs in **cache-only mode**: it serves whatever the root
/// already holds (which may be read-only — assets bundled into a
/// deployment are served in place).
pub struct Assetify {
   store: Store,
   resolver: Option<Box<dyn SourceResolver>>,
   #[cfg(feature = "http")]
   http_client: reqwest::Client,
   /// One async mutex per slot: concurrent requests for the same
   /// asset coalesce instead of racing the acquisition. Followers
   /// re-check the cache after the leader finishes and hit it.
   flights: Mutex<HashMap<Slot, Arc<tokio::sync::Mutex<()>>>>,
   /// The revision each slot last served, so a rejection echo poisons
   /// the right directory.
   last_served: Mutex<HashMap<Slot, String>>,
}

impl Assetify {
   /// Start building an engine over a cache root.
   pub fn builder(cache_root: impl Into<PathBuf>) -> AssetifyBuilder {
      AssetifyBuilder {
         root: cache_root.into(),
         resolver: None,
      }
   }

   /// Single-asset convenience over [`Provider::provide`].
   pub async fn asset(&self, request: AssetRequest) -> AssetResponse {
      self
         .provide(std::slice::from_ref(&request))
         .await
         .into_iter()
         .next()
         .expect("provide returns one outcome per request")
   }

   async fn prepare_one(&self, request: &AssetRequest) -> AssetResponse {
      match self.try_prepare(request).await {
         Ok(asset) => AssetResponse::Available { asset },
         Err(reason) => AssetResponse::Unavailable { reason },
      }
   }

   async fn try_prepare(&self, request: &AssetRequest) -> Result<PreparedAsset, String> {
      layout::validate_id(&request.id)?;
      for spec in &request.files {
         layout::validate_file_name(&spec.name)?;
      }

      let slot = (request.id.clone(), request.format_major);
      if let Some(rejected) = &request.rejected {
         self.poison_last_served(&slot, &rejected.reason);
      }

      // Coalesce concurrent acquisition of the same slot; serving
      // afterwards is cheap and runs per request.
      let flight = self.flight(&slot);
      let revision = {
         let _leader = flight.lock().await;
         self
            .ensure_revision(&request.id, request.format_major)
            .await?
      };

      // Recover from a poisoned lock: the maps hold plain
      // bookkeeping, valid regardless of a panicking peer.
      self
         .last_served
         .lock()
         .unwrap_or_else(PoisonError::into_inner)
         .insert(slot, revision.clone());

      let revision_dir = self
         .store
         .revision_dir(&request.id, request.format_major, &revision);
      let asset = self.serve(request, &revision_dir)?;
      tracing::info!(
         asset = %request.id,
         lane = request.format_major,
         revision = %revision,
         files = asset.files.len(),
         "delivered"
      );
      Ok(asset)
   }

   /// The revision this request will be served from — the resolver's
   /// choice when present (fetching it if need be), else the newest
   /// serviceable revision on disk.
   async fn ensure_revision(&self, id: &str, format_major: u32) -> Result<String, String> {
      let Some(resolver) = &self.resolver else {
         return self.store.newest_revision(id, format_major).ok_or_else(|| {
            format!("cache-only mode, and lane v{format_major} of {id:?} holds nothing servable")
         });
      };

      let source = match resolver.resolve(id, format_major).await {
         Ok(Some(source)) => source,
         Ok(None) => {
            return self.fallback(
               id,
               format_major,
               "the resolver knows no source for this asset",
            );
         }
         Err(e) => return self.fallback(id, format_major, &format!("resolution failed: {e}")),
      };

      if let Err(reason) = layout::validate_revision(&source.revision) {
         return self.fallback(id, format_major, &reason);
      }
      if self.store.has_revision(id, format_major, &source.revision) {
         tracing::debug!(
            asset = %id,
            lane = format_major,
            revision = %source.revision,
            "cache hit"
         );
         return Ok(source.revision);
      }
      if self
         .store
         .revision_dir(id, format_major, &source.revision)
         .exists()
      {
         // Present but poisoned: the same revision would carry the
         // same bytes, so re-fetching it cannot help.
         return self.fallback(
            id,
            format_major,
            &format!(
               "revision {:?} was rejected by a previous load",
               source.revision
            ),
         );
      }

      match self.acquire(id, format_major, &source).await {
         Ok(()) => Ok(source.revision),
         Err(reason) => self.fallback(id, format_major, &format!("acquisition failed: {reason}")),
      }
   }

   /// Fetch every file of `source` into staging, verify every digest,
   /// and place the set atomically. All-or-nothing: any failure
   /// leaves the cache untouched.
   async fn acquire(
      &self,
      id: &str,
      format_major: u32,
      source: &AssetSource,
   ) -> Result<(), String> {
      if source.files.is_empty() {
         return Err(format!(
            "source for revision {:?} lists no files",
            source.revision
         ));
      }

      let staged = self
         .store
         .stage()
         .map_err(|e| format!("cannot create a staging directory: {e}"))?;

      for file in &source.files {
         layout::validate_file_name(&file.name)?;
         let destination = staged.path().join(&file.name);
         let computed = match &file.locator {
            Locator::File { path } => local::fetch(path, &destination)
               .await
               .map_err(|e| format!("cannot acquire {:?} from {path:?}: {e}", file.name))?,
            Locator::HTTP { url } => self.fetch_http(url, &file.name, &destination).await?,
         };
         if !file.digest.matches_sha256(&computed) {
            return Err(format!("digest mismatch for {:?}", file.name));
         }
         tracing::info!(
         asset = %id,
            revision = %source.revision,
            file = %file.name,
            "staged"
         );
      }

      self
         .store
         .place_revision(staged, id, format_major, &source.revision)
         .map(|_| ()) // AlreadyPresent: a racing writer won; same result.
         .map_err(|e| format!("cannot place revision {:?}: {e}", source.revision))?;
      tracing::info!(
         asset = %id,
         lane = format_major,
         revision = %source.revision,
         "placed"
      );
      Ok(())
   }

   #[cfg(feature = "http")]
   async fn fetch_http(
      &self,
      url: &str,
      name: &str,
      destination: &Path,
   ) -> Result<[u8; 32], String> {
      crate::source::http::fetch(&self.http_client, url, destination)
         .await
         .map_err(|e| format!("cannot acquire {name:?}: {e}"))
   }

   #[cfg(not(feature = "http"))]
   async fn fetch_http(&self, url: &str, name: &str, _: &Path) -> Result<[u8; 32], String> {
      Err(format!(
         "cannot acquire {name:?} from {url:?}: assetify was built without the `http` feature"
      ))
   }

   /// The newest serviceable on-disk revision, or the reason there is
   /// nothing to serve.
   fn fallback(&self, id: &str, format_major: u32, reason: &str) -> Result<String, String> {
      match self.store.newest_revision(id, format_major) {
         Some(revision) => {
            tracing::warn!(
               asset = %id,
               lane = format_major,
               revision = %revision,
               %reason,
               "serving newest on-disk revision"
            );
            Ok(revision)
         }
         None => Err(format!(
            "{reason}; nothing servable on disk in lane v{format_major} of {id:?}"
         )),
      }
   }

   /// Open every requested file of a served revision behind the
   /// backing its declared kind deserves.
   fn serve(&self, request: &AssetRequest, revision_dir: &Path) -> Result<PreparedAsset, String> {
      let mut files = Vec::with_capacity(request.files.len());
      for spec in &request.files {
         let path = self.store.find_file(revision_dir, &spec.name)?;
         let access = open_backing(&path, spec.access)
            .map_err(|e| format!("cannot open {:?}: {e}", spec.name))?;
         files.push(PreparedFile::new(spec.name.clone(), access));
      }
      Ok(PreparedAsset::new(files))
   }

   fn flight(&self, slot: &Slot) -> Arc<tokio::sync::Mutex<()>> {
      Arc::clone(
         self
            .flights
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(slot.clone())
            .or_default(),
      )
   }

   fn poison_last_served(&self, slot: &Slot, reason: &str) {
      let revision = self
         .last_served
         .lock()
         .unwrap_or_else(PoisonError::into_inner)
         .get(slot)
         .cloned()
         .or_else(|| self.store.newest_revision(&slot.0, slot.1));
      if let Some(revision) = revision {
         tracing::warn!(
               asset = %slot.0,
            lane = slot.1,
            revision = %revision,
            %reason,
            "poisoned rejected revision"
         );
         self
            .store
            .poison_revision(&slot.0, slot.1, &revision, reason);
      }
   }
}

#[async_trait::async_trait]
impl Provider for Assetify {
   async fn provide(&self, requests: &[AssetRequest]) -> Vec<AssetResponse> {
      let mut outcomes = Vec::with_capacity(requests.len());
      for request in requests {
         outcomes.push(self.prepare_one(request).await);
      }
      outcomes
   }
}

/// The backing each access kind deserves, over a served file.
fn open_backing(path: &Path, kind: AccessKind) -> std::io::Result<FileAccess> {
   Ok(match kind {
      AccessKind::Stream => FileAccess::Stream(Box::new(std::fs::File::open(path)?)),
      #[cfg(feature = "mmap")]
      AccessKind::Random => FileAccess::Random(Box::new(MmapRandom::open(path)?)),
      #[cfg(not(feature = "mmap"))]
      AccessKind::Random => FileAccess::Random(Box::new(FileRandom::open(path)?)),
      AccessKind::AssetPath => FileAccess::AssetPath(AssetPath::new(path)),
   })
}

/// Builds an [`Assetify`].
pub struct AssetifyBuilder {
   root: PathBuf,
   resolver: Option<Box<dyn SourceResolver>>,
}

impl AssetifyBuilder {
   /// Where acquired bytes come from. Omit for cache-only mode, which
   /// serves what the root already holds and permits a read-only
   /// root.
   pub fn resolver(mut self, resolver: impl SourceResolver + 'static) -> Self {
      self.resolver = Some(Box::new(resolver));
      self
   }

   /// Build the engine. With a resolver configured, the cache root
   /// and its staging area are created here — acquisition needs a
   /// writable root, and failing at build beats failing on the first
   /// request.
   pub fn build(self) -> Result<Assetify, AssetifyError> {
      if self.resolver.is_some() {
         crate::store::place::ensure_staging(&self.root).map_err(|source| {
            AssetifyError::CacheRoot {
               root: self.root.clone(),
               source,
            }
         })?;
      }
      Ok(Assetify {
         store: Store::new(self.root),
         resolver: self.resolver,
         #[cfg(feature = "http")]
         http_client: reqwest::Client::builder()
            .build()
            .map_err(|source| AssetifyError::HTTPClient { source })?,
         flights: Mutex::new(HashMap::new()),
         last_served: Mutex::new(HashMap::new()),
      })
   }
}
