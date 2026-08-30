//! The admission seam: whether the engine may acquire *right now*.
//!
//! A [`Resolver`] answers "where can this asset be acquired?"; a
//! [`FetchPolicy`] answers the separate question "may I go and get it
//! at this moment?" — the host's business rules (an offline-mode
//! switch, a metered connection, battery state) live here, not in
//! resolver or fetcher errors.
//!
//! Denial is deliberately gentle: the engine behaves exactly as if
//! resolution had failed — it serves the newest on-disk revision, and
//! reports the asset unavailable only when nothing is on disk. A
//! denied request therefore usually succeeds silently from cache,
//! which is the offline-first behavior a host wants for free. The
//! check runs *before* resolution, so a denied request does no
//! resolver or network work at all.
//!
//! [`Resolver`]: crate::Resolver

/// A [`FetchPolicy`]'s answer for one asset.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub enum Admission {
   /// Acquire as normal.
   Allow,
   /// Declined right now (offline mode, metered link, battery…).
   /// The engine falls back to the newest on-disk revision, else
   /// reports the asset unavailable with this reason.
   Deny {
      /// Why acquisition was declined; surfaces in `Unavailable`
      /// reasons when there is nothing to fall back to.
      reason: String,
   },
}

/// The host's "may I fetch right now?" hook, consulted once per
/// requested asset, before resolution. Omit it entirely (the default)
/// and every acquisition is allowed.
#[async_trait::async_trait]
pub trait FetchPolicy: Send + Sync {
   /// Whether asset `id` may be acquired at this moment.
   async fn admit(&self, id: &str) -> Admission;
}
