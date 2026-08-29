//! The built-in [`Fetcher`]: HTTP(S) via reqwest.

use std::io::Write;

use crate::source::fetch::{FetchError, Fetcher};

/// A [`Fetcher`] over a [`reqwest::Client`].
///
/// Configure the client with reqwest's own builder — user agent,
/// timeouts, proxies, TLS — and hand it over:
///
/// ```no_run
/// # fn demo() -> Result<(), Box<dyn std::error::Error>> {
/// use assetify::ReqwestFetcher;
///
/// let client = reqwest::Client::builder()
///    .user_agent("my-app/1.4.0")
///    .build()?;
/// let fetcher = ReqwestFetcher::new(client);
/// # Ok(())
/// # }
/// ```
///
/// Built and wired automatically when the `reqwest` feature is enabled
/// and no fetcher is supplied.
pub struct ReqwestFetcher {
   client: reqwest::Client,
}

impl ReqwestFetcher {
   /// A fetcher over an already-configured client.
   pub fn new(client: reqwest::Client) -> Self {
      ReqwestFetcher { client }
   }
}

#[async_trait::async_trait]
impl Fetcher for ReqwestFetcher {
   async fn fetch(&self, url: &str, sink: &mut (dyn Write + Send)) -> Result<(), FetchError> {
      let mut response = self
         .client
         .get(url)
         .send()
         .await
         .map_err(|e| FetchError::new(format!("GET {url} failed: {e}")))?;
      let status = response.status();
      if !status.is_success() {
         return Err(FetchError::new(format!("GET {url} returned {status}")));
      }

      loop {
         let chunk = response
            .chunk()
            .await
            .map_err(|e| FetchError::new(format!("GET {url} failed mid-body: {e}")))?;
         let Some(chunk) = chunk else { break };
         sink
            .write_all(&chunk)
            .map_err(|e| FetchError::new(format!("cannot write to staging: {e}")))?;
      }
      Ok(())
   }
}
