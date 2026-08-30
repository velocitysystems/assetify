//! Download, verify, cache, serve — multiple assets against a local
//! mock server, so the example is self-contained.
//!
//! Run with: `cargo run --example http_assets --features reqwest`

use std::io::Read as _;

use assetify::{
   AssetRequest, AssetResponse, AssetSource, Assetify, FileSource, Provider as _, StaticResolver,
};
use rand::distr::{Alphanumeric, SampleString};
use sha2::Digest as _;

// The fixtures — shared verbatim with the local_assets example: two
// language-scoped assets, each a few files of realistic random data.

/// Random payload bytes, so sizes and samples look like real data.
fn random_text(len: usize) -> String {
   Alphanumeric.sample_string(&mut rand::rng(), len)
}

/// What the consumer asks for: every access kind across two assets.
fn requests() -> [AssetRequest; 2] {
   [
      AssetRequest::new("tokenizer/en", ["meta.json", "index.dat", "rules.txt"]),
      AssetRequest::new("models/classifier/en", ["model.bin", "labels.txt"]),
   ]
}

/// Publish one asset's files on the mock server and describe them as
/// a source: per-file URL + SHA-256, under one revision.
async fn publish(
   server: &wiremock::MockServer,
   id: &str,
   revision: &str,
   files: &[(String, String)],
) -> AssetSource {
   let mut sources = Vec::new();
   for (name, content) in files {
      wiremock::Mock::given(wiremock::matchers::method("GET"))
         .and(wiremock::matchers::path(format!("/{id}/{revision}/{name}")))
         .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_bytes(content.as_bytes().to_vec()),
         )
         .mount(server)
         .await;
      sources.push(
         FileSource::url(
            name.clone(),
            format!("{}/{id}/{revision}/{name}", server.uri()),
            &hex::encode(sha2::Sha256::digest(content.as_bytes())),
         )
         .expect("a hex-encoded sha-256 is a valid digest"),
      );
   }
   AssetSource::new(revision, sources)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
   // Show the engine's lifecycle events (staging, delivery, fallback).
   tracing_subscriber::fmt()
      .without_time()
      .with_target(false)
      .compact()
      .init();

   // Publish the fixtures on a stand-in for wherever your assets live.
   let server = wiremock::MockServer::start().await;
   let tokenizer = publish(
      &server,
      "tokenizer/en",
      "20260821",
      &[
         ("meta.json".into(), r#"{"format":1,"language":"en"}"#.into()),
         ("index.dat".into(), random_text(2048)),
         ("rules.txt".into(), random_text(256)),
      ],
   )
   .await;
   let classifier = publish(
      &server,
      "models/classifier/en",
      "20260815",
      &[
         ("model.bin".into(), random_text(4096)),
         ("labels.txt".into(), "positive\nnegative\nneutral".into()),
      ],
   )
   .await;

   let cache = tempfile::tempdir()?;
   let engine = Assetify::builder(cache.path())
      .resolver(StaticResolver::new([
         ("tokenizer/en", tokenizer),
         ("models/classifier/en", classifier),
      ]))
      .build()?;

   // One batched call: each asset downloads, verifies, and caches —
   // then one match arm per access kind to consume the deliveries.
   let requests = requests();
   for (request, outcome) in requests.iter().zip(engine.provide(&requests).await) {
      let AssetResponse::Available { asset } = outcome else {
         panic!("published assets serve");
      };
      for name in &request.files {
         let file = asset.file(name).expect("requested files are delivered");
         let mut bytes = Vec::new();
         file.stream()?.read_to_end(&mut bytes)?;
         let has_window = file.random()?.as_bytes().is_some();
         tracing::info!(
            asset = %request.id,
            file = %name,
            bytes = bytes.len(),
            window = has_window,
            path = file.path().map(|p| p.display().to_string()).unwrap_or_default(),
            "read"
         );
      }
   }

   // Offline: the server is gone, yet everything still serves — both
   // verified revisions are cached under <root>/<id>/<rev>/.
   drop(server);
   for (request, outcome) in requests.iter().zip(engine.provide(&requests).await) {
      let AssetResponse::Available { asset } = outcome else {
         panic!("cached revisions serve offline");
      };
      tracing::info!(asset = %request.id, files = asset.files().len(), "served from cache, offline");
   }
   Ok(())
}
