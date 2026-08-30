//! assetify inside an AWS Lambda function: cache-only mode over
//! assets bundled into the deployment package — deterministic, no
//! network, read-only filesystem friendly.
//!
//! Local: `cargo lambda watch`, then
//!        `cargo lambda invoke lambda-demo --data-file event.json`
//! Deploy: copy the shared fixtures into the package (see README),
//!         then `cargo lambda build --release && cargo lambda deploy`

use std::io::Read as _;
use std::path::PathBuf;

use assetify::{AssetRequest, AssetResponse, Assetify, Provider};
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use serde_json::{Value, json};

/// Deployed, the bundled `assets/` directory sits in the function's
/// task root next to the binary; locally, serve the shared fixture
/// tree at `demos/assets` in place.
fn assets_root() -> PathBuf {
   if let Ok(task_root) = std::env::var("LAMBDA_TASK_ROOT") {
      let bundled = PathBuf::from(task_root).join("assets");
      if bundled.exists() {
         return bundled;
      }
   }
   PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../assets")
}

async fn handler(_event: LambdaEvent<Value>) -> Result<Value, Error> {
   // Cache-only mode over a read-only root: the bundle is the cache.
   let engine = Assetify::builder(assets_root()).build()?;
   let request = AssetRequest::new(
      "tokenizer/en",
      [
         "meta.json",
         "index.dat",
         "vocab.txt",
      ],
   );

   let asset = match engine.asset(request).await {
      AssetResponse::Available { asset } => asset,
      AssetResponse::Unavailable { reason } => return Ok(json!({ "unavailable": reason })),
   };

   // Stream: one forward parse of the model card.
   let mut card = String::new();
   asset
      .file("meta.json")
      .unwrap()
      .stream()?
      .read_to_string(&mut card)?;
   let meta: Value = serde_json::from_str(&card)?;
   let language = meta["language"].as_str().unwrap_or("unknown").to_string();
   let declared = meta["vocabSize"].as_u64().unwrap_or(0) as u32;

   // Path: read the vocabulary by real path, the way a tokenizer
   // library opening its own files would.
   let vocab_path = asset.file("vocab.txt").unwrap().path().expect("a filesystem path");
   let vocab = std::fs::read_to_string(vocab_path)?;
   let vocab_words = vocab.lines().count() as u32;

   // Random: decode the index header, then look tokens up through it
   // — a positioned read per entry, never a scan.
   let index = asset.file("index.dat").unwrap().random()?;
   let mut header = [0u8; 8];
   index.read_at_exact(0, &mut header)?;
   if &header[0..4] != b"AIDX" {
      return Err("index.dat: invalid header".into());
   }
   let entries = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
   if entries == 0 {
      return Err("index.dat: empty index".into());
   }

   let mut sample_tokens = Vec::new();
   // Entries picked for recognizable words in this revision.
   for entry in [320u32, 643, 810] {
      let mut raw = [0u8; 4];
      index.read_at_exact(8 + u64::from(entry) * 4, &mut raw)?;
      let offset = u32::from_le_bytes(raw) as usize;
      let token = vocab[offset..].lines().next().unwrap_or("").to_string();
      sample_tokens.push(token);
   }

   let consistent =
      vocab_words == declared && entries == declared && sample_tokens.iter().all(|t| !t.is_empty());
   Ok(json!({
      "id": "tokenizer/en",
      "language": language,
      "vocabWords": vocab_words,
      "indexEntries": entries,
      "sampleTokens": sample_tokens,
      "consistent": consistent,
   }))
}

#[tokio::main]
async fn main() -> Result<(), Error> {
   lambda_runtime::tracing::init_default_subscriber();
   run(service_fn(handler)).await
}
