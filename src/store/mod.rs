//! The on-disk store: a validated, versioned cache of asset
//! revisions.
//!
//! Layout: `<root>/<id>/<revision>/…`. The id is the compatibility
//! boundary (every revision under one id must stay readable by its
//! consumers); the revision is the freshness axis (lexicographic,
//! newest wins, serve what you have when acquisition fails). Revision directories
//! are placed atomically and never mutated; a revision the consumer
//! could not load is poisoned and skipped thereafter.

pub(crate) mod layout;
pub(crate) mod place;
pub(crate) mod poison;
pub(crate) mod resolve;

use std::io;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use crate::store::place::Placement;
use crate::store::poison::PoisonLedger;

/// One cache root and its poison ledger. The root may be read-only
/// (bundled assets served in place); everything that writes reports
/// failure gracefully or degrades, and nothing here deletes.
pub(crate) struct Store {
   root: PathBuf,
   poison: PoisonLedger,
}

impl Store {
   pub(crate) fn new(root: PathBuf) -> Self {
      Store {
         root,
         poison: PoisonLedger::new(),
      }
   }

   /// `<root>/<id>`. Callers validate `id` first.
   pub(crate) fn asset_dir(&self, id: &str) -> PathBuf {
      layout::asset_dir(&self.root, id)
   }

   /// `<root>/<id>/<revision>`. Callers validate first.
   pub(crate) fn revision_dir(&self, id: &str, revision: &str) -> PathBuf {
      self.asset_dir(id).join(revision)
   }

   /// Whether this revision is on disk and serviceable.
   pub(crate) fn has_revision(&self, id: &str, revision: &str) -> bool {
      let dir = self.revision_dir(id, revision);
      dir.is_dir() && !self.poison.is_poisoned(&dir)
   }

   /// The asset's newest serviceable revision, if any.
   pub(crate) fn newest_revision(&self, id: &str) -> Option<String> {
      resolve::newest_unpoisoned(&self.asset_dir(id), &self.poison)
   }

   /// A unique staging directory on the root's filesystem.
   pub(crate) fn stage(&self) -> io::Result<TempDir> {
      place::stage(&self.root)
   }

   /// Atomically place a staged revision; idempotent under races.
   pub(crate) fn place_revision(
      &self,
      staged: TempDir,
      id: &str,
      revision: &str,
   ) -> io::Result<Placement> {
      place::place(staged, &self.revision_dir(id, revision))
   }

   /// Poison a revision the consumer could not load.
   pub(crate) fn poison_revision(&self, id: &str, revision: &str, reason: &str) {
      self.poison.poison(&self.revision_dir(id, revision), reason);
   }

   /// Locate a delivered file by unique name within a revision.
   pub(crate) fn find_file(&self, revision_dir: &Path, name: &str) -> Result<PathBuf, String> {
      resolve::find_file(revision_dir, name)
   }
}

#[cfg(test)]
mod tests {
   use super::*;

   #[test]
   fn a_revision_round_trips_through_stage_place_resolve() {
      let root = tempfile::tempdir().unwrap();
      let store = Store::new(root.path().to_path_buf());

      let staged = store.stage().unwrap();
      std::fs::write(staged.path().join("meta.json"), b"{}").unwrap();
      assert_eq!(
         store
            .place_revision(staged, "nlp/tokenizer/en", "20260821")
            .unwrap(),
         Placement::Placed
      );

      assert!(store.has_revision("nlp/tokenizer/en", "20260821"));
      assert_eq!(
         store.newest_revision("nlp/tokenizer/en").as_deref(),
         Some("20260821")
      );
      assert!(
         store.newest_revision("nlp/tokenizer/de").is_none(),
         "assets never bleed into each other"
      );

      let dir = store.revision_dir("nlp/tokenizer/en", "20260821");
      assert!(store.find_file(&dir, "meta.json").is_ok());
   }

   #[test]
   fn poisoned_revisions_stop_being_served() {
      let root = tempfile::tempdir().unwrap();
      let store = Store::new(root.path().to_path_buf());

      for revision in ["20260812", "20260821"] {
         let staged = store.stage().unwrap();
         std::fs::write(staged.path().join("model.bin"), revision).unwrap();
         store
            .place_revision(staged, "models/sentiment", revision)
            .unwrap();
      }

      store.poison_revision("models/sentiment", "20260821", "unloadable");
      assert!(!store.has_revision("models/sentiment", "20260821"));
      assert_eq!(
         store.newest_revision("models/sentiment").as_deref(),
         Some("20260812")
      );
   }
}
