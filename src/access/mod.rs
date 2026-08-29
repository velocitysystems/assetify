//! Backings: concrete [`RandomAccess`](crate::RandomAccess)
//! implementations a provider chooses among per declared access kind.
//!
//! Public deliberately, for one audience: implementors of
//! [`Provider`](crate::Provider) other than the built-in engine.
//! Consumers never construct backings — they receive them behind
//! [`FileAccess`](crate::FileAccess) — but a custom provider gets the
//! hard parts (positioned reads, the zero-copy window, the mmap
//! safety contract) ready-made instead of rebuilding them.

pub mod file;
pub mod memory;
#[cfg(feature = "mmap")]
pub mod mmap;

pub use file::FileRandom;
pub use memory::MemoryRandom;
#[cfg(feature = "mmap")]
pub use mmap::MmapRandom;
