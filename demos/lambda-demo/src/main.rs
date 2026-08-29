//! assetify inside an AWS Lambda function: cache-only mode over
//! assets bundled into the deployment package — deterministic, no
//! network, read-only filesystem friendly.
//!
//! Local: `cargo lambda watch`, then
//!        `cargo lambda invoke lambda-demo --data-file event.json`
//! Deploy: `cargo lambda build --release && cargo lambda deploy`
//!         (the `include = ["assets"]` metadata ships the fixtures)

use std::io::Read as _;
use std::path::PathBuf;

use assetify::{AccessKind, AssetResponse, AssetRequest, Assetify, FileAccess, FileSpec};
use lambda_runtime::{Error, LambdaEvent, run, service_fn};
use serde_json::{Value, json};

/// Deployed, the bundled `assets/` directory sits in the function's
/// task root next to the binary; locally, use the committed fixtures.
fn assets_root() -> PathBuf {
   if let Ok(task_root) = std::env::var("LAMBDA_TASK_ROOT") {
      let bundled = PathBuf::from(task_root).join("assets");
      if bundled.exists() {
         return bundled;
      }
   }
   PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets")
}

async fn handler(_event: LambdaEvent<Value>) -> Result<Value, Error> {
   // Cache-only mode over a read-only root: the bundle is the cache.
   let engine = Assetify::builder(assets_root()).build()?;
   let request = AssetRequest::new(
      "nlp/tokenizer/en",
      1,
      vec![
         FileSpec::new("meta.json", AccessKind::Stream),
         FileSpec::new("index.dat", AccessKind::Random),
      ],
   );

   match engine.asset(request).await {
      AssetResponse::Available { mut asset } => {
         let FileAccess::Stream(mut stream) = asset.take_file("meta.json").unwrap().access else {
            unreachable!()
         };
         let mut meta = String::new();
         stream.read_to_string(&mut meta)?;

         let FileAccess::Random(index) = asset.take_file("index.dat").unwrap().access else {
            unreachable!()
         };
         Ok(json!({
            "id": "nlp/tokenizer/en",
            "revisionMeta": meta,
            "indexBytes": index.len(),
         }))
      }
      AssetResponse::Unavailable { reason } => Ok(json!({ "unavailable": reason })),
   }
}

#[tokio::main]
async fn main() -> Result<(), Error> {
   lambda_runtime::tracing::init_default_subscriber();
   run(service_fn(handler)).await
}
