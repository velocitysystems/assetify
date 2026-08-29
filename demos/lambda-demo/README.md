# lambda-demo

assetify inside an AWS Lambda function (Rust runtime): **cache-only mode over
assets bundled into the deployment package** — deterministic, no network, and
happy on Lambda's read-only filesystem. The shared fixture tree at
`demos/assets` is the cache; the handler serves from it and returns a JSON
summary.

## Run locally (needs [cargo-lambda](https://www.cargo-lambda.info))

```sh
cargo lambda watch
# in another shell:
cargo lambda invoke lambda-demo --data-file event.json
```

Expected response:

```json
{ "consistent": true, "id": "tokenizer/en", "indexEntries": 1000, "language": "en", "sampleTokens": ["falcon", "pyramid", "starfish"], "vocabWords": 1000 }
```

Without cargo-lambda, `cargo build` verifies the function compiles.

## Deploy

```sh
cp -R ../assets assets   # bring the shared fixtures into the package (gitignored)
cargo lambda build --release
cargo lambda deploy
```

The `[package.metadata.lambda.deploy] include = ["assets"]` entry ships the
copied tree next to the `bootstrap` binary, where the handler finds it under
`LAMBDA_TASK_ROOT`. Locally, the handler falls back to `demos/assets`
directly — no copy needed.
