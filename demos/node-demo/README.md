# node-demo

assetify behind a [napi-rs](https://napi.rs) addon: JavaScript calls one async
function, every asset read happens on the Rust side, and only a serializable
summary crosses the JS bridge — assetify's Node.js embedding story in ~60
lines of Rust and 10 of JS.

## Run

```sh
npm install
npm run build
npm start
```

Expected output:

```json
{
  "id": "tokenizer/en",
  "language": "en",
  "vocabWords": 1000,
  "indexEntries": 1000,
  "sampleTokens": ["falcon", "pyramid", "starfish"],
  "consistent": true
}
```

The addon serves the shared fixture tree at `demos/assets` in place
(cache-only mode over a read-only root); every file read happens in Rust.
