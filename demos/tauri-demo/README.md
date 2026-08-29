# tauri-demo

assetify inside a Tauri 2 app: the engine is built in the `setup` hook over
the app's real cache directory (`app_cache_dir()`) and managed as state; the
webview invokes one `#[tauri::command]` over IPC and renders the summary it
gets back. Every asset read happens on the Rust side — only serializable
results cross the bridge.

## Run on desktop

```sh
cd src-tauri
cargo run
```

A window opens showing the delivered summary (vocabulary size, index
entries, sample tokens looked up through the binary index, and a
`consistent` cross-check); the console logs `delivered` from assetify's
lifecycle events. The shared fixture tree at `demos/assets` is compiled into
the binary, so devices and simulators carry the assets with the app. Close
the window to exit.

## Run on iOS simulator / Android emulator

The Tauri CLI is a local dev-dependency so the generated host projects
(gitignored under `src-tauri/gen/`) resolve it correctly — install it once
with `npm install`, then:

```sh
npm run tauri -- ios init                  # once; needs Xcode
npm run tauri -- ios dev "iPhone 17 Pro"   # builds, installs, launches

npm run tauri -- android init              # once; needs Android SDK + NDK
npm run tauri -- android dev               # builds, installs, launches
```

Pick any simulator name from `xcrun simctl list devices available`. The asset
logs appear in the `dev` command's console. On mobile the app stays alive
after loading (platforms dislike self-termination); on desktop it exits once
assets are served.
