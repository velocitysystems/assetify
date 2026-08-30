//! The engine: assetify's own [`Provider`] implementation.
//!
//! Per requested asset, single-flighted per id:
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
use crate::source::fetch::{Fetcher, HashingSink};
use crate::source::policy::{Admission, FetchPolicy};
use crate::source::{ArchiveFormat, AssetSource, Locator, Payload, Resolver, local};
use crate::store::{Store, layout};

/// Key of one acquisition flight: one asset id.
type Slot = String;

/// The engine: a cache root, an optional resolver, and the
/// [`Provider`] implementation over them.
///
/// Construct with [`Assetify::builder`]. Without a resolver the
/// engine runs in **cache-only mode**: it serves whatever the root
/// already holds (which may be read-only — assets bundled into a
/// deployment are served in place).
pub struct Assetify {
   store: Store,
   resolver: Option<Box<dyn Resolver>>,
   /// Retrieves `Locator::Url` bytes. Explicitly supplied via the
   /// builder, defaulted to reqwest under the `reqwest` feature, or
   /// absent (URL sources report unavailable).
   fetcher: Option<Box<dyn Fetcher>>,
   /// The host's "may I fetch right now?" hook. Absent, every
   /// acquisition is admitted.
   policy: Option<Box<dyn FetchPolicy>>,
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
         fetcher: None,
         policy: None,
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

      let slot = request.id.clone();
      if let Some(rejected) = &request.rejected {
         self.poison_last_served(&slot, &rejected.reason);
      }

      // Coalesce concurrent acquisition of the same slot; serving
      // afterwards is cheap and runs per request.
      let flight = self.flight(&slot);
      let revision = {
         let _leader = flight.lock().await;
         self.ensure_revision(&request.id).await?
      };

      // Recover from a poisoned lock: the maps hold plain
      // bookkeeping, valid regardless of a panicking peer.
      self
         .last_served
         .lock()
         .unwrap_or_else(PoisonError::into_inner)
         .insert(slot, revision.clone());

