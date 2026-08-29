# assetify

[![CI](https://github.com/velocitysystems/assetify/actions/workflows/ci.yml/badge.svg)](https://github.com/velocitysystems/assetify/actions/workflows/ci.yml)

Declare your assets; assetify fetches, verifies, caches, and serves them — as a
stream, random access, or a path, online or offline.

Assetify is for crates that embed data they don't ship: ML models, dictionaries,
lookup tables, structured data. The consumer declares *what* it needs — which
assets, which files, and how each file will be read — and assetify does the
rest: download (or ingest from disk), SHA-256 verification, an atomically
managed versioned cache, offline fallback, and the right backing per file
(buffered stream, memory map, plain descriptor, or a real path).

```rust
use assetify::{
   AccessKind, AssetRequest, AssetSource, Assetify, Digest, FileSource, FileSpec, Locator,
   StaticResolver,
};

let engine = Assetify::builder(cache_dir)
   .resolver(StaticResolver::new([(
      "nlp/tokenizer/en",
      1,
      AssetSource::new(
         "20260821",
         vec![FileSource::new(
            "model.bin",
            Locator::HTTP { url: model_url },
            Digest::sha256_hex(model_sha256)?,
         )],
      ),
   )]))
   .build()?;

let outcome = engine
   .asset(AssetRequest::new(
      "nlp/tokenizer/en",
      1,
      vec![FileSpec::new("model.bin", AccessKind::Random)],
   ))
   .await;
```

## Access kinds: say how you read, not how to store

Each requested file names the shape it will be read in. First match wins:

| You are… | Declare |
|---|---|
| loading through a library that takes a filesystem path | `MaterializedPath` |
| seeking, reading byte ranges, or probing the file in place | `Random` |
| anything else (one forward parse) | `Stream` |

The backing is assetify's choice — `Random` files arrive memory-mapped when the
`mmap` feature is on (with an optional zero-copy `as_bytes()` window), and every
consumer runs correctly on the `read_at` floor alone. Don't care? Declaring
`MaterializedPath` for everything is legal and gives you plain paths.

## How it behaves

- **Two-tier versioning.** A request names a hard `format_major` lane — the
  payload format your build can read — and the cache lays out
  `<root>/<id>/v<lane>/<revision>/`. Which revision serves is the resolver's
  business; lanes are never crossed, so an app upgrade can't be handed a
  payload it cannot parse.
- **Offline-first.** If resolution or download fails, the newest verified
  revision already on disk serves instead. A missing asset is a degraded
  capability (`Unavailable` with a reason), never an error to handle.
- **All-or-nothing, never partial.** Files stage under the cache root, every
  digest is verified, and the complete set renames into place atomically.
  Readers never observe a half-written revision, placed directories are never
  mutated (what makes the memory maps sound), and concurrent writers — other
  threads or other processes — race harmlessly.
- **Poison memory.** A delivery that verified but failed *your* load is echoed
  back (`rejected`), marked on disk, and never served again; the asset recovers
  when the resolver names a newer revision.
- **Single-flight.** Concurrent requests for one asset coalesce into one
  download.

## Feature flags

| Feature | Default | Provides |
|---|---|---|
| `mmap` | ✓ | memory-mapped `Random` backing with the zero-copy window |
| `http` | | downloading via `Locator::HTTP` (reqwest + rustls) |
| `test-util` | | `testing::MemoryProvider` — test your loading code with no filesystem or network |

## Where it runs

- **Desktop / mobile (e.g. Tauri):** pass your app data directory as the cache
  root; everything else is plain Rust.
- **AWS Lambda:** point the cache root at `/tmp` and enable `http`
  (`default-features = false, features = ["http"]`), or bundle assets into the
  deployment package and serve them in place — a builder without a resolver
  runs cache-only and accepts a read-only root.
- **Node.js (napi-rs):** assetify is a plain library on tokio; call it from
  `#[napi]` async functions. Reads stay on the Rust side — only your results
  cross the JS bridge. Expose long-running analysis as *async* functions so
  memory-map page faults never stall the event loop, and prefer an explicit
  `dispose()` over relying on the JS GC to drop Rust resources.

## License

MIT.
