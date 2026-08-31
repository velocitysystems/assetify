//! The delivery side: what comes back for each requested asset.

use std::io;
use std::path::{Path, PathBuf};

use crate::contract::access::{FileBacking, RandomAccess, StreamAccess};

/// One delivered file, matched to the request by name. Read it in
/// whichever shape you need — as a forward [`stream`](PreparedFile::stream),
/// as positioned [`random`](PreparedFile::random) access, or by its
/// real [`path`](PreparedFile::path) when the provider has one on
/// disk. A mode a provider cannot serve reports it rather than
/// guessing: `path` returns `None`, the openers return an error.
pub struct PreparedFile {
   name: String,
   backing: Box<dyn FileBacking>,
}

impl PreparedFile {
   /// One named file behind a provider-supplied backing.
   pub fn new(name: impl Into<String>, backing: impl FileBacking + 'static) -> Self {
      PreparedFile {
         name: name.into(),
         backing: Box::new(backing),
      }
   }

   /// One named file served from a real path on disk — the common
   /// case. Reads as a stream or positioned access open the file on
   /// demand, and [`path`](PreparedFile::path) returns it.
   pub fn from_path(name: impl Into<String>, path: impl Into<PathBuf>) -> Self {
      PreparedFile::new(name, PathBacking { path: path.into() })
   }

   /// The name the request asked for.
   pub fn name(&self) -> &str {
      &self.name
   }

   /// A real filesystem path, when the provider has one on disk. The
   /// path stays valid while this delivery is held. `None` for a
   /// provider serving from memory.
   pub fn path(&self) -> Option<&Path> {
      self.backing.path()
   }

   /// Open a forward reader over the file.
   pub fn stream(&self) -> io::Result<StreamAccess> {
      self.backing.open_stream()
   }

   /// Open positioned access over the file, with the zero-copy window
   /// when the backing offers one (see [`RandomAccess::as_bytes`]).
   pub fn random(&self) -> io::Result<Box<dyn RandomAccess>> {
      self.backing.open_random()
   }
}

impl std::fmt::Debug for PreparedFile {
   fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
      f.debug_struct("PreparedFile")
         .field("name", &self.name)
         .field("path", &self.backing.path())
         .finish()
   }
}

/// The filesystem backing behind [`PreparedFile::from_path`]: opens
/// the file on demand, choosing the memory-mapped positioned backing
/// when the `mmap` feature is on and a plain file descriptor
/// otherwise.
struct PathBacking {
   path: PathBuf,
}

impl FileBacking for PathBacking {
   fn path(&self) -> Option<&Path> {
      Some(&self.path)
   }

   fn open_stream(&self) -> io::Result<StreamAccess> {
      Ok(Box::new(std::fs::File::open(&self.path)?))
   }

   fn open_random(&self) -> io::Result<Box<dyn RandomAccess>> {
      #[cfg(feature = "mmap")]
      {
         Ok(Box::new(crate::access::MmapRandom::open(&self.path)?))
      }
      #[cfg(not(feature = "mmap"))]
      {
         Ok(Box::new(crate::access::FileRandom::open(&self.path)?))
      }
   }
}

/// An opaque handle to one delivery. A rejection names the delivery
/// it came from by this receipt, so the provider poisons *exactly*
/// the copy the consumer could not load, never a guess — the identity
/// travels with the delivery and round-trips through the rejection.
/// No storage detail is exposed: the consumer obtains one only from
/// [`PreparedAsset::receipt`] and never inspects it — there is no
/// public constructor, so a rejection can only ever name a delivery
/// that actually happened.
///
/// Rejection itself is provider-side API rather than part of this
/// contract; the built-in engine's is
/// [`Assetify::reject`](crate::Assetify::reject).
#[derive(Clone, Debug)]
pub struct DeliveryReceipt {
   /// The revision served, when the provider versions its cache.
   /// Absent for providers that don't (an in-memory test double), in
   /// which case a rejection has nothing to poison.
   pub(crate) revision: Option<String>,
}

impl DeliveryReceipt {
   /// A receipt naming the revision a delivery was served from.
   /// Provider-side API.
   pub(crate) fn for_revision(revision: impl Into<String>) -> Self {
      DeliveryReceipt {
         revision: Some(revision.into()),
      }
   }

   /// A receipt for a delivery with no revision to poison (a provider
   /// that doesn't version its cache).
   pub(crate) fn none() -> Self {
      DeliveryReceipt { revision: None }
   }
}

/// One delivered asset: every requested file, ready to read.
///
/// The consumer never sees a storage location; "prepared" means the
/// provider is ready to answer reads — not that bytes were copied
/// anywhere in particular. Reach each file by name with
/// [`file`](PreparedAsset::file).
#[non_exhaustive]
pub struct PreparedAsset {
   files: Vec<PreparedFile>,
   receipt: DeliveryReceipt,
}

impl PreparedAsset {
   /// A delivery of the given files. Provider-side API; consumers
   /// receive these inside [`AssetResponse::Available`]. A provider
   /// that versions its cache stamps a receipt with
   /// [`with_receipt`](PreparedAsset::with_receipt).
   pub fn new(files: Vec<PreparedFile>) -> Self {
      PreparedAsset {
         files,
         receipt: DeliveryReceipt::none(),
      }
   }

   /// Attach the delivery receipt. Provider-side API.
   pub(crate) fn with_receipt(mut self, receipt: DeliveryReceipt) -> Self {
      self.receipt = receipt;
      self
   }

   /// This delivery's opaque receipt. Hand it back when rejecting
   /// exactly this delivery — see [`DeliveryReceipt`].
   pub fn receipt(&self) -> DeliveryReceipt {
      self.receipt.clone()
   }

   /// The delivered file with this name, if present. Absence is a
   /// named gap — the loud failure the name-matched contract is for.
   pub fn file(&self, name: &str) -> Option<&PreparedFile> {
      self.files.iter().find(|f| f.name() == name)
   }

   /// Every delivered file, in request order.
   pub fn files(&self) -> &[PreparedFile] {
      &self.files
   }
}

impl std::fmt::Debug for PreparedAsset {
   fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
      f.debug_struct("PreparedAsset")
         .field("files", &self.files)
         .finish()
   }
}

/// Per-request result of a [`Provider::provide`](crate::Provider::provide)
/// call.
#[derive(Debug)]
pub enum AssetResponse {
   /// The asset is prepared and every named file is readable. The
   /// consumer still validates content against its own format checks;
   /// a failed load is rejected via the delivery's
   /// [`DeliveryReceipt`] so that copy is never re-served.
   Available {
      /// The delivery.
      asset: PreparedAsset,
   },
   /// The asset could not be made available this time. A missing
   /// asset is a degraded capability, never an error: the consumer
   /// runs at whatever level its loaded assets allow, and a later
   /// request retries.
   Unavailable {
      /// Provider-side detail for logging and telemetry only;
      /// consumers do not branch on it.
      reason: String,
   },
}
