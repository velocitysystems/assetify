//! The request side: what a consumer declares it needs.

use crate::contract::access::AccessKind;

/// One file the asset must contain, with the access the consumer
/// requires for it.
#[derive(Clone, Debug)]
pub struct FileSpec {
   /// The file's name (`"index.dat"`). Delivery is matched by this
   /// name, never by position or path; names must be unique within an
   /// asset.
   pub name: String,
   /// The access kind the consumer requires for this file.
   pub access: AccessKind,
}

impl FileSpec {
   /// A spec for one named file.
   pub fn new(name: impl Into<String>, access: AccessKind) -> Self {
      FileSpec {
         name: name.into(),
         access,
      }
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
   /// consumer's own namespace (`"nlp/tokenizer/en"`). Identifiers
   /// are validated before touching the filesystem: no `.` or `..`
   /// segments, no absolute paths, no empty segments, and segment
   /// characters limited to alphanumerics, `-`, `_`, and `.`.
   pub id: String,
   /// The payload format major version this consumer build reads —
   /// the **hard** half of versioning. A payload outside this lane is
   /// unreadable by the consumer and must never be served; which
   /// revision to serve *within* the lane is the provider's choice.
   pub format_major: u32,
   /// Every file the asset must contain, each with its required
   /// access kind.
   pub files: Vec<FileSpec>,
   /// Present when the consumer rejected this asset's previous
   /// delivery at load. A provider holding a cached copy must treat
   /// it as poisoned.
   pub rejected: Option<RejectedDelivery>,
}

impl AssetRequest {
   /// A request for one asset, with no rejection echo.
   pub fn new(id: impl Into<String>, format_major: u32, files: Vec<FileSpec>) -> Self {
      AssetRequest {
         id: id.into(),
         format_major,
         files,
         rejected: None,
      }
   }
}
