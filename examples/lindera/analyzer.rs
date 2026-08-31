//! **The analyzer** — the consumer side of the seam.
//!
//! The analyzer *declares*; the client *prepares*. Everything the
//! analyzer knows is compiled in: its **needs** (which asset, and
//! which files the asset must contain) and its **asset contract**
//! (the payload format major this build reads, folded into the asset
//! id so an incompatible payload is a different asset with a
//! disjoint revision tree). Everything else — where bytes live,
//! which generation is current, how acquisition works, whether the
//! device is offline — never crosses the seam: the analyzer's entire
//! view of the world is one [`Provider`].
//!
//! Note what the declaration does **not** carry: access shapes. A
//! delivered file answers any read shape on demand — a forward
//! stream, positioned reads, or its real location — so *how* the
//! delivery is consumed is an implementation detail of the loader,
//! not part of what the analyzer asks of a client. To prove it, this
//! PoC ships two interchangeable loaders behind the same
//! declaration and the same [`Analyzer::prepare`]:
//!
//! * [`Loader::ByPath`] — hand the wrapped library the
//!   delivered location; it does its own directory IO;
//! * [`Loader::ByFileAccessObjects`] — read every file through delivered
//!   access objects: forward streams for the small files, positioned
//!   reads (borrowing the zero-copy window when the backing offers
//!   one) for the large ones.
//!
//! A real analyzer compiles in exactly one; the choice is exposed
//! here only to show that nothing above the loader moves.

use std::io::Read;
use std::path::PathBuf;

use anyhow::{Context, Result, bail, ensure};
use assetify::{AssetRequest, AssetResponse, PreparedAsset, Provider};
use lindera::mode::Mode;
use lindera::segmenter::Segmenter;
use lindera::tokenizer::Tokenizer;
use lindera_dictionary::dictionary::Dictionary;
use lindera_dictionary::dictionary::character_definition::CharacterDefinition;
use lindera_dictionary::dictionary::connection_cost_matrix::ConnectionCostMatrix;
use lindera_dictionary::dictionary::prefix_dictionary::PrefixDictionary;
use lindera_dictionary::dictionary::unknown_dictionary::UnknownDictionary;

/// The needs id — the asset's logical name in the analyzer's own
/// namespace. No path, no version: those belong to other parties.
const NEEDS_ID: &str = "wordbreak/ko/dict";

/// The compile-time asset contract: the payload format major this
/// build reads. Published so the client's tooling can assert that
/// the assets it packages match the analyzer it links; folded into
/// the versioned id so offline fallback can never cross a format
/// break.
const DICT_FORMAT_MAJOR: u32 = 4;

/// The needs: every file the asset must contain, by name — the
/// compiled-in source of truth for what this analyzer asks of a
/// client. Even a loader that consumes the delivery as one location
/// still names every file: the names are the completeness contract,
/// so a gap fails loudly at the seam as a named gap instead of
/// surfacing later as the wrapped library choking on a half-filled
/// directory.
const NEEDS: [&str; 8] = [
   "metadata.json",
   "char_def.bin",
   "unk.bin",
   "dict.da",
   "dict.vals",
   "dict.words",
   "dict.wordsidx",
   "matrix.mtx",
];

/// How the loader consumes a delivery — the analyzer's own business,
/// invisible to the client. Both variants satisfy the identical
/// declaration.
#[derive(Clone, Copy, Debug)]
pub enum Loader {
   /// Hand the wrapped library a delivered file's real path — here,
   /// the dictionary directory derived from it — and let it read
   /// the files itself. Needs a disk-backed provider: a loader
   /// constraint discovered at read time, not a declared one.
   ByPath,
   /// Read every file through its delivered access object; no path
   /// crosses the seam at all.
   ByFileAccessObjects,
}

pub struct Analyzer {
   tokenizer: Tokenizer,
}

impl Analyzer {
   /// The one thing the analyzer ever sends across the seam: its
   /// needs, chunked into a request under the versioned id.
   fn request() -> AssetRequest {
      AssetRequest::new(
         AssetRequest::versioned_id(NEEDS_ID, DICT_FORMAT_MAJOR),
         NEEDS,
      )
   }

   /// Prepare: request the needs, validate the delivered payload,
   /// load through the chosen loader, adopt. Re-calling this is the
   /// whole update path — the analyzer states the same needs every
   /// time, and a repeat prepare is how a newer generation flows in
   /// once the client's channel points at one (the same generation
   /// is a cache hit).
   pub async fn prepare(provider: &dyn Provider, loader: Loader) -> Result<Analyzer> {
      let asset = deliver_asset(provider).await?;
      validate_asset(&asset)?;
      let dictionary = match loader {
         Loader::ByPath => {
            let location = location(&asset)?;
            lindera::dictionary::load_fs_dictionary(&location)
               .map_err(|e| anyhow::anyhow!("dictionary load from {}: {e}", location.display()))?
         }
         Loader::ByFileAccessObjects => dictionary_from_delivery(&asset)?,
      };
      Analyzer::adopt(dictionary)
   }

   pub fn analyze(&self, text: &str) -> Result<Vec<String>> {
      let tokens = self
         .tokenizer
         .tokenize(text)
         .map_err(|e| anyhow::anyhow!("analyze: {e}"))?;
      Ok(tokens.into_iter().map(|t| t.surface.into_owned()).collect())
   }

