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

use std::io::{Read, Write};
use std::path::Path;

use sha2::{Digest as _, Sha256};
use thiserror::Error;

/// Retrieves one locator's bytes. Implementations write body chunks
/// into `sink` as they arrive and return once the body is complete.
#[async_trait::async_trait]
pub trait Fetcher: Send + Sync {
   /// Stream the resource at `url` into `sink`. Any non-success
   /// response or transport failure is a [`FetchError`].
   ///
   /// The engine awaits this while holding the asset's acquisition
   /// slot, so an implementation **must** bound its own runtime — a
   /// connect and between-bytes timeout at minimum. A fetch that
   /// never returns wedges every request for that asset, including
   /// the offline fallback. The built-in `ReqwestFetcher` sets these
   /// deadlines; a custom fetcher is responsible for its own.
   async fn fetch(&self, url: &str, sink: &mut (dyn Write + Send)) -> Result<(), FetchError>;

   /// Whether this fetcher writes the file itself via
   /// [`fetch_to_path`](Fetcher::fetch_to_path) rather than streaming
   /// through [`fetch`](Fetcher::fetch). Override to `true` for a
   /// fetcher that owns the transfer — a native background or
   /// resumable downloader. Left `false`, the engine streams via
   /// `fetch` and hashes inline, in a single pass with no re-read.
   fn writes_to_path(&self) -> bool {
      false
   }

   /// Write the resource at `url` to `dest`, owning the transfer.
   /// Reached only when [`writes_to_path`](Fetcher::writes_to_path)
   /// returns `true`; the engine then verifies the landed file by
   /// re-reading it, so verification still stays on the engine's
   /// side. This is the seam for handing a download to a platform's
   /// native machinery (background transfer, resume, progress) — the
   /// bytes never stream through the process. The default streams via
   /// [`fetch`](Fetcher::fetch) into `dest`, a fallback for a fetcher
   /// that opts in without overriding this.
   async fn fetch_to_path(&self, url: &str, dest: &Path) -> Result<(), FetchError> {
      let mut file = std::fs::File::create(dest)
         .map_err(|e| FetchError::new(format!("cannot create {dest:?}: {e}")))?;
      self.fetch(url, &mut file).await
   }
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

/// The sink handed to a [`Fetcher`]: it forwards body chunks to the
/// engine's blocking writer over a bounded channel, so the file write
/// and the hashing both run off the async runtime. Backpressure is
/// the channel's bound; a dead receiver (the writer failed) surfaces
/// as a write error that stops the fetch.
pub(crate) struct ChannelSink {
   tx: std::sync::mpsc::SyncSender<Vec<u8>>,
}

impl ChannelSink {
   pub(crate) fn new(tx: std::sync::mpsc::SyncSender<Vec<u8>>) -> Self {
      ChannelSink { tx }
   }
}

impl Write for ChannelSink {
   fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
      self.tx.send(buf.to_vec()).map_err(|_| {
         std::io::Error::new(std::io::ErrorKind::BrokenPipe, "staging writer stopped")
      })?;
      Ok(buf.len())
   }

   fn flush(&mut self) -> std::io::Result<()> {
      Ok(())
   }
}

/// The engine's blocking-side sink: writes through to the staging file
/// while hashing every byte, so verification cannot be skipped by any
/// fetcher.
pub(crate) struct HashingSink<W: Write> {
   inner: W,
   hasher: Sha256,
}

/// SHA-256 a file already on disk, in one read pass. Used to verify a
/// file a [`Fetcher::fetch_to_path`] implementation wrote itself —
/// verification stays engine-side even when the transfer does not.
/// Blocking; call from the blocking pool.
pub(crate) fn hash_file(path: &Path) -> std::io::Result<[u8; 32]> {
   let mut file = std::fs::File::open(path)?;
   let mut hasher = Sha256::new();
   let mut buf = [0u8; 64 * 1024];
   loop {
      let n = file.read(&mut buf)?;
      if n == 0 {
         break;
      }
      hasher.update(&buf[..n]);
   }
   Ok(hasher.finalize().into())
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
