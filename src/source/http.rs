//! Acquisition over HTTP(S): stream a [`Locator::HTTP`] source into
//! staging, hashing as the body arrives.
//!
//! One pass: chunks are hashed and written as they stream, so
//! verification costs no second read. The digest check itself happens
//! in the engine, before anything is placed — a corrupt body never
//! reaches the cache.
//!
//! [`Locator::HTTP`]: crate::Locator::HTTP

use std::path::Path;

use sha2::{Digest as _, Sha256};
use tokio::io::AsyncWriteExt as _;

/// GET `url` into `destination`, returning the SHA-256 of the body.
/// Any non-success status is an error naming the URL and status.
pub(crate) async fn fetch(
   client: &reqwest::Client,
   url: &str,
   destination: &Path,
) -> Result<[u8; 32], String> {
   let mut response = client
      .get(url)
      .send()
      .await
      .map_err(|e| format!("GET {url} failed: {e}"))?;
   let status = response.status();
   if !status.is_success() {
      return Err(format!("GET {url} returned {status}"));
   }

   let mut file = tokio::fs::File::create(destination)
      .await
      .map_err(|e| format!("cannot write staging file: {e}"))?;
   let mut hasher = Sha256::new();
   loop {
      let chunk = response
         .chunk()
         .await
         .map_err(|e| format!("GET {url} failed mid-body: {e}"))?;
      let Some(chunk) = chunk else { break };
      hasher.update(&chunk);
      file
         .write_all(&chunk)
         .await
         .map_err(|e| format!("cannot write staging file: {e}"))?;
   }
   file
      .flush()
      .await
      .map_err(|e| format!("cannot write staging file: {e}"))?;
   Ok(hasher.finalize().into())
}
