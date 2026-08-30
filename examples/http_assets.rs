//! Download, verify, cache, serve — multiple assets against a local
//! mock server, so the example is self-contained.
//!
//! Run with: `cargo run --example http_assets --features reqwest`

use std::io::Read as _;

use assetify::{
   AccessKind, AssetRequest, AssetResponse, AssetSource, Assetify, FileAccess, FileSource,
   Provider as _, StaticResolver,
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
      AssetRequest::new(
         "tokenizer/en",
         [
            ("meta.json", AccessKind::Stream),
            ("index.dat", AccessKind::Random),
            ("rules.txt", AccessKind::AssetPath),
         ],
      ),
      AssetRequest::new(
         "models/classifier/en",
         [
            ("model.bin", AccessKind::Random),
            ("labels.txt", AccessKind::Stream),
         ],
      ),
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
      let AssetResponse::Available { mut asset } = outcome else {
         panic!("published assets serve");
      };
      for spec in &request.files {
         match asset
            .take_file(&spec.name)
            .expect("requested files are delivered")
            .access
         {
            FileAccess::Stream(mut stream) => {
               let mut bytes = Vec::new();
               stream.read_to_end(&mut bytes)?;
               tracing::info!(asset = %request.id, file = %spec.name, bytes = bytes.len(), "streamed");
            }
            FileAccess::Random(random) => {
               let mut sample = [0u8; 12];
               random.read_at_exact(64, &mut sample)?;
               tracing::info!(
                  asset = %request.id,
                  file = %spec.name,
                  len = random.len(),
                  sample = %String::from_utf8_lossy(&sample),
                  "ranged read"
               );
            }
            FileAccess::AssetPath(path) => {
               tracing::info!(asset = %request.id, file = %spec.name, path = %path.display(), "materialized");
            }
            _ => unreachable!("access kinds a request can name are covered above"),
         }
      }
   }

   // Offline: the server is gone, yet everything still serves — both
   // verified revisions are cached under <root>/<id>/<rev>/.
   drop(server);
   for (request, outcome) in requests.iter().zip(engine.provide(&requests).await) {
      let AssetResponse::Available { asset } = outcome else {
         panic!("cached revisions serve offline");
      };
      tracing::info!(asset = %request.id, files = asset.files.len(), "served from cache, offline");
   }
   Ok(())
}
