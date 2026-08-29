//! An in-memory [`Provider`] for consumer tests: no filesystem, no
//! network, and a switchable window mode so consumers can prove they
//! run correctly whether or not a backing offers the zero-copy
//! window.

use std::collections::HashMap;
use std::io::{self, Read};
use std::sync::Arc;

use crate::access::MemoryRandom;
use crate::contract::access::{AccessKind, FileAccess, RandomAccess};
use crate::contract::delivery::{AssetResponse, PreparedAsset, PreparedFile};
use crate::contract::provider::Provider;
use crate::contract::request::AssetRequest;

/// How the provider's random-access objects answer
/// [`RandomAccess::as_bytes`] — and how honestly they fill reads.
/// Consumer code must be correct under every mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowMode {
   /// `as_bytes` returns the whole file: the mmap-like happy path.
   Offered,
   /// `as_bytes` returns `None`: consumers must fall back to
   /// `read_at`.
   Declined,
   /// `as_bytes` returns `None` and every `read_at` returns at most
   /// one byte: flushes out callers that assume full reads instead of
   /// using `read_at_exact`.
   ShortReads,
}

/// One asset's files, by name.
#[derive(Clone, Default)]
pub struct MemoryAsset {
   files: HashMap<String, Arc<Vec<u8>>>,
}

impl MemoryAsset {
   /// An asset with no files; add them with
   /// [`with_file`](MemoryAsset::with_file).
   pub fn new() -> Self {
      MemoryAsset::default()
   }

   /// Add one named file.
   pub fn with_file(mut self, name: impl Into<String>, bytes: impl Into<Vec<u8>>) -> Self {
      self.files.insert(name.into(), Arc::new(bytes.into()));
      self
   }
}

/// An in-memory [`Provider`]: a map of asset id → files, served
/// straight from heap buffers.
///
/// [`AccessKind::MaterializedPath`] requests are answered
/// `Unavailable` — this provider holds no filesystem. Consumers
/// testing path-based loading should use a filesystem-backed provider
/// over a temporary directory.
pub struct MemoryProvider {
   assets: HashMap<String, MemoryAsset>,
   mode: WindowMode,
}

impl MemoryProvider {
   /// An empty provider serving in the given window mode.
   pub fn new(mode: WindowMode) -> Self {
      MemoryProvider {
         assets: HashMap::new(),
         mode,
      }
   }

   /// Add one asset under an id.
   pub fn insert(&mut self, id: impl Into<String>, asset: MemoryAsset) {
      self.assets.insert(id.into(), asset);
   }

   /// Builder-style [`insert`](MemoryProvider::insert).
   pub fn with_asset(mut self, id: impl Into<String>, asset: MemoryAsset) -> Self {
      self.insert(id, asset);
      self
   }

   fn prepare_one(&self, request: &AssetRequest) -> AssetResponse {
      let Some(asset) = self.assets.get(&request.id) else {
         return AssetResponse::Unavailable {
            reason: format!("no asset registered under id {:?}", request.id),
         };
      };

      let mut files = Vec::with_capacity(request.files.len());
      for spec in &request.files {
         let Some(bytes) = asset.files.get(&spec.name) else {
            return AssetResponse::Unavailable {
               reason: format!("asset {:?} is missing file {:?}", request.id, spec.name),
            };
         };
         let access = match spec.access {
            AccessKind::Stream => FileAccess::Stream(Box::new(SliceReader {
               bytes: Arc::clone(bytes),
               pos: 0,
            })),
            AccessKind::Random => FileAccess::Random(self.random(Arc::clone(bytes))),
            AccessKind::MaterializedPath => {
               return AssetResponse::Unavailable {
                  reason: format!(
                     "MemoryProvider cannot materialize a path for {:?}; \
                      use a filesystem-backed provider",
                     spec.name
                  ),
               };
            }
         };
         files.push(PreparedFile::new(spec.name.clone(), access));
      }

      AssetResponse::Available {
         asset: PreparedAsset::new(files),
      }
   }

   fn random(&self, bytes: Arc<Vec<u8>>) -> Box<dyn RandomAccess> {
      match self.mode {
         WindowMode::Offered => Box::new(MemoryRandom::from_shared(bytes)),
         WindowMode::Declined => Box::new(NoWindow {
            inner: MemoryRandom::from_shared(bytes),
            trickle: false,
         }),
         WindowMode::ShortReads => Box::new(NoWindow {
            inner: MemoryRandom::from_shared(bytes),
            trickle: true,
         }),
      }
   }
}

#[async_trait::async_trait]
impl Provider for MemoryProvider {
   async fn provide(&self, requests: &[AssetRequest]) -> Vec<AssetResponse> {
      requests.iter().map(|r| self.prepare_one(r)).collect()
   }
}

/// Forward-only reader over shared bytes.
struct SliceReader {
   bytes: Arc<Vec<u8>>,
   pos: usize,
}

impl Read for SliceReader {
   fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
      let remaining = &self.bytes[self.pos.min(self.bytes.len())..];
      let n = buf.len().min(remaining.len());
      buf[..n].copy_from_slice(&remaining[..n]);
      self.pos += n;
      Ok(n)
   }
}

/// A backing that declines the window — and optionally trickles reads
/// out one byte at a time.
struct NoWindow {
   inner: MemoryRandom,
   trickle: bool,
}

impl RandomAccess for NoWindow {
   fn len(&self) -> u64 {
      self.inner.len()
   }

   fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
      if self.trickle && buf.len() > 1 {
         self.inner.read_at(offset, &mut buf[..1])
      } else {
         self.inner.read_at(offset, buf)
      }
   }

   // as_bytes: default `None` — the whole point of this wrapper.
}
