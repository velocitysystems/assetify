//! Backings: concrete [`RandomAccess`](crate::RandomAccess)
//! implementations a provider chooses among per declared access kind.

pub mod file;
pub mod memory;
#[cfg(feature = "mmap")]
pub mod mmap;

pub use file::FileRandom;
pub use memory::MemoryRandom;
#[cfg(feature = "mmap")]
pub use mmap::MmapRandom;
