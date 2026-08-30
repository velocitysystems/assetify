//! Archive extraction: turn a verified [`Payload::Archive`] download
//! into the revision's file tree.
//!
//! Extraction runs *after* the digest verifies and *into* the staging
//! directory, so atomic placement is untouched — a revision still
//! publishes as a complete extracted tree or not at all. Entry paths
//! are sanitized by the extractor; an entry that would escape the
//! staging directory is never written.
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
         .map_err(|e| format!("extract archive: {e}"))
   })
   .await
   .map_err(|e| format!("extraction task failed: {e}"))?
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
}
