//! The boundary contract: everything that crosses between a consumer
//! and its asset provider, and nothing that doesn't.
//!
//! The consumer **declares** (which assets, which files, what access);
//! the provider **prepares** (acquire, verify, place, choose
//! backings). No storage path and no revision choice crosses the
//! seam: which revision serves is wholly the provider's business.

pub mod access;
pub mod delivery;
pub mod provider;
pub mod request;

pub use access::{FileBacking, RandomAccess, StreamAccess};
pub use delivery::{AssetResponse, DeliveryReceipt, PreparedAsset, PreparedFile};
pub use provider::Provider;
pub use request::{AssetRequest, RejectedDelivery};
