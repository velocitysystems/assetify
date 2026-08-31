//! **The client** — the host side of the seam.
//!
//! The analyzer *declares*; the client *prepares*. The client owns
//! everything the analyzer must never learn: the **distribution
//! channel** (here `channel/`: a pack manifest naming the current
//! generation, its archive, and its digest — packed with the PoC,
//! standing in for a CDN), the cache root under its own app data,
//! and the assetify engine wired between them.
//!
//! assetify is what keeps this side thin: the client answers one
//! question — "where can this asset be acquired right now?"
//! ([`Resolver`]) — and assetify owns fetch, digest verification,
//! archive extraction, atomic placement, lane-scoped offline
//! fallback, and serving. The only thing that crosses to the
//! analyzer is a [`Provider`](assetify::Provider).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use assetify::{
   ArchiveFormat, AssetSource, Assetify, FileSource, Provider, ResolveError, Resolver,
};
use serde_json::Value;

/// The host application, reduced to its asset-IO duties.
pub struct Client {
   engine: Assetify,
   /// The cache root's guard. Generations live here for the client's
   /// lifetime; a real app uses a persistent app-data dir instead —
   /// which is how an offline launch keeps working on what it has.
   _cache: tempfile::TempDir,
}

impl Client {
   /// The client's whole asset-IO setup: channel + cache root +
   /// engine. A real app would also inject its business rules here
   /// (`.fetch_policy(..)` — an offline-mode switch, a metered
   /// connection) and its transport (`.fetcher(..)` — a native
   /// background downloader).
   pub fn new() -> Result<Client> {
      let channel = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/lindera/channel");
      let cache = tempfile::tempdir().context("cannot create a cache root")?;
      let engine = Assetify::builder(cache.path())
         .resolver(PackResolver { channel })
         .build()?;
      Ok(Client {
         engine,
         _cache: cache,
      })
   }

   /// The one thing the client ever hands the analyzer. No channel,
   /// no cache path, no generation knowledge crosses with it.
   pub fn provider(&self) -> &dyn Provider {
      &self.engine
   }
}

/// The client's answer to "where can this asset be acquired right
/// now?" — resolved from its pack manifest at request time. That
/// timing is the point: generations publish out of band (a pack
/// update lands, the manifest moves, the next prepare picks it up),
/// and URLs can be minted against whatever endpoint is current. Here
/// the channel is a local directory; a real client fetches and
/// memoizes this manifest and uses `FileSource::url(..)` against its
/// live CDN.
struct PackResolver {
   channel: PathBuf,
}

#[async_trait::async_trait]
impl Resolver for PackResolver {
   async fn resolve(&self, id: &str) -> Result<Option<AssetSource>, ResolveError> {
      let manifest: Value = serde_json::from_slice(
         &std::fs::read(self.channel.join("manifest.json"))
            .map_err(|e| ResolveError::new(format!("pack manifest unreadable: {e}")))?,
      )
      .map_err(|e| ResolveError::new(format!("pack manifest unparsable: {e}")))?;

      // This pack carries one asset; a real manifest lists many.
      if manifest["asset"] != id {
         return Ok(None);
      }
      let field = |name: &str| {
         manifest[name]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| ResolveError::new(format!("manifest is missing {name:?}")))
      };

      // The generation is the client-side word for what assetify
      // calls a revision: a dated directory name whose string order
      // is age order. It never crosses to the analyzer.
      let generation = field("generation")?;
      let archive = FileSource::local(
         field("archive")?,
         self.channel.join(field("archive")?),
         &field("sha256")?,
      )
      .map_err(|e| ResolveError::new(format!("manifest digest invalid: {e}")))?
      .extracted(ArchiveFormat::Zip);

      Ok(Some(AssetSource::new(generation, vec![archive])))
   }
}
