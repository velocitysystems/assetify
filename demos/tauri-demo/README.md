# tauri-demo

assetify inside a **headless** Tauri 2 app — no windows, no webview
(`"windows": []`): assets load in the `setup` hook against the app's real
cache directory (`app_cache_dir()`), exactly where a shipping app would warm
them up. The `src-web/` stub only satisfies Tauri's frontend requirement;
nothing renders.

## Run on desktop

```sh
cd src-tauri
cargo run
```

Expected output: `delivered` from assetify's lifecycle events, then
`assets loaded in the app shell`, and the app exits.

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
