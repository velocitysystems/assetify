//! End-to-end provision from local sources: resolve → acquire →
//! verify → place → serve, plus every degraded path — offline
//! fallback, poison, validation, and all-or-nothing staging.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use assetify::{
   AssetRequest, AssetResponse, AssetSource, Assetify, Digest, FileSource, Locator, Provider,
   ResolveError, Resolver, StaticResolver,
};
use sha2::Digest as _;

fn sha256_of(bytes: &[u8]) -> Digest {
   Digest::sha256_hex(&hex::encode(sha2::Sha256::digest(bytes))).unwrap()
}

/// Write a "remote" file and describe it as a source.
fn file_source(dir: &Path, name: &str, bytes: &[u8]) -> FileSource {
   let path = dir.join(name);
   std::fs::write(&path, bytes).unwrap();
   FileSource::new(name, Locator::File(path), sha256_of(bytes))
}

fn tokenizer_request() -> AssetRequest {
   AssetRequest::new("tokenizer/en", vec!["meta.json", "index.dat", "rules.txt"])
}

fn tokenizer_source(remote: &Path, revision: &str) -> AssetSource {
   AssetSource::new(
      revision,
      vec![
         file_source(remote, "meta.json", br#"{"format":4}"#),
         file_source(remote, "index.dat", b"positioned bytes"),
         file_source(remote, "rules.txt", b"rule one"),
      ],
   )
}

fn unwrap_available(outcome: AssetResponse) -> assetify::PreparedAsset {
   match outcome {
      AssetResponse::Available { asset } => asset,
      AssetResponse::Unavailable { reason } => panic!("expected availability, got: {reason}"),
   }
}

fn unwrap_unavailable(outcome: AssetResponse) -> String {
   match outcome {
      AssetResponse::Unavailable { reason } => reason,
      AssetResponse::Available { .. } => panic!("expected unavailability"),
   }
}

#[tokio::test]
async fn acquires_verifies_places_and_serves_every_access_kind() {
   let remote = tempfile::tempdir().unwrap();
   let cache = tempfile::tempdir().unwrap();

   let engine = Assetify::builder(cache.path())
      .resolver(StaticResolver::new([(
         "tokenizer/en",
         tokenizer_source(remote.path(), "20260821"),
      )]))
      .build()
      .unwrap();

   let asset = unwrap_available(engine.asset(tokenizer_request()).await);

   // Read each delivered file in a different shape.
   let mut drained = String::new();
   asset
      .file("meta.json")
      .unwrap()
      .stream()
      .unwrap()
      .read_to_string(&mut drained)
      .unwrap();
   assert_eq!(drained, r#"{"format":4}"#);

   let random = asset.file("index.dat").unwrap().random().unwrap();
   let mut word = [0u8; 5];
   random.read_at_exact(11, &mut word).unwrap();
   assert_eq!(&word, b"bytes");

   let path = asset.file("rules.txt").unwrap().path().unwrap();
   assert_eq!(std::fs::read(path).unwrap(), b"rule one");

   // The cache now holds the placed revision.
   assert!(
      cache
         .path()
         .join("tokenizer/en/20260821/meta.json")
         .is_file()
   );
}

#[tokio::test]
async fn cache_only_mode_serves_a_preseeded_root() {
   let cache = tempfile::tempdir().unwrap();
   let revision = cache.path().join("tokenizer/en/20260812");
   std::fs::create_dir_all(&revision).unwrap();
   for (name, bytes) in [
      ("meta.json", br#"{"format":4}"#.as_slice()),
      ("index.dat", b"positioned bytes"),
      ("rules.txt", b"rule one"),
   ] {
      std::fs::write(revision.join(name), bytes).unwrap();
   }

   let engine = Assetify::builder(cache.path()).build().unwrap();
   unwrap_available(engine.asset(tokenizer_request()).await);

   let missing = unwrap_unavailable(
      engine
         .asset(AssetRequest::new("models/sentiment", Vec::<&str>::new()))
         .await,
   );
   assert!(missing.contains("cache-only"), "{missing}");
}

/// A resolver that fails every time — the offline case.
struct OfflineResolver;

#[async_trait::async_trait]
impl Resolver for OfflineResolver {
   async fn resolve(&self, _: &str) -> Result<Option<AssetSource>, ResolveError> {
      Err(ResolveError::new("network unreachable"))
   }
}

#[tokio::test]
async fn resolution_failure_falls_back_to_the_newest_on_disk_revision() {
   let remote = tempfile::tempdir().unwrap();
   let cache = tempfile::tempdir().unwrap();

   // First run online: populate the cache.
   let online = Assetify::builder(cache.path())
      .resolver(StaticResolver::new([(
         "tokenizer/en",
         tokenizer_source(remote.path(), "20260812"),
      )]))
      .build()
      .unwrap();
   unwrap_available(online.asset(tokenizer_request()).await);

   // Second run offline: the cached revision still serves.
   let offline = Assetify::builder(cache.path())
      .resolver(OfflineResolver)
      .build()
      .unwrap();
   unwrap_available(offline.asset(tokenizer_request()).await);

   // Offline with an empty asset: unavailable, and the reason carries
   // both the failure and the nothing-servable fact.
   let reason = unwrap_unavailable(
      offline
         .asset(AssetRequest::new("models/sentiment", vec!["model.bin"]))
         .await,
   );
   assert!(reason.contains("resolution failed"), "{reason}");
   assert!(reason.contains("nothing servable"), "{reason}");
}

#[tokio::test]
async fn a_foreign_entry_at_a_revision_path_is_reported_as_such() {
   let remote = tempfile::tempdir().unwrap();
   let cache = tempfile::tempdir().unwrap();

   // A plain file squats where the resolved revision's directory
   // belongs — junk in the cache, not a rejected load.
   std::fs::create_dir_all(cache.path().join("tokenizer/en")).unwrap();
   std::fs::write(cache.path().join("tokenizer/en/20260821"), b"junk").unwrap();

   let engine = Assetify::builder(cache.path())
      .resolver(StaticResolver::new([(
         "tokenizer/en",
         tokenizer_source(remote.path(), "20260821"),
      )]))
      .build()
      .unwrap();

   let reason = unwrap_unavailable(engine.asset(tokenizer_request()).await);
   assert!(reason.contains("foreign entry"), "{reason}");
   assert!(
      !reason.contains("rejected by a previous load"),
      "junk must not read as a rejection: {reason}"
   );
}

#[tokio::test]
async fn a_rejection_poisons_the_served_revision_until_a_newer_one_exists() {
   let remote = tempfile::tempdir().unwrap();
   let cache = tempfile::tempdir().unwrap();

   let engine = Assetify::builder(cache.path())
      .resolver(StaticResolver::new([(
         "tokenizer/en",
         tokenizer_source(remote.path(), "20260812"),
      )]))
      .build()
      .unwrap();
   let delivered = unwrap_available(engine.asset(tokenizer_request()).await);

   // The consumer could not load the delivery; rejecting its receipt
   // poisons that revision, and the resolver still names it — so
   // nothing serves.
   engine.reject(
      "tokenizer/en",
      delivered.receipt(),
      "payload failed content validation",
   );
   let reason = unwrap_unavailable(engine.asset(tokenizer_request()).await);
   assert!(reason.contains("rejected by a previous load"), "{reason}");

   // Poison survives a process restart (a fresh engine, same root).
   let restarted = Assetify::builder(cache.path())
      .resolver(StaticResolver::new([(
         "tokenizer/en",
         tokenizer_source(remote.path(), "20260812"),
      )]))
      .build()
      .unwrap();
   unwrap_unavailable(restarted.asset(tokenizer_request()).await);

   // A newer revision from the resolver recovers the asset.
   let recovered = Assetify::builder(cache.path())
      .resolver(StaticResolver::new([(
         "tokenizer/en",
         tokenizer_source(remote.path(), "20260821"),
      )]))
      .build()
      .unwrap();
   unwrap_available(recovered.asset(tokenizer_request()).await);
}

#[tokio::test]
async fn a_rejection_poisons_its_own_revision_not_the_newest() {
   let remote = tempfile::tempdir().unwrap();
   let cache = tempfile::tempdir().unwrap();

   // Consumer A is delivered the older revision and keeps its receipt.
   let old = Assetify::builder(cache.path())
      .resolver(StaticResolver::new([(
         "tokenizer/en",
         tokenizer_source(remote.path(), "20260812"),
      )]))
      .build()
      .unwrap();
   let delivered_a = unwrap_available(old.asset(tokenizer_request()).await);

   // Meanwhile the resolver rolls forward: a newer, good revision is
   // fetched, placed, and served to someone else.
   let new = Assetify::builder(cache.path())
      .resolver(StaticResolver::new([(
         "tokenizer/en",
         tokenizer_source(remote.path(), "20260821"),
      )]))
      .build()
      .unwrap();
   unwrap_available(new.asset(tokenizer_request()).await);

   // A now rejects the delivery it actually held (the older one). A
   // guessing design would have poisoned whatever was served most
   // recently — the good new revision. The receipt pins the target.
   new.reject(
      "tokenizer/en",
      delivered_a.receipt(),
      "payload failed content validation",
   );
   unwrap_available(new.asset(tokenizer_request()).await);

   // The newer revision is untouched and still serves; only the older
   // one carries a poison marker.
   assert!(
      !cache
         .path()
         .join("tokenizer/en/20260821/.poisoned")
         .exists()
   );
   assert!(
      cache
         .path()
         .join("tokenizer/en/20260812/.poisoned")
         .exists()
   );
}

#[tokio::test]
async fn a_direct_rejection_poisons_immediately_and_falls_back() {
   let remote = tempfile::tempdir().unwrap();
   let cache = tempfile::tempdir().unwrap();

   // Seed an older revision, then serve the newer one.
   let old = Assetify::builder(cache.path())
      .resolver(StaticResolver::new([(
         "tokenizer/en",
         tokenizer_source(remote.path(), "20260812"),
      )]))
      .build()
      .unwrap();
   unwrap_available(old.asset(tokenizer_request()).await);

   let engine = Assetify::builder(cache.path())
      .resolver(StaticResolver::new([(
         "tokenizer/en",
         tokenizer_source(remote.path(), "20260821"),
      )]))
      .build()
      .unwrap();
   let delivered = unwrap_available(engine.asset(tokenizer_request()).await);

   // The load failed; rejecting directly poisons without waiting for
   // a next request.
   engine.reject(
      "tokenizer/en",
      delivered.receipt(),
      "payload failed content validation",
   );
   assert!(
      cache
         .path()
         .join("tokenizer/en/20260821/.poisoned")
         .exists(),
      "a direct rejection poisons before any further request"
   );

   // Re-requesting falls back to the older, unpoisoned revision.
   let again = unwrap_available(engine.asset(tokenizer_request()).await);
   let path = again.file("rules.txt").unwrap().path().unwrap().to_owned();
   assert!(
      path.to_string_lossy().contains("20260812"),
      "expected the older revision, got {path:?}"
   );
}

/// Counts resolutions, then delegates to a static map.
struct CountingResolver {
   calls: Arc<AtomicUsize>,
   inner: StaticResolver,
}

#[async_trait::async_trait]
impl Resolver for CountingResolver {
   async fn resolve(&self, id: &str) -> Result<Option<AssetSource>, ResolveError> {
      self.calls.fetch_add(1, Ordering::SeqCst);
      self.inner.resolve(id).await
   }
}

/// A policy switched by a flag — the offline-mode shape.
struct OfflineSwitch {
   offline: bool,
}

#[async_trait::async_trait]
impl assetify::FetchPolicy for OfflineSwitch {
   async fn admit(&self, _: &str) -> assetify::Admission {
      if self.offline {
         assetify::Admission::Deny {
            reason: "offline mode is on".to_string(),
         }
      } else {
         assetify::Admission::Allow
      }
   }
}

#[tokio::test]
async fn a_denied_acquisition_serves_the_cache_without_resolving() {
   let remote = tempfile::tempdir().unwrap();
   let cache = tempfile::tempdir().unwrap();
   let calls = Arc::new(AtomicUsize::new(0));
   let resolver = |calls: &Arc<AtomicUsize>| CountingResolver {
      calls: Arc::clone(calls),
      inner: StaticResolver::new([("tokenizer/en", tokenizer_source(remote.path(), "20260821"))]),
   };

   // Warm the cache with acquisition admitted.
   let online = Assetify::builder(cache.path())
      .resolver(resolver(&calls))
      .fetch_policy(OfflineSwitch { offline: false })
      .build()
      .unwrap();
   unwrap_available(online.asset(tokenizer_request()).await);
   assert_eq!(calls.load(Ordering::SeqCst), 1);

   // Denied: the cached revision serves silently, and the resolver is
   // never consulted.
   let offline = Assetify::builder(cache.path())
      .resolver(resolver(&calls))
      .fetch_policy(OfflineSwitch { offline: true })
      .build()
      .unwrap();
   unwrap_available(offline.asset(tokenizer_request()).await);
   assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_denied_acquisition_with_an_empty_cache_reports_the_reason() {
   let remote = tempfile::tempdir().unwrap();
   let cache = tempfile::tempdir().unwrap();

   let engine = Assetify::builder(cache.path())
      .resolver(StaticResolver::new([(
         "tokenizer/en",
         tokenizer_source(remote.path(), "20260821"),
      )]))
      .fetch_policy(OfflineSwitch { offline: true })
      .build()
      .unwrap();

   let reason = unwrap_unavailable(engine.asset(tokenizer_request()).await);
   assert!(reason.contains("acquisition declined"), "{reason}");
   assert!(reason.contains("offline mode is on"), "{reason}");
}

#[tokio::test]
async fn one_provide_acquires_distinct_assets_concurrently_in_request_order() {
   let remote = tempfile::tempdir().unwrap();
   let cache = tempfile::tempdir().unwrap();
   let calls = Arc::new(AtomicUsize::new(0));

   let engine = Assetify::builder(cache.path())
      .resolver(CountingResolver {
         calls: Arc::clone(&calls),
         inner: StaticResolver::new([
            ("tokenizer/en", tokenizer_source(remote.path(), "20260821")),
            (
               "models/sentiment",
               AssetSource::new(
                  "20260821",
                  vec![file_source(remote.path(), "labels.txt", b"pos neg")],
               ),
            ),
         ]),
      })
      .build()
      .unwrap();

   // Two distinct assets plus a duplicate id, in one call.
   let outcomes = engine
      .provide(&[
         tokenizer_request(),
         AssetRequest::new("models/sentiment", vec!["labels.txt"]),
         tokenizer_request(),
      ])
      .await;

   // Order-preserving: each outcome matches its request's files.
   assert_eq!(outcomes.len(), 3);
   let mut outcomes = outcomes.into_iter();
   let first = unwrap_available(outcomes.next().unwrap());
   assert!(first.file("index.dat").is_some());
   let second = unwrap_available(outcomes.next().unwrap());
   assert!(second.file("labels.txt").is_some());
   let third = unwrap_available(outcomes.next().unwrap());
   assert!(third.file("index.dat").is_some());

   // The duplicate id coalesced onto one acquisition: its second
   // request either waited on the same flight or hit the cache, and
   // only one copy of each revision was placed.
   assert!(cache.path().join("tokenizer/en/20260821").is_dir());
   assert!(cache.path().join("models/sentiment/20260821").is_dir());
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_requests_for_one_asset_all_succeed() {
   let remote = tempfile::tempdir().unwrap();
   let cache = tempfile::tempdir().unwrap();
   let calls = Arc::new(AtomicUsize::new(0));

   let engine = Arc::new(
      Assetify::builder(cache.path())
         .resolver(CountingResolver {
            calls: Arc::clone(&calls),
            inner: StaticResolver::new([(
               "tokenizer/en",
               tokenizer_source(remote.path(), "20260821"),
            )]),
         })
         .build()
         .unwrap(),
   );

   let handles: Vec<_> = (0..8)
      .map(|_| {
         let engine = Arc::clone(&engine);
         tokio::spawn(async move { engine.asset(tokenizer_request()).await })
      })
      .collect();
   for handle in handles {
      unwrap_available(handle.await.unwrap());
   }
   assert!(calls.load(Ordering::SeqCst) >= 1);
}

#[tokio::test]
async fn materialized_paths_stay_valid_after_the_delivery_is_dropped() {
   let remote = tempfile::tempdir().unwrap();
   let cache = tempfile::tempdir().unwrap();

   let engine = Assetify::builder(cache.path())
      .resolver(StaticResolver::new([(
         "tokenizer/en",
         tokenizer_source(remote.path(), "20260821"),
      )]))
      .build()
      .unwrap();

   // The owned path outlives the delivery `asset`, which is dropped
   // at the end of this statement.
   let path: PathBuf = unwrap_available(engine.asset(tokenizer_request()).await)
      .file("rules.txt")
      .unwrap()
      .path()
      .unwrap()
      .to_path_buf();
   assert_eq!(
      std::fs::read(&path).unwrap(),
      b"rule one",
      "nothing deletes placed revisions in v1, so the path holds"
   );
}

#[tokio::test]
async fn duplicate_file_names_in_a_revision_are_a_delivery_error() {
   let cache = tempfile::tempdir().unwrap();
   let revision = cache.path().join("dicts/spellcheck-de/r1");
   std::fs::create_dir_all(revision.join("a")).unwrap();
   std::fs::create_dir_all(revision.join("b")).unwrap();
   std::fs::write(revision.join("a/words.dat"), b"one").unwrap();
   std::fs::write(revision.join("b/words.dat"), b"two").unwrap();

   let engine = Assetify::builder(cache.path()).build().unwrap();
   let reason = unwrap_unavailable(
      engine
         .asset(AssetRequest::new("dicts/spellcheck-de", vec!["words.dat"]))
         .await,
   );
   assert!(reason.contains("ambiguous"), "{reason}");
}

#[tokio::test]
async fn acquisition_is_all_or_nothing() {
   let remote = tempfile::tempdir().unwrap();
   let cache = tempfile::tempdir().unwrap();

   // Two files; the second's digest is a lie.
   let good = file_source(remote.path(), "meta.json", b"{}");
   let mut bad = file_source(remote.path(), "index.dat", b"real bytes");
   bad.digest = sha256_of(b"other bytes");

   let engine = Assetify::builder(cache.path())
      .resolver(StaticResolver::new([(
         "tokenizer/en",
         AssetSource::new("20260821", vec![good, bad]),
      )]))
      .build()
      .unwrap();

   let reason = unwrap_unavailable(
      engine
         .asset(AssetRequest::new("tokenizer/en", vec!["meta.json"]))
         .await,
   );
   assert!(reason.contains("digest mismatch"), "{reason}");
   assert!(reason.contains("index.dat"), "{reason}");

   // Nothing was placed — not even the file that verified — and
   // staging holds no leftovers.
   assert!(!cache.path().join("tokenizer/en").exists());
   let staging = cache.path().join(".staging");
   assert_eq!(std::fs::read_dir(staging).unwrap().count(), 0);
}

#[cfg(not(feature = "reqwest"))]
#[tokio::test]
async fn http_sources_explain_the_missing_feature() {
   let cache = tempfile::tempdir().unwrap();
   let engine = Assetify::builder(cache.path())
      .resolver(StaticResolver::new([(
         "models/sentiment",
         AssetSource::new(
            "r1",
            vec![FileSource::new(
               "model.bin",
               Locator::Url("https://example.invalid/model.bin".to_string()),
               sha256_of(b"weights"),
            )],
         ),
      )]))
      .build()
      .unwrap();

   let reason = unwrap_unavailable(
      engine
         .asset(AssetRequest::new("models/sentiment", vec!["model.bin"]))
         .await,
   );
   assert!(reason.contains("`reqwest` feature"), "{reason}");
}

#[cfg(not(feature = "zip"))]
#[tokio::test]
async fn archive_sources_explain_the_missing_feature() {
   use assetify::ArchiveFormat;

   let remote = tempfile::tempdir().unwrap();
   let cache = tempfile::tempdir().unwrap();

   let file = file_source(remote.path(), "pack.zip", b"pretend archive bytes")
      .extracted(ArchiveFormat::Zip);
   let engine = Assetify::builder(cache.path())
      .resolver(StaticResolver::new([(
         "tokenizer/en",
         AssetSource::new("r1", vec![file]),
      )]))
      .build()
      .unwrap();

   let reason = unwrap_unavailable(
      engine
         .asset(AssetRequest::new("tokenizer/en", vec!["meta.json"]))
         .await,
   );
   assert!(reason.contains("`zip` feature"), "{reason}");
}

#[tokio::test]
async fn invalid_ids_and_names_never_touch_the_filesystem() {
   let cache = tempfile::tempdir().unwrap();
   let engine = Assetify::builder(cache.path()).build().unwrap();

   let reason = unwrap_unavailable(
      engine
         .asset(AssetRequest::new("../escape", Vec::<&str>::new()))
         .await,
   );
   assert!(reason.contains("invalid"), "{reason}");

   let reason = unwrap_unavailable(
      engine
         .asset(AssetRequest::new(
            "models/sentiment",
            vec!["../../etc/passwd"],
         ))
         .await,
   );
   assert!(reason.contains("invalid"), "{reason}");
}

/// A bring-your-own fetcher: serves bodies from a map, no HTTP stack
/// at all — the locator's "url" is opaque to the engine.
struct MapFetcher {
   bodies: std::collections::HashMap<String, Vec<u8>>,
}

#[async_trait::async_trait]
impl assetify::Fetcher for MapFetcher {
   async fn fetch(
      &self,
      url: &str,
      sink: &mut (dyn std::io::Write + Send),
   ) -> Result<(), assetify::FetchError> {
      let bytes = self
         .bodies
         .get(url)
         .ok_or_else(|| assetify::FetchError::new(format!("no body for {url}")))?;
      sink
         .write_all(bytes)
         .map_err(|e| assetify::FetchError::new(e.to_string()))
   }
}

#[tokio::test]
async fn a_custom_fetcher_supplies_locator_bytes() {
   let cache = tempfile::tempdir().unwrap();

   // A non-HTTP scheme: the engine never interprets the URL.
   let url = "custom://releases/tokenizer/meta.json";
   let engine = Assetify::builder(cache.path())
      .resolver(StaticResolver::new([(
         "tokenizer/en",
         AssetSource::new(
            "20260821",
            vec![FileSource::new(
               "meta.json",
               Locator::Url(url.to_string()),
               sha256_of(b"{}"),
            )],
         ),
      )]))
      .fetcher(MapFetcher {
         bodies: [(url.to_string(), b"{}".to_vec())].into(),
      })
      .build()
      .unwrap();

   let asset = unwrap_available(
      engine
         .asset(AssetRequest::new("tokenizer/en", ["meta.json"]))
         .await,
   );
   let mut meta = String::new();
   asset
      .file("meta.json")
      .unwrap()
      .stream()
      .unwrap()
      .read_to_string(&mut meta)
      .unwrap();
   assert_eq!(meta, "{}");

   // Verification stayed with the engine: a fetcher returning bytes
   // that miss the digest places nothing.
   let engine = Assetify::builder(cache.path())
      .resolver(StaticResolver::new([(
         "models/other",
         AssetSource::new(
            "r1",
            vec![FileSource::new(
               "model.bin",
               Locator::Url(url.to_string()),
               sha256_of(b"the promised bytes"),
            )],
         ),
      )]))
      .fetcher(MapFetcher {
         bodies: [(url.to_string(), b"tampered".to_vec())].into(),
      })
      .build()
      .unwrap();
   let reason = unwrap_unavailable(
      engine
         .asset(AssetRequest::new("models/other", ["model.bin"]))
         .await,
   );
   assert!(reason.contains("digest mismatch"), "{reason}");
   assert!(!cache.path().join("models/other").exists());
}

/// A fetcher that owns the transfer: it writes the file itself (as a
/// native background downloader would) rather than streaming through
/// the engine's sink.
struct PathFetcher {
   bodies: std::collections::HashMap<String, Vec<u8>>,
}

#[async_trait::async_trait]
impl assetify::Fetcher for PathFetcher {
   async fn fetch(
      &self,
      _url: &str,
      _sink: &mut (dyn std::io::Write + Send),
   ) -> Result<(), assetify::FetchError> {
      Err(assetify::FetchError::new("this fetcher writes to a path"))
   }

   fn writes_to_path(&self) -> bool {
      true
   }

   async fn fetch_to_path(
      &self,
      url: &str,
      dest: &std::path::Path,
   ) -> Result<(), assetify::FetchError> {
      let bytes = self
         .bodies
         .get(url)
         .ok_or_else(|| assetify::FetchError::new(format!("no body for {url}")))?;
      std::fs::write(dest, bytes).map_err(|e| assetify::FetchError::new(e.to_string()))
   }
}

#[tokio::test]
async fn a_path_writing_fetcher_is_verified_by_the_engine() {
   let cache = tempfile::tempdir().unwrap();
   let url = "native://tokenizer/model.bin";

   // Happy path: the fetcher wrote the file; the engine re-read and
   // verified it, and serves it.
   let engine = Assetify::builder(cache.path())
      .resolver(StaticResolver::new([(
         "tokenizer/en",
         AssetSource::new(
            "20260821",
            vec![FileSource::new(
               "model.bin",
               Locator::Url(url.to_string()),
               sha256_of(b"weights"),
            )],
         ),
      )]))
      .fetcher(PathFetcher {
         bodies: [(url.to_string(), b"weights".to_vec())].into(),
      })
      .build()
      .unwrap();
   unwrap_available(
      engine
         .asset(AssetRequest::new("tokenizer/en", ["model.bin"]))
         .await,
   );

   // Verification still stays engine-side: a path-writing fetcher whose
   // file misses the digest places nothing.
   let engine = Assetify::builder(cache.path())
      .resolver(StaticResolver::new([(
         "models/other",
         AssetSource::new(
            "r1",
            vec![FileSource::new(
               "model.bin",
               Locator::Url(url.to_string()),
               sha256_of(b"the promised bytes"),
            )],
         ),
      )]))
      .fetcher(PathFetcher {
         bodies: [(url.to_string(), b"tampered".to_vec())].into(),
      })
      .build()
      .unwrap();
   let reason = unwrap_unavailable(
      engine
         .asset(AssetRequest::new("models/other", ["model.bin"]))
         .await,
   );
   assert!(reason.contains("digest mismatch"), "{reason}");
   assert!(!cache.path().join("models/other").exists());
}
