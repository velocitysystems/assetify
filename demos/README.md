# assetify demos

Three bare-bones apps proving assetify inside real platform shells. Each is a
standalone project (not a workspace member, excluded from the published
crate), depending on assetify by path. The crate's `examples/` tour the API;
these demos prove the *integrations*.

| Demo | Proves | Run |
| --- | --- | --- |
| [`tauri-demo`](tauri-demo/) | assetify in a headless Tauri app (no UI) — desktop, iOS, Android | `cargo run` / `npm run tauri -- ios dev` |
| [`node-demo`](node-demo/) | assetify behind a napi-rs addon — reads stay in Rust, JS gets a summary | `npm install && npm run build && npm start` |
| [`lambda-demo`](lambda-demo/) | assetify in AWS Lambda — cache-only over assets bundled in the package | `cargo lambda watch` + `cargo lambda invoke` |

All three load the same fixture asset (`nlp/tokenizer/en`, revision
`20260821`) through one `engine.asset(...)` call and report what was
delivered. See each demo's README for prerequisites and expected output.
