//! Cache-only mode: serve assets from a directory that already holds
//! them — the shape of a serverless function serving assets bundled
//! into its deployment package, or a test running over fixtures.
//!
//! Run with: `cargo run --example local_assets`

use std::io::Read as _;

use assetify::{AccessKind, AssetOutcome, AssetRequest, Assetify, FileAccess, FileSpec};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
   // Show the engine's lifecycle events (staging, delivery, fallback).
   tracing_subscriber::fmt()
      .with_max_level(tracing::Level::INFO)
      .without_time()
      .with_target(false)
      .compact()
      .init();

   // A pre-seeded tree, laid out as <root>/<id>/v<lane>/<revision>/.
   let root = tempfile::tempdir()?;
   let revision = root.path().join("nlp/tokenizer/en/v1/20260821");
   std::fs::create_dir_all(&revision)?;
   std::fs::write(revision.join("meta.json"), br#"{"format":1}"#)?;
   std::fs::write(revision.join("index.dat"), b"positioned bytes")?;
   std::fs::write(revision.join("rules.txt"), b"rule one")?;

   // No resolver: cache-only. A read-only root works too.
   let engine = Assetify::builder(root.path()).build()?;

   let outcome = engine
      .asset(AssetRequest::new(
         "nlp/tokenizer/en",
         1,
         vec![
            FileSpec::new("meta.json", AccessKind::Stream),
            FileSpec::new("index.dat", AccessKind::Random),
            FileSpec::new("rules.txt", AccessKind::MaterializedPath),
         ],
      ))
      .await;

   let AssetOutcome::Available { mut asset } = outcome else {
      panic!("the seeded tree serves");
   };

   let FileAccess::Stream(mut stream) = asset.take_file("meta.json").unwrap().access else {
      unreachable!()
   };
   let mut meta = String::new();
   stream.read_to_string(&mut meta)?;
   tracing::info!(content = %meta, "streamed meta.json");

   let FileAccess::Random(index) = asset.take_file("index.dat").unwrap().access else {
      unreachable!()
   };
   let mut word = [0u8; 5];
   index.read_at_exact(11, &mut word)?;
   tracing::info!(
      content = %String::from_utf8_lossy(&word),
      "ranged read from index.dat"
   );

   let FileAccess::Path(path) = asset.take_file("rules.txt").unwrap().access else {
      unreachable!()
   };
   tracing::info!(path = %path.display(), "materialized path");
   Ok(())
}
