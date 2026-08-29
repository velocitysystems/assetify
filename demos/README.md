# assetify demos

Three bare-bones apps proving assetify inside real platform shells. Each is a
standalone project (not a workspace member, excluded from the published
crate), depending on assetify by path. The crate's `examples/` tour the API;
these demos prove the *integrations*.

| Demo | Proves | Run |
| --- | --- | --- |
| [`tauri-demo`](tauri-demo/) | assetify behind Tauri IPC — the webview renders a summary served from Rust; desktop, iOS, Android | `cargo run` / `npm run tauri -- ios dev` |
| [`node-demo`](node-demo/) | assetify behind a napi-rs addon — reads stay in Rust, JS gets a summary | `npm install && npm run build && npm start` |
| [`lambda-demo`](lambda-demo/) | assetify in AWS Lambda — cache-only over assets bundled in the package | `cargo lambda watch` + `cargo lambda invoke` |

All three load the same fixture asset from the shared tree at
[`assets/`](assets/) (`tokenizer/en`, revision `20260821`): a model card
(`meta.json`, read as a **stream**), a binary offset index (`index.dat`, read
by **random access**), and a real 1,000-word English vocabulary (`vocab.txt`,
read by **path**, as a library would; sampled from the
[EFF large wordlist](https://www.eff.org/dice), CC BY 3.0). Each demo looks
tokens up *through* the index — a positioned read fetches an entry's byte
offset, and the token is sliced from the vocabulary at that offset — so the
`sampleTokens` in the output are words that can only appear if every access
kind read real content; `consistent: true` cross-checks all three files
against each other. Each platform carries the tree its own way: bundled
beside the Lambda binary, served in place by Node, compiled into the Tauri
app.
