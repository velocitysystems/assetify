//! Plain-file backing: positioned reads straight off an open file
//! descriptor, and no zero-copy window (`as_bytes` stays `None`) — the
//! minimal correct backing, and the crate's own proof the window is
//! optional.

use std::fs::File;
use std::io;
use std::path::Path;

use crate::contract::access::RandomAccess;

/// [`RandomAccess`] over an open file descriptor.
///
/// Both platforms' positioned-read primitives take `&self`, so the
/// trait's shared-reference contract is free here — no locking.
pub struct FileRandom {
   file: File,
   len: u64,
}

impl FileRandom {
   /// Open a file for positioned reads. The length is captured at
   /// open; the file is expected not to change while served, which
   /// the store guarantees by never mutating a placed revision.
   pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
      let file = File::open(path)?;
      let len = file.metadata()?.len();
      Ok(FileRandom { file, len })
   }
}

impl RandomAccess for FileRandom {
   fn len(&self) -> u64 {
      self.len
   }

   fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
      #[cfg(unix)]
      {
         std::os::unix::fs::FileExt::read_at(&self.file, buf, offset)
      }
      #[cfg(windows)]
      {
         // Moves the file cursor as a side effect; harmless because
         // this backing never uses the cursor.
         std::os::windows::fs::FileExt::seek_read(&self.file, buf, offset)
      }
   }

   // as_bytes: default `None`. Nothing is kept addressable.
}

#[cfg(test)]
mod tests {
   use super::*;
   use crate::access::conformance::{PAYLOAD, assert_conformant};

   #[test]
   fn plain_file_backing_conforms_without_a_window() {
      let dir = tempfile::tempdir().unwrap();
      let path = dir.path().join("payload.bin");
      std::fs::write(&path, PAYLOAD).unwrap();

      let backing = FileRandom::open(&path).unwrap();
      assert_conformant(&backing);
      assert!(
         backing.as_bytes().is_none(),
         "plain-file backing keeps nothing addressable"
      );
   }
}
