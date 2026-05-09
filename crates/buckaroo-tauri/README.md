# buckaroo-tauri

Tauri 2.x plugin for embedding [buckaroo](https://github.com/paddymul/buckaroo) — a DataFrame viewer widget — inside a desktop app.

## Architecture

The webview never opens a WebSocket. It talks to Rust via Tauri IPC; Rust talks to a user-supplied Python sidecar over an internal localhost WebSocket. This eliminates CSP / cross-origin / firewall concerns entirely.

```
webview ──invoke()/listen()── Rust plugin ──ws://127.0.0.1:N── python -m buckaroo.server
```

## Quick start

`Cargo.toml`:

```toml
[dependencies]
buckaroo-tauri = "0.13"
tauri-plugin-shell = "2"
```

`src-tauri/src/main.rs`:

```rust
fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(buckaroo_tauri::init(buckaroo_tauri::BuckarooConfig::xorq()))
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

`tauri.conf.json` highlights:

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

`entitlements.plist` (required for hardened runtime + bundled Python):

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

The user must have buckaroo installed in the Python the plugin spawns:

```
pip install 'buckaroo[xorq]'   # or buckaroo[pandas], buckaroo[polars]
```

## Configuration

```rust
use buckaroo_tauri::{BuckarooConfig, BackendKind};

let cfg = BuckarooConfig::xorq()
    .with_python("/opt/homebrew/bin/python3.13")
    .with_working_dir("/Users/me/data")
    .with_env("XORQ_HOME", "/Users/me/.xorq")
    .with_max_restarts(5);
```

Python resolution order:
1. `BuckarooConfig::with_python(...)` — explicit override
2. `BUCKAROO_PYTHON` env var
3. `python3` on PATH

## IPC contract

The plugin exposes these `invoke` commands to the webview:

- `buckaroo_health()` → `{ port, sessionId, wsOpen }` — diagnostics
- `buckaroo_load_path({ path, session? })` → `{ sessionId, rows, metadata }` — opens a file
- `buckaroo_send({ msg })` → `()` — forwards a JSON message over the internal WS
- `buckaroo_pick_file()` → `string | null` — *stub*; use `tauri-plugin-dialog` until v1.1

And these events:

- `buckaroo:sidecar_ready` (payload: `port: u16`)
- `buckaroo:sidecar_failed` (payload: error string)
- `buckaroo:msg` (payload: every JSON message from the buckaroo server)

The companion JS package `buckaroo-tauri-adapter` provides a `TauriIPCModel` class that wraps these for use with buckaroo's React components.

## Status

v0.13.x — preview. Binary parquet frames over IPC are deferred (the server emits an `infinite_resp` JSON followed by a binary frame; the plugin currently drops the binary and logs). This affects scroll/streaming-row scenarios; static-render of small DataFrames works today.

## License

BSD-3-Clause.
