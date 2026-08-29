# assetify

[![CI](https://github.com/velocitysystems/assetify/actions/workflows/ci.yml/badge.svg)](https://github.com/velocitysystems/assetify/actions/workflows/ci.yml)

Declare your assets; assetify fetches, verifies, caches, and serves them — as
a stream, random access, or a path, online or offline. For crates and
applications that embed data they don't ship: ML models, dictionaries, lookup
tables, structured data.

## Features

- **Verified** — every file is SHA-256-checked before it reaches the cache
- **Atomic** — a revision lands whole or not at all, and is never mutated
- **Lane-versioned** — an upgraded app is never handed a payload it can't parse
- **Offline-first** — acquisition failures fall back to the newest revision on disk
- **Access by intent** — files arrive as streams, random access (mmap), or real paths
- **Poisoning** — a payload that failed your load is never re-served
- **Single-flight** — concurrent requests for one asset share one download
- **Testable** — an in-memory provider runs your loading code without disk or network

## Installation

```sh
cargo add assetify
```

Downloading over HTTP(S) is feature-gated; enable it if your assets are
remote:

```toml
[dependencies]
assetify = { version = "0.1", features = ["http"] }
```

## Quick start

```rust
use assetify::{
   AccessKind, AssetResponse, AssetRequest, AssetSource, Assetify, FileSource, FileSpec,
   StaticResolver,
};
```

**1. Say where the asset lives** — a revision and its files, each a URL plus
its SHA-256:

```rust
let source = AssetSource::new(
   "20260821", // revision: newest (lexicographically) wins
   vec![FileSource::http(
      "model.bin",
      "https://assets.example.com/tokenizer/20260821/model.bin",
      "…the file's sha-256, 64 hex chars…",
   )?],
);
```

**2. Build an engine** over a cache directory, registering the source under
the asset's id and format lane:

```rust
let engine = Assetify::builder("/var/cache/my-app/assets")
   .resolver(StaticResolver::new([("nlp/tokenizer/en", 1, source)]))
   .build()?;
```

**3. Ask for the asset**, naming the files you need and how you'll read them:

```rust
let outcome = engine
   .asset(AssetRequest::new(
      "nlp/tokenizer/en",
      1,
      vec![FileSpec::new("model.bin", AccessKind::Random)],
   ))
   .await;

match outcome {
   AssetResponse::Available { asset } => { /* read the files — see Usage */ }
   AssetResponse::Unavailable { reason } => eprintln!("degraded: {reason}"),
}
```

The first request downloads, verifies, and caches; every later request — and
every request while offline — serves from disk.

## Usage

### Choosing an access kind

Declare how you'll *read* each file; assetify picks the backing. First match
wins:

| You are… | Declare |
|---|---|
| Loading through a library that takes a filesystem path | `AccessKind::MaterializedPath` |
| Seeking, reading byte ranges, or probing the file in place | `AccessKind::Random` |
| Anything else (one forward parse) | `AccessKind::Stream` |

Don't care? `MaterializedPath` for everything is legal and gives you plain
paths.

### Reading delivered files

Files come back by name, each behind the access object you declared:

```rust
use std::io::Read;
use assetify::FileAccess;

let mut asset = /* AssetResponse::Available { asset } */;

match asset.take_file("model.bin").unwrap().access {
   FileAccess::Stream(mut stream) => {
      let mut bytes = Vec::new();
      stream.read_to_end(&mut bytes)?;
   }
   FileAccess::Random(random) => {
      // Positioned reads from any thread…
      let mut header = [0u8; 16];
      random.read_at_exact(0, &mut header)?;
      // …or the whole file zero-copy, when the backing offers it.
      if let Some(bytes) = random.as_bytes() { /* mmap window */ }
   }
   FileAccess::Path(path) => {
      some_library::load_from(&*path)?; // a real, stable file path
   }
}
```

### Custom sources

One rule: **sources known up front → `StaticResolver`; sources computed at
runtime → implement `SourceResolver`** (one async method). Either way, a
resolver answers a single question — *where can this asset be acquired right
now?* — and assetify handles everything after that (download, verify, cache,
serve).

```rust
use assetify::{AssetSource, FileSource, ResolveError, SourceResolver};

/// Resolves against a manifest your app fetched from its own backend.
struct ManifestResolver {
   manifest: Manifest,
}

#[async_trait::async_trait]
impl SourceResolver for ManifestResolver {
   async fn resolve(
      &self,
      id: &str,
      format_major: u32,
   ) -> Result<Option<AssetSource>, ResolveError> {
      // Unknown asset: Ok(None) — assetify serves its cache, or
      // reports the asset unavailable.
      let Some(entry) = self.manifest.lookup(id, format_major) else {
         return Ok(None);
      };

      let mut files = Vec::new();
      for file in &entry.files {
         files.push(
            FileSource::http(&file.name, &file.url, &file.sha256)
               .map_err(|e| ResolveError::new(e.to_string()))?,
         );
      }
      Ok(Some(AssetSource::new(entry.revision.clone(), files)))
   }
}
```

Return `Err(ResolveError)` when resolution fails *right now* (offline, backend
down): assetify falls back to the newest revision already on disk. Resolvers
run on every request not already in flight — if resolution is expensive, cache
your own lookups inside it.

### Cache-only mode

Omit the resolver and assetify serves whatever the root already holds — the
root may be read-only. This is the shape for assets bundled into an AWS Lambda
deployment package or an app bundle:

```rust
let engine = Assetify::builder("/opt/bundled-assets").build()?;
```

Seed the tree in assetify's layout: `<root>/<id>/v<lane>/<revision>/<files>`.

### Handling unavailability

`Unavailable { reason }` is a degraded capability, not an error to branch on:
keep running and request again later. If a delivery verified but failed *your*
load (corrupt content, wrong schema), echo it back so the copy is never
re-served:

```rust
use assetify::RejectedDelivery;

let mut retry = request.clone();
retry.rejected = Some(RejectedDelivery { reason: "schema check failed".into() });
// The next provide poisons that revision and recovers via a newer one.
```

### Testing your consumer

With the `test-util` feature, `MemoryProvider` serves files from heap buffers
— no filesystem, no network. Its window modes prove your code works whether or
not a backing offers the zero-copy window:

```rust
use assetify::testing::{MemoryAsset, MemoryProvider, WindowMode};

let provider = MemoryProvider::new(WindowMode::Declined)
   .with_asset("nlp/tokenizer/en", MemoryAsset::new().with_file("model.bin", b"…".to_vec()));
```

### Logging

Assetify emits structured `tracing` events (`staged`, `placed`, `delivered`,
plus warnings for fallback and poison). Install any subscriber to see them:

```rust
tracing_subscriber::fmt().init();
```

## How it works

```
<root>/
├── .staging/                  downloads assemble and verify here…
└── nlp/tokenizer/en/          …then the whole set renames into place
    └── v1/                    the lane: format compatibility (hard)
        ├── 20260812/          revisions: freshness (soft, newest wins)
        └── 20260821/model.bin
```

Per request: validate the id and file names (they become paths — traversal is
rejected outright) → ask the resolver where bytes live → serve the named
revision from cache if present, otherwise fetch every file into staging,
verify every digest, and atomically rename the complete set into place → open
each requested file behind its declared access kind.

Two properties fall out of the layout. The **lane** (`format_major`) is never
crossed, so the offline fallback only ever serves payloads your build can
read. And placed revisions are **immutable** — new content is always a new
directory — so memory maps are safe and concurrent writers (threads *or*
processes) race harmlessly.

## Where it runs

Anywhere Rust does — assetify is a plain library on tokio, with no
platform-specific setup:

- **Desktop / mobile (e.g. Tauri)** — pass your app data directory as the
  cache root.
- **AWS Lambda** — cache to `/tmp` with the `http` feature, or run cache-only
  over assets bundled read-only into the deployment.
- **Node.js (napi-rs)** — call it from `#[napi]` async functions; reads stay
  in Rust, only results cross the JS bridge.
- **Not WASM/browser** — assetify is tokio + filesystem + mmap; the browser
  has none of these.

## Where it fits

- **ML inference apps** — models, tokenizers, dictionaries fetched on first
  run and served from cache after; the demos exercise exactly this shape.
  Large-model niceties (download progress, resumption) are not built in yet —
  `tracing` events are the current signal.
- **Data-file CLIs and servers** — timezone, geo, and lookup databases;
  concurrent engines over one cache root are race-safe by design.
- **Asset packs with many files** — works today at the cost of one request
  per file; archive support can arrive later as a non-breaking
  `AssetSource` addition.
- **Mobile and serverless** — see [`demos/`](demos/): verified on the iOS
  simulator and via a local Lambda invoke.
- **Private sources** — presigned URLs work today; authenticated requests
  (custom headers) are the expected next `Locator` capability.

## Feature flags

| Feature | Default | Provides |
|---|---|---|
| `mmap` | ✓ | memory-mapped `Random` backing with the zero-copy window |
| `http` | | downloading via `Locator::HTTP` (reqwest + rustls) |
| `test-util` | | `testing::MemoryProvider` for consumer tests |

## Examples

```sh
cargo run --example local_assets
cargo run --example http_assets --features http
```

Both are self-contained (temp directories; the HTTP one runs its own mock
server) and log the engine's lifecycle: `staged` per verified file, `placed`
for the committed revision, `delivered` for the served asset.

## License

MIT.
