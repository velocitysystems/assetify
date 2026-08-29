//! Download, verify, cache, serve — against a local mock server so
//! the example is self-contained.
//!
//! Run with: `cargo run --example http_assets --features http`

use sha2::Digest as _;

use assetify::{
   AccessKind, AssetOutcome, AssetRequest, AssetSource, Assetify, Digest, FileAccess, FileSource,
   FileSpec, Locator, StaticResolver,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
   // Show the engine's lifecycle events (staging, delivery, fallback).
   tracing_subscriber::fmt()
      .with_max_level(tracing::Level::INFO)
      .without_time()
      .with_target(false)
      .compact()
      .init();

   // A stand-in for wherever your assets are published.
   let server = wiremock::MockServer::start().await;
   let body = b"model weights".to_vec();
   wiremock::Mock::given(wiremock::matchers::method("GET"))
      .and(wiremock::matchers::path("/20260821/model.bin"))
      .respond_with(wiremock::ResponseTemplate::new(200).set_body_bytes(body.clone()))
      .mount(&server)
      .await;

   let cache = tempfile::tempdir()?;
   let engine = Assetify::builder(cache.path())
      .resolver(StaticResolver::new([(
         "models/sentiment",
         1,
         AssetSource::new(
            "20260821",
            vec![FileSource::new(
               "model.bin",
               Locator::HTTP {
                  url: format!("{}/20260821/model.bin", server.uri()),
               },
               Digest::sha256_hex(&hex::encode(sha2::Sha256::digest(&body)))?,
            )],
         ),
      )]))
      .build()?;

   let outcome = engine
      .asset(AssetRequest::new(
         "models/sentiment",
         1,
         vec![FileSpec::new("model.bin", AccessKind::Random)],
      ))
      .await;

   match outcome {
      AssetOutcome::Available { asset } => {
         let FileAccess::Random(model) = &asset.file("model.bin").unwrap().access else {
            unreachable!()
         };
         tracing::info!(bytes = model.len(), "downloaded, verified, and cached");
      }
      AssetOutcome::Unavailable { reason } => tracing::warn!(%reason, "unavailable"),
   }

   // A second request never touches the network: the revision is
   // cached under <root>/models/sentiment/v1/20260821/.
   Ok(())
}
