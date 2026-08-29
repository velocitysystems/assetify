//! The batteries-included resolver: an in-code map from asset to
//! source. No trait to implement — the shortest path from "I have a
//! URL and a digest" to a served asset.

use std::collections::HashMap;

use crate::source::{AssetSource, ResolveError, SourceResolver};

/// A [`SourceResolver`] over a fixed in-code map of
/// `(id, format_major)` → [`AssetSource`].
///
/// The right tool when sources are known at build time or loaded from
/// the application's own configuration. Anything dynamic — a remote
/// catalog, per-user entitlements — implements [`SourceResolver`]
/// directly instead.
pub struct StaticResolver {
   sources: HashMap<(String, u32), AssetSource>,
}

impl StaticResolver {
   /// A resolver over `(id, format_major, source)` entries.
   pub fn new<I, S>(entries: I) -> Self
   where
      I: IntoIterator<Item = (S, u32, AssetSource)>,
      S: Into<String>,
   {
      StaticResolver {
         sources: entries
            .into_iter()
            .map(|(id, major, source)| ((id.into(), major), source))
            .collect(),
      }
   }
}

#[async_trait::async_trait]
impl SourceResolver for StaticResolver {
   async fn resolve(
      &self,
      id: &str,
      format_major: u32,
   ) -> Result<Option<AssetSource>, ResolveError> {
      Ok(self.sources.get(&(id.to_string(), format_major)).cloned())
   }
}

#[cfg(test)]
mod tests {
   use super::*;
   use crate::digest::Digest;
   use crate::source::{FileSource, Locator};

   #[tokio::test]
   async fn resolves_known_entries_and_declines_unknown_ones() {
      let digest =
         Digest::sha256_hex("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
            .unwrap();
      let resolver = StaticResolver::new([(
         "nlp/tokenizer/en",
         4,
         AssetSource::new(
            "20260821",
            vec![FileSource::new(
               "meta.json",
               Locator::File {
                  path: "/somewhere/meta.json".into(),
               },
               digest,
            )],
         ),
      )]);

      let hit = resolver.resolve("nlp/tokenizer/en", 4).await.unwrap();
      assert_eq!(hit.unwrap().revision, "20260821");

      assert!(
         resolver
            .resolve("nlp/tokenizer/en", 5)
            .await
            .unwrap()
            .is_none(),
         "a different lane is a different entry"
      );
      assert!(resolver.resolve("unknown", 4).await.unwrap().is_none());
   }
}
