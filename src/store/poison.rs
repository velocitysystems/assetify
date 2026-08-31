//! Poison markers: remembering revisions that verified but failed the
//! consumer's load.
//!
//! A poisoned revision is never served again — without that memory, a
//! payload the consumer cannot load becomes a re-serve/reject loop.
//! The marker is a `.poisoned` file inside the revision directory so
//! it survives process restarts with no separate state store; when
//! the root is read-only the marker degrades to in-memory-only, which
//! still breaks the loop for this process's lifetime.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

/// The marker file's name. Reserved: validated revision and file
/// names can never collide with it (they cannot start with `.`).
const MARKER: &str = ".poisoned";

/// Which revisions are poisoned: marker files first, an in-memory
/// set as the read-only-root fallback.
pub(crate) struct PoisonLedger {
   memory: Mutex<HashSet<PathBuf>>,
}

impl PoisonLedger {
   pub(crate) fn new() -> Self {
      PoisonLedger {
         memory: Mutex::new(HashSet::new()),
      }
   }

   /// Mark a revision directory poisoned, with the consumer's reason
   /// as the marker's content for post-mortems.
   pub(crate) fn poison(&self, revision_dir: &Path, reason: &str) {
      if std::fs::write(revision_dir.join(MARKER), reason).is_err() {
         // Read-only root (or the directory vanished): remember for
         // this process's lifetime instead.
         self
            .memory
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(revision_dir.to_path_buf());
      }
   }

   /// Whether a revision directory has been poisoned.
   pub(crate) fn is_poisoned(&self, revision_dir: &Path) -> bool {
      revision_dir.join(MARKER).exists()
         || self
            .memory
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(revision_dir)
   }
}

#[cfg(test)]
mod tests {
   use super::*;

   #[test]
   fn marker_persists_on_disk() {
      let dir = tempfile::tempdir().unwrap();
      let revision = dir.path().join("20260821");
      std::fs::create_dir(&revision).unwrap();

      let ledger = PoisonLedger::new();
      assert!(!ledger.is_poisoned(&revision));
      ledger.poison(&revision, "payload failed to load");
      assert!(ledger.is_poisoned(&revision));

      // A fresh ledger (a restarted process) still sees the marker.
      assert!(PoisonLedger::new().is_poisoned(&revision));
      let reason = std::fs::read_to_string(revision.join(".poisoned")).unwrap();
      assert_eq!(reason, "payload failed to load");
   }

   #[cfg(unix)]
   #[test]
   fn read_only_directory_degrades_to_memory() {
      use std::os::unix::fs::PermissionsExt;

      let dir = tempfile::tempdir().unwrap();
      let revision = dir.path().join("20260821");
      std::fs::create_dir(&revision).unwrap();
      std::fs::set_permissions(&revision, std::fs::Permissions::from_mode(0o555)).unwrap();

      let ledger = PoisonLedger::new();
      ledger.poison(&revision, "unloadable");
      assert!(ledger.is_poisoned(&revision), "in-memory fallback holds");
      assert!(
         !PoisonLedger::new().is_poisoned(&revision),
         "nothing could be persisted on a read-only root"
      );

      // Restore write permission so the tempdir can clean up.
      std::fs::set_permissions(&revision, std::fs::Permissions::from_mode(0o755)).unwrap();
   }
}
