//! The provider seam: the one trait a consumer calls.

use crate::contract::delivery::AssetResponse;
use crate::contract::request::AssetRequest;

/// Makes requested assets readable. The consumer declares *what* it
/// needs ([`AssetRequest`]); the provider decides *how* — download,
/// cache, serve from disk, or synthesize in memory — and hands each
/// asset back behind access objects of the declared kinds.
///
/// `Send + Sync` because consumers are typically shared across
/// threads; `async` because provision may ride slow or flaky
/// networks.
///
/// One hard rule: **the access objects are synchronous, so a provider
/// must finish all slow work — download, verification, placement —
/// before handing them over.** A consumer probing a resident file
/// cannot await mid-read.
#[async_trait::async_trait]
pub trait Provider: Send + Sync {
   /// Make the requested assets readable and hand each one back
   /// behind access objects of the declared kinds. One outcome per
   /// request, in request order.
   async fn provide(&self, requests: &[AssetRequest]) -> Vec<AssetResponse>;

   /// Single-asset convenience over [`provide`](Provider::provide).
   async fn asset(&self, request: AssetRequest) -> AssetResponse {
      self
         .provide(std::slice::from_ref(&request))
         .await
         .into_iter()
         .next()
         .expect("provide returns one outcome per request")
   }
}
