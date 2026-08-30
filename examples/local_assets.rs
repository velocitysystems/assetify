//! Cache-only mode: serve assets from a directory that already holds
//! them — the shape of a serverless function serving assets bundled
//! into its deployment package, or a test running over fixtures.
//!
//! Run with: `cargo run --example local_assets`

use std::io::Read as _;
use std::path::Path;

use assetify::{AccessKind, AssetRequest, AssetResponse, Assetify, FileAccess, Provider as _};
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

   // One batched call for everything this launch needs, then one
   // match arm per access kind to consume the deliveries.
   let requests = requests();
   for (request, outcome) in requests.iter().zip(engine.provide(&requests).await) {
      let AssetResponse::Available { mut asset } = outcome else {
         panic!("seeded assets serve");
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
   Ok(())
}
