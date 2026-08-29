//! assetify inside a headless Tauri app: no windows, no webview —
//! assets load in the `setup` hook against the app's real cache
//! directory, exactly where a shipping app would warm them up.

use std::io::Read as _;
use std::path::Path;

use assetify::{AccessKind, AssetResponse, AssetRequest, Assetify, FileSpec};
use tauri::Manager as _;

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

         let engine = Assetify::builder(&cache_root).build()?;
         tauri::async_runtime::block_on(load(&engine));

         // Headless demo: done once assets are served. On mobile the
         // app stays alive (platforms dislike self-termination).
         #[cfg(desktop)]
         app.handle().exit(0);
         Ok(())
      })
      .run(tauri::generate_context!())
      .expect("error while running tauri application");
}

/// Seed the cache tree a shipping app would have downloaded earlier:
/// <root>/<id>/v<lane>/<revision>/<files>. Idempotent across runs.
fn seed(cache_root: &Path) -> std::io::Result<()> {
   let revision = cache_root.join("nlp/tokenizer/en/v1/20260821");
   std::fs::create_dir_all(&revision)?;
   std::fs::write(
      revision.join("meta.json"),
      r#"{"format":1,"language":"en"}"#,
   )?;
   std::fs::write(revision.join("index.dat"), "tokenizer index bytes")
}

async fn load(engine: &Assetify) {
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
         let mut stream = asset.take_stream("meta.json").expect("requested as a stream");
         let mut meta = String::new();
         stream.read_to_string(&mut meta).unwrap();

         let index = asset.take_random("index.dat").expect("requested as random access");
         tracing::info!(meta = %meta, index_bytes = index.len(), "assets loaded in the app shell");
      }
      AssetResponse::Unavailable { reason } => tracing::warn!(%reason, "unavailable"),
   }
}
