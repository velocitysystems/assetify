//! Archive extraction: turn a verified [`Payload::Archive`] download
//! into the revision's file tree.
//!
//! Extraction runs *after* the digest verifies and *into* the staging
//! directory, so atomic placement is untouched — a revision still
//! publishes as a complete extracted tree or not at all.
//!
//! The extractor sanitizes entry *names* (a `../` path cannot escape
//! the destination), but the archive format also allows symlink
//! entries whose *target* is arbitrary — a link the extractor writes
//! verbatim. A later same-named file write would follow such a link
//! out of the store, and serving would hand back the link's target.
//! So after extraction the whole tree is checked and any symlink
//! fails the acquisition: nothing a placed revision contains is ever
//! a link.
//!
//! [`Payload::Archive`]: crate::Payload

use std::path::{Path, PathBuf};

/// Extract a zip archive at `archive` into `destination`, on the
/// blocking pool. The archive file itself is left in place (its
/// caller owns the temp file's lifetime).
pub(crate) async fn extract_zip(archive: &Path, destination: &Path) -> Result<(), String> {
   let archive: PathBuf = archive.to_path_buf();
   let destination: PathBuf = destination.to_path_buf();
   tokio::task::spawn_blocking(move || {
      let file = std::fs::File::open(&archive).map_err(|e| format!("open archive: {e}"))?;
      let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file))
         .map_err(|e| format!("read archive: {e}"))?;
      zip.extract(&destination)
         .map_err(|e| format!("extract archive: {e}"))?;
      // One walk validates the tree and counts its files: an archive
      // that yields no files (empty, or directory entries only) would
      // otherwise publish an empty revision that every later request
      // cache-hits and fails to serve, forever.
      if scan_extracted(&destination)? == 0 {
         return Err("archive contains no files".to_string());
      }
      Ok(())
   })
   .await
   .map_err(|e| format!("extraction task failed: {e}"))?
}

/// Walk the extracted tree, rejecting any symlink (via
/// `symlink_metadata`, which never follows, so a link pointing
/// anywhere is caught rather than traversed) and returning how many
/// regular files it holds.
fn scan_extracted(dir: &Path) -> Result<usize, String> {
   let mut files = 0;
   let entries = std::fs::read_dir(dir).map_err(|e| format!("scan extracted tree: {e}"))?;
   for entry in entries {
      let entry = entry.map_err(|e| format!("scan extracted tree: {e}"))?;
      let meta = std::fs::symlink_metadata(entry.path())
         .map_err(|e| format!("scan extracted tree: {e}"))?;
      if meta.is_symlink() {
         return Err(format!(
            "archive contains a symlink entry {:?}; symlinks are not allowed",
            entry.file_name()
         ));
      }
      if meta.is_dir() {
         files += scan_extracted(&entry.path())?;
      } else if meta.is_file() {
         files += 1;
      }
   }
   Ok(files)
}

#[cfg(test)]
mod tests {
   use std::io::Write as _;

   use super::*;

   fn fixture_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
      let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
      for (name, bytes) in entries {
         writer
            .start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
         writer.write_all(bytes).unwrap();
      }
      writer.finish().unwrap().into_inner()
   }

   #[tokio::test]
   async fn extracts_nested_entries() {
      let dir = tempfile::tempdir().unwrap();
      let archive = dir.path().join("pack.zip");
      std::fs::write(
         &archive,
         fixture_zip(&[("meta.json", b"{}"), ("sub/model.bin", b"weights")]),
      )
      .unwrap();

      let dest = dir.path().join("out");
      std::fs::create_dir(&dest).unwrap();
      extract_zip(&archive, &dest).await.unwrap();

      assert_eq!(std::fs::read(dest.join("meta.json")).unwrap(), b"{}");
      assert_eq!(
         std::fs::read(dest.join("sub/model.bin")).unwrap(),
         b"weights"
      );
   }

   #[tokio::test]
   async fn a_traversal_entry_cannot_escape_the_destination() {
      let dir = tempfile::tempdir().unwrap();
      let archive = dir.path().join("evil.zip");
      std::fs::write(&archive, fixture_zip(&[("../evil.txt", b"escape")])).unwrap();

      let dest = dir.path().join("out");
      std::fs::create_dir(&dest).unwrap();
      // Skipped or rejected — both are fine; escaping is not.
      let _ = extract_zip(&archive, &dest).await;
      assert!(!dir.path().join("evil.txt").exists());
   }

   // Symlink handling is unix-specific: on Windows, creating a link
   // from an archive entry is privilege-gated, so the outcome depends
   // on the platform rather than on our defense.
   #[cfg(unix)]
   #[tokio::test]
   async fn a_symlink_entry_fails_extraction_and_is_not_left_behind() {
      let dir = tempfile::tempdir().unwrap();
      let archive = dir.path().join("link.zip");
      let bytes = {
         let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
         writer
            .add_symlink(
               "model.bin",
               "/etc/passwd",
               zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
         writer.finish().unwrap().into_inner()
      };
      std::fs::write(&archive, bytes).unwrap();

      let dest = dir.path().join("out");
      std::fs::create_dir(&dest).unwrap();
      // Extraction fails; the caller (which owns a staging TempDir)
      // discards the partial tree, so nothing is ever placed.
      let result = extract_zip(&archive, &dest).await;
      assert!(result.is_err(), "a symlink entry must fail the extraction");
      assert!(
         result.unwrap_err().contains("symlink"),
         "the reason names the symlink"
      );
   }

   #[tokio::test]
   async fn an_empty_archive_is_rejected() {
      let dir = tempfile::tempdir().unwrap();
      let archive = dir.path().join("empty.zip");
      std::fs::write(&archive, fixture_zip(&[])).unwrap();

      let dest = dir.path().join("out");
      std::fs::create_dir(&dest).unwrap();
      let result = extract_zip(&archive, &dest).await;
      assert!(result.is_err(), "an archive with no files must fail");
      assert!(
         result.unwrap_err().contains("no files"),
         "the reason says so"
      );
   }
}
