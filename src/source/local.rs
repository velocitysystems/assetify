//! Acquisition from the local filesystem: copy a [`Locator::File`]
//! source into staging, hashing as it streams.
//!
//! Local sources are verified exactly like downloads — the digest
//! seam does not care where bytes came from, only that they are the
//! bytes the resolver promised.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};

/// Copy `source` to `destination`, returning the SHA-256 of the bytes
/// copied. Runs the blocking IO on the runtime's blocking pool.
pub(crate) async fn fetch(source: &Path, destination: &Path) -> io::Result<[u8; 32]> {
   let source: PathBuf = source.to_path_buf();
   let destination: PathBuf = destination.to_path_buf();
   tokio::task::spawn_blocking(move || copy_and_hash(&source, &destination))
      .await
      .map_err(io::Error::other)?
}

fn copy_and_hash(source: &Path, destination: &Path) -> io::Result<[u8; 32]> {
   let mut reader = std::fs::File::open(source)?;
   let mut writer = std::fs::File::create(destination)?;
   let mut hasher = Sha256::new();
   let mut buf = [0u8; 64 * 1024];
   loop {
      let n = reader.read(&mut buf)?;
      if n == 0 {
         break;
      }
      hasher.update(&buf[..n]);
      writer.write_all(&buf[..n])?;
   }
   writer.flush()?;
   Ok(hasher.finalize().into())
}

#[cfg(test)]
mod tests {
   use super::*;

   #[tokio::test]
   async fn copies_bytes_and_reports_their_hash() {
      let dir = tempfile::tempdir().unwrap();
      let source = dir.path().join("source.bin");
      let destination = dir.path().join("dest.bin");
      std::fs::write(&source, b"payload bytes").unwrap();

      let computed = fetch(&source, &destination).await.unwrap();
      assert_eq!(std::fs::read(&destination).unwrap(), b"payload bytes");

      let expected: [u8; 32] = Sha256::digest(b"payload bytes").into();
      assert_eq!(computed, expected);
   }

   #[tokio::test]
   async fn a_missing_source_is_an_error() {
      let dir = tempfile::tempdir().unwrap();
      let missing = dir.path().join("absent.bin");
      let destination = dir.path().join("dest.bin");
      assert!(fetch(&missing, &destination).await.is_err());
   }
}
