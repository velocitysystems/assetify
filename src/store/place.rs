//! Staging and atomic placement.
//!
//! A revision is assembled in a unique staging directory under
//! `<root>/.staging/` — the same filesystem as its destination, so
//! the final `rename` is atomic — and only a *complete, verified* set
//! of files is renamed into place. A revision directory therefore
//! either does not exist or is whole; readers never observe a partial
//! one, and a placed directory is never mutated again (which is what
//! makes memory-mapping its files sound).
//!
//! Placement is idempotent: two writers racing the same revision
//! (another thread, another process) both stage independently, and
//! whichever renames second simply loses to an identical result. No
//! advisory locks — they are unreliable on some mobile filesystems;
//! losing gracefully is the cheaper invariant.

use std::io;
use std::path::Path;

use tempfile::TempDir;

/// Name of the staging area under the store root. Reserved: validated
/// asset ids can never collide with it (segments cannot start with
/// `.`).
const STAGING: &str = ".staging";

/// Create the root and its staging area. Called at build time when a
/// resolver is configured: acquisition needs a writable root, and
/// failing at build beats failing on the first request.
pub(crate) fn ensure_staging(root: &Path) -> io::Result<()> {
   std::fs::create_dir_all(root.join(STAGING))
}

/// A fresh, unique staging directory under the root's staging area.
/// Dropped un-placed, it cleans itself up.
pub(crate) fn stage(root: &Path) -> io::Result<TempDir> {
   ensure_staging(root)?;
   tempfile::Builder::new()
      .prefix("stage-")
      .tempdir_in(root.join(STAGING))
}

/// What placement accomplished.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Placement {
   /// The staged set is now the revision directory.
   Placed,
   /// The revision already exists — an earlier call, another thread,
   /// or another process got there first. The staged copy was
   /// discarded; the outcome is equivalent.
   AlreadyPresent,
}

/// Atomically rename a staged revision into its destination.
pub(crate) fn place(staged: TempDir, destination: &Path) -> io::Result<Placement> {
   if destination.exists() {
      return Ok(Placement::AlreadyPresent);
   }
   if let Some(parent) = destination.parent() {
      std::fs::create_dir_all(parent)?;
   }

   // Disarm the TempDir's cleanup: from here the directory is either
   // renamed away or removed explicitly on the failure paths.
   let staged = staged.keep();
   match std::fs::rename(&staged, destination) {
      Ok(()) => Ok(Placement::Placed),
      Err(e) => {
         let _ = std::fs::remove_dir_all(&staged);
         if destination.exists() {
            // Lost the race between our existence check and rename;
            // the winner's directory is just as good.
            Ok(Placement::AlreadyPresent)
         } else {
            Err(e)
         }
      }
   }
}

#[cfg(test)]
mod tests {
   use super::*;

   #[test]
   fn staged_files_arrive_whole_at_the_destination() {
      let root = tempfile::tempdir().unwrap();
      let staged = stage(root.path()).unwrap();
      std::fs::write(staged.path().join("index.dat"), b"payload").unwrap();

      let destination = root.path().join("nlp/tokenizer/en/v4/20260821");
      assert_eq!(place(staged, &destination).unwrap(), Placement::Placed);
      assert_eq!(
         std::fs::read(destination.join("index.dat")).unwrap(),
         b"payload"
      );
      let staging_root = root.path().join(".staging");
      assert_eq!(
         std::fs::read_dir(staging_root).unwrap().count(),
         0,
         "staging area is left empty"
      );
   }

   #[test]
   fn double_placement_is_idempotent() {
      let root = tempfile::tempdir().unwrap();
      let destination = root.path().join("models/sentiment/v1/r2");

      let first = stage(root.path()).unwrap();
      std::fs::write(first.path().join("model.bin"), b"first").unwrap();
      assert_eq!(place(first, &destination).unwrap(), Placement::Placed);

      let second = stage(root.path()).unwrap();
      std::fs::write(second.path().join("model.bin"), b"second").unwrap();
      assert_eq!(
         place(second, &destination).unwrap(),
         Placement::AlreadyPresent
      );

      assert_eq!(
         std::fs::read(destination.join("model.bin")).unwrap(),
         b"first",
         "the winner's content is never clobbered"
      );
   }

   #[test]
   fn abandoned_staging_cleans_itself_up() {
      let root = tempfile::tempdir().unwrap();
      let staged = stage(root.path()).unwrap();
      let staged_path = staged.path().to_path_buf();
      std::fs::write(staged_path.join("partial.bin"), b"incomplete").unwrap();
      drop(staged);
      assert!(!staged_path.exists(), "dropped staging leaves nothing");
   }
}