   /// Adopt a loaded dictionary as this analyzer's working state.
   /// (A long-lived engine would build the new state aside and swap
   /// it in whole, releasing the superseded one after in-flight
   /// analyses finish.)
   fn adopt(dictionary: Dictionary) -> Result<Analyzer> {
      Ok(Analyzer {
         tokenizer: Tokenizer::new(Segmenter::new(Mode::Normal, dictionary, None)),
      })
   }
}

/// One trip across the seam. An unavailable asset is a degraded
/// capability, never a crash — this PoC just surfaces it as an
/// error.
async fn deliver_asset(provider: &dyn Provider) -> Result<PreparedAsset> {
   match provider.asset(Analyzer::request()).await {
      AssetResponse::Available { asset } => Ok(asset),
      AssetResponse::Unavailable { reason } => bail!("asset unavailable: {reason}"),
   }
}

/// Check what the payload declares about itself before loading it —
/// one forward stream pass over the delivered metadata. A failure
/// here is exactly what would ride back to the provider on the next
/// request as a `RejectedDelivery` carrying this delivery's receipt,
/// so the provider poisons this copy rather than re-serving it
/// (elided in this PoC).
fn validate_asset(asset: &PreparedAsset) -> Result<()> {
   let metadata: serde_json::Value = serde_json::from_slice(&streamed(asset, "metadata.json")?)?;
   ensure!(
      metadata["dictionary_schema"]["fields"]
         .as_array()
         .is_some_and(|fields| !fields.is_empty()),
      "payload does not self-declare a dictionary schema this build reads"
   );
   Ok(())
}

/// The delivered location the wrapped library will read from — the
/// one place a path crosses the seam, and it crosses
/// provider→analyzer, never the reverse. The provider promises the
/// location stays in place, unmodified, as long as the delivery is
/// held. A provider serving from memory has no path, which surfaces
/// right here at read time.
fn location(asset: &PreparedAsset) -> Result<PathBuf> {
   Ok(named(asset, "metadata.json")?
      .path()
      .context("this loader needs a disk-backed provider")?
      .parent()
      .context("a delivered file has a parent directory")?
      .to_path_buf())
}

/// The access-object loader: rebuild the dictionary from delivered
/// reads, each file in the shape its use deserves. Nothing here is
/// visible across the seam — same request, same delivery, different
/// consumption.
fn dictionary_from_delivery(asset: &PreparedAsset) -> Result<Dictionary> {
   // Small files: one forward stream pass each.
   let metadata = serde_json::from_slice(&streamed(asset, "metadata.json")?)?;
   let character_definition = CharacterDefinition::load(&aligned(streamed(asset, "char_def.bin")?))
      .map_err(|e| anyhow::anyhow!("char_def: {e}"))?;
   let unknown_dictionary = UnknownDictionary::load(&aligned(streamed(asset, "unk.bin")?))
      .map_err(|e| anyhow::anyhow!("unk: {e}"))?;

   // Large files: positioned access.
   let connection_cost_matrix = ConnectionCostMatrix::load(random_bytes(asset, "matrix.mtx")?)
      .map_err(|e| anyhow::anyhow!("matrix: {e}"))?;
   let prefix_dictionary = PrefixDictionary::load(
      random_bytes(asset, "dict.da")?,
      random_bytes(asset, "dict.vals")?,
      random_bytes(asset, "dict.wordsidx")?,
      random_bytes(asset, "dict.words")?,
      true,
   )
   .map_err(|e| anyhow::anyhow!("prefix dict: {e}"))?;

   Ok(Dictionary {
      prefix_dictionary,
      connection_cost_matrix,
      character_definition,
      unknown_dictionary,
      metadata,
   })
}

/// Stream access: drain forward, hand back owned bytes, drop the
/// reader.
fn streamed(asset: &PreparedAsset, name: &str) -> Result<Vec<u8>> {
   let mut stream = named(asset, name)?.stream()?;
   let mut bytes = Vec::new();
   stream.read_to_end(&mut bytes)?;
   Ok(bytes)
}

/// Positioned-read access, window-or-materialize: borrow the
/// zero-copy window when the backing offers one, assemble `read_at`
/// calls when it declines. Owned either way here, because the
/// wrapped library keeps its own buffers.
fn random_bytes(asset: &PreparedAsset, name: &str) -> Result<Vec<u8>> {
   let random = named(asset, name)?.random()?;
   match random.as_bytes() {
      Some(window) => Ok(window.to_vec()),
      None => {
         let mut bytes = vec![0u8; usize::try_from(random.len())?];
         random.read_at_exact(0, &mut bytes)?;
         Ok(bytes)
      }
   }
}

fn named<'a>(asset: &'a PreparedAsset, name: &str) -> Result<&'a assetify::PreparedFile> {
   asset
      .file(name)
      .with_context(|| format!("{name} delivered — absence is a named gap"))
}

/// The binary components deserialize via rkyv, which wants 16-byte
/// alignment — the same buffer type the wrapped library's own file
/// loader uses.
fn aligned(bytes: Vec<u8>) -> rkyv::util::AlignedVec<16> {
   let mut buffer = rkyv::util::AlignedVec::<16>::new();
   buffer.extend_from_slice(&bytes);
   buffer
}
