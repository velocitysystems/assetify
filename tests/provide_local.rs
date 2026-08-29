//! End-to-end provision from local sources: resolve → acquire →
//! verify → place → serve, plus every degraded path — offline
//! fallback, poison, validation, and all-or-nothing staging.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use assetify::{
   AccessKind, AssetOutcome, AssetRequest, AssetSource, Assetify, Digest, FileAccess, FileSource,
   FileSpec, Locator, RejectedDelivery, ResolveError, SourceResolver, StaticResolver,
};
use sha2::Digest as _;

fn sha256_of(bytes: &[u8]) -> Digest {
   Digest::sha256_hex(&hex::encode(sha2::Sha256::digest(bytes))).unwrap()
}

/// Write a "remote" file and describe it as a source.
fn file_source(dir: &Path, name: &str, bytes: &[u8]) -> FileSource {
   let path = dir.join(name);
   std::fs::write(&path, bytes).unwrap();
   FileSource::new(name, Locator::File { path }, sha256_of(bytes))
}

fn tokenizer_request() -> AssetRequest {
   AssetRequest::new(
      "nlp/tokenizer/en",
      4,
      vec![
         FileSpec::new("meta.json", AccessKind::Stream),
         FileSpec::new("index.dat", AccessKind::Random),
         FileSpec::new("rules.txt", AccessKind::MaterializedPath),
      ],
   )
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

fn unwrap_available(outcome: AssetOutcome) -> assetify::PreparedAsset {
   match outcome {
      AssetOutcome::Available { asset } => asset,
      AssetOutcome::Unavailable { reason } => panic!("expected availability, got: {reason}"),
   }
}

fn unwrap_unavailable(outcome: AssetOutcome) -> String {
   match outcome {
      AssetOutcome::Unavailable { reason } => reason,
      AssetOutcome::Available { .. } => panic!("expected unavailability"),
   }
}

#[tokio::test]
async fn acquires_verifies_places_and_serves_every_access_kind() {
   let remote = tempfile::tempdir().unwrap();
   let cache = tempfile::tempdir().unwrap();

   let engine = Assetify::builder(cache.path())
      .resolver(StaticResolver::new([(
         "nlp/tokenizer/en",
         4,
         tokenizer_source(remote.path(), "20260821"),
      )]))
      .build()
      .unwrap();

   let mut asset = unwrap_available(engine.asset(tokenizer_request()).await);

   let FileAccess::Stream(mut stream) = asset.take_file("meta.json").unwrap().access else {
      panic!("stream kind delivers a stream");
   };
   let mut drained = String::new();
   stream.read_to_string(&mut drained).unwrap();
   assert_eq!(drained, r#"{"format":4}"#);

   let FileAccess::Random(random) = asset.take_file("index.dat").unwrap().access else {
      panic!("random kind delivers positioned access");
   };
   let mut word = [0u8; 5];
   random.read_at_exact(11, &mut word).unwrap();
   assert_eq!(&word, b"bytes");

   let FileAccess::Path(path) = asset.take_file("rules.txt").unwrap().access else {
      panic!("materialized kind delivers a path");
   };
   assert_eq!(std::fs::read(&*path).unwrap(), b"rule one");

   // The cache now holds the placed revision under the lane.
   assert!(
      cache
         .path()
         .join("nlp/tokenizer/en/v4/20260821/meta.json")
         .is_file()
   );
}

#[tokio::test]
async fn cache_only_mode_serves_a_preseeded_root() {
   let cache = tempfile::tempdir().unwrap();
   let revision = cache.path().join("nlp/tokenizer/en/v4/20260812");
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
         .asset(AssetRequest::new("models/sentiment", 1, vec![]))
         .await,
   );
   assert!(missing.contains("cache-only"), "{missing}");
}

/// A resolver that fails every time — the offline case.
struct OfflineResolver;

#[async_trait::async_trait]
impl SourceResolver for OfflineResolver {
   async fn resolve(&self, _: &str, _: u32) -> Result<Option<AssetSource>, ResolveError> {
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
         "nlp/tokenizer/en",
         4,
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

   // Offline with an empty lane: unavailable, and the reason carries
   // both the failure and the empty-lane fact.
   let reason = unwrap_unavailable(
      offline
         .asset(AssetRequest::new(
            "models/sentiment",
            1,
            vec![FileSpec::new("model.bin", AccessKind::Random)],
         ))
         .await,
   );
   assert!(reason.contains("resolution failed"), "{reason}");
   assert!(reason.contains("nothing servable"), "{reason}");
}

#[tokio::test]
async fn a_rejection_echo_poisons_the_served_revision_until_a_newer_one_exists() {
   let remote = tempfile::tempdir().unwrap();
   let cache = tempfile::tempdir().unwrap();

   let engine = Assetify::builder(cache.path())
      .resolver(StaticResolver::new([(
         "nlp/tokenizer/en",
         4,
         tokenizer_source(remote.path(), "20260812"),
      )]))
      .build()
      .unwrap();
   unwrap_available(engine.asset(tokenizer_request()).await);

   // The consumer could not load the delivery; the echo poisons the
   // revision, and the resolver still names it — so nothing serves.
   let mut echoed = tokenizer_request();
   echoed.rejected = Some(RejectedDelivery {
      reason: "payload failed content validation".to_string(),
   });
   let reason = unwrap_unavailable(engine.asset(echoed).await);
   assert!(reason.contains("rejected by a previous load"), "{reason}");

   // Poison survives a process restart (a fresh engine, same root).
   let restarted = Assetify::builder(cache.path())
      .resolver(StaticResolver::new([(
         "nlp/tokenizer/en",
         4,
         tokenizer_source(remote.path(), "20260812"),
      )]))
      .build()
      .unwrap();
   unwrap_unavailable(restarted.asset(tokenizer_request()).await);

   // A newer revision from the resolver recovers the asset.
   let recovered = Assetify::builder(cache.path())
      .resolver(StaticResolver::new([(
         "nlp/tokenizer/en",
         4,
         tokenizer_source(remote.path(), "20260821"),
      )]))
      .build()
      .unwrap();
   unwrap_available(recovered.asset(tokenizer_request()).await);
}

/// Counts resolutions, then delegates to a static map.
struct CountingResolver {
   calls: Arc<AtomicUsize>,
   inner: StaticResolver,
}

#[async_trait::async_trait]
impl SourceResolver for CountingResolver {
   async fn resolve(
      &self,
      id: &str,
      format_major: u32,
   ) -> Result<Option<AssetSource>, ResolveError> {
      self.calls.fetch_add(1, Ordering::SeqCst);
      self.inner.resolve(id, format_major).await
   }
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
               "nlp/tokenizer/en",
               4,
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
         "nlp/tokenizer/en",
         4,
         tokenizer_source(remote.path(), "20260821"),
      )]))
      .build()
      .unwrap();

   let path: PathBuf = {
      let mut asset = unwrap_available(engine.asset(tokenizer_request()).await);
      let FileAccess::Path(materialized) = asset.take_file("rules.txt").unwrap().access else {
         panic!("materialized kind delivers a path");
      };
      materialized.into_path_buf()
      // `asset` (the rest of the delivery) drops here.
   };
   assert_eq!(
      std::fs::read(&path).unwrap(),
      b"rule one",
      "nothing deletes placed revisions in v1, so the path holds"
   );
}

