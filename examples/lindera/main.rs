//! An analyzer and its client meeting over assetify, end to end: a
//! real Lindera dictionary (~34 MB zipped) is resolved from a pack
//! manifest, verified, extracted, atomically placed, and consumed —
//! two different ways — by a library that never learns where any of
//! that happened.
//!
//! Run with:
//!
//! ```sh
//! cargo run --example lindera --features zip
//! ```
//!
//! **The analyzer declares; the client prepares.** Two sides, one
//! seam:
//!
//! * **[`analyzer`]** — the consumer: a library that needs data
//!   files it does not ship. Its needs and its format contract are
//!   compiled in; it declares them against a
//!   [`Provider`](assetify::Provider) and consumes what comes back.
//!   It never sees a URL, a generation, a cache directory, or the
//!   client's code.
//! * **[`client`]** — the host: owns the distribution channel (the
//!   pack manifest and its archive), the cache root, and the
//!   assetify engine wired between them.
//!
//! This file is only the meeting point. It runs the same
//! `prepare()` twice — once per loader — to show that consumption
//! is an implementation detail behind an unchanged declaration: the
//! first prepare acquires (verify, extract, place, invisibly to the
//! analyzer), the second is a cache hit, and both analyze the same
//! sentence identically. A pack update landing later is just this
//! again: call prepare, the client's manifest names a newer
//! generation, the analyzer adopts it.

mod analyzer;
mod client;

use anyhow::{Result, ensure};

use crate::analyzer::{Analyzer, Loader};
use crate::client::Client;

const SENTENCE: &str = "서울에서 부산까지 기차로 세 시간 걸립니다";
const EXPECTED: [&str; 9] = [
   "서울",
   "에서",
   "부산",
   "까지",
   "기차",
   "로",
   "세",
   "시간",
   "걸립니다",
];

#[tokio::main]
async fn main() -> Result<()> {
   let client = Client::new()?;

   // First prepare acquires: resolve → verify → extract → place.
   let by_path = Analyzer::prepare(client.provider(), Loader::ByPath).await?;
   let via_path = by_path.analyze(SENTENCE)?;
   println!("loader ByPath:              {via_path:?}");

   // Second prepare is a cache hit — same generation, no
   // re-acquisition — consumed through the other loader.
   let by_objects = Analyzer::prepare(client.provider(), Loader::ByFileAccessObjects).await?;
   let via_objects = by_objects.analyze(SENTENCE)?;
   println!("loader ByFileAccessObjects: {via_objects:?}");

   ensure!(
      via_path == EXPECTED && via_objects == EXPECTED,
      "the loaders disagree with the expected analysis"
   );
   println!("OK: both loaders analyze identically behind one declaration");
   Ok(())
}
