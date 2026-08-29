//! Revision resolution within an asset.s directory, and name-matched file lookup
//! within a revision.

use std::path::{Path, PathBuf};

use crate::store::layout;
use crate::store::poison::PoisonLedger;

/// The newest unpoisoned revision in an asset.s directory, by
/// lexicographic order of the directory names (`YYYYMMDD`-style names
/// sort correctly by construction). Foreign entries — files, dot
/// directories, anything that is not a well-formed revision name —
/// are ignored rather than served.
pub(crate) fn newest_unpoisoned(asset_dir: &Path, ledger: &PoisonLedger) -> Option<String> {
   let mut newest: Option<String> = None;
   for entry in std::fs::read_dir(asset_dir).ok()? {
      let Ok(entry) = entry else { continue };
      if !entry.path().is_dir() {
         continue;
      }
      let Ok(name) = entry.file_name().into_string() else {
         continue;
      };
      if !layout::is_revision_name(&name) || ledger.is_poisoned(&entry.path()) {
         continue;
      }
      if newest.as_deref().is_none_or(|n| name.as_str() > n) {
         newest = Some(name);
      }
   }
   newest
}

/// Locate a delivered file by name anywhere under the revision
/// directory. Where a file sits inside the revision is placement
/// detail the consumer never learns — but the name must be unique:
/// two same-named files is a delivery error, never a silent
/// first-match.
pub(crate) fn find_file(revision_dir: &Path, name: &str) -> Result<PathBuf, String> {
   let mut matches = Vec::new();
   collect_matches(revision_dir, name, &mut matches);
   match matches.len() {
      0 => Err(format!("missing file {name:?}")),
      1 => Ok(matches.remove(0)),
      n => Err(format!(
         "file name {name:?} is ambiguous: {n} files carry it within the revision"
      )),
   }
}

fn collect_matches(dir: &Path, name: &str, matches: &mut Vec<PathBuf>) {
   let Ok(entries) = std::fs::read_dir(dir) else {
      return;
   };
   for entry in entries.flatten() {
      let path = entry.path();
      let entry_name = entry.file_name();
      // Skip reserved dot-entries (`.poisoned`) and anything hidden.
      if entry_name.to_string_lossy().starts_with('.') {
         continue;
      }
      if path.is_dir() {
         collect_matches(&path, name, matches);
      } else if entry_name.to_string_lossy() == name {
         matches.push(path);
      }
   }
}

#[cfg(test)]
mod tests {
   use super::*;

   fn touch(path: &Path) {
      std::fs::create_dir_all(path.parent().unwrap()).unwrap();
      std::fs::write(path, b"x").unwrap();
   }

   #[test]
   fn newest_wins_lexicographically_and_poison_is_skipped() {
      let dir = tempfile::tempdir().unwrap();
      let asset = dir.path();
      for revision in ["20260812", "20260821", "20250101"] {
         std::fs::create_dir(asset.join(revision)).unwrap();
      }
      // Foreign entries a real cache accumulates.
      std::fs::write(asset.join("notes.txt"), b"not a revision").unwrap();
      std::fs::create_dir(asset.join(".partial")).unwrap();

      let ledger = PoisonLedger::new();
      assert_eq!(
         newest_unpoisoned(asset, &ledger).as_deref(),
         Some("20260821")
      );

      ledger.poison(&asset.join("20260821"), "unloadable");
      assert_eq!(
         newest_unpoisoned(asset, &ledger).as_deref(),
         Some("20260812"),
         "poisoned newest falls back to the next revision"
      );

      ledger.poison(&asset.join("20260812"), "unloadable");
      ledger.poison(&asset.join("20250101"), "unloadable");
      assert_eq!(newest_unpoisoned(asset, &ledger), None);
   }

   #[test]
   fn missing_asset_dir_resolves_to_none() {
      let dir = tempfile::tempdir().unwrap();
      let ledger = PoisonLedger::new();
      assert_eq!(newest_unpoisoned(&dir.path().join("absent"), &ledger), None);
   }

   #[test]
   fn files_are_found_by_name_anywhere_in_the_revision() {
      let dir = tempfile::tempdir().unwrap();
      let revision = dir.path();
      touch(&revision.join("meta.json"));
      touch(&revision.join("bundle/inner/index.dat"));

      assert!(find_file(revision, "meta.json").is_ok());
      let found = find_file(revision, "index.dat").unwrap();
      assert!(found.ends_with("bundle/inner/index.dat"));

      let missing = find_file(revision, "absent.bin").unwrap_err();
      assert!(missing.contains("absent.bin"));
   }

   #[test]
   fn duplicate_names_are_a_delivery_error() {
      let dir = tempfile::tempdir().unwrap();
      let revision = dir.path();
      touch(&revision.join("a/index.dat"));
      touch(&revision.join("b/index.dat"));

      let err = find_file(revision, "index.dat").unwrap_err();
      assert!(err.contains("ambiguous"), "{err}");
   }

   #[test]
   fn reserved_dot_entries_are_invisible() {
      let dir = tempfile::tempdir().unwrap();
      let revision = dir.path();
      touch(&revision.join(".poisoned"));
      assert!(find_file(revision, ".poisoned").is_err());
   }
}
