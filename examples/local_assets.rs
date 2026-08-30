//! Cache-only mode: serve assets from a directory that already holds
//! them — the shape of a serverless function serving assets bundled
//! into its deployment package, or a test running over fixtures.
//!
//! Run with: `cargo run --example local_assets`

use std::io::Read as _;
use std::path::Path;

use assetify::{AssetRequest, AssetResponse, Assetify, Provider as _};
use rand::distr::{Alphanumeric, SampleString};

// The fixtures — shared verbatim with the http_assets example: two
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

fn seed(revision: &Path, files: &[(String, String)]) -> std::io::Result<()> {
   std::fs::create_dir_all(revision)?;
   for (name, content) in files {
      std::fs::write(revision.join(name), content)?;
   }
   Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
   // Show the engine's lifecycle events (staging, delivery, fallback).
   tracing_subscriber::fmt()
      .without_time()
      .with_target(false)
      .compact()
      .init();

   // Pre-seed the tree, laid out as <root>/<id>/<revision>/.
   let root = tempfile::tempdir()?;
   seed(
      &root.path().join("tokenizer/en/20260821"),
      &[
         ("meta.json".into(), r#"{"format":1,"language":"en"}"#.into()),
         ("index.dat".into(), random_text(2048)),
         ("rules.txt".into(), random_text(256)),
      ],
   )?;
   seed(
      &root.path().join("models/classifier/en/20260815"),
      &[
         ("model.bin".into(), random_text(4096)),
         ("labels.txt".into(), "positive\nnegative\nneutral".into()),
      ],
   )?;

   // No resolver: cache-only. A read-only root works too.
   let engine = Assetify::builder(root.path()).build()?;

   // One batched call for everything this launch needs, then read
   // each delivered file whichever way suits it.
   let requests = requests();
   for (request, outcome) in requests.iter().zip(engine.provide(&requests).await) {
      let AssetResponse::Available { asset } = outcome else {
         panic!("seeded assets serve");
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
   Ok(())
}
