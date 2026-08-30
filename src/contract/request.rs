//! The request side: what a consumer declares it needs.

use crate::contract::delivery::DeliveryReceipt;

/// A delivery the consumer could not load: the payload verified and
/// arrived intact, yet failed the consumer's own checks (a named gap,
/// an access-kind mismatch, an unreadable payload format, failed
/// content validation).
///
/// Echoed back on the next request for the same asset so the provider
/// poisons *exactly the delivery being rejected* — re-acquire, don't
/// re-serve. Carrying the delivery's [`DeliveryReceipt`] is what makes
/// the target precise: even with several concurrent deliveries of one
/// asset, the rejection names the one it came from, never "whatever
/// was served most recently".
#[derive(Clone, Debug)]
pub struct RejectedDelivery {
   /// The receipt from the [`PreparedAsset`](crate::PreparedAsset)
   /// being rejected, via [`PreparedAsset::receipt`](crate::PreparedAsset::receipt).
   pub receipt: DeliveryReceipt,
   /// The consumer's load-failure detail. Diagnostic text; providers
   /// do not branch on it.
   pub reason: String,
}

impl RejectedDelivery {
   /// Reject the delivery this receipt came from, with a
   /// human-readable reason.
   pub fn new(receipt: DeliveryReceipt, reason: impl Into<String>) -> Self {
      RejectedDelivery {
         receipt,
         reason: reason.into(),
      }
   }
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
   /// the id (`"tokenizer/en/v2"`) — see
   /// [`AssetRequest::versioned_id`], which spells out the idiom and
   /// its one rule (ids must be prefix-free).
   pub id: String,
   /// The names of the files the asset must contain. Delivery is
   /// matched by name, never by position or path; names must be
   /// unique within the asset.
   pub files: Vec<String>,
   /// Present when the consumer rejected this asset's previous
   /// delivery at load. A provider holding a cached copy must treat
   /// it as poisoned.
   pub rejected: Option<RejectedDelivery>,
}

impl AssetRequest {
   /// THE compatibility mechanism, as an id: encode the payload
   /// format's major version in the asset id, so incompatible
   /// payloads are different assets with disjoint revision trees.
   /// `versioned_id("tokenizer/en", 2)` is `"tokenizer/en/v2"` —
   /// offline fallback can never cross a format break, because it
   /// only ever picks among one id's own revisions.
   ///
   /// Keep versioned and unversioned ids prefix-free: never use an id
   /// that is a path-prefix of another (`a/b` alongside `a/b/v2`
   /// makes `v2` look like a revision of `a/b`).
   pub fn versioned_id(base: &str, major: u32) -> String {
      format!("{base}/v{major}")
   }

   /// A request for one asset, naming the files it must contain, with
   /// no rejection echo.
   pub fn new(id: impl Into<String>, files: impl IntoIterator<Item = impl Into<String>>) -> Self {
      AssetRequest {
         id: id.into(),
         files: files.into_iter().map(Into::into).collect(),
         rejected: None,
      }
   }
}

#[cfg(test)]
mod tests {
   use super::*;
   use crate::store::layout;

   #[test]
   fn versioned_ids_compose_and_validate() {
      let id = AssetRequest::versioned_id("tokenizer/en", 2);
      assert_eq!(id, "tokenizer/en/v2");
      assert!(layout::validate_id(&id).is_ok());
      assert!(layout::validate_id(&AssetRequest::versioned_id("a", 10)).is_ok());
   }
}
