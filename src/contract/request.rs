//! The request side: what a consumer declares it needs.

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

   /// A request for one asset, naming the files it must contain.
   pub fn new(id: impl Into<String>, files: impl IntoIterator<Item = impl Into<String>>) -> Self {
      AssetRequest {
         id: id.into(),
         files: files.into_iter().map(Into::into).collect(),
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
