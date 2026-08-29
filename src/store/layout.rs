//! Path math and name validation for the on-disk store.
//!
//! Layout: `<root>/<id>/v<format_major>/<revision>/…`, with staging
//! under `<root>/.staging/`. Asset ids, revisions, and file names are
//! consumer- and resolver-supplied strings used directly in
//! filesystem paths, so they are validated before any path is built:
//! traversal (`..`), absolute paths, and hidden-file collisions (the
//! `.staging` and `.poisoned` reserved names) are all unrepresentable
//! by construction.

use std::path::PathBuf;

/// One segment: starts with an ASCII alphanumeric (which excludes
/// `.`, `..`, and anything colliding with dot-prefixed reserved
/// names), then ASCII alphanumerics, `-`, `_`, or `.`.
fn valid_segment(segment: &str) -> bool {
   let mut chars = segment.chars();
   chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
      && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// Validate an asset id: `/`-separated segments, each valid on its
/// own. Rejects empty ids, empty segments (`a//b`), leading or
/// trailing `/`, `.`/`..`, and non-portable characters.
pub(crate) fn validate_id(id: &str) -> Result<(), String> {
   if id.is_empty() {
      return Err("asset id is empty".to_string());
   }
   if id.split('/').all(valid_segment) {
      Ok(())
   } else {
      Err(format!(
         "asset id {id:?} is invalid: ids are /-separated segments, each starting with an \
          ASCII alphanumeric and containing only alphanumerics, '-', '_', or '.'"
      ))
   }
}

/// Validate a revision: a single segment.
pub(crate) fn validate_revision(revision: &str) -> Result<(), String> {
   if valid_segment(revision) {
      Ok(())
   } else {
      Err(format!(
         "revision {revision:?} is invalid: a revision is one path segment starting with an \
          ASCII alphanumeric and containing only alphanumerics, '-', '_', or '.'"
      ))
   }
}

/// Validate a delivered file's name: a single segment (the file may
/// sit in a subdirectory of the revision, but it is *named* — never
/// addressed by path — across the boundary).
pub(crate) fn validate_file_name(name: &str) -> Result<(), String> {
   if valid_segment(name) {
      Ok(())
   } else {
      Err(format!(
         "file name {name:?} is invalid: a name is one path segment starting with an ASCII \
          alphanumeric and containing only alphanumerics, '-', '_', or '.'"
      ))
   }
}

/// Whether a directory entry name is a well-formed revision. Used
/// when scanning a lane, so foreign entries (including dot-files)
/// are ignored rather than served.
pub(crate) fn is_revision_name(name: &str) -> bool {
   valid_segment(name)
}

/// `<root>/<id>/v<format_major>` — the lane holding an asset's
/// revisions readable by builds of that format major. Callers
/// validate `id` first.
pub(crate) fn lane_dir(root: &std::path::Path, id: &str, format_major: u32) -> PathBuf {
   let mut dir = root.to_path_buf();
   for segment in id.split('/') {
      dir.push(segment);
   }
   dir.push(format!("v{format_major}"));
   dir
}

#[cfg(test)]
mod tests {
   use super::*;

   #[test]
   fn accepts_reasonable_names() {
      for id in ["nlp/tokenizer/en", "models/sentiment", "a", "x2/y-z_1.0"] {
         assert!(validate_id(id).is_ok(), "{id:?} should be valid");
      }
      for revision in ["20260821", "2026-08-21", "r1.2"] {
         assert!(validate_revision(revision).is_ok());
      }
      for name in ["model.bin", "index.dat", "meta.json", "a"] {
         assert!(validate_file_name(name).is_ok());
      }
   }

   #[test]
   fn rejects_traversal_and_reserved_shapes() {
      for id in [
         "",
         "/abs",
         "trailing/",
         "a//b",
         "../up",
         "a/../b",
         "a/./b",
         ".staging",
         "a/.hidden",
         "sp ace",
         "uni\u{2603}code",
         "back\\slash",
      ] {
         assert!(validate_id(id).is_err(), "{id:?} should be rejected");
      }
      for revision in ["", "..", ".", ".poisoned", "a/b", "20 26"] {
         assert!(validate_revision(revision).is_err(), "{revision:?}");
      }
      for name in ["", "..", ".poisoned", "dir/file.bin", ".hidden"] {
         assert!(validate_file_name(name).is_err(), "{name:?}");
      }
   }

   #[test]
   fn lane_dir_nests_id_segments_under_the_root() {
      let dir = lane_dir(std::path::Path::new("/cache"), "nlp/tokenizer/en", 4);
      assert_eq!(dir, PathBuf::from("/cache/nlp/tokenizer/en/v4"));
   }
}
