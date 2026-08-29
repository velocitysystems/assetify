//! End-to-end provision over HTTP (feature `http`), against a local
//! mock server: happy path, cache hits, digest mismatch, server
//! failures with and without an on-disk fallback, and download
//! deduplication under concurrency.

#![cfg(feature = "http")]

use std::sync::Arc;

use assetify::{
   AccessKind, AssetOutcome, AssetRequest, AssetSource, Assetify, Digest, FileSource, FileSpec,
   Locator, StaticResolver,
};
use sha2::Digest as _;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn sha256_of(bytes: &[u8]) -> Digest {
   Digest::sha256_hex(&hex::encode(sha2::Sha256::digest(bytes))).unwrap()
}

fn http_source(server_uri: &str, revision: &str, files: &[(&str, &[u8])]) -> AssetSource {
   AssetSource::new(
      revision,
      files
         .iter()
         .map(|(name, bytes)| {
            FileSource::new(
               *name,
               Locator::HTTP {
                  url: format!("{server_uri}/{revision}/{name}"),
               },
               sha256_of(bytes),
            )
         })
         .collect(),
   )
}

/// Serve one file; `hits` asserts an exact GET count at server drop,
/// `None` leaves the count unchecked.
async fn mount(server: &MockServer, revision: &str, name: &str, bytes: &[u8], hits: Option<u64>) {
   let mock = Mock::given(method("GET"))
      .and(path(format!("/{revision}/{name}")))
      .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes.to_vec()));
   match hits {
      Some(hits) => mock.expect(hits).mount(server).await,
      None => mock.mount(server).await,
   }
}

fn request() -> AssetRequest {
   AssetRequest::new(
      "models/sentiment",
      1,
      vec![
         FileSpec::new("model.bin", AccessKind::Random),
         FileSpec::new("labels.txt", AccessKind::Stream),
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
async fn downloads_verify_and_later_requests_hit_the_cache() {
   let server = MockServer::start().await;
   // expect(1): the second provide must not touch the network.
   mount(&server, "r1", "model.bin", b"weights", Some(1)).await;
   mount(&server, "r1", "labels.txt", b"pos\nneg", Some(1)).await;

   let cache = tempfile::tempdir().unwrap();
   let engine = Assetify::builder(cache.path())
      .resolver(StaticResolver::new([(
         "models/sentiment",
         1,
         http_source(
            &server.uri(),
            "r1",
            &[("model.bin", b"weights"), ("labels.txt", b"pos\nneg")],
         ),
      )]))
      .build()
      .unwrap();

   unwrap_available(engine.asset(request()).await);
   unwrap_available(engine.asset(request()).await);
   // Mock expectations (exactly one GET per file) verify on drop.
}

#[tokio::test]
async fn a_corrupt_body_cleans_staging_and_places_nothing() {
   let server = MockServer::start().await;
   mount(&server, "r1", "model.bin", b"tampered bytes", Some(1)).await;
   mount(&server, "r1", "labels.txt", b"pos\nneg", None).await;

   let cache = tempfile::tempdir().unwrap();
   let engine = Assetify::builder(cache.path())
      .resolver(StaticResolver::new([(
         "models/sentiment",
         1,
         // The resolver promises different bytes than the server has.
         http_source(
            &server.uri(),
            "r1",
            &[("model.bin", b"weights"), ("labels.txt", b"pos\nneg")],
         ),
      )]))
      .build()
      .unwrap();

   let reason = unwrap_unavailable(engine.asset(request()).await);
   assert!(reason.contains("digest mismatch"), "{reason}");
   assert!(!cache.path().join("models/sentiment").exists());
   assert_eq!(
      std::fs::read_dir(cache.path().join(".staging"))
         .unwrap()
         .count(),
      0
   );
}

#[tokio::test]
async fn server_errors_fall_back_to_the_cached_revision() {
   let cache = tempfile::tempdir().unwrap();

   // Populate the cache from a healthy server.
   {
      let server = MockServer::start().await;
      mount(&server, "r1", "model.bin", b"weights", Some(1)).await;
      mount(&server, "r1", "labels.txt", b"pos\nneg", Some(1)).await;
      let engine = Assetify::builder(cache.path())
         .resolver(StaticResolver::new([(
            "models/sentiment",
            1,
            http_source(
               &server.uri(),
               "r1",
               &[("model.bin", b"weights"), ("labels.txt", b"pos\nneg")],
            ),
         )]))
         .build()
         .unwrap();
      unwrap_available(engine.asset(request()).await);
   } // server drops: the host is now unreachable

   // A resolver naming a newer revision on a dead server: the cached
   // r1 still serves.
   let engine = Assetify::builder(cache.path())
      .resolver(StaticResolver::new([(
         "models/sentiment",
         1,
         http_source(
            "http://127.0.0.1:9", // discard port: connection refused
            "r2",
            &[("model.bin", b"newer"), ("labels.txt", b"pos\nneg")],
         ),
      )]))
      .build()
      .unwrap();
   unwrap_available(engine.asset(request()).await);

   // The same dead server with an empty lane: unavailable, reason
   // carries the acquisition failure.
   let reason = unwrap_unavailable(
      engine
         .asset(AssetRequest::new(
            "models/other",
            1,
            vec![FileSpec::new("model.bin", AccessKind::Random)],
         ))
         .await,
   );
   assert!(reason.contains("nothing servable"), "{reason}");
}

#[tokio::test]
async fn a_404_on_one_file_of_a_set_places_nothing() {
   let server = MockServer::start().await;
   mount(&server, "r1", "model.bin", b"weights", None).await;
   // labels.txt is not mounted: the server answers 404.

   let cache = tempfile::tempdir().unwrap();
   let engine = Assetify::builder(cache.path())
      .resolver(StaticResolver::new([(
         "models/sentiment",
         1,
         http_source(
            &server.uri(),
            "r1",
            &[("model.bin", b"weights"), ("labels.txt", b"pos\nneg")],
         ),
      )]))
      .build()
      .unwrap();

   let reason = unwrap_unavailable(engine.asset(request()).await);
   assert!(reason.contains("404"), "{reason}");
   assert!(!cache.path().join("models/sentiment").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_requests_download_each_file_exactly_once() {
   let server = MockServer::start().await;
   mount(&server, "r1", "model.bin", b"weights", Some(1)).await;
   mount(&server, "r1", "labels.txt", b"pos\nneg", Some(1)).await;

   let cache = tempfile::tempdir().unwrap();
   let engine = Arc::new(
      Assetify::builder(cache.path())
         .resolver(StaticResolver::new([(
            "models/sentiment",
            1,
            http_source(
               &server.uri(),
               "r1",
               &[("model.bin", b"weights"), ("labels.txt", b"pos\nneg")],
            ),
         )]))
         .build()
         .unwrap(),
   );

   let handles: Vec<_> = (0..8)
      .map(|_| {
         let engine = Arc::clone(&engine);
         tokio::spawn(async move { engine.asset(request()).await })
      })
      .collect();
   for handle in handles {
      unwrap_available(handle.await.unwrap());
   }
   // expect(1) on each mock verifies single-flight on drop.
}
