//! Backings: concrete [`RandomAccess`](crate::RandomAccess)
//! implementations the engine reads delivered files through.
//!
//! Crate-internal on purpose. Consumers reach these through
//! [`PreparedFile`](crate::PreparedFile), never directly, and the
//! `unsafe` inside [`MmapRandom`](mmap::MmapRandom) is sound only
//! because the sole caller, the store, never mutates a placed
//! revision. Exposing the backings would hand that memory-safety
//! obligation to callers with no such guarantee, so they stay private.

// Each backing is compiled only where it is the one the engine (or
// the test-util provider) actually uses, so nothing dead ships.
#[cfg(not(feature = "mmap"))]
pub(crate) mod file;
#[cfg(feature = "test-util")]
pub(crate) mod memory;
#[cfg(feature = "mmap")]
pub(crate) mod mmap;

#[cfg(not(feature = "mmap"))]
pub(crate) use file::FileRandom;
#[cfg(feature = "test-util")]
pub(crate) use memory::MemoryRandom;
#[cfg(feature = "mmap")]
pub(crate) use mmap::MmapRandom;

/// Shared backing-conformance checks, so every [`RandomAccess`]
/// implementation is proven to behave identically window or no window.
#[cfg(test)]
pub(crate) mod conformance {
   use crate::contract::access::RandomAccess;

   pub(crate) const PAYLOAD: &[u8] = b"the quick brown fox jumps over the lazy dog";

   /// Every backing must pass this, regardless of how it answers
   /// `as_bytes`.
   pub(crate) fn assert_conformant(access: &dyn RandomAccess) {
      let len = PAYLOAD.len() as u64;
      assert_eq!(access.len(), len);
      assert!(!access.is_empty());

      // Reads past the end return zero bytes rather than erroring.
      let mut buf = [0u8; 8];
      assert_eq!(access.read_at(len, &mut buf).unwrap(), 0);
      assert_eq!(access.read_at(len + 100, &mut buf).unwrap(), 0);

      // A read straddling the end is truncated, never padded.
      let mut whole = vec![0u8; PAYLOAD.len() + 16];
      let mut filled = 0;
      let mut offset = 0u64;
      loop {
         let n = access.read_at(offset, &mut whole[filled..]).unwrap();
         if n == 0 {
            break;
         }
         filled += n;
         offset += n as u64;
      }
      assert_eq!(&whole[..filled], PAYLOAD);

      // read_at_exact assembles short reads, at any offset and in any
      // order.
      let mut tail = [0u8; 8];
      access.read_at_exact(len - 8, &mut tail).unwrap();
      assert_eq!(&tail, &PAYLOAD[PAYLOAD.len() - 8..]);
      let mut head = [0u8; 9];
      access.read_at_exact(0, &mut head).unwrap();
      assert_eq!(&head, &PAYLOAD[..9]);

      // read_at_exact refuses ranges the file cannot fill.
      let mut too_far = [0u8; 4];
      let err = access.read_at_exact(len - 2, &mut too_far).unwrap_err();
      assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);

      // The window is optional, but when offered it must be the file.
      if let Some(bytes) = access.as_bytes() {
         assert_eq!(bytes, PAYLOAD);
      }
   }
}
