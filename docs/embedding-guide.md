# Embedding buckaroo in a Tauri 2 desktop app

> **Status: preview.** Architecture is validated end-to-end. Binary parquet
> frames over IPC are deferred to v1.1 — works today for static-rendered
> DataFrames; scroll/streaming rows are limited until the JSON+binary frame
> pairing lands. See `TAURI_EMBEDDING_PLAN.md` for the full roadmap.

## What this is

A way to ship buckaroo's grid as part of a desktop app, where:

- Your app is a Tauri 2 binary.
- The user's Python (with buckaroo installed) is the data engine.
- The webview talks to your Rust supervisor; your Rust supervisor talks to Python; Python never directly talks to the webview.

```
┌─────────────────────────── Tauri desktop app ───────────────────────────┐
│                                                                          │
│   ┌─ Rust ─────────────────────────────────────────────────────────────┐ │
│   │  buckaroo-tauri plugin                                              │ │
│   │   • spawns python -m buckaroo.server via tauri-plugin-shell         │ │
│   │   • parses BUCKAROO_PORT=<n> from stdout                            │ │
│   │   • opens an internal localhost WebSocket to Python                 │ │
│   │   • exposes invoke commands and forwards pushed messages via emit() │ │
│   │   • supervises restarts, kills sidecar on app exit                  │ │
│   └────────────────┬─────────────────────────────────────────┬─────────┘ │
│                    │                                          │           │
│                    │ Tauri IPC                                │ ws://127  │
│                    │ (invoke + listen)                        │ (internal)│
│                    │                                          │           │
│   ┌─ webview ──────┴────────┐         ┌─ Python (user-supplied) ───────┐ │
│   │  TauriIPCModel          │         │  python -m buckaroo.server     │ │
│   │  (impl IModel)          │         │   • Tornado WS on 127.0.0.1    │ │
│   │   • get/set/save_changes│         │   • prints BUCKAROO_PORT=<n>   │ │
│   │   • on/off events       │         │   • analysis pipeline (xorq /  │ │
│   │  buckaroo's React grid  │         │     pandas / polars)           │ │
│   └─────────────────────────┘         └────────────────────────────────┘ │
│                                                                          │
└──────────────────────────────────────────────────────────────────────────┘
```

The webview never sees a localhost port. CSP, cross-origin, and firewall prompts go away by construction.

## Quick start

### 1. User-supplied Python

Your users install buckaroo themselves:

```bash
pip install 'buckaroo[xorq]'   # or [pandas] or [polars]
```

The Tauri app spawns this Python via PATH (or `BUCKAROO_PYTHON`/`BuckarooConfig::with_python`).

### 2. Rust side

`Cargo.toml`:

```toml
[dependencies]
tauri = "2"
tauri-plugin-shell = "2"
buckaroo-tauri = "0.13"
```

`src-tauri/src/lib.rs`:

```rust
use buckaroo_tauri::BuckarooConfig;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(buckaroo_tauri::init(BuckarooConfig::xorq()))
        .run(tauri::generate_context!())
        .expect("...");
}
```

### 3. Frontend side

`package.json`:

```json
{
  "dependencies": {
    "@tauri-apps/api": "^2",
    "buckaroo-tauri-adapter": "^0.13"
  }
}
```

