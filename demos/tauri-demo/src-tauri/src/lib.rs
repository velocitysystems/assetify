//! assetify inside a Tauri app: the engine lives in managed state,
//! the webview asks for assets over IPC, every read happens in Rust,
//! and only a serializable summary crosses into JavaScript.

use std::io::Read as _;
use std::path::Path;

use assetify::{AssetRequest, AssetResponse, Assetify, Provider};
use tauri::Manager as _;

/// The engine, built once in `setup` and shared with every command.
struct Engine(Assetify);

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
   tracing_subscriber::fmt()
      .without_time()
      .with_target(false)
      .compact()
      .init();

   tauri::Builder::default()
      .setup(|app| {
         // The real Tauri integration point: the app's cache
         // directory is assetify's cache root.
         let cache_root = app.path().app_cache_dir()?;
         seed(&cache_root)?;
         app.manage(Engine(Assetify::builder(&cache_root).build()?));
         Ok(())
      })
      .invoke_handler(tauri::generate_handler![load_assets])
      .run(tauri::generate_context!())
      .expect("error while running tauri application");
}

/// Seed the cache tree a shipping app would have downloaded earlier:
/// <root>/<id>/<revision>/<files>. The shared fixture tree at
/// `demos/assets` is compiled in, so the demo carries its assets onto
/// devices and simulators. Idempotent across runs.
fn seed(cache_root: &Path) -> std::io::Result<()> {
   const FILES: [(&str, &[u8]); 3] = [
      (
         "meta.json",
         include_bytes!("../../../assets/tokenizer/en/20260821/meta.json"),
      ),
      (
         "index.dat",
         include_bytes!("../../../assets/tokenizer/en/20260821/index.dat"),
      ),
      (
         "vocab.txt",
         include_bytes!("../../../assets/tokenizer/en/20260821/vocab.txt"),
      ),
   ];

   let revision = cache_root.join("tokenizer/en/20260821");
   std::fs::create_dir_all(&revision)?;
   for (name, bytes) in FILES {
      std::fs::write(revision.join(name), bytes)?;
   }
   Ok(())
}

/// The IPC boundary: the webview invokes this, the reads happen here,
/// and only the summary crosses back.
#[tauri::command]
async fn load_assets(engine: tauri::State<'_, Engine>) -> Result<serde_json::Value, String> {
   let request = AssetRequest::new(
      "tokenizer/en",
      [
         "meta.json",
         "index.dat",
         "vocab.txt",
      ],
   );

   let asset = match engine.0.asset(request).await {
      AssetResponse::Available { asset } => asset,
      AssetResponse::Unavailable { reason } => return Err(reason),
   };

   // Stream: one forward parse of the model card.
   let mut card = String::new();
   asset
      .file("meta.json")
      .unwrap()
      .stream()
      .map_err(|e| e.to_string())?
      .read_to_string(&mut card)
      .map_err(|e| e.to_string())?;
   let meta: serde_json::Value = serde_json::from_str(&card).map_err(|e| e.to_string())?;
   let language = meta["language"].as_str().unwrap_or("unknown").to_string();
   let declared = meta["vocabSize"].as_u64().unwrap_or(0) as u32;

   // Path: read the vocabulary by real path, the way a tokenizer
   // library opening its own files would.
   let vocab_path = asset.file("vocab.txt").unwrap().path().expect("a filesystem path");
   let vocab = std::fs::read_to_string(vocab_path).map_err(|e| e.to_string())?;
   let vocab_words = vocab.lines().count() as u32;

   // Random: decode the index header, then look tokens up through it
   // — a positioned read per entry, never a scan.
   let index = asset
      .file("index.dat")
      .unwrap()
      .random()
      .map_err(|e| e.to_string())?;
   let mut header = [0u8; 8];
   index
      .read_at_exact(0, &mut header)
      .map_err(|e| e.to_string())?;
   if &header[0..4] != b"AIDX" {
      return Err("index.dat: invalid header".to_string());
   }
   let entries = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
   if entries == 0 {
      return Err("index.dat: empty index".to_string());
   }

   let mut sample_tokens = Vec::new();
   // Entries picked for recognizable words in this revision.
   for entry in [320u32, 643, 810] {
      let mut raw = [0u8; 4];
      index
         .read_at_exact(8 + u64::from(entry) * 4, &mut raw)
         .map_err(|e| e.to_string())?;
      let offset = u32::from_le_bytes(raw) as usize;
      let token = vocab[offset..].lines().next().unwrap_or("").to_string();
      sample_tokens.push(token);
   }

   let consistent =
      vocab_words == declared && entries == declared && sample_tokens.iter().all(|t| !t.is_empty());
   tracing::info!(
      %language,
      vocab_words,
      index_entries = entries,
      consistent,
      "assets loaded and summarized for the webview"
   );
   Ok(serde_json::json!({
      "id": "tokenizer/en",
      "language": language,
      "vocabWords": vocab_words,
      "indexEntries": entries,
      "sampleTokens": sample_tokens,
      "consistent": consistent,
   }))
}
