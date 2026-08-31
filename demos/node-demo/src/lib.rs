//! assetify behind a napi-rs addon: JavaScript asks for assets, every
//! read happens on the Rust side, and only a serializable summary
//! crosses the JS bridge.

use std::io::Read as _;

use assetify::{AssetRequest, AssetResponse, Assetify, Provider};
use napi_derive::napi;

/// What crosses the bridge: names and derived values, never bytes or
/// handles.
#[napi(object)]
pub struct AssetSummary {
   pub id: String,
   pub language: String,
   pub vocab_words: u32,
   pub index_entries: u32,
   pub sample_tokens: Vec<String>,
   pub consistent: bool,
}

/// Serve the shared demo asset from `assets_root` — cache-only mode:
/// the committed fixture tree is the cache, read-only, served in
/// place. Runs on the napi-managed tokio runtime; the returned
/// promise resolves in JS.
#[napi]
pub async fn load_asset(assets_root: String) -> napi::Result<AssetSummary> {
   let engine = Assetify::builder(&assets_root).build().map_err(reason)?;
   let request = AssetRequest::new("tokenizer/en", ["meta.json", "index.dat", "vocab.txt"]);

   let asset = match engine.asset(request).await {
      AssetResponse::Available { asset } => asset,
      AssetResponse::Unavailable { reason } => return Err(napi::Error::from_reason(reason)),
   };

   // Stream: one forward parse of the model card.
   let mut card = String::new();
   asset
      .file("meta.json")
      .unwrap()
      .stream()
      .map_err(reason)?
      .read_to_string(&mut card)
      .map_err(reason)?;
   let meta: serde_json::Value = serde_json::from_str(&card).map_err(reason)?;
   let language = meta["language"].as_str().unwrap_or("unknown").to_string();
   let declared = meta["vocabSize"].as_u64().unwrap_or(0) as u32;

   // Path: read the vocabulary by real path, the way a tokenizer
   // library opening its own files would.
   let vocab_path = asset
      .file("vocab.txt")
      .unwrap()
      .path()
      .expect("a filesystem path");
   let vocab = std::fs::read_to_string(vocab_path).map_err(reason)?;
   let vocab_words = vocab.lines().count() as u32;

   // Random: decode the index header, then look tokens up through it
   // — a positioned read per entry, never a scan.
   let index = asset.file("index.dat").unwrap().random().map_err(reason)?;
   let mut header = [0u8; 8];
   index.read_at_exact(0, &mut header).map_err(reason)?;
   if &header[0..4] != b"AIDX" {
      return Err(napi::Error::from_reason("index.dat: invalid header"));
   }
   let entries = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
   if entries == 0 {
      return Err(napi::Error::from_reason("index.dat: empty index"));
   }

   let mut sample_tokens = Vec::new();
   // Entries picked for recognizable words in this revision.
   for entry in [320u32, 643, 810] {
      let mut raw = [0u8; 4];
      index
         .read_at_exact(8 + u64::from(entry) * 4, &mut raw)
         .map_err(reason)?;
      let offset = u32::from_le_bytes(raw) as usize;
      let token = vocab[offset..].lines().next().unwrap_or("").to_string();
      sample_tokens.push(token);
   }

   let consistent =
      vocab_words == declared && entries == declared && sample_tokens.iter().all(|t| !t.is_empty());
   Ok(AssetSummary {
      id: "tokenizer/en".to_string(),
      language,
      vocab_words,
      index_entries: entries,
      sample_tokens,
      consistent,
   })
}

fn reason(e: impl std::fmt::Display) -> napi::Error {
   napi::Error::from_reason(e.to_string())
}
