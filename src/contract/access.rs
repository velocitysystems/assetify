//! Access kinds and access objects: how delivered files are read.
//!
//! An [`AccessKind`] is an **intent declaration**, not a mechanism. The
//! consumer states what it needs on two axes — *how long it holds the
//! file* (load-scoped vs. resident) and *what shape it reads it in*
//! (forward, positioned, path) — and the provider chooses the backing:
//! heap buffer, memory map, plain file descriptor, or a real path.
//!
//! That split matters because only the provider can weigh the backing
//! trade-offs. On memory-tight platforms, mapped pages are clean and
//! evict for free while heap copies are dirty and count against
//! process-kill thresholds; nothing in this contract lets a consumer
//! force the expensive choice on a platform that cannot afford it.

use std::io::{self, Read};
use std::ops::Deref;
use std::path::{Path, PathBuf};

/// What shape the consumer reads a delivered file in.
///
/// Pick with a first-match rule:
///
/// 1. Loading through a library that takes a filesystem path? →
///    [`AssetPath`](AccessKind::AssetPath)
/// 2. Seeking, ranged reads, or probing the file in place? →
///    [`Random`](AccessKind::Random)
/// 3. Otherwise → [`Stream`](AccessKind::Stream)
///
/// Don't care? Declaring `AssetPath` for everything is legal and
/// recovers a plain "give me paths" design; the finer kinds only pay
/// off when you opt in.
///
/// How long you hold an access object is your business — a ranged
/// pass at load time and a resident structure probed per query both
/// arrive as the same object. (`#[non_exhaustive]` leaves room for
/// finer, hold-duration-aware variants later without breaking
/// consumers.)
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessKind {
   /// One forward pass at load time; nothing stays addressable
   /// afterwards. Deliberately the one kind that never promises a
   /// file exists on disk, so a provider may serve it from any
   /// byte source.
   Stream,
   /// Positioned reads: byte ranges fetched during load, or a file
   /// probed in place for the consumer's lifetime — the case
   /// [`RandomAccess::as_bytes`] exists for.
   Random,
   /// The consumer needs a real file on the local filesystem — for
   /// example, to hand its path to a wrapped library that insists on
   /// opening files itself. The delivered path stays valid while the
   /// delivery is held.
   AssetPath,
}

/// Forward-only access: one pass, at load time.
///
/// Deliberately plain [`std::io::Read`] — the provider may back it
/// with a file, a memory buffer, or a decoding stream; the consumer
/// cannot tell, which is the point.
pub type StreamAccess = Box<dyn Read + Send>;

/// Positioned access, for [`AccessKind::Random`] files.
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

/// A real path on the local filesystem, delivered for
/// [`AccessKind::AssetPath`] files.
///
/// A newtype rather than a bare [`PathBuf`] so the validity promise
/// has a place to live: the path stays valid while this value (or the
/// delivery it came from) is held. Dereferences to [`Path`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetPath {
   path: PathBuf,
}

impl AssetPath {
   /// Wrap a path for delivery. Provider-side API; consumers receive
   /// these inside [`FileAccess::AssetPath`].
   pub fn new(path: impl Into<PathBuf>) -> Self {
      AssetPath { path: path.into() }
   }

   /// The path, borrowed.
   pub fn as_path(&self) -> &Path {
      &self.path
   }

   /// The path, owned. The validity promise ends with the delivery it
   /// came from, so prefer borrowing where you can.
   pub fn into_path_buf(self) -> PathBuf {
      self.path
   }
}

impl Deref for AssetPath {
   type Target = Path;

   fn deref(&self) -> &Path {
      &self.path
   }
}

impl AsRef<Path> for AssetPath {
   fn as_ref(&self) -> &Path {
      &self.path
   }
}

/// The provider's answer to one file's declared [`AccessKind`].
///
/// Non-exhaustive to match [`AccessKind`]: a finer access kind can
/// arrive with the backing that satisfies it, without breaking
/// consumers that match this.
#[non_exhaustive]
pub enum FileAccess {
   /// Satisfies [`AccessKind::Stream`].
   Stream(StreamAccess),
   /// Satisfies [`AccessKind::Random`].
   Random(Box<dyn RandomAccess>),
   /// Satisfies [`AccessKind::AssetPath`].
   AssetPath(AssetPath),
}

impl FileAccess {
   /// Whether this object satisfies the declared kind. A mismatch is
   /// a rejection at load, on the same channel as an unavailable
   /// asset.
   pub fn satisfies(&self, kind: AccessKind) -> bool {
      matches!(
         (self, kind),
         (FileAccess::Stream(_), AccessKind::Stream)
            | (FileAccess::Random(_), AccessKind::Random)
            | (FileAccess::AssetPath(_), AccessKind::AssetPath)
      )
   }
}

impl std::fmt::Debug for FileAccess {
   fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
      match self {
         FileAccess::Stream(_) => f.write_str("FileAccess::Stream(..)"),
         FileAccess::Random(r) => write!(f, "FileAccess::Random(len = {})", r.len()),
         FileAccess::AssetPath(p) => write!(f, "FileAccess::AssetPath({:?})", p.as_path()),
      }
   }
}
