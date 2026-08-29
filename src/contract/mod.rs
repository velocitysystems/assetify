//! The boundary contract: everything that crosses between a consumer
//! and its asset provider, and nothing that doesn't.
//!
//! The consumer **declares** (which assets, which files, what access);
//! the provider **prepares** (acquire, verify, place, choose
//! backings). No storage path and no revision choice crosses the
//! seam: versioning splits into a hard format lane the consumer
//! states per request and a soft revision that stays wholly the
//! provider's business.

pub mod access;
pub mod delivery;
pub mod provider;
pub mod request;

pub use access::{AccessKind, AssetPath, FileAccess, RandomAccess, StreamAccess};
pub use delivery::{AssetResponse, PreparedAsset, PreparedFile};
pub use provider::Provider;
pub use request::{AssetRequest, FileSpec, RejectedDelivery};
