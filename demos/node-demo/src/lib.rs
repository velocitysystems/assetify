//! assetify behind a napi-rs addon: JavaScript asks for assets, every
//! read happens on the Rust side, and only a serializable summary
//! crosses the JS bridge.

use std::io::Read as _;

use assetify::{AccessKind, AssetOutcome, AssetRequest, Assetify, FileAccess, FileSpec};
use napi_derive::napi;

/// What crosses the bridge: names and sizes, never bytes or handles.
#[napi(object)]
pub struct AssetSummary {
   pub id: String,
   pub revision_meta: String,
   pub index_bytes: u32,
}

/// Load the demo asset into `cache_dir` and summarize it. Runs on the
/// napi-managed tokio runtime; the returned promise resolves in JS.
#[napi]
pub async fn load_asset(cache_dir: String) -> napi::Result<AssetSummary> {
   // Seed the cache tree a real app would have downloaded earlier:
   // <root>/<id>/v<lane>/<revision>/<files>.
   let revision = std::path::Path::new(&cache_dir).join("nlp/tokenizer/en/v1/20260821");
   std::fs::create_dir_all(&revision).map_err(reason)?;
   std::fs::write(
      revision.join("meta.json"),
      r#"{"format":1,"language":"en"}"#,
   )
   .map_err(reason)?;
   std::fs::write(revision.join("index.dat"), "tokenizer index bytes").map_err(reason)?;

   let engine = Assetify::builder(&cache_dir).build().map_err(reason)?;
   let request = AssetRequest::new(
      "nlp/tokenizer/en",
      1,
      vec![
         FileSpec::new("meta.json", AccessKind::Stream),
         FileSpec::new("index.dat", AccessKind::Random),
      ],
   );

   match engine.asset(request).await {
      AssetOutcome::Available { mut asset } => {
         let FileAccess::Stream(mut stream) = asset.take_file("meta.json").unwrap().access else {
            unreachable!()
         };
         let mut meta = String::new();
         stream.read_to_string(&mut meta).map_err(reason)?;

         let FileAccess::Random(index) = asset.take_file("index.dat").unwrap().access else {
            unreachable!()
         };
         Ok(AssetSummary {
            id: "nlp/tokenizer/en".to_string(),
            revision_meta: meta,
            index_bytes: index.len() as u32,
         })
      }
      AssetOutcome::Unavailable { reason } => Err(napi::Error::from_reason(reason)),
   }
}

fn reason(e: impl std::fmt::Display) -> napi::Error {
   napi::Error::from_reason(e.to_string())
}
