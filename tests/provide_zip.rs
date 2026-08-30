//! End-to-end provision from archive payloads: one verified zip
//! becomes the revision's file tree, served by unique file name.

#![cfg(feature = "zip")]

use std::io::{Read, Write};
use std::path::Path;

use assetify::{
   AccessKind, ArchiveFormat, AssetRequest, AssetResponse, AssetSource, Assetify, Digest,
   FileSource, Locator, StaticResolver,
};
use sha2::Digest as _;

fn sha256_of(bytes: &[u8]) -> Digest {
   Digest::sha256_hex(&hex::encode(sha2::Sha256::digest(bytes))).unwrap()
}

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

/// Write an archive to "remote" storage and describe it as a source.
fn archive_source(dir: &Path, bytes: &[u8]) -> FileSource {
   let path = dir.join("pack.zip");
   std::fs::write(&path, bytes).unwrap();
   FileSource::new("pack.zip", Locator::File(path), sha256_of(bytes)).extracted(ArchiveFormat::Zip)
}

fn engine_over(root: &Path, source: AssetSource) -> Assetify {
   Assetify::builder(root)
      .resolver(StaticResolver::new([("tokenizer/en", source)]))
      .build()
      .unwrap()
}

#[tokio::test]
async fn an_archive_becomes_the_revision_and_serves_by_name() {
   let remote = tempfile::tempdir().unwrap();
   let root = tempfile::tempdir().unwrap();

   let archive = fixture_zip(&[
      ("meta.json", b"{\"format\":4}".as_slice()),
      ("system/model.bin", b"weights"),
   ]);
   let source = AssetSource::new("20260830", vec![archive_source(remote.path(), &archive)]);
   let engine = engine_over(root.path(), source);

   let outcome = engine
      .asset(AssetRequest::new(
         "tokenizer/en",
         [
            ("meta.json", AccessKind::Stream),
            ("model.bin", AccessKind::Random),
         ],
      ))
      .await;

   let AssetResponse::Available { mut asset } = outcome else {
      panic!("expected availability");
   };
   let mut meta = String::new();
   asset
      .take_stream("meta.json")
      .unwrap()
      .read_to_string(&mut meta)
      .unwrap();
   assert_eq!(meta, "{\"format\":4}");

   let model = asset.take_random("model.bin").unwrap();
   let mut start = [0u8; 7];
   model.read_at_exact(0, &mut start).unwrap();
   assert_eq!(&start, b"weights");

   // The archive itself was never placed.
   let revision_dir = root.path().join("tokenizer/en/20260830");
   assert!(!revision_dir.join("pack.zip").exists());
   assert!(revision_dir.join("system/model.bin").exists());
}

#[tokio::test]
async fn an_archive_digest_mismatch_places_nothing() {
   let remote = tempfile::tempdir().unwrap();
   let root = tempfile::tempdir().unwrap();

   let archive = fixture_zip(&[("meta.json", b"{}".as_slice())]);
   let path = remote.path().join("pack.zip");
   std::fs::write(&path, &archive).unwrap();
   let file = FileSource::new("pack.zip", Locator::File(path), sha256_of(b"other bytes"))
      .extracted(ArchiveFormat::Zip);

   let engine = engine_over(root.path(), AssetSource::new("20260830", vec![file]));
   let outcome = engine
      .asset(AssetRequest::new(
         "tokenizer/en",
         [("meta.json", AccessKind::Stream)],
      ))
      .await;

   let AssetResponse::Unavailable { reason } = outcome else {
      panic!("expected unavailability");
   };
   assert!(reason.contains("digest mismatch"), "{reason}");
   assert!(!root.path().join("tokenizer/en/20260830").exists());
}

#[tokio::test]
async fn a_traversal_entry_never_escapes_the_store() {
   let remote = tempfile::tempdir().unwrap();
   let root = tempfile::tempdir().unwrap();

   let archive = fixture_zip(&[("../../evil.txt", b"escape".as_slice())]);
   let source = AssetSource::new("20260830", vec![archive_source(remote.path(), &archive)]);
   let engine = engine_over(root.path(), source);

   // Rejected or skipped — either way nothing may land outside the
   // staging directory it was extracted into.
   let _ = engine
      .asset(AssetRequest::new(
         "tokenizer/en",
         [("evil.txt", AccessKind::Stream)],
      ))
      .await;

   assert!(!root.path().join("evil.txt").exists());
   assert!(!root.path().join(".staging/evil.txt").exists());
   assert!(!root.path().parent().unwrap().join("evil.txt").exists());
}

#[tokio::test]
async fn duplicate_names_across_the_extracted_tree_stay_ambiguous() {
   let remote = tempfile::tempdir().unwrap();
   let root = tempfile::tempdir().unwrap();

   let archive = fixture_zip(&[("dup.txt", b"top".as_slice()), ("sub/dup.txt", b"nested")]);
   let source = AssetSource::new("20260830", vec![archive_source(remote.path(), &archive)]);
   let engine = engine_over(root.path(), source);

   let outcome = engine
      .asset(AssetRequest::new(
         "tokenizer/en",
         [("dup.txt", AccessKind::Stream)],
      ))
      .await;

   let AssetResponse::Unavailable { reason } = outcome else {
      panic!("expected unavailability");
   };
   assert!(reason.contains("ambiguous"), "{reason}");
}

#[tokio::test]
async fn archives_mix_with_plain_files_all_or_nothing() {
   let remote = tempfile::tempdir().unwrap();
   let root = tempfile::tempdir().unwrap();

   let archive = fixture_zip(&[("model.bin", b"weights".as_slice())]);
   let plain = remote.path().join("rules.txt");
   std::fs::write(&plain, b"rule one").unwrap();

   let source = AssetSource::new(
      "20260830",
      vec![
         archive_source(remote.path(), &archive),
         FileSource::new("rules.txt", Locator::File(plain), sha256_of(b"rule one")),
      ],
   );
   let engine = engine_over(root.path(), source);

   let outcome = engine
      .asset(AssetRequest::new(
         "tokenizer/en",
         [
            ("model.bin", AccessKind::Random),
            ("rules.txt", AccessKind::AssetPath),
         ],
      ))
      .await;
   assert!(matches!(outcome, AssetResponse::Available { .. }));
}
