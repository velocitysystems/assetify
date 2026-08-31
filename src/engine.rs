//! The engine: assetify's own [`Provider`] implementation.
//!
//! Per requested asset, single-flighted per id:
//!
//! 1. **Validate** the id and file names — they become filesystem
//!    paths, so traversal and reserved shapes are rejected before any
//!    path is built.
//! 2. **Ensure a revision**: ask the resolver where bytes live; serve
//!    the named revision from cache when present; otherwise fetch
//!    every file into staging (hashing as it streams), verify every
//!    digest, and atomically place the whole set. On any resolution
//!    or acquisition failure, fall back to the newest serviceable
//!    revision already on disk.
//! 3. **Serve**: locate each requested file by unique name and open
//!    each requested file for reading.
//!
//! A delivery the consumer could not load is reported back through
//! [`Assetify::reject`], which poisons that revision so it is never
//! re-served.
//!
//! A missing asset is a degraded capability, never an error: every
//! failure lands as [`AssetResponse::Unavailable`] with a reason, and
//! the next request retries.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use crate::contract::delivery::{AssetResponse, DeliveryReceipt, PreparedAsset, PreparedFile};
use crate::contract::provider::Provider;
use crate::contract::request::AssetRequest;
use crate::error::AssetifyError;
use crate::source::fetch::{ChannelSink, Fetcher, HashingSink};
use crate::source::policy::{Admission, FetchPolicy};
use crate::source::{ArchiveFormat, AssetSource, Locator, Payload, Resolver, local};
use crate::store::{Store, layout};

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
   flights: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
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

   /// Reject a delivery the consumer could not load (a named gap, an
   /// unreadable payload format, failed content validation), poisoning
   /// the revision it was served from immediately: re-acquire, don't
   /// re-serve. Call it the moment the load fails — the next request
   /// for the asset recovers via another revision.
   ///
   /// `receipt` comes from the rejected delivery
   /// ([`PreparedAsset::receipt`]), which is what makes the target
   /// precise: even with several concurrent deliveries of one asset,
   /// the rejection poisons the revision it came from, never "whatever
   /// was served most recently". A receipt naming no revision (a
   /// provider without a versioned cache) poisons nothing. An invalid
   /// `id` is logged and ignored — rejection, like poisoning itself,
   /// is best-effort and never fails the caller.
   pub fn reject(&self, id: &str, receipt: DeliveryReceipt, reason: impl Into<String>) {
      if let Err(invalid) = layout::validate_id(id) {
         tracing::warn!(asset = %id, reason = %invalid, "rejection ignored: invalid id");
         return;
      }
      let Some(revision) = &receipt.revision else {
         return;
      };
      let reason = reason.into();
      tracing::warn!(
         asset = %id,
         revision = %revision,
         reason = %reason,
         "poisoned rejected revision"
      );
      self.store.poison_revision(id, revision, &reason);
   }

   async fn prepare_one(&self, request: &AssetRequest) -> AssetResponse {
      match self.try_prepare(request).await {
         Ok(asset) => AssetResponse::Available { asset },
         Err(reason) => AssetResponse::Unavailable { reason },
      }
   }

   async fn try_prepare(&self, request: &AssetRequest) -> Result<PreparedAsset, String> {
      layout::validate_id(&request.id)?;
      for name in &request.files {
         layout::validate_file_name(name)?;
      }

      let slot = request.id.clone();

      // Coalesce concurrent acquisition of the same slot; serving
      // afterwards is cheap and runs per request.
      let flight = self.flight(&slot);
      let ensured = {
         let _leader = flight.lock().await;
         self.ensure_revision(&request.id).await
      };
      self.prune_flight(&slot, &flight);
      let revision = ensured?;

      let revision_dir = self.store.revision_dir(&request.id, &revision);
      let asset = self
         .serve(request, &revision_dir)?
         .with_receipt(DeliveryReceipt::for_revision(&revision));
      tracing::info!(
         asset = %request.id,
         revision = %revision,
         files = asset.files().len(),
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
         // resolver or network work.
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
      let revision_dir = self.store.revision_dir(id, &source.revision);
      if revision_dir.exists() {
         // Present yet unserviceable, two ways: poisoned (re-fetching
         // the same bytes cannot help), or a foreign non-directory
         // entry squatting on the revision's path.
         let reason = if revision_dir.is_dir() {
            format!(
               "revision {:?} was rejected by a previous load",
               source.revision
            )
         } else {
            format!(
               "a foreign entry occupies the path of revision {:?}",
               source.revision
            )
         };
         return self.fallback(id, &reason);
      }

      if let Some(newest) = self.store.newest_revision(id)
         && source.revision < newest
      {
         // Lexicographic order is the freshness axis, so acquiring a
         // revision that sorts below one already on disk is either a
         // deliberate rollback or a revision-naming bug (an unpadded
         // counter: "10" sorts below "9"). Selection is unchanged —
         // this only makes the naming bug diagnosable.
         tracing::warn!(
            asset = %id,
            resolved = %source.revision,
            newest_on_disk = %newest,
            "acquiring a revision that sorts below the newest on disk"
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

      for (index, file) in source.files.iter().enumerate() {
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
            // Each archive extracts into its own `_archive_N` subdir,
            // so its entries can't overwrite a sibling's verified file.
            // The `_` prefix can't collide with a delivered name (those
            // start alphanumeric) yet isn't hidden, so `find_file`
            // still reaches the files; a real collision surfaces as an
            // ambiguity, not a silent overwrite.
            let archive_dir = staged.path().join(format!("_archive_{index}"));
            std::fs::create_dir(&archive_dir)
               .map_err(|e| format!("cannot create an extraction directory: {e}"))?;
            extract(format, &destination, &archive_dir)
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
         .await
         .map(|_| ()) // AlreadyPresent: a racing writer won; same result.
         .map_err(|e| format!("cannot place revision {:?}: {e}", source.revision))?;
      tracing::info!(
         asset = %id,
         revision = %source.revision,
         "placed"
      );
      Ok(())
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

      // The fetcher owns the transfer; verify the landed file by
      // re-reading it — one extra pass, on the blocking pool.
      if fetcher.writes_to_path() {
         fetcher
            .fetch_to_path(url, destination)
            .await
            .map_err(|e| format!("cannot acquire {name:?}: {e}"))?;
         let path = destination.to_path_buf();
         return tokio::task::spawn_blocking(move || crate::source::fetch::hash_file(&path))
            .await
            .map_err(|e| format!("staging hash task failed: {e}"))?
            .map_err(|e| format!("cannot hash staging file: {e}"));
      }

      let file = std::fs::File::create(destination)
         .map_err(|e| format!("cannot create staging file: {e}"))?;

      // Body chunks are written and hashed on the blocking pool; the
      // fetcher only feeds them over a bounded channel, so neither the
      // write syscalls nor the SHA-256 ever run on the async runtime.
      let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(8);
      let writer = tokio::task::spawn_blocking(move || -> std::io::Result<[u8; 32]> {
         let mut sink = HashingSink::new(std::io::BufWriter::with_capacity(256 * 1024, file));
         while let Ok(chunk) = rx.recv() {
            sink.write_all(&chunk)?;
         }
         sink.finish()
      });

      let mut sink = ChannelSink::new(tx);
      let fetched = fetcher.fetch(url, &mut sink).await;
      drop(sink); // close the channel so the writer loop ends

      // The writer's error is the root cause (a disk failure also
      // trips the fetcher's sink writes), so report it first.
      let digest = match writer
         .await
         .map_err(|e| format!("staging writer task failed: {e}"))?
      {
         Ok(digest) => digest,
         Err(e) => return Err(format!("cannot write staging file: {e}")),
      };
      fetched.map_err(|e| format!("cannot acquire {name:?}: {e}"))?;
      Ok(digest)
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
      for name in &request.files {
         let path = self.store.find_file(revision_dir, name)?;
         files.push(PreparedFile::from_path(name.clone(), path));
      }
      Ok(PreparedAsset::new(files))
   }

   fn flight(&self, slot: &str) -> Arc<tokio::sync::Mutex<()>> {
      Arc::clone(
         self
            .flights
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(slot.to_string())
            .or_default(),
      )
   }

   /// Drop a completed flight's map entry when no other request
   /// holds it — the map otherwise grows by one entry per distinct
   /// id, for the life of the engine.
   fn prune_flight(&self, slot: &str, flight: &Arc<tokio::sync::Mutex<()>>) {
      let mut flights = self.flights.lock().unwrap_or_else(PoisonError::into_inner);
      // Exactly two strong refs — the map's and ours — means no
      // concurrent request shares this flight, and holding the map
      // lock means none can appear before the entry is gone. A later
      // request simply creates a fresh one.
      if let Some(entry) = flights.get(slot)
         && Arc::ptr_eq(entry, flight)
         && Arc::strong_count(flight) == 2
      {
         flights.remove(slot);
      }
   }
}

/// Extract a verified archive into `destination`. Format support is
/// feature-gated; without the matching feature the acquisition
/// degrades like any other failure.
#[cfg(feature = "zip")]
async fn extract(format: &ArchiveFormat, archive: &Path, destination: &Path) -> Result<(), String> {
   match format {
      ArchiveFormat::Zip => crate::source::archive::extract_zip(archive, destination).await,
   }
}

#[cfg(not(feature = "zip"))]
async fn extract(
   format: &ArchiveFormat,
   _archive: &Path,
   _destination: &Path,
) -> Result<(), String> {
   let _ = format;
   Err("archive payloads need the `zip` feature".to_string())
}

#[async_trait::async_trait]
impl Provider for Assetify {
   async fn provide(&self, requests: &[AssetRequest]) -> Vec<AssetResponse> {
      // Concurrent, order-preserving: distinct assets acquire in
      // parallel, while the per-id single-flight still coalesces
      // duplicates onto one acquisition.
      futures_util::future::join_all(requests.iter().map(|request| self.prepare_one(request))).await
   }
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
            // A connect deadline and a between-bytes read deadline, so
            // a stalled or dribbling download fails rather than
            // holding the asset's acquisition open forever. No total
            // deadline: a large asset may legitimately take minutes.
            reqwest::Client::builder()
               .connect_timeout(std::time::Duration::from_secs(30))
               .read_timeout(std::time::Duration::from_secs(30))
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
      })
   }
}

#[cfg(test)]
mod tests {
   use super::*;

   #[tokio::test]
   async fn completed_flights_are_pruned() {
      let cache = tempfile::tempdir().unwrap();
      let revision = cache.path().join("tokenizer/en/20260821");
      std::fs::create_dir_all(&revision).unwrap();
      std::fs::write(revision.join("meta.json"), b"{}").unwrap();

      let engine = Assetify::builder(cache.path()).build().unwrap();
      for _ in 0..2 {
         let outcome = engine
            .asset(AssetRequest::new("tokenizer/en", ["meta.json"]))
            .await;
         assert!(matches!(outcome, AssetResponse::Available { .. }));
      }

      let flights = engine
         .flights
         .lock()
         .unwrap_or_else(PoisonError::into_inner);
      assert!(flights.is_empty(), "completed flights must not accumulate");
   }
}
