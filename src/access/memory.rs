//! Heap backing: the whole file owned in memory.
//!
//! The resident backing on builds without the `mmap` feature, and a
//! convenient backing for tests. Offers the zero-copy window, since
//! the bytes are already addressable.

use std::io;
use std::sync::Arc;

use crate::contract::access::RandomAccess;

/// [`RandomAccess`] over an owned (or shared) byte buffer.
pub struct MemoryRandom {
   bytes: Arc<Vec<u8>>,
}

impl MemoryRandom {
   /// Back a file with an already-shared buffer, avoiding a copy.
   pub(crate) fn from_shared(bytes: Arc<Vec<u8>>) -> Self {
      MemoryRandom { bytes }
   }
}

impl RandomAccess for MemoryRandom {
   fn len(&self) -> u64 {
      self.bytes.len() as u64
   }

   fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
      let len = self.bytes.len() as u64;
      if offset >= len {
         return Ok(0);
      }
      let start = offset as usize;
      let n = buf.len().min(self.bytes.len() - start);
      buf[..n].copy_from_slice(&self.bytes[start..start + n]);
      Ok(n)
   }

   fn as_bytes(&self) -> Option<&[u8]> {
      Some(&self.bytes)
   }
}

#[cfg(test)]
mod tests {
   use super::*;
   use crate::access::conformance::{PAYLOAD, assert_conformant};

   #[test]
   fn heap_backing_conforms_and_offers_the_window() {
      let backing = MemoryRandom::from_shared(Arc::new(PAYLOAD.to_vec()));
      assert_conformant(&backing);
      assert!(
         backing.as_bytes().is_some(),
         "heap backing offers the window"
      );
   }
}
