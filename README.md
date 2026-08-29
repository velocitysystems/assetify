# assetify

[![CI](https://github.com/velocitysystems/assetify/actions/workflows/ci.yml/badge.svg)](https://github.com/velocitysystems/assetify/actions/workflows/ci.yml)

Declare your assets; assetify fetches, verifies, caches, and serves them — as
a stream, random access, or a path, online or offline. For crates and
applications that embed data they don't ship: ML models, dictionaries, lookup
tables, structured data.

## Features

- **Verified** — every file is SHA-256-checked before it reaches the cache
- **Atomic** — a revision lands whole or not at all, and is never mutated
- **Versioned** — revisions are immutable; the newest one an asset holds wins
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
assetify = { version = "0.1", features = ["reqwest"] }
```

## Quick start

```rust
use assetify::{
   AccessKind, AssetRequest, AssetResponse, AssetSource, Assetify, FileSource, StaticResolver,
};
```

**1. Say where the asset lives** — a revision and its files, each a URL plus
its SHA-256:

```rust
let source = AssetSource::new(
   "20260821", // revision: newest (lexicographically) wins
   vec![FileSource::url(
      "model.bin",
      "https://assets.example.com/tokenizer/20260821/model.bin",
      "…the file's sha-256, 64 hex chars…",
   )?],
);
```

**2. Build an engine** over a cache directory, registering the source under
the asset's id:

```rust
let engine = Assetify::builder("/var/cache/my-app/assets")
   .resolver(StaticResolver::new([("nlp/tokenizer/en", source)]))
   .build()?;
```

**3. Ask for the asset**, naming the files you need and how you'll read them:

```rust
let outcome = engine
   .asset(AssetRequest::new(
      "nlp/tokenizer/en",
      [("model.bin", AccessKind::Random)],
   ))
   .await;

match outcome {
   AssetResponse::Available { asset } => { /* read the files — see Usage */ }
   AssetResponse::Unavailable { reason } => eprintln!("degraded: {reason}"),
}
```

What the first run logs:

```text
 INFO staged    asset=nlp/tokenizer/en revision=20260821 file=model.bin
 INFO placed    asset=nlp/tokenizer/en revision=20260821
 INFO delivered asset=nlp/tokenizer/en revision=20260821 files=1
```

Every later request — including every request made offline — skips straight to
serving from disk: the revision is cached under
`<root>/nlp/tokenizer/en/20260821/`.

## Usage

### Choosing an access kind

Declare how you'll *read* each file; assetify picks the backing. First match
wins:

| You are… | Declare |
|---|---|
| Loading through a library that takes a filesystem path | `AccessKind::AssetPath` |
| Seeking, reading byte ranges, or probing the file in place | `AccessKind::Random` |
| Anything else (one forward parse) | `AccessKind::Stream` |

Don't care? `AccessKind::AssetPath` for everything is legal and gives you plain
paths.

### Reading delivered files

Files come back by name. You know the kind you asked for, so take it
directly:

```rust
use std::io::Read;

let mut asset = /* AssetResponse::Available { asset } */;

// Stream: one forward parse — config, metadata, vocabularies.
let mut stream = asset.take_stream("meta.json").expect("requested as a stream");
let mut meta = String::new();
stream.read_to_string(&mut meta)?;

// Random: positioned reads from any thread, plus a zero-copy window
// when the backing offers one (mmap does).
let index = asset.take_random("index.dat").expect("requested as random access");
let mut header = [0u8; 16];
index.read_at_exact(0, &mut header)?;
if let Some(bytes) = index.as_bytes() { /* the whole file, zero-copy */ }

// AssetPath: a real, stable path — for libraries that insist on
// opening files themselves.
let path = asset.take_asset_path("rules.txt").expect("requested as a path");
some_library::load_from(&*path)?;
```

To handle any kind generically, match `take_file(name)`'s `FileAccess`
(`Stream`, `Random`, or `AssetPath`) instead.

### Static vs. dynamic resolvers

A resolver answers one question for the engine: *where can this asset be
acquired right now?* Everything after the answer — download, verify, cache,
offline fallback — is identical. The only choice you make is **when the
answer is decided**:

| | Static | Dynamic |
| --- | --- | --- |
| The answer is decided… | when you write the code | on every request |
| Assets can change… | with an app update or restart | while the app runs |
| You write… | a `StaticResolver` map | a type implementing `Resolver` |
| Typical case | URLs + checksums pinned per release | your backend publishes new revisions; per-user entitlements |

The decision test: *can "where does this asset live?" change while your app
is running?* No → `StaticResolver` (as in the quick start). Yes → implement
`Resolver`, one async method. Swapping later touches one line — the
`.resolver(…)` call.

A dynamic resolver that asks your backend which release of an asset is
current:

```rust
use assetify::{AssetSource, FileSource, ResolveError, Resolver};

/// Your type, named however you like — this one asks a backend API.
struct DynamicResolver {
   base_url: String, // e.g. "https://api.example.com"
}

#[async_trait::async_trait]
impl Resolver for DynamicResolver {
   async fn resolve(&self, id: &str) -> Result<Option<AssetSource>, ResolveError> {
      // GET {base_url}/releases/nlp/tokenizer/en returns e.g.
      //   { "version": "20260821",
      //     "files": [ { "name": "model.bin", "sha256": "9f86d0…" } ] }
      let Some(release) = fetch_release(&self.base_url, id).await? else {
         return Ok(None); // the backend has no such asset
      };

      let files = release
         .files
         .iter()
         .map(|f| {
            FileSource::url(
               &f.name,
               format!("{}/releases/{id}/{}/{}", self.base_url, release.version, f.name),
               &f.sha256,
            )
         })
         .collect::<Result<_, _>>()
         .map_err(|e| ResolveError::new(e.to_string()))?;

      Ok(Some(AssetSource::new(release.version, files)))
   }
}
```

The next time your backend's response says `"version": "20260901"`, every
device picks the new release up on its next request — no app update.

The three return values:

- `Ok(Some(source))` — acquire from here (a cached copy of that revision skips
  the network entirely).
- `Ok(None)` — you know of no source: assetify serves its cache, or reports
  the asset unavailable.
- `Err(…)` — resolution failed *right now* (offline, backend down): assetify
  falls back to the newest revision on disk.

Resolvers run on every request not already in flight — if resolution is
expensive, cache your own lookups inside it.

### Configuring the fetcher

URL sources are retrieved through a `Fetcher`. Three rungs, each one step up
in effort:

```rust
// 1. Nothing: the `reqwest` feature wires a default reqwest fetcher.

// 2. Configure the client — user agent, timeouts, proxies — with
//    reqwest's own builder, and hand it over:
let client = reqwest::Client::builder().user_agent("my-app/1.4.0").build()?;
let engine = Assetify::builder(root)
   .resolver(resolver)
   .fetcher(ReqwestFetcher::new(client))
   .build()?;

// 3. Bring your own client entirely: implement `Fetcher` (one async
//    method that streams a URL's bytes into a sink). Auth headers,
//    request signing, non-HTTP schemes — the URL is opaque to the
//    engine, and verification always stays on the engine's side.
```

### Cache-only mode

Omit the resolver and assetify serves whatever the root already holds — the
root may be read-only. This is the shape for assets bundled into an AWS Lambda
deployment package or an app bundle:

```rust
let engine = Assetify::builder("/opt/bundled-assets").build()?;
```

Seed the tree in assetify's layout: `<root>/<id>/<revision>/<files>`.

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

The poison marker persists on disk, so a restarted app never loops on the
same bad payload.

### Testing your consumer

With the `test-util` feature, `MemoryProvider` serves files from heap buffers
— no filesystem, no network. Its window modes prove your code works whether or
not a backing offers the zero-copy window:

```rust
use assetify::testing::{MemoryAsset, MemoryProvider, WindowMode};

let provider = MemoryProvider::new(WindowMode::Declined)
   .with_asset("nlp/tokenizer/en", MemoryAsset::new().with_file("model.bin", b"…".to_vec()));
```

Run your loader under all three modes — `Offered`, `Declined`, `ShortReads` —
and it is proven correct on every backing assetify will ever hand it.

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
    ├── 20260812/              revisions: immutable, newest wins
    └── 20260821/model.bin
```

Per request: validate the id and file names (they become paths — traversal is
rejected outright) → ask the resolver where bytes live → serve the named
revision from cache if present, otherwise fetch every file into staging,
verify every digest, and atomically rename the complete set into place → open
each requested file behind its declared access kind.

Two properties fall out of the layout. The **id is the compatibility
boundary** — fallback only ever picks among one asset's own revisions, so if
your payload format can change incompatibly, encode it in the id
(`nlp/tokenizer/en/v2`) and incompatible payloads are simply different
assets. And placed revisions are **immutable** — new content is always a new
directory — so memory maps are safe and concurrent writers (threads *or*
processes) race harmlessly.

## Where it runs

Anywhere Rust does — assetify is a plain library on tokio, with no
platform-specific setup:

- **Desktop / mobile (e.g. Tauri)** — pass your app data directory as the
  cache root.
- **AWS Lambda** — cache to `/tmp` with the `reqwest` feature, or run cache-only
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
| `http` | | downloading via `Locator::Url` (reqwest + rustls) |
| `test-util` | | `testing::MemoryProvider` for consumer tests |

## Examples

```sh
cargo run --example local_assets
cargo run --example http_assets --features reqwest
```

Both are self-contained (temp directories; the HTTP one runs its own mock
server) and log the engine's lifecycle: `staged` per verified file, `placed`
for the committed revision, `delivered` for the served asset.

## License

MIT.