#[tokio::test]
async fn duplicate_file_names_in_a_revision_are_a_delivery_error() {
   let cache = tempfile::tempdir().unwrap();
   let revision = cache.path().join("dicts/spellcheck-de/v2/r1");
   std::fs::create_dir_all(revision.join("a")).unwrap();
   std::fs::create_dir_all(revision.join("b")).unwrap();
   std::fs::write(revision.join("a/words.dat"), b"one").unwrap();
   std::fs::write(revision.join("b/words.dat"), b"two").unwrap();

   let engine = Assetify::builder(cache.path()).build().unwrap();
   let reason = unwrap_unavailable(
      engine
         .asset(AssetRequest::new(
            "dicts/spellcheck-de",
            2,
            vec![FileSpec::new("words.dat", AccessKind::Random)],
         ))
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
         "nlp/tokenizer/en",
         4,
         AssetSource::new("20260821", vec![good, bad]),
      )]))
      .build()
      .unwrap();

   let reason = unwrap_unavailable(
      engine
         .asset(AssetRequest::new(
            "nlp/tokenizer/en",
            4,
            vec![FileSpec::new("meta.json", AccessKind::Stream)],
         ))
         .await,
   );
   assert!(reason.contains("digest mismatch"), "{reason}");
   assert!(reason.contains("index.dat"), "{reason}");

   // Nothing was placed — not even the file that verified — and
   // staging holds no leftovers.
   assert!(!cache.path().join("nlp/tokenizer/en").exists());
   let staging = cache.path().join(".staging");
   assert_eq!(std::fs::read_dir(staging).unwrap().count(), 0);
}

#[tokio::test]
async fn invalid_ids_and_names_never_touch_the_filesystem() {
   let cache = tempfile::tempdir().unwrap();
   let engine = Assetify::builder(cache.path()).build().unwrap();

   let reason = unwrap_unavailable(
      engine
         .asset(AssetRequest::new("../escape", 1, vec![]))
         .await,
   );
   assert!(reason.contains("invalid"), "{reason}");

   let reason = unwrap_unavailable(
      engine
         .asset(AssetRequest::new(
            "models/sentiment",
            1,
            vec![FileSpec::new("../../etc/passwd", AccessKind::Stream)],
         ))
         .await,
   );
   assert!(reason.contains("invalid"), "{reason}");
}