`src/main.tsx` (with React + buckaroo's grid components):

```tsx
import { mountBuckaroo } from "buckaroo-tauri-adapter";
mountBuckaroo({ rootEl: document.getElementById("root")! });
```

Or roll your own mount using the building blocks:

```tsx
import { TauriIPCModel, waitForInitialState } from "buckaroo-tauri-adapter";
import srt from "buckaroo-js-core";

const initialState = await waitForInitialState();
const model = new TauriIPCModel(initialState);
const src = srt.getKeySmartRowCache(model, console.error);
// render <BuckarooInfiniteWidget model={model} src={src} ... />
```

### 4. `tauri.conf.json`

```json
{
  "app": {
    "security": { "csp": null }
  },
  "bundle": {
    "macOS": {
      "entitlements": "entitlements.plist",
      "hardenedRuntime": true,
      "minimumSystemVersion": "11.0"
    }
  }
}
```

### 5. `entitlements.plist`

The five-key set required for hardened-runtime macOS apps that spawn Python:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
    <key>com.apple.security.cs.allow-unsigned-executable-memory</key><true/>
    <key>com.apple.security.cs.disable-library-validation</key><true/>
    <key>com.apple.security.network.client</key><true/>
    <key>com.apple.security.files.user-selected.read-write</key><true/>
    <key>com.apple.security.inherit</key><true/>
</dict></plist>
```

What each entitlement does:
- `cs.allow-unsigned-executable-memory` — Python's JIT / dynamic code paths
- `cs.disable-library-validation` — bundled `.so`/`.dylib` files (pyarrow, etc.)
- `network.client` — internal localhost WebSocket the Rust plugin opens
- `files.user-selected.read-write` — file open dialogs
- `inherit` — child processes (the Python sidecar) inherit entitlements

(Set cribbed from nteract-desktop's production-validated configuration.)

## Customizing the backend

`BuckarooConfig` is a builder:

```rust
let cfg = BuckarooConfig::xorq()
    .with_python("/opt/homebrew/bin/python3.13")
    .with_working_dir("/Users/me/data")
    .with_env("XORQ_HOME", "/Users/me/.xorq")
    .with_max_restarts(5);
```

Python resolution order:
1. `with_python(...)` — explicit override
2. `BUCKAROO_PYTHON` env var
3. `python3` on PATH

Backend variants: `BuckarooConfig::xorq()`, `::pandas()`, `::polars()`. The choice affects diagnostic messages and (in v1.1) startup-time package detection.

## The IPC contract

For non-Tauri shells (Electron, Wails) or custom Tauri integrations, the underlying protocol is:

### Plugin commands

| Command | Payload | Returns |
|---|---|---|
| `plugin:buckaroo|buckaroo_health` | none | `{ port, sessionId, wsOpen }` |
| `plugin:buckaroo|buckaroo_load_path` | `{ args: { path, session? } }` | `{ sessionId, rows, metadata }` |
| `plugin:buckaroo|buckaroo_send` | `{ msg }` | `null` |
| `plugin:buckaroo|buckaroo_pick_file` | none | `string \| null` *(stub)* |

### Plugin events

| Event | Payload |
|---|---|
| `buckaroo:sidecar_ready` | `port: u16` |
| `buckaroo:sidecar_failed` | `error: string` |
| `buckaroo:msg` | `serde_json::Value` (raw WS message from server) |

### Sidecar contract (Python side)

`python -m buckaroo.server` (the entry point being spawned):

- **Flags**: `--port=<n>` (0 = random), `--no-browser`, `--stdio-control` (exit on stdin close).
- **Stdout handshake**: prints `BUCKAROO_PORT=<n>\n` before any other output, line-buffered. Other warnings/logs go to stderr.
- **HTTP endpoints** (used by Rust supervisor only): `/load`, `/load_compare`, `/health`, `/diagnostics`.
- **WebSocket**: `/ws/<session-id>` — the Tornado WS the Rust supervisor connects to.
- **Server-mints sessionId**: `/load` accepts a request without a `session` field and returns one in the response.

This contract is stable and versioned via the `protocol_version` field in `initial_state` messages. Lockstep with the buckaroo PyPI release is expected; mismatches surface in the JS console.

## Building & shipping

### Code-signing

For non-developer distribution:

- **macOS**: Apple Developer ID Application certificate (~$99/yr). `tauri build` reads `signingIdentity`. After signing, run `xcrun notarytool submit ... --wait` to notarize, then `xcrun stapler staple ...`. The `entitlements.plist` above is what notarization validates.
- **Windows**: code-signing certificate (Azure Trusted Signing, EV cert from a CA, etc.). Wire via `bundle.windows.signCommand` in `tauri.conf.json` — see nteract-desktop for a working `trusted-signing-cli` example.
- **Linux**: AppImage / `.deb` / `.rpm` typically don't require signing. Ship as-is.

### Bundle sizes

Without bundling Python, your Tauri app is small (~10–30 MB). The user's Python install carries the data-engine bytes (pyarrow ~80 MB, datafusion ~30 MB, etc.). If you need to ship Python with the app for non-Python audiences, see the `python-build-standalone` follow-up in the plan — not v1.

### Auto-update

Tauri's updater plugin works as-is. Wire `pubkey` + `endpoints` in `tauri.conf.json`'s `plugins.updater` and ship signed updates via your release pipeline. Pattern documented in `TAURI_EMBEDDING_PLAN.md` and demonstrated in nteract-desktop.

### Troubleshooting

`sidecar exited before handshake (status=...); is buckaroo installed in <python>?`
→ The chosen Python doesn't have buckaroo. Run `<python> -m pip install 'buckaroo[xorq]'` or set `BUCKAROO_PYTHON` to one that does.

`internal WS not connected — call buckaroo_load_path first`
→ The webview tried `buckaroo_send` before any file was loaded. Call `buckaroo_load_path` first to open a session.

Sidecar restart loop in logs (`attempt=1, attempt=2, ...`)
→ Each spawn fails. Check the server log at `~/.buckaroo/logs/server.log` for the underlying error. Often it's a missing buckaroo dependency in Python.

## Status & roadmap

What works today (v0.13.x):
- Sidecar spawn + handshake + supervised restarts
- Webview ↔ Rust IPC ↔ Python WS round-trip
- `initial_state` and `metadata` push events relayed to webview
- Server-mint sessionId via `/load`
- `protocol_version` runtime check

What's deferred:
- Binary parquet frame pairing for `infinite_resp` (affects scroll-streaming)
- Native file dialog (use `tauri-plugin-dialog` directly until we ship `buckaroo_pick_file`)
- Bundled-Python auto-installer (use `python-build-standalone` yourself)
- Windows + aarch64-linux platform validation
- Auto-update integration in the example app

## See also

- `TAURI_EMBEDDING_PLAN.md` — the full architecture rationale and decision log.
- `examples/tauri-app/` — the canonical minimum host.
- `crates/buckaroo-tauri-rs/README.md` — Rust crate API reference.
- `spike/README.md` — the original architecture validation spike (kept as evidence).
