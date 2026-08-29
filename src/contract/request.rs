//! The request side: what a consumer declares it needs.

use crate::contract::access::AccessKind;

/// One file the asset must contain, with the access the consumer
/// requires for it.
#[derive(Clone, Debug)]
pub struct FileRequest {
   /// The file's name (`"index.dat"`). Delivery is matched by this
   /// name, never by position or path; names must be unique within an
   /// asset.
   pub name: String,
   /// The access kind the consumer requires for this file.
   pub access: AccessKind,
}

impl FileRequest {
   /// A request for one named file.
   pub fn new(name: impl Into<String>, access: AccessKind) -> Self {
      FileRequest {
         name: name.into(),
         access,
      }
   }
}

impl<S: Into<String>> From<(S, AccessKind)> for FileRequest {
   /// `("model.bin", AccessKind::Random)` reads as a file request
   /// directly — the flat spelling request call sites use.
   fn from((name, access): (S, AccessKind)) -> Self {
      FileRequest::new(name, access)
   }
}

/// A delivery the consumer could not load: the payload verified and
/// arrived intact, yet failed the consumer's own checks (a named gap,
/// an access-kind mismatch, an unreadable payload format, failed
/// content validation).
///
/// Echoed back on the next request for the same asset so the provider
/// treats its copy as poisoned — re-acquire, don't re-serve.
#[derive(Clone, Debug)]
pub struct RejectedDelivery {
   /// The consumer's load-failure detail. Diagnostic text; providers
   /// do not branch on it.
   pub reason: String,
}

/// One asset the consumer wants — named logically, never by path.
#[derive(Clone, Debug)]
pub struct AssetRequest {
   /// The asset's identifier, a `/`-separated logical name in the
   /// consumer's own namespace (`"tokenizer/en"`). Identifiers
   /// are validated before touching the filesystem: no `.` or `..`
   /// segments, no absolute paths, no empty segments, and segment
   /// characters limited to alphanumerics, `-`, `_`, and `.`.
   ///
   /// The id is the compatibility boundary: every revision under one
   /// id must be readable by every consumer that requests it. If your
   /// payload format can change incompatibly, encode the format in
   /// the id (`"tokenizer/en/v2"`) so incompatible payloads are
   /// simply different assets.
   pub id: String,
   /// Every file the asset must contain, each with its required
   /// access kind.
   pub files: Vec<FileRequest>,
   /// Present when the consumer rejected this asset's previous
   /// delivery at load. A provider holding a cached copy must treat
   /// it as poisoned.
   pub rejected: Option<RejectedDelivery>,
}

impl AssetRequest {
   /// A request for one asset, with no rejection echo. Files are
   /// `FileRequest` values or plain `(name, AccessKind)` pairs.
   pub fn new(
      id: impl Into<String>,
      files: impl IntoIterator<Item = impl Into<FileRequest>>,
   ) -> Self {
      AssetRequest {
         id: id.into(),
         files: files.into_iter().map(Into::into).collect(),
         rejected: None,
      }
   }
}
