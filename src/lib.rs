//! Portable asset access for Rust.
//!
//! A consumer — typically a library embedding data files it does not
//! ship (ML models, dictionaries, structured data) — **declares** what
//! it needs: which assets, which files each must contain, and how each
//! file will be read. A [`Provider`] **prepares**: acquires the bytes
//! however its world works, and hands every file back behind an access
//! object of the declared kind. No storage path and no revision choice
//! crosses the seam.
//!
//! # Access kinds are intent, not mechanism
//!
//! Each requested file carries an [`AccessKind`] naming the shape it
//! will be read in — forward ([`Stream`](AccessKind::Stream)),
//! positioned ([`Random`](AccessKind::Random)), or by real path
//! ([`MaterializedPath`](AccessKind::MaterializedPath)). The provider
//! chooses the backing: heap, memory map, plain file descriptor, or a
//! real path. See [`AccessKind`] for the first-match rule for picking
//! one.
//!
//! # The window contract
//!
//! [`RandomAccess`] has one required read operation and one optional
//! accelerator: [`read_at`](RandomAccess::read_at) is the correctness
//! floor every backing can serve, and
//! [`as_bytes`](RandomAccess::as_bytes) is a zero-copy window a
//! backing *may* offer when it already keeps the whole file
//! addressable. Returning `None` is always correct, so consumers must
//! run — if slower — on `read_at` alone. This is what lets the backing
//! decision (and its memory-accounting consequences) stay with the
//! provider.
//!
//! # Degraded operation
//!
//! A missing asset is never an error. [`AssetOutcome::Unavailable`]
//! degrades one capability; the consumer keeps running at whatever
//! level its loaded assets allow, and a later request retries. A
//! delivery the consumer *could not load* is echoed back as a
//! [`RejectedDelivery`] so a cached copy is re-acquired rather than
//! re-served.
//!
//! # Testing consumers
//!
//! Enable the `test-util` feature for
//! [`testing::MemoryProvider`] — an in-memory [`Provider`] that serves
//! declared files from heap buffers under a switchable
//! [`WindowMode`](testing::WindowMode), so consumer code can prove it
//! behaves identically whether or not a backing offers the window.

pub mod access;
mod contract;
#[cfg(feature = "test-util")]
pub mod testing;

pub use contract::{
   AccessKind, AssetOutcome, AssetRequest, FileAccess, FileSpec, MaterializedPath, PreparedAsset,
   PreparedFile, Provider, RandomAccess, RejectedDelivery, StreamAccess,
};
