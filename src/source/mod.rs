//! The acquisition seam: where the embedding application says *where
//! asset bytes live right now*.
//!
//! Assetify owns download, verification, placement, and cache
//! serving; the one thing it cannot know is which URL (or local file)
//! currently holds each asset for *this* application's distribution
//! channel. A [`Resolver`] answers exactly that question — no
//! manifest wire format, no versioning negotiation, just "asset `id`
//! is available as revision `r`, from these per-file locations, with
//! these digests."

#[cfg(feature = "zip")]
pub(crate) mod archive;
pub mod fetch;
pub mod local;
pub mod policy;
#[cfg(feature = "reqwest")]
pub mod reqwest;
pub mod static_resolver;

use std::path::PathBuf;

use thiserror::Error;

use crate::digest::{Digest, InvalidDigest};

/// Where one file's bytes can be acquired from.
///
/// Non-exhaustive so acquisition methods can arrive without breakage.
/// (Authentication needs no new variant: supply a
/// [`Fetcher`](crate::Fetcher) that adds credentials.)
#[non_exhaustive]
#[derive(Clone, Debug)]
pub enum Locator {
   /// A URL, retrieved through the configured
   /// [`Fetcher`](crate::Fetcher) — HTTP(S) via the built-in reqwest
   /// fetcher (`reqwest` feature), or any scheme your own fetcher
   /// understands. The URL is opaque to the engine.
   Url(String),
   /// A file already on the local filesystem, copied in and verified
   /// exactly like a download.
   File(PathBuf),
}

/// What the acquired bytes are: the file itself, or an archive to
/// extract. Set via [`FileSource::extracted`]; internal to the engine.
#[derive(Clone, Debug)]
pub(crate) enum Payload {
   File,
   Archive(ArchiveFormat),
}

/// Archive formats assetify can extract. Non-exhaustive so formats
/// can arrive without breakage.
#[non_exhaustive]
#[derive(Clone, Copy, Debug)]
pub enum ArchiveFormat {
   /// A zip archive, extracted with the `zip` feature enabled.
   Zip,
}

/// One file of an asset revision: its delivered name, where its bytes
/// live, and the digest they must hash to.
///
/// Non-exhaustive to match [`AssetSource`]: a per-file option can be
/// added without breaking construction through the constructors.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct FileSource {
   /// The name consumers request the file by (one path segment). For
   /// an archive payload this names the archive in diagnostics only —
   /// consumers request the *extracted* files by their own names.
   pub name: String,
   /// Where the bytes live.
   pub locator: Locator,
   /// What the bytes must hash to, verified before placement.
   pub digest: Digest,
   /// What the bytes are — a plain file (the default) or an archive
   /// to extract. Engine mechanism; set via
   /// [`extracted`](FileSource::extracted), not read by consumers.
   pub(crate) payload: Payload,
}

impl FileSource {
   /// One file's source.
   pub fn new(name: impl Into<String>, locator: Locator, digest: Digest) -> Self {
      FileSource {
         name: name.into(),
         locator,
         digest,
         payload: Payload::File,
      }
   }

   /// A file retrieved from a URL — HTTP(S) by default — verified
   /// against a SHA-256 given as 64 hex characters. The one-line
   /// spelling of the common case.
   pub fn url(
      name: impl Into<String>,
      url: impl Into<String>,
      sha256_hex: &str,
   ) -> Result<Self, InvalidDigest> {
      Ok(FileSource::new(
         name,
         Locator::Url(url.into()),
         Digest::sha256_hex(sha256_hex)?,
      ))
   }

   /// Mark the source's bytes as an archive to extract into the
   /// revision, rather than a file to place. Composes with any
   /// locator: a downloaded archive (`FileSource::url(..)`) and a
   /// local one (`FileSource::local(..)`) verify and extract
   /// identically.
   pub fn extracted(mut self, format: ArchiveFormat) -> Self {
      self.payload = Payload::Archive(format);
      self
   }

   /// A file copied from the local filesystem, verified against a
   /// SHA-256 given as 64 hex characters.
   pub fn local(
      name: impl Into<String>,
      path: impl Into<PathBuf>,
      sha256_hex: &str,
   ) -> Result<Self, InvalidDigest> {
      Ok(FileSource::new(
         name,
         Locator::File(path.into()),
         Digest::sha256_hex(sha256_hex)?,
      ))
   }
}

/// Everything needed to acquire one asset revision. Construct with
/// [`AssetSource::new`] — the struct is non-exhaustive so later
/// additions (an archive payload, say) stay non-breaking.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct AssetSource {
   /// The revision these files constitute: one path segment,
   /// lexicographically ordered against its siblings (a `YYYYMMDD`
   /// date stamp sorts correctly by construction; so does a
   /// zero-padded counter). Newest wins within an asset.
   pub revision: String,
   /// Every file of the revision. Acquisition is all-or-nothing: if
   /// any file fails to fetch or verify, nothing is placed.
   pub files: Vec<FileSource>,
}

impl AssetSource {
   /// A source naming one revision and its files.
   pub fn new(revision: impl Into<String>, files: Vec<FileSource>) -> Self {
      AssetSource {
         revision: revision.into(),
         files,
      }
   }
}

/// Resolution failed *right now* — the network is down, a catalog
/// could not be read. Assetify falls back to the newest revision
/// already on disk; the message only surfaces in `Unavailable`
/// reasons when there is nothing to fall back to.
#[derive(Debug, Error)]
#[error("{message}")]
pub struct ResolveError {
   message: String,
}

impl ResolveError {
   /// A resolution failure with a human-readable explanation.
   pub fn new(message: impl Into<String>) -> Self {
      ResolveError {
         message: message.into(),
      }
   }
}

/// The application's answer to "where can this asset be acquired?".
///
/// `Ok(None)` means this resolver knows of no source — assetify
/// serves what the cache holds, or reports the asset unavailable.
/// `Err` means resolution failed *this time* (offline, say) — same
/// fallback, different reason. Implementations should be fast or
/// cache their own lookups: a resolver is consulted on every request
/// for an asset that is not already being acquired.
#[async_trait::async_trait]
pub trait Resolver: Send + Sync {
   /// Where asset `id` can currently be acquired.
   async fn resolve(&self, id: &str) -> Result<Option<AssetSource>, ResolveError>;
}

#[cfg(test)]
mod tests {
   use super::*;

   const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

   #[test]
   fn convenience_constructors_build_the_common_locators() {
      let http = FileSource::url("model.bin", "https://example.com/m", EMPTY_SHA256).unwrap();
      assert!(matches!(http.locator, Locator::Url(_)));

      let local = FileSource::local("model.bin", "/tmp/m", EMPTY_SHA256).unwrap();
      assert!(matches!(local.locator, Locator::File(_)));

      assert!(FileSource::url("model.bin", "u", "not-hex").is_err());
   }
}
