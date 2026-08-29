# lambda-demo

assetify inside an AWS Lambda function (Rust runtime): **cache-only mode over
assets bundled into the deployment package** — deterministic, no network, and
happy on Lambda's read-only filesystem. The committed `assets/` tree is the
cache; the handler serves from it and returns a JSON summary.

## Run locally (needs [cargo-lambda](https://www.cargo-lambda.info))

```sh
cargo lambda watch
# in another shell:
cargo lambda invoke lambda-demo --data-file event.json
```

Expected response:

```json
{ "id": "nlp/tokenizer/en", "revisionMeta": "{\"format\":1,\"language\":\"en\"}\n", "indexBytes": 22 }
```

Without cargo-lambda, `cargo build` verifies the function compiles.

## Deploy

```sh
cargo lambda build --release
cargo lambda deploy
```

The `[package.metadata.lambda.deploy] include = ["assets"]` entry ships the
fixture tree next to the `bootstrap` binary, where the handler finds it under
`LAMBDA_TASK_ROOT`.
