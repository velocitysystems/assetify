//! Reading delivered files: the forward-stream alias, the
//! positioned-read trait, and the backing a provider supplies so a
//! delivered file can be read as a stream, positioned access, or a
//! real path.

use std::io::{self, Read};
use std::path::Path;

/// Forward-only access: one pass over a file's bytes.
///
/// Deliberately plain [`std::io::Read`] — the provider may back it
/// with a file, a memory buffer, or a decoding stream; the consumer
/// cannot tell, which is the point.
pub type StreamAccess = Box<dyn Read + Send>;

/// Positioned access over a delivered file.
///
/// `&self` on [`read_at`](RandomAccess::read_at) is deliberate: a
/// consumer serving many threads from one shared object must not need
/// exclusive access to read (the shape of `FileExt::read_at`, not
/// `Seek + Read`). Backings over a raw file descriptor get this for
/// free; stateful backings synchronize internally.
pub trait RandomAccess: Send + Sync {
   /// Total length in bytes.
   fn len(&self) -> u64;

   /// True when the file is empty.
   fn is_empty(&self) -> bool {
      self.len() == 0
   }

   /// Read up to `buf.len()` bytes starting at `offset`, returning
   /// how many were read. Short reads are legal anywhere, not only at
   /// end of file — callers needing exact counts use
   /// [`read_at_exact`](RandomAccess::read_at_exact).
   fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize>;

   /// Zero-copy window over the whole file, if the backing keeps it
   /// addressable (a memory map returns its mapping; a heap backing
   /// may return its buffer). Returning `None` is always correct —
   /// consumers must run correctly, if slower, on
   /// [`read_at`](RandomAccess::read_at) alone. Meaningful for files
   /// probed in place over a long lifetime; consumers that convert
   /// everything they read to owned state gain nothing from a
   /// window.
   fn as_bytes(&self) -> Option<&[u8]> {
      None
   }

   /// Fill `buf` exactly, assembling short reads; the loop every
   /// consumer would otherwise write. Reaching a read of zero bytes
   /// before `buf` is full is `UnexpectedEof`.
   fn read_at_exact(&self, mut offset: u64, mut buf: &mut [u8]) -> io::Result<()> {
      while !buf.is_empty() {
         match self.read_at(offset, buf) {
            Ok(0) => {
               return Err(io::Error::new(
                  io::ErrorKind::UnexpectedEof,
                  "file ended before the requested range was read",
               ));
            }
            Ok(n) => {
               offset += n as u64;
               buf = &mut buf[n..];
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
         }
      }
      Ok(())
   }
}

/// How a provider serves the read modes of one delivered file: a
/// forward stream, positioned access, and — when it has one on disk —
/// a real path. The consumer reaches these through
/// [`PreparedFile`](crate::PreparedFile) and picks a mode at read
/// time; a provider that holds bytes in memory simply has no path.
///
/// Most providers hand back a file on disk and never implement this
/// directly — [`PreparedFile::from_path`](crate::PreparedFile::from_path)
/// supplies a filesystem backing. Implement it to serve a delivered
/// file from a source of your own.
pub trait FileBacking: Send + Sync {
   /// A real filesystem path for the file, when the provider has one.
   /// `None` for a backing that holds its bytes in memory, so a
   /// consumer that needs a path must fall back or use a
   /// filesystem-backed provider.
   fn path(&self) -> Option<&Path> {
      None
   }

   /// Open a fresh forward reader over the file.
   fn open_stream(&self) -> io::Result<StreamAccess>;

   /// Open positioned access over the file.
   fn open_random(&self) -> io::Result<Box<dyn RandomAccess>>;
}
