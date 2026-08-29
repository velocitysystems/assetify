//! Construction-time errors.
//!
//! Deliberately small: the boundary itself never returns these —
//! per-asset failures travel as
//! [`AssetOutcome::Unavailable`](crate::AssetOutcome::Unavailable)
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
}
