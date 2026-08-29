//! Memory-mapped backing: the window-offering backing.
//!
//! The right default for files probed in place: mapped pages are
//! clean and evict for free, so a large index costs near nothing
//! while idle, and [`RandomAccess::as_bytes`] gives consumers the
//! zero-copy path.

use std::fs::File;
use std::io;
use std::path::Path;

use memmap2::Mmap;

use crate::contract::access::RandomAccess;

/// [`RandomAccess`] over a memory map, offering the whole file as a
/// zero-copy window.
pub struct MmapRandom {
   /// `None` for an empty file — mapping zero bytes is an error on
   /// some platforms, and an empty window serves it exactly as well.
   map: Option<Mmap>,
}

impl MmapRandom {
   /// Map a file for positioned reads.
   ///
   /// Safety contract (the caller's to keep, and exactly why
   /// placement belongs to the provider): the mapped file must not be
   /// truncated or rewritten in place while mapped. The store writes
   /// each revision to a fresh directory and never mutates a placed
   /// one, so its mappings are stable by construction.
   pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
      let file = File::open(path)?;
      if file.metadata()?.len() == 0 {
         return Ok(MmapRandom { map: None });
      }
      let map = unsafe { Mmap::map(&file)? };
      Ok(MmapRandom { map: Some(map) })
   }

   fn bytes(&self) -> &[u8] {
      self.map.as_deref().unwrap_or(&[])
   }
}

impl RandomAccess for MmapRandom {
   fn len(&self) -> u64 {
      self.bytes().len() as u64
   }

   fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
      let bytes = self.bytes();
      if offset >= bytes.len() as u64 {
         return Ok(0);
      }
      let start = offset as usize;
      let n = buf.len().min(bytes.len() - start);
      buf[..n].copy_from_slice(&bytes[start..start + n]);
      Ok(n)
   }

   fn as_bytes(&self) -> Option<&[u8]> {
      Some(self.bytes())
   }
}
