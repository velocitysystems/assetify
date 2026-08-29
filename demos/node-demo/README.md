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
  "id": "nlp/tokenizer/en",
  "revisionMeta": "{\"format\":1,\"language\":\"en\"}",
  "indexBytes": 21
}
```
