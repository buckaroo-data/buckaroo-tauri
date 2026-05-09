# buckaroo-tauri-example

Minimal canonical example of embedding [buckaroo](https://github.com/paddymul/buckaroo) inside a Tauri 2 desktop app.

## Architecture

```
webview ──invoke()/listen()── Rust plugin (buckaroo-tauri) ──ws://127.0.0.1── python -m buckaroo.server
```

The webview never opens a WebSocket. All buckaroo data flows through the Rust plugin via Tauri IPC. CSP, cross-origin, and firewall concerns disappear by construction.

## Prerequisites

- Rust 1.77+ with `cargo`
- Node 18+ with `pnpm`
- A Python 3.11+ with buckaroo installed:
  ```
  pip install 'buckaroo[xorq]'
  ```
  (or `buckaroo[pandas]` / `buckaroo[polars]` matching the `BuckarooConfig::*` you choose in `lib.rs`)

If your Python isn't on PATH as `python3`, set the env var the plugin reads:
```
export BUCKAROO_PYTHON=/path/to/your/venv/bin/python
```

## Run

```
pnpm install
pnpm tauri dev
```

The window opens, the Rust plugin spawns the Python sidecar, parses `BUCKAROO_PORT=<n>` from stdout, opens an internal WS to it, and emits `buckaroo:sidecar_ready`. Paste a parquet path in the input, click Load, and watch `initial_state` flow through the IPC relay.

## What's wired

### `src-tauri/src/lib.rs`

```rust
fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(buckaroo_tauri::init(BuckarooConfig::xorq()))
        .run(tauri::generate_context!())
        .expect("...");
}
```

Three lines for the integration. Everything else (process spawn, handshake parsing, internal WS, event relay, supervisor restart logic) lives in the `buckaroo-tauri` crate.

### `src-tauri/tauri.conf.json`

- `csp: null` — production precedent from nteract-desktop
- macOS hardened runtime + `entitlements.plist` — required for Python's JIT and bundled .so files

### `src-tauri/entitlements.plist`

The five-key set validated against Apple notarization in nteract's production builds. See file comments for what each entitlement covers.

### `src/index.html`

Minimal vanilla-JS demo of the IPC contract:

- `listen("buckaroo:sidecar_ready", ...)` — surfaces when the sidecar is up
- `invoke("plugin:buckaroo-tauri|buckaroo_load_path", { args: { path } })` — loads a file
- `listen("buckaroo:msg", ...)` — receives every message the buckaroo server pushes

Self-contained — no external bundle deps, testable without a real Tauri runtime.

### Mounting the real grid

To render an actual DataFrame grid, swap the inline script for buckaroo's prebuilt React+AG Grid bundle:

```bash
PY=$(python3 -c "import buckaroo, os; print(os.path.dirname(buckaroo.__file__))")
cp $PY/static/tauri.js  src/tauri.js
cp $PY/static/tauri.css src/tauri.css
```

Then in `src/index.html`, replace the `<script>` block with:

```html
<link rel="stylesheet" href="tauri.css">
<div id="filename-bar"></div>
<div id="prompt-bar"></div>
<div id="root"></div>
<script type="module" src="tauri.js"></script>
```

A real host app would skip the file-copy and `import { TauriIPCModel } from "buckaroo-tauri-adapter"` directly via Vite, then mount their own React tree.

### Tests

```bash
pnpm test:e2e
```

Playwright tests in `tests/dom.spec.ts` install a controllable `window.__TAURI__` mock, fire fake `buckaroo:*` events, and assert DOM updates. Mock exposes `window.__test.fire(name, payload)` to drive events and `window.__test.calls` to inspect invoke calls. ~1 second total, no Python/Rust/Tauri runtime needed.

## Code-signing

For non-developer distribution:

- **macOS**: get an Apple Developer ID certificate (~$99/yr), use `tauri build` with `signingIdentity` set, then `xcrun notarytool` for notarization. The `entitlements.plist` here is the file Apple notarization will validate against.
- **Windows**: use `trusted-signing-cli` (Azure) — see the `signCommand` pattern in nteract-desktop's `tauri.conf.json`.

For internal/dev distribution: leave unsigned. macOS users will see a Gatekeeper prompt; Windows users will see SmartScreen.

## Status

Preview. Architecture is validated end-to-end (see `../../spike/README.md` for the spike that proved it). Binary parquet frames over IPC are deferred to v1.1 — the Rust plugin currently drops binary frames that follow `infinite_resp` JSON messages, which affects scroll/streaming rendering. Static rendering of small DataFrames works today.
