//! The delivery side: what comes back for each requested asset.

use crate::contract::access::{AssetPath, FileAccess, RandomAccess, StreamAccess};

/// One delivered file, matched to the request by name.
#[derive(Debug)]
#[non_exhaustive]
pub struct PreparedFile {
   /// The name as the request's [`FileRequest`](crate::FileRequest) listed
   /// it. A partial or reordered delivery fails loudly as a named
   /// gap.
   pub name: String,
   /// The access object, of the kind the spec declared. A kind
   /// mismatch is a rejection at load.
   pub access: FileAccess,
}

impl PreparedFile {
   /// One named file behind its access object.
   pub fn new(name: impl Into<String>, access: FileAccess) -> Self {
      PreparedFile {
         name: name.into(),
         access,
      }
   }
}

/// An opaque handle to one delivery. A consumer echoes it back inside
/// a [`RejectedDelivery`](crate::RejectedDelivery) so the provider
/// poisons *exactly* the copy the consumer could not load, never a
/// guess — the identity travels with the delivery and round-trips
/// through the rejection. No storage detail is exposed: the consumer
/// obtains one only from [`PreparedAsset::receipt`] and never
/// inspects it — there is no public constructor, so a rejection can
/// only ever name a delivery that actually happened.
#[derive(Clone, Debug)]
pub struct DeliveryReceipt {
   /// The revision served, when the provider versions its cache.
   /// Absent for providers that don't (an in-memory test double), in
   /// which case a rejection echo has nothing to poison.
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

/// One delivered asset: every requested file, each behind its access
/// object.
///
/// The consumer never sees a storage location; "prepared" means the
/// provider is ready to answer reads — not that bytes were copied
/// anywhere in particular.
#[derive(Debug)]
#[non_exhaustive]
pub struct PreparedAsset {
   /// Every file the request named for this asset.
   pub files: Vec<PreparedFile>,
   /// The opaque handle to this delivery, echoed back to reject it.
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

   /// This delivery's opaque receipt. Echo it in a
   /// [`RejectedDelivery`](crate::RejectedDelivery) to reject exactly
   /// this delivery.
   pub fn receipt(&self) -> DeliveryReceipt {
      self.receipt.clone()
   }

   /// The delivered file with this name, if present. Absence is a
   /// named gap — the loud failure the name-matched contract is for.
   pub fn file(&self, name: &str) -> Option<&PreparedFile> {
      self.files.iter().find(|f| f.name == name)
   }

   /// Take ownership of the named file's access object, for loaders
   /// that consume deliveries file by file.
   pub fn take_file(&mut self, name: &str) -> Option<PreparedFile> {
      let i = self.files.iter().position(|f| f.name == name)?;
      Some(self.files.swap_remove(i))
   }

   /// Take the named file as a forward reader. When the file was
   /// requested with [`AccessKind::Stream`](crate::AccessKind::Stream),
   /// this cannot miss — the delivered kind always matches the
   /// requested kind. `None` means the file is absent or was requested
   /// with a different kind (the file is consumed either way).
   pub fn take_stream(&mut self, name: &str) -> Option<StreamAccess> {
      match self.take_file(name)?.access {
         FileAccess::Stream(stream) => Some(stream),
         _ => None,
      }
   }

   /// Take the named file as positioned access. The counterpart of
   /// [`take_stream`](PreparedAsset::take_stream) for
   /// [`AccessKind::Random`](crate::AccessKind::Random) files.
   pub fn take_random(&mut self, name: &str) -> Option<Box<dyn RandomAccess>> {
      match self.take_file(name)?.access {
         FileAccess::Random(random) => Some(random),
         _ => None,
      }
   }

   /// Take the named file as a real filesystem path. The counterpart
   /// of [`take_stream`](PreparedAsset::take_stream) for
   /// [`AccessKind::AssetPath`](crate::AccessKind::AssetPath)
   /// files.
   pub fn take_asset_path(&mut self, name: &str) -> Option<AssetPath> {
      match self.take_file(name)?.access {
         FileAccess::AssetPath(path) => Some(path),
         _ => None,
      }
   }
}

#[cfg(test)]
mod tests {
   use super::*;

   #[test]
   fn typed_accessors_match_kind_and_consume() {
      let mut asset = PreparedAsset::new(vec![
         PreparedFile::new("meta.json", FileAccess::Stream(Box::new(&b"{}"[..]))),
         PreparedFile::new("rules.txt", FileAccess::AssetPath(AssetPath::new("/r"))),
      ]);

      assert!(asset.take_stream("meta.json").is_some());
      assert!(asset.take_stream("meta.json").is_none(), "consumed");

      assert!(asset.take_stream("rules.txt").is_none(), "wrong kind");
      assert!(asset.take_random("absent.bin").is_none(), "named gap");
   }
}

/// Per-request result of a [`Provider::provide`](crate::Provider::provide)
/// call.
#[derive(Debug)]
pub enum AssetResponse {
   /// The asset is prepared and every named file is readable behind
   /// its access object. The consumer still validates content against
   /// its own format checks; a failed load is echoed back as a
   /// [`RejectedDelivery`](crate::RejectedDelivery) on the next
   /// request.
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
