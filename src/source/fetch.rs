//! The fetch seam: how bytes for a [`Locator::Url`] source are
//! retrieved.
//!
//! A [`Fetcher`] handles transport only: it streams one URL's
//! response body into a sink the engine controls. Verification never
//! crosses this seam — the sink hashes as bytes arrive, and the
//! digest check, staging, and atomic placement all stay on the
//! engine's side, so no fetcher implementation can weaken them.
//!
//! Ships with [`ReqwestFetcher`](crate::ReqwestFetcher) behind the
//! `reqwest` feature (wired automatically). Supply your own
//! implementation via
//! [`AssetifyBuilder::fetcher`](crate::AssetifyBuilder::fetcher) to
//! use a different client, add authentication, or fetch by a scheme
//! that isn't HTTP at all — the URL string is opaque to the engine.
//!
//! [`Locator::Url`]: crate::Locator::Url

use std::io::Write;

use sha2::{Digest as _, Sha256};
use thiserror::Error;

/// Retrieves one locator's bytes. Implementations write body chunks
/// into `sink` as they arrive and return once the body is complete.
#[async_trait::async_trait]
pub trait Fetcher: Send + Sync {
   /// Stream the resource at `url` into `sink`. Any non-success
   /// response or transport failure is a [`FetchError`].
   async fn fetch(&self, url: &str, sink: &mut (dyn Write + Send)) -> Result<(), FetchError>;
}

/// A fetch failed: transport error, non-success status, or a sink
/// write failure.
#[derive(Debug, Error)]
#[error("{message}")]
pub struct FetchError {
   message: String,
}

impl FetchError {
   /// A fetch failure with a human-readable explanation.
   pub fn new(message: impl Into<String>) -> Self {
      FetchError {
         message: message.into(),
      }
   }
}

/// The engine's sink: writes through to the staging file while
/// hashing every byte, so verification cannot be skipped by any
/// fetcher.
pub(crate) struct HashingSink<W: Write> {
   inner: W,
   hasher: Sha256,
}

impl<W: Write> HashingSink<W> {
   pub(crate) fn new(inner: W) -> Self {
      HashingSink {
         inner,
         hasher: Sha256::new(),
      }
   }

   /// Flush and return the SHA-256 of everything written.
   pub(crate) fn finish(mut self) -> std::io::Result<[u8; 32]> {
      self.inner.flush()?;
      Ok(self.hasher.finalize().into())
   }
}

impl<W: Write> Write for HashingSink<W> {
   fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
      let n = self.inner.write(buf)?;
      self.hasher.update(&buf[..n]);
      Ok(n)
   }

   fn flush(&mut self) -> std::io::Result<()> {
      self.inner.flush()
   }
}

#[cfg(test)]
mod tests {
   use super::*;

   #[test]
   fn hashing_sink_hashes_exactly_what_it_writes() {
      let mut buffer = Vec::new();
      let mut sink = HashingSink::new(&mut buffer);
      sink.write_all(b"payload bytes").unwrap();
      let digest = sink.finish().unwrap();

      assert_eq!(buffer, b"payload bytes");
      let expected: [u8; 32] = Sha256::digest(b"payload bytes").into();
      assert_eq!(digest, expected);
   }
}