      let revision_dir = self.store.revision_dir(&request.id, &revision);
      let asset = self.serve(request, &revision_dir)?;
      tracing::info!(
         asset = %request.id,
         revision = %revision,
         files = asset.files.len(),
         "delivered"
      );
      Ok(asset)
   }

   /// The revision this request will be served from — the resolver's
   /// choice when present (fetching it if need be), else the newest
   /// serviceable revision on disk.
   async fn ensure_revision(&self, id: &str) -> Result<String, String> {
      let Some(resolver) = &self.resolver else {
         return self
            .store
            .newest_revision(id)
            .ok_or_else(|| format!("cache-only mode, and {id:?} holds nothing servable"));
      };

      if let Some(policy) = &self.policy
         && let Admission::Deny { reason } = policy.admit(id).await
      {
         // Before resolution on purpose: a denied request does no
         // resolver or network work, serves silently from cache, and
         // surfaces only when nothing is on disk.
         return self.fallback(id, &format!("acquisition declined: {reason}"));
      }

      let source = match resolver.resolve(id).await {
         Ok(Some(source)) => source,
         Ok(None) => {
            return self.fallback(id, "the resolver knows no source for this asset");
         }
         Err(e) => return self.fallback(id, &format!("resolution failed: {e}")),
      };

      if let Err(reason) = layout::validate_revision(&source.revision) {
         return self.fallback(id, &reason);
      }
      if self.store.has_revision(id, &source.revision) {
         tracing::debug!(
            asset = %id,
            revision = %source.revision,
            "cache hit"
         );
         return Ok(source.revision);
      }
      if self.store.revision_dir(id, &source.revision).exists() {
         // Present but poisoned: the same revision would carry the
         // same bytes, so re-fetching it cannot help.
         return self.fallback(
            id,
            &format!(
               "revision {:?} was rejected by a previous load",
               source.revision
            ),
         );
      }

      match self.acquire(id, &source).await {
         Ok(()) => Ok(source.revision),
         Err(reason) => self.fallback(id, &format!("acquisition failed: {reason}")),
      }
   }

   /// Fetch every file of `source` into staging, verify every digest,
   /// and place the set atomically. All-or-nothing: any failure
   /// leaves the cache untouched.
   async fn acquire(&self, id: &str, source: &AssetSource) -> Result<(), String> {
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

         // Archive bytes land in a temp file *beside* the staged
         // revision: verified there, extracted in, never placed.
         let archive_temp = match &file.payload {
            Payload::Archive(_) => Some(
               self
                  .store
                  .stage_file()
                  .map_err(|e| format!("cannot create an archive staging file: {e}"))?,
            ),
            Payload::File => None,
         };
         let destination = match &archive_temp {
            Some(temp) => temp.path().to_path_buf(),
            None => staged.path().join(&file.name),
         };

         let computed = match &file.locator {
            Locator::File(path) => local::copy(path, &destination)
               .await
               .map_err(|e| format!("cannot acquire {:?} from {path:?}: {e}", file.name))?,
            Locator::Url(url) => self.fetch_url(url, &file.name, &destination).await?,
         };
         if !file.digest.matches_sha256(&computed) {
            return Err(format!("digest mismatch for {:?}", file.name));
         }

         if let Payload::Archive(format) = &file.payload {
            self
               .extract(format, &destination, staged.path())
               .await
               .map_err(|e| format!("cannot extract {:?}: {e}", file.name))?;
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
         .place_revision(staged, id, &source.revision)
         .map(|_| ()) // AlreadyPresent: a racing writer won; same result.
         .map_err(|e| format!("cannot place revision {:?}: {e}", source.revision))?;
      tracing::info!(
         asset = %id,
         revision = %source.revision,
         "placed"
      );
      Ok(())
   }

   /// Extract a verified archive into the staged revision directory.
   /// Format support is feature-gated; without the matching feature
   /// the acquisition degrades like any other failure.
   #[cfg(feature = "zip")]
   async fn extract(
      &self,
      format: &ArchiveFormat,
      archive: &Path,
      destination: &Path,
   ) -> Result<(), String> {
      match format {
         ArchiveFormat::Zip => crate::source::archive::extract_zip(archive, destination).await,
      }
   }

   #[cfg(not(feature = "zip"))]
   async fn extract(
      &self,
      format: &ArchiveFormat,
      _archive: &Path,
      _destination: &Path,
   ) -> Result<(), String> {
      let _ = format;
      Err("archive payloads need the `zip` feature".to_string())
   }

   /// Retrieve one URL through the configured [`Fetcher`], hashing
   /// the bytes as they land in staging — verification stays here,
   /// on the engine's side of the seam.
   async fn fetch_url(
      &self,
      url: &str,
      name: &str,
      destination: &Path,
   ) -> Result<[u8; 32], String> {
      let Some(fetcher) = &self.fetcher else {
         return Err(format!(
            "cannot acquire {name:?} from {url:?}: no fetcher is configured — enable the \
             `reqwest` feature, or supply one with Assetify::builder(..).fetcher(..)"
         ));
      };
      let file = std::fs::File::create(destination)
         .map_err(|e| format!("cannot create staging file: {e}"))?;
      let mut sink = HashingSink::new(file);
      fetcher
         .fetch(url, &mut sink)
         .await
         .map_err(|e| format!("cannot acquire {name:?}: {e}"))?;
      sink
         .finish()
         .map_err(|e| format!("cannot flush staging file: {e}"))
   }

   /// The newest serviceable on-disk revision, or the reason there is
   /// nothing to serve.
   fn fallback(&self, id: &str, reason: &str) -> Result<String, String> {
      match self.store.newest_revision(id) {
         Some(revision) => {
            tracing::warn!(
               asset = %id,
               revision = %revision,
               %reason,
               "serving newest on-disk revision"
            );
            Ok(revision)
         }
         None => Err(format!("{reason}; nothing servable on disk for {id:?}")),
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

   fn poison_last_served(&self, slot: &str, reason: &str) {
      let revision = self
         .last_served
         .lock()
         .unwrap_or_else(PoisonError::into_inner)
         .get(slot)
         .cloned()
         .or_else(|| self.store.newest_revision(slot));
      if let Some(revision) = revision {
         tracing::warn!(
               asset = %slot,
            revision = %revision,
            %reason,
            "poisoned rejected revision"
         );
         self.store.poison_revision(slot, &revision, reason);
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
   resolver: Option<Box<dyn Resolver>>,
   fetcher: Option<Box<dyn Fetcher>>,
   policy: Option<Box<dyn FetchPolicy>>,
}

impl AssetifyBuilder {
   /// Where acquired bytes come from. Omit for cache-only mode, which
   /// serves what the root already holds and permits a read-only
   /// root.
   pub fn resolver(mut self, resolver: impl Resolver + 'static) -> Self {
      self.resolver = Some(Box::new(resolver));
      self
   }

   /// How `Locator::Url` bytes are retrieved. Omit to use the
   /// built-in reqwest fetcher (`reqwest` feature). Supply your own to
   /// configure the client (user agent, proxies, auth) or to use a
   /// different one entirely — verification always stays with the
   /// engine.
   pub fn fetcher(mut self, fetcher: impl Fetcher + 'static) -> Self {
      self.fetcher = Some(Box::new(fetcher));
      self
   }

   /// The host's "may I fetch right now?" hook (offline mode, metered
   /// connections). Consulted once per requested asset, before
   /// resolution; a denial serves the newest on-disk revision, so it
   /// usually succeeds silently from cache. Omit to admit every
   /// acquisition.
   pub fn fetch_policy(mut self, policy: impl FetchPolicy + 'static) -> Self {
      self.policy = Some(Box::new(policy));
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
      #[cfg(feature = "reqwest")]
      let fetcher = match self.fetcher {
         Some(fetcher) => Some(fetcher),
         None => Some(Box::new(crate::source::reqwest::ReqwestFetcher::new(
            reqwest::Client::builder()
               .build()
               .map_err(|source| AssetifyError::DefaultFetcher { source })?,
         )) as Box<dyn Fetcher>),
      };
      #[cfg(not(feature = "reqwest"))]
      let fetcher = self.fetcher;

      Ok(Assetify {
         store: Store::new(self.root),
         resolver: self.resolver,
         fetcher,
         policy: self.policy,
         flights: Mutex::new(HashMap::new()),
         last_served: Mutex::new(HashMap::new()),
      })
   }
}
