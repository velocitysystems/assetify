# Agent Configuration (CLAUDE.md)

Guidance for coding agents working in this repository. Keep shared conventions here rather than in
a tool-specific file.

## Build and check

```
cargo lint-fmt         # rustfmt check (3-space indent; config in rustfmt.toml)
cargo lint-clippy      # clippy, all targets + features, -D warnings
cargo test --all-features
cargo test --no-default-features   # the serverless profile must stay green
```

Run all four before submitting anything. The toolchain is pinned (`rust-toolchain.toml`); demos
inherit it and the rustfmt config by directory discovery. Demos build in their own directories
(`demos/*/`), each with its own lockfile.

## Commits

Follow [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/):
`<type>: <description>`.

- Single line only — no body, no footer. Lowercase after the type, imperative mood,
  72 characters max, no trailing period (e.g. `feat: add versioned on-disk store`).
- No agent attribution: no `Co-Authored-By` trailer naming the tool, and no "Generated with …"
  line. Commits are attributed to the human author only.
- Renames and moves land in their own commits, separate from content changes.
