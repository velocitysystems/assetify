//! The contract as runnable assertions: any [`RandomAccess`] backing
//! must behave identically from the consumer's point of view, window
//! or no window — and a [`Provider`] must deliver by name, in request
//! order, with loud named gaps.

use std::io::Read;
use std::sync::Arc;

use assetify::testing::{MemoryAsset, MemoryProvider, WindowMode};
use assetify::{
   AccessKind, AssetRequest, AssetResponse, FileAccess, FileRequest, Provider, RandomAccess,
};

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
   AssetRequest::new(
      "tokenizer/en",
      vec![
         FileRequest::new("meta.json", AccessKind::Stream),
         FileRequest::new("model.bin", AccessKind::Random),
         FileRequest::new("index.dat", AccessKind::Random),
      ],
   )
}

#[test]
fn materialized_path_dereferences_to_its_path() {
   let materialized = assetify::AssetPath::new("/data/assets/index.dat");
   assert_eq!(
      materialized.file_name().and_then(|n| n.to_str()),
      Some("index.dat"),
      "Deref<Target = Path> exposes Path methods directly"
   );
   assert_eq!(
      materialized.as_path(),
      std::path::Path::new("/data/assets/index.dat")
   );
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
      let AssetResponse::Available { mut asset } = outcomes.into_iter().next().unwrap() else {
         panic!("fixture asset must be available under {mode:?}");
      };

      // Kind checks at the boundary, before any payload work.
      for spec in &fixture_request().files {
         let file = asset.file(&spec.name).expect("no named gaps");
         assert!(
            file.access.satisfies(spec.access),
            "kind mismatch on {:?} under {mode:?}",
            spec.name
         );
      }

      // Stream: drained in one forward pass.
      let FileAccess::Stream(mut stream) = asset.take_file("meta.json").unwrap().access else {
         unreachable!()
      };
      let mut drained = Vec::new();
      stream.read_to_end(&mut drained).unwrap();
      assert_eq!(drained, br#"{"format":1}"#);

      // Load-time use of Random: ranged reads in arbitrary order.
      let FileAccess::Random(weights) = asset.take_file("model.bin").unwrap().access else {
         unreachable!()
      };
      let mut high = [0u8; 4];
      weights.read_at_exact(200, &mut high).unwrap();
      assert_eq!(high, [200, 201, 202, 203]);
      let mut low = [0u8; 4];
      weights.read_at_exact(0, &mut low).unwrap();
      assert_eq!(low, [0, 1, 2, 3]);

      // Resident-style use of Random: identical bytes whether the
      // window is offered, declined, or trickled.
      let FileAccess::Random(index) = asset.take_file("index.dat").unwrap().access else {
         unreachable!()
      };
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
   let AssetResponse::Available { mut asset } = outcomes.into_iter().next().unwrap() else {
      panic!("fixture asset must be available");
   };
   let FileAccess::Random(index) = asset.take_file("index.dat").unwrap().access else {
      unreachable!()
   };

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
      AssetRequest::new(
         "tokenizer/en",
         vec![FileRequest::new("missing.dat", AccessKind::Stream)],
      ),
      fixture_request(),
      AssetRequest::new("no/such/asset", Vec::<FileRequest>::new()),
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
async fn memory_provider_declines_materialized_paths_with_guidance() {
   let request = AssetRequest::new(
      "tokenizer/en",
      vec![FileRequest::new("index.dat", AccessKind::AssetPath)],
   );
   let outcomes = provider(WindowMode::Offered).provide(&[request]).await;
   let AssetResponse::Unavailable { reason } = &outcomes[0] else {
      panic!("MemoryProvider holds no filesystem");
   };
   assert!(
      reason.contains("filesystem-backed"),
      "guides the reader: {reason}"
   );
}
