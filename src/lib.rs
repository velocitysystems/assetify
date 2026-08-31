//! Portable asset access for Rust.
//!
//! A consumer — usually a library embedding data it does not ship (ML
//! models, dictionaries, structured data) — **declares** which assets
//! and files it needs; a [`Provider`] **prepares** them, acquiring the
//! bytes however its world works. Each [`PreparedFile`] is read in
//! whichever shape suits: a forward [`stream`](PreparedFile::stream),
//! positioned [`random`](PreparedFile::random) access, or its real
//! [`path`](PreparedFile::path) when the provider has one on disk. The
//! consumer picks the mode at read time; a mode the provider cannot
//! serve reports it (an in-memory provider's `path` is `None`). No
//! storage location and no revision choice crosses the seam.
//!
//! # The window contract
//!
//! [`RandomAccess`] requires only [`read_at`](RandomAccess::read_at);
//! [`as_bytes`](RandomAccess::as_bytes) is an optional zero-copy
//! window a backing may offer when it keeps the whole file
//! addressable. `None` is always correct, so consumers run — if
//! slower — on `read_at` alone. That is what lets the backing choice,
//! and its memory cost, stay with the provider.
//!
//! # Degraded operation
//!
//! A missing asset is never an error: [`AssetResponse::Unavailable`]
//! degrades one capability and a later request retries. A delivery the
//! consumer *could not load* is rejected ([`Assetify::reject`]) so the
//! cached copy is re-acquired rather than re-served.
//!
//! # Quick start
//!
//! ```no_run
//! use assetify::{AssetRequest, AssetSource, Assetify, FileSource, Provider, StaticResolver};
//!
//! # async fn demo() -> Result<(), Box<dyn std::error::Error>> {
//! let source = AssetSource::new(
//!    "20260821",
//!    vec![FileSource::url(
//!       "model.bin",
//!       "https://assets.example.com/tokenizer/20260821/model.bin",
//!       "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
//!    )?],
//! );
//!
//! let engine = Assetify::builder("/var/cache/my-app/assets")
//!    .resolver(StaticResolver::new([("tokenizer/en", source)]))
//!    .build()?;
//!
//! let outcome = engine.asset(AssetRequest::new("tokenizer/en", ["model.bin"])).await;
//! # Ok(())
//! # }
//! ```
//!
//! Omit the resolver for **cache-only mode** — the engine serves what
//! the root already holds (a read-only bundle is fine).
//!
//! # Versioning: the id is the compatibility boundary
//!
//! Offline fallback serves the newest revision on disk for an id, so
//! an offline device keeps working. If your payload format can change
//! incompatibly, put its major version in the id (`"tokenizer/en/v2"`,
//! via [`AssetRequest::versioned_id`]): incompatible payloads become
//! different assets with disjoint revision trees. Keep ids prefix-free
//! — never use an id that is a path-prefix of another.
//!
//! # Embedding
//!
//! **Serverless:** point the cache root at writable scratch (`/tmp` on
//! AWS Lambda) with the `reqwest` feature, or bundle assets and run
//! cache-only over them.
//!
//! **Node.js (napi-rs):** call it from `#[napi]` async functions; read
//! objects stay in Rust, only results cross the JS bridge. Expose
//! memory-mapped reads through *async* exports so page faults never
//! stall the event loop.
//!
//! # Testing consumers
//!
//! The `test-util` feature ships [`testing::MemoryProvider`], an
//! in-memory [`Provider`] with a switchable
//! [`WindowMode`](testing::WindowMode), so consumer code can prove it
//! behaves the same whether or not a backing offers the window.

mod access;
mod contract;
mod digest;
mod engine;
mod error;
mod source;
mod store;
#[cfg(feature = "test-util")]
pub mod testing;

pub use contract::{
   AssetRequest, AssetResponse, DeliveryReceipt, FileBacking, PreparedAsset, PreparedFile,
   Provider, RandomAccess, StreamAccess,
};
pub use digest::{Digest, InvalidDigest};
pub use engine::{Assetify, AssetifyBuilder};
pub use error::AssetifyError;
pub use source::fetch::{FetchError, Fetcher};
pub use source::policy::{Admission, FetchPolicy};
#[cfg(feature = "reqwest")]
pub use source::reqwest::ReqwestFetcher;
pub use source::static_resolver::StaticResolver;
pub use source::{ArchiveFormat, AssetSource, FileSource, Locator, ResolveError, Resolver};
