//! The delivery side: what comes back for each requested asset.

use crate::contract::access::{AssetPath, FileAccess, RandomAccess, StreamAccess};

/// One delivered file, matched to the request by name.
#[derive(Debug)]
pub struct PreparedFile {
   /// The name as the request's [`FileSpec`](crate::FileSpec) listed
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
}

impl PreparedAsset {
   /// A delivery of the given files. Provider-side API; consumers
   /// receive these inside [`AssetResponse::Available`].
   pub fn new(files: Vec<PreparedFile>) -> Self {
      PreparedAsset { files }
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
