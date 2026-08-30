//! The contract as runnable assertions: any [`RandomAccess`] backing
//! must behave identically from the consumer's point of view, window
//! or no window — and a [`Provider`] must deliver by name, in request
//! order, with loud named gaps.

use std::io::Read;
use std::sync::Arc;

use assetify::testing::{MemoryAsset, MemoryProvider, WindowMode};
use assetify::{AssetRequest, AssetResponse, Provider, RandomAccess};

const PAYLOAD: &[u8] = b"the quick brown fox jumps over the lazy dog";

/// Every backing a provider delivers must behave identically from the
/// consumer's point of view, regardless of how it answers `as_bytes`.
fn assert_random_access_conformant(access: &dyn RandomAccess) {
   let len = PAYLOAD.len() as u64;
   assert_eq!(access.len(), len);
   assert!(!access.is_empty());

   // A read straddling the end is truncated, never padded.
   let mut whole = vec![0u8; PAYLOAD.len() + 16];
   let mut filled = 0;
   let mut offset = 0u64;
   loop {
      let n = access.read_at(offset, &mut whole[filled..]).unwrap();
      if n == 0 {
         break;
      }
      filled += n;
      offset += n as u64;
   }
   assert_eq!(&whole[..filled], PAYLOAD);

   // read_at_exact assembles short reads and refuses ranges the file
   // cannot fill.
   let mut head = [0u8; 9];
   access.read_at_exact(0, &mut head).unwrap();
   assert_eq!(&head, &PAYLOAD[..9]);
   let mut too_far = [0u8; 4];
   let err = access.read_at_exact(len - 2, &mut too_far).unwrap_err();
   assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);

   // The window is optional, but when offered it must be the file.
   if let Some(bytes) = access.as_bytes() {
      assert_eq!(bytes, PAYLOAD);
   }
}

fn provider(mode: WindowMode) -> MemoryProvider {
   MemoryProvider::new(mode).with_asset(
      "tokenizer/en",
      MemoryAsset::new()
         .with_file("meta.json", br#"{"format":1}"#.to_vec())
         .with_file("model.bin", (0u8..=255).collect::<Vec<u8>>())
         .with_file("index.dat", PAYLOAD.to_vec()),
   )
}

fn fixture_request() -> AssetRequest {
   AssetRequest::new("tokenizer/en", vec!["meta.json", "model.bin", "index.dat"])
}

#[tokio::test]
async fn every_window_mode_is_consumer_equivalent() {
   let mut resident_views = Vec::new();

   for mode in [
      WindowMode::Offered,
      WindowMode::Declined,
      WindowMode::ShortReads,
   ] {
      let outcomes = provider(mode).provide(&[fixture_request()]).await;
      let AssetResponse::Available { asset } = outcomes.into_iter().next().unwrap() else {
         panic!("fixture asset must be available under {mode:?}");
      };

      // Stream: drained in one forward pass.
      let mut stream = asset
         .file("meta.json")
         .expect("no named gaps")
         .stream()
         .unwrap();
      let mut drained = Vec::new();
      stream.read_to_end(&mut drained).unwrap();
      assert_eq!(drained, br#"{"format":1}"#);

      // Load-time use of Random: ranged reads in arbitrary order.
      let weights = asset.file("model.bin").unwrap().random().unwrap();
      let mut high = [0u8; 4];
      weights.read_at_exact(200, &mut high).unwrap();
      assert_eq!(high, [200, 201, 202, 203]);
      let mut low = [0u8; 4];
      weights.read_at_exact(0, &mut low).unwrap();
      assert_eq!(low, [0, 1, 2, 3]);

      // Resident-style use of Random: identical bytes whether the
      // window is offered, declined, or trickled.
      let index = asset.file("index.dat").unwrap().random().unwrap();
      assert_random_access_conformant(index.as_ref());
      let view = match index.as_bytes() {
         Some(bytes) => bytes.to_vec(),
         None => {
            let mut copy = vec![0u8; index.len() as usize];
            index.read_at_exact(0, &mut copy).unwrap();
            copy
         }
      };
      resident_views.push(view);
      assert_eq!(
         index.as_bytes().is_some(),
         mode == WindowMode::Offered,
         "window offered exactly in Offered mode"
      );
   }

   assert!(
      resident_views.windows(2).all(|w| w[0] == w[1]),
      "consumer-visible bytes are identical across window modes"
   );
}

#[tokio::test]
async fn resident_access_is_shareable_across_threads() {
   let outcomes = provider(WindowMode::ShortReads)
      .provide(&[fixture_request()])
      .await;
   let AssetResponse::Available { asset } = outcomes.into_iter().next().unwrap() else {
      panic!("fixture asset must be available");
   };
   let index = asset.file("index.dat").unwrap().random().unwrap();

   // &self positioned reads from many threads at once.
   let shared: Arc<dyn RandomAccess> = Arc::from(index);
   let handles: Vec<_> = (0..8)
      .map(|i| {
         let access = Arc::clone(&shared);
         std::thread::spawn(move || {
            let mut buf = [0u8; 5];
            access.read_at_exact((i % 4) as u64, &mut buf).unwrap();
            assert_eq!(&buf, &PAYLOAD[(i % 4) as usize..(i % 4) as usize + 5]);
         })
      })
      .collect();
   for handle in handles {
      handle.join().unwrap();
   }
}

#[tokio::test]
async fn outcomes_arrive_in_request_order_and_gaps_are_named() {
   let requests = [
      AssetRequest::new("tokenizer/en", vec!["missing.dat"]),
      fixture_request(),
      AssetRequest::new("no/such/asset", Vec::<&str>::new()),
   ];
   let outcomes = provider(WindowMode::Offered).provide(&requests).await;
   assert_eq!(outcomes.len(), 3, "one outcome per request, in order");

   let AssetResponse::Unavailable { reason } = &outcomes[0] else {
      panic!("a named gap must be unavailable");
   };
   assert!(
      reason.contains("missing.dat"),
      "gap names the file: {reason}"
   );

   assert!(matches!(&outcomes[1], AssetResponse::Available { .. }));

   let AssetResponse::Unavailable { reason } = &outcomes[2] else {
      panic!("an unknown asset must be unavailable");
   };
   assert!(reason.contains("no/such/asset"), "names the id: {reason}");
}

#[tokio::test]
async fn memory_provider_delivers_but_has_no_path() {
   let request = AssetRequest::new("tokenizer/en", vec!["index.dat"]);
   let outcomes = provider(WindowMode::Offered).provide(&[request]).await;
   let AssetResponse::Available { asset } = &outcomes[0] else {
      panic!("the file is available, just not on a filesystem");
   };
   // The file reads fine, but there is no path to hand a
   // path-taking library — that's a filesystem-backed provider's job.
   let file = asset.file("index.dat").unwrap();
   assert!(file.stream().is_ok());
   assert!(
      file.path().is_none(),
      "an in-memory provider has no filesystem path"
   );
}
