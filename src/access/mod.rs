//! Backings: concrete [`RandomAccess`](crate::RandomAccess)
//! implementations a provider chooses among per declared access kind.

pub mod memory;

pub use memory::MemoryRandom;
