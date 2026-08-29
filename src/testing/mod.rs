//! Test doubles for consumers (feature `test-util`): exercise your
//! loading code against an in-memory [`Provider`](crate::Provider)
//! with no filesystem or network, across every window mode.

pub mod memory;

pub use memory::{MemoryAsset, MemoryProvider, WindowMode};
