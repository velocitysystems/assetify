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
//! A missing asset is never an error. [`AssetResponse::Unavailable`]
//! degrades one capability; the consumer keeps running at whatever
//! level its loaded assets allow, and a later request retries. A
//! delivery the consumer *could not load* is echoed back as a
//! [`RejectedDelivery`] so a cached copy is re-acquired rather than
//! re-served.
//!
//! # Quick start
//!
//! ```no_run
//! use assetify::{
//!    AccessKind, AssetRequest, AssetSource, Assetify, FileSource, FileSpec, StaticResolver,
//! };
//!
//! # async fn demo() -> Result<(), Box<dyn std::error::Error>> {
//! let source = AssetSource::new(
//!    "20260821",
//!    vec![FileSource::http(
//!       "model.bin",
//!       "https://assets.example.com/tokenizer/20260821/model.bin",
//!       "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
//!    )?],
//! );
//!
//! let engine = Assetify::builder("/var/cache/my-app/assets")
//!    .resolver(StaticResolver::new([("nlp/tokenizer/en", 1, source)]))
//!    .build()?;
//!
//! let outcome = engine
//!    .asset(AssetRequest::new(
//!       "nlp/tokenizer/en",
//!       1,
//!       vec![FileSpec::new("model.bin", AccessKind::Random)],
//!    ))
//!    .await;
//! # Ok(())
//! # }
//! ```
//!
//! Omit the resolver for **cache-only mode**: the engine serves what
//! the root already holds (a read-only root is fine — assets bundled
//! into a deployment are served in place).
//!
//! # Versioning: one hard axis, one soft
//!
//! A request names a `format_major` — the payload format lane this
//! consumer build can read. That axis is hard: lanes are never
//! crossed, so an upgraded application is never handed a payload it
//! cannot parse, and the cache keys revisions inside their lane
//! (`<root>/<id>/v<lane>/<revision>/`). Which *revision* serves is
//! soft and wholly the provider's: prefer what the resolver names,
//! fall back to the newest verified revision on disk when
//! acquisition fails — an offline device keeps working on what it
//! has rather than refusing service over staleness.
//!
//! # Embedding
//!
//! **Serverless (read-only filesystems):** point the cache root at
//! the writable scratch area (`/tmp` on AWS Lambda) with the `http`
//! feature, or bundle assets into the deployment and run cache-only
//! over the bundle directory.
//!
//! **Node.js (napi-rs):** assetify is a plain library on tokio; call
//! it from `#[napi]` async functions. Access objects stay on the Rust
//! side — only serializable results cross the JS bridge. Expose
//! long-running probing (memory-mapped `Random` files) through
//! *async* exports so page faults never stall the event loop, and
//! prefer an explicit `dispose()` on wrapper objects over relying on
//! the JS garbage collector to drop Rust resources.
//!
//! # Testing consumers
//!
//! Enable the `test-util` feature for
//! [`testing::MemoryProvider`] — an in-memory [`Provider`] that serves
//! declared files from heap buffers under a switchable
//! [`WindowMode`](testing::WindowMode), so consumer code can prove it
//! behaves identically whether or not a backing offers the window.

#![allow(clippy::upper_case_acronyms)]

pub mod access;
mod contract;
mod digest;
mod engine;
mod error;
mod source;
mod store;
#[cfg(feature = "test-util")]
pub mod testing;

pub use contract::{
   AccessKind, AssetRequest, AssetResponse, FileAccess, FileSpec, MaterializedPath, PreparedAsset,
   PreparedFile, Provider, RandomAccess, RejectedDelivery, StreamAccess,
};
pub use digest::{Digest, InvalidDigest};
pub use engine::{Assetify, AssetifyBuilder};
pub use error::AssetifyError;
pub use source::static_resolver::StaticResolver;
pub use source::{AssetSource, FileSource, Locator, ResolveError, SourceResolver};
