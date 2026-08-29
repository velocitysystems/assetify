//! Construction-time errors.
//!
//! Deliberately small: the boundary itself never returns these —
//! per-asset failures travel as
//! [`AssetResponse::Unavailable`](crate::AssetResponse::Unavailable)
//! reasons, because consumers degrade uniformly rather than branch.
//! Only operations with no degraded answer (building the engine) get
//! a typed error.

use std::path::PathBuf;

use thiserror::Error;

/// Why an [`Assetify`](crate::Assetify) could not be built.
#[non_exhaustive]
#[derive(Debug, Error)]
pub enum AssetifyError {
   /// The cache root (or its staging area) could not be prepared.
   /// With a resolver configured the root must be writable; pair a
   /// read-only root with cache-only mode instead.
   #[error("cannot prepare cache root {root:?}: {source}")]
   CacheRoot {
      /// The root that could not be prepared.
      root: PathBuf,
      /// The underlying filesystem error.
      source: std::io::Error,
   },
   /// The default reqwest fetcher could not be constructed —
   /// typically a TLS backend initialization failure.
   #[cfg(feature = "reqwest")]
   #[error("cannot construct the default fetcher: {source}")]
   DefaultFetcher {
      /// The underlying client error.
      source: reqwest::Error,
   },
}
