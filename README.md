# buckaroo-tauri

Embed [buckaroo](https://github.com/buckaroo-data/buckaroo) — a DataFrame viewer widget — inside a [Tauri 2](https://tauri.app/) desktop app.

> **Status: preview.** End-to-end working on macOS aarch64. Architecture validated against ~100k-row real datasets. Public API may shift before 1.0.

## Architecture

```
webview ──invoke()/listen()── Rust plugin ──ws://127.0.0.1:N── python -m buckaroo.server
                              (this repo)                       (user-supplied)
```

The webview never opens a WebSocket. It talks to Rust via Tauri IPC; Rust talks to a user-supplied Python sidecar over an internal localhost WebSocket. CSP, cross-origin, and firewall concerns disappear by construction.

## Repository layout

| Path | What | Publishes as |
|---|---|---|
| `crates/buckaroo-tauri/` | The Rust plugin: spawns Python, parses handshake, IPC commands, event relay, supervised restarts | `buckaroo-tauri` on crates.io |
| `packages/buckaroo-tauri-adapter/` | TS/JS adapter — `TauriIPCModel` implementing buckaroo's `IModel` against Tauri IPC; binary parquet frame decoder | `buckaroo-tauri-adapter` on npm |
| `examples/tauri-app/` | Minimal canonical host showing the integration | not published |
| `docs/embedding-guide.md` | User-facing embedding guide (architecture, quick start, signing, troubleshooting) | |
| `docs/PLAN.md` | The architectural plan — decisions, rationale, alternatives considered | |
| `docs/SPIKE_NOTES.md` | Running log of design alternatives prototyped during the spike | |

## Quick start

### Prerequisites

- Rust 1.77+ with `cargo`
- Node 18+ with `pnpm`
- A Python 3.11+ environment with buckaroo installed:
  ```
  pip install 'buckaroo[xorq]'   # or [pandas] / [polars]
  ```

### Run the example app

```bash
cd examples/tauri-app
pnpm install

# Optional: copy the prebuilt JS bundle from your buckaroo install so the
# example mounts the real React grid (otherwise the example shows a vanilla
# state-display fallback).
PY_BUCKAROO=$(python3 -c "import buckaroo, os; print(os.path.dirname(buckaroo.__file__))")
cp $PY_BUCKAROO/static/tauri.js  src/tauri.js
cp $PY_BUCKAROO/static/tauri.css src/tauri.css

# Headless verification — autoload a parquet, log every stage:
PATH=/path/to/your/venv/bin:$PATH \
BUCKAROO_AUTOLOAD_PARQUET=/abs/path/to/file.parquet \
RUST_LOG=info pnpm tauri dev
```

What you should see in the logs:

```
[buckaroo-tauri] sidecar start attempt=1
[buckaroo-tauri] resolve_python: derived from `which buckaroo-server`: ...
[buckaroo-tauri] sidecar listening on 127.0.0.1:NNNNN
[buckaroo-tauri] autoload /load ok: session=... rows=NNN
[buckaroo-tauri] internal WS open to ws://...
[buckaroo-tauri] relay msg type=initial_state bytes=NNNN
[buckaroo-tauri] relay infinite_resp + binary (NNNNN parquet bytes)
```

### Embed in your own Tauri app

`Cargo.toml`:
```toml
[dependencies]
tauri = "2"
tauri-plugin-shell = "2"
buckaroo-tauri = "0.13"
```

`src-tauri/src/lib.rs`:
```rust
fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(buckaroo_tauri::init(buckaroo_tauri::BuckarooConfig::xorq()))
        .run(tauri::generate_context!())
        .expect("...");
}
```

Permission file (`src-tauri/capabilities/default.json`):
```json
{
  "permissions": ["core:default", "shell:default", "buckaroo-tauri:default"]
}
```

See [`docs/embedding-guide.md`](docs/embedding-guide.md) for the full integration recipe including macOS hardened-runtime entitlements and Windows code-signing setup.

## What needs the host buckaroo PR

The Python-side handshake contract this plugin depends on (`BUCKAROO_PORT=<n>` stdout line, `--stdio-control` flag, `protocol_version` field, server-mint sessionId, `buckaroo-server` console script) lands in [buckaroo PR #717](https://github.com/buckaroo-data/buckaroo/pull/717). Until that ships, point at a buckaroo install built from the PR branch.

## Status / roadmap

What works:
- Rust plugin spawns Python via `tauri-plugin-shell`, parses handshake, supervises restarts
- Internal WebSocket from Rust to Python; webview never sees the port
- IPC commands: `buckaroo_health`, `buckaroo_load_path`, `buckaroo_send`, `buckaroo_pick_file` (stub)
- Events: `buckaroo:sidecar_ready`, `buckaroo:sidecar_failed`, `buckaroo:msg`
- Binary parquet frame pairing: `infinite_resp` JSON + binary frame combined into one event with `data_b64` injected; JS adapter decodes back to `DataView` matching `WebSocketModel`'s contract
- Python auto-discovery via `which buckaroo-server`
- Tauri 2 permission system properly wired
- macOS aarch64 verified end-to-end with 100k-row real data

What's deferred:
- Native file dialog (`buckaroo_pick_file` is a stub; integrate `tauri-plugin-dialog`)
- Multi-window — current state holds one WS connection; needs per-webview map
- Auto-update wiring in the example app
- Windows + aarch64-linux platform validation
- Playwright CI for the example

See [`docs/SPIKE_NOTES.md`](docs/SPIKE_NOTES.md) for alternatives considered, including approaches to the deferred items.

## License

BSD-3-Clause. Same as buckaroo.
