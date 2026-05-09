# Tauri Embedding Plan (revised v3 — B2 / nteract pattern)

Status: proposal, not started. Phase 0 spike pivots from "WS-direct" to "IPC-via-Rust."
Audience: buckaroo maintainers + downstream Tauri-app integrators (initial driver: xorq desktop).

This revision adopts the nteract-desktop architecture after surveying their shipped patterns. See `## Decision log` at the end for the diff history.

## Goal

Add a fifth, first-class embedding target to buckaroo:

| Target | JS entry | Built artifact | Backend |
|---|---|---|---|
| anywidget (Jupyter / Marimo / Solara) | `packages/js/widget.tsx` | `buckaroo/static/widget.js` | anywidget model |
| Standalone browser tab | `packages/js/standalone.tsx` | `buckaroo/static/standalone.js` | Tornado WS server |
| Static HTML embed | `packages/js/static-embed.tsx` | `buckaroo/static/static-embed.js` | none (in-page data) |
| JS library | n/a | `buckaroo-js-core` npm pkg | host-supplied |
| **Tauri desktop app** *(new)* | `packages/js/tauri.tsx` | `buckaroo/static/tauri.js` | Tauri-spawned user-supplied Python; webview ↔ Rust ↔ Python via IPC |

Drop-in for any Tauri app: ~10 lines of Rust and ~10 lines of TypeScript should yield a working buckaroo UI inside a desktop window.

## Non-goals

- A full xorq desktop app — drives this work but is built elsewhere.
- **Bundling Python.** Users `pip install buckaroo` (or get it via xorq's installer flow). The desktop app is a small Rust shim + JS bundle, not a self-contained 200 MB freeze.
- Replacing the Python analysis pipeline.
- Mobile (Tauri 2.x mobile targets).
- WASM Python (Pyodide).
- Windows + aarch64-linux platforms in v1 (added in v1.1 if there's a real user). v1 targets aarch64-apple-darwin, x86_64-apple-darwin, x86_64-unknown-linux-gnu.

## Architecture (B2: IPC via Rust)

```
+---------------------------- Tauri desktop app ----------------------------+
|                                                                            |
|   src-tauri/  (Rust)                                                       |
|   +------------------------------------------------------------------+    |
|   | buckaroo_tauri::init(BuckarooConfig::xorq()) plugin               |    |
|   |                                                                   |    |
|   |  setup hook:                                                      |    |
|   |   - locate Python (PATH, configured venv, or xorq's pinned env)   |    |
|   |   - spawn `python -m buckaroo.server --port=0 --no-browser`       |    |
|   |     via tauri-plugin-shell's .shell().sidecar() / .command()      |    |
|   |   - parse BUCKAROO_PORT=<n> from stdout                           |    |
|   |   - open internal WebSocket to ws://127.0.0.1:<n>/ws/<sid>        |    |
|   |   - cache the WS connection in app state                          |    |
|   |                                                                   |    |
|   |  IPC commands (~30 #[tauri::command] handlers):                   |    |
|   |   - buckaroo_get(key) / buckaroo_set(key, value)                  |    |
|   |   - buckaroo_save_changes()                                       |    |
|   |   - buckaroo_send(msg)                                            |    |
|   |   - buckaroo_load_path(path) / buckaroo_pick_file()               |    |
|   |   - (etc — every WS message type maps to one)                     |    |
|   |                                                                   |    |
|   |  Event relay (Rust → JS):                                         |    |
|   |   - tokio task reads pushed WS messages, calls app.emit(...)      |    |
|   |   - JS-side listen("buckaroo:change:<key>", handler)              |    |
|   +-----------+--------------------------------+---------------------+    |
|               | invoke / emit                  | TCP loopback (internal)   |
|               v                                v                          |
|   webview                              python -m buckaroo.server          |
|   +----------------------------+       +-------------------------+        |
|   | mountBuckaroo({ rootEl })  |       | Tornado WS server       |        |
|   | TauriIPCModel implements   |       | binds 127.0.0.1:<rand>  |        |
|   |   IModel, calls invoke()   |       | prints BUCKAROO_PORT=n  |        |
|   |   and listen() under hood  |       +-------------------------+        |
|   +----------------------------+                                          |
|                                                                            |
+----------------------------------------------------------------------------+
```

The TCP localhost WS between Rust and Python is **internal** — never seen by the webview. So the CSP / cross-origin / firewall questions go away. Rust exposes its own IPC contract to the webview via Tauri's `invoke` and `emit` mechanisms.

## Why this architecture

- **No Python bundling.** xorq desktop's audience already has Python; bundling would be 200 MB of waste per install.
- **No CSP question.** Webview only talks to Rust. nteract ships `csp: null` in production with this pattern.
- **No port discovery from JS.** The internal port is a Rust implementation detail; users never see firewall prompts.
- **Process isolation preserved.** Python crash → Rust supervisor restarts; UI shows "reconnecting…" without the app dying.
- **Battle-tested.** nteract has shipped this pattern at scale; the entitlements, signing, updater paths are known-good.
- **Tradeoff accepted.** Layer 5 grows (~30 `#[tauri::command]` handlers proxying WS messages); Layer 4 (PyInstaller, freeze CI matrix) collapses to "Rust binary build per target."

The cost: the Tauri JS bundle now diverges from `standalone.tsx` — a `TauriIPCModel` replaces `WebSocketModel`. **Layer 2 (`IModel` interface) becomes mandatory** to keep the React components transport-agnostic.

## Phase 0 — B2 spike

Architecture is no longer the question (nteract proves it). The spike now answers a smaller question: **does our specific WS-message-to-IPC-command translation hold up?**

1. Reuse existing `__main__.py` handshake patch (line-buffered stdout, `bind_sockets(0)`, `BUCKAROO_PORT=<n>`).
2. In `spike-app/src-tauri/src/lib.rs`, swap raw `std::process::Command` for `tauri-plugin-shell`'s `.shell().command()` (we won't have an externalBin until Layer 4 — for the spike, point at `python` on PATH).
3. After handshake, Rust opens an internal `ws://127.0.0.1:<n>/ws/spike-session` connection (using `tokio-tungstenite`).
4. Add Rust `#[tauri::command]` handlers for `buckaroo_get(key)`, `buckaroo_load_path(path)`, plus a `tokio::spawn`'d task that forwards pushed WS messages via `app.emit()`.
5. Frontend: a stub `TauriIPCModel` class that implements `IModel.get/set/save_changes/send/on/off` against `invoke()` and `listen()`. Mount one of buckaroo's grid components (e.g., `DFViewerInfiniteDS`) using it.
6. Verify: load sample.parquet, grid renders 1000 rows, scroll-driven `send()` requests reach the server, pushed updates render.

Target: ~3–5 days. Existing spike code is mostly salvageable — the Rust handshake parser and the buckaroo server patch carry over verbatim.

If this works → proceed to layers. If `TauriIPCModel`'s emit-relay can't keep up with parquet binary-frame throughput, we re-plan (e.g., add a streaming variant of invoke for binary payloads).

## Versioning

Lockstep across **3 artifacts** (no PyPI sidecar binary anymore — buckaroo PyPI *is* the Python side):

- `buckaroo` Python package — PyPI
- `buckaroo-tauri` Rust plugin — crates.io
- `buckaroo-tauri-adapter` JS adapter — npm

`initial_state` WS message gains a `protocol_version: <n>` field. Rust's startup handshake validates it before exposing IPC. Mismatch surfaces as a clear startup error to JS.

One tag → three publishes via release CI.

## Deliverables

### Layer 1 — JS bootstrap bundle

**New file:** `packages/js/tauri.tsx`
**Built artifact:** `buckaroo/static/tauri.js` + `buckaroo/static/tauri.css`
**Build command:** `"build:tauri": "esbuild tauri.tsx --format=esm --bundle --minify --sourcemap --outdir=../../buckaroo/static/"`.

Mounts `BuckarooInfiniteWidget` / `DFViewerInfiniteDS` against a `TauriIPCModel`:

```ts
export function mountBuckaroo(opts: {
  rootEl: HTMLElement;
}): { unmount: () => void };
```

No `BuckarooBackend` interface — the IPC contract is the contract. The bundle imports from `@tauri-apps/api/core` and `@tauri-apps/api/event` directly.

Shared code with `standalone.tsx`: refactor `ViewerApp`/`BuckarooApp`/`patchDisplayArgsHeight` into `packages/js/_shared/standaloneApp.tsx` so both entries can use them with whichever IModel implementation.

### Layer 2 — `IModel` interface (now mandatory)

Promote to a public export of `buckaroo-js-core`:

```ts
export interface IModel {
  get(key: string): any;
  set(key: string, value: any): void;
  save_changes(): void;
  send(msg: unknown): void;
  on(event: string, handler: (...args: any[]) => void): void;
  off(event: string, handler: (...args: any[]) => void): void;
}
```

`WebSocketModel` retypes as `class WebSocketModel implements IModel` (already drop-in compatible).
`TauriIPCModel` is a new class that implements the same surface against Tauri IPC.
anywidget's model already satisfies the surface.

Components like `getKeySmartRowCache`, `useModelState`, `BuckarooInfiniteWidget`, `DFViewerInfiniteDS` accept `IModel` instead of a concrete type. Mostly type-only refactor.

### Layer 3 — Sidecar entry contract

Already prototyped in `buckaroo/server/__main__.py`. Final contract:

- `sys.stdout.reconfigure(line_buffering=True)`.
- Default `--port` becomes **8100** (was 8700).
- Sidecar invocations pass `--port=0`. `tornado.netutil.bind_sockets(0)` + `tornado.httpserver.HTTPServer(app).add_sockets(sockets)` recovers the OS-assigned port.
- On listen, before any other stdout: `BUCKAROO_PORT=<n>\n`.
- `--no-browser` flag stays.
- New `--stdio-control`: process exits when stdin closes (Rust drops stdin to signal shutdown).
- Suppress the `must be running inside ipython` warning that currently leaks to stdout (route to stderr or silence at import).
- Add `protocol_version: <integer>` to `initial_state` WS message.
- Existing HTTP endpoints (`/load`, `/load_compare`, `/health`, `/diagnostics`) unchanged.
- `LoadHandler` is updated to mint a server-side `sessionId` when one isn't provided, so the JS side never has to invent one.

### Layer 4 — Sidecar binary distribution (drastically simplified)

**No PyInstaller. No 5-platform freeze matrix. No 200 MB binaries.**

The "sidecar" is just `python -m buckaroo.server`. Rust supervisor finds Python via:

1. **Configured venv path** in `BuckarooConfig::venv("/abs/path/to/python")`. Highest priority.
2. **`xorq` python in `BUCKAROO_PYTHON` env var**. Set by xorq desktop's launcher.
3. **`python3` on PATH** with a startup check that imports buckaroo. If not found, surface a clear "buckaroo not installed in $PYTHON" error to the user with a copy-pasteable `pip install buckaroo` command.

Optional follow-up: bundle a `python-build-standalone` interpreter behind a feature flag for hosts that want zero-config distribution. **Not v1.**

### Layer 4b — JS adapter package

**New package:** `packages/buckaroo-tauri-adapter/` published as `buckaroo-tauri-adapter` on npm.

Exports:
- `TauriIPCModel` class implementing `IModel`. Calls `invoke` for sync ops, `listen` for pushed events.
- Helper `mountBuckarooTauri(rootEl)` that constructs the model and mounts the bundle.

`@tauri-apps/api` is a peer dep. ~200 LOC.

### Layer 5 — Rust plugin crate

**New crate:** `crates/buckaroo-tauri-rs/` (`buckaroo-tauri` on crates.io).

Public API:

```rust
fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(buckaroo_tauri::init(BuckarooConfig::xorq()))
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

What `init()` does:

- **Spawn:** uses `tauri_plugin_shell::ShellExt::shell().command(python_path)` with args `["-m", "buckaroo.server", "--port=0", "--no-browser", "--stdio-control"]`. Parses `BUCKAROO_PORT=<n>` from `CommandEvent::Stdout`.
- **Internal WS connect:** `tokio-tungstenite` to `ws://127.0.0.1:<n>/ws/<session>`. Reconnect on close.
- **IPC handlers** (~30 commands, mirroring the WS protocol). Examples:
  - `buckaroo_get(key) → JsonValue`
  - `buckaroo_set(key, value)`
  - `buckaroo_save_changes()`
  - `buckaroo_send(msg)`
  - `buckaroo_load_path(path) → { sessionId }`
  - `buckaroo_pick_file() → string | null`
  - `buckaroo_health() → { status, uptime }`
- **Event relay:** `tokio::spawn` task forwards every pushed WS message to the webview via `app.emit("buckaroo:event", payload)`. JS filters server-side or via event name.
- **Supervision:** restart child on crash with backoff (max 3), kill on app exit. Adopt nteract's "try existing daemon first" pattern as a future option.
- **Config:** `BuckarooConfig::xorq()`, `::pandas()`, `::polars()` — chooses which Python to look for. Builder for advanced overrides (working dir, env vars, log file, port range, restart policy).

### Layer 6 — Example app + integration test

**New directory:** `examples/tauri-app/`.

```tsx
// src/main.tsx
import { mountBuckarooTauri } from "buckaroo-tauri-adapter";
mountBuckarooTauri(document.getElementById("root")!);
```

```rust
// src-tauri/src/lib.rs
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(buckaroo_tauri::init(buckaroo_tauri::BuckarooConfig::xorq()))
        .run(tauri::generate_context!()).unwrap();
}
```

`tauri.conf.json` highlights (cribbed from nteract):
- `"csp": null` — no CSP in production.
- `"externalBin": []` (none if Python is user-supplied; or include a small bootstrap bin if needed).
- `"macOS": { "entitlements": "entitlements.plist", "hardenedRuntime": true, "minimumSystemVersion": "11.0" }`.
- File associations for `.parquet`, `.csv` (optional v1 nicety).

`entitlements.plist` (the production-validated set):
```xml
<key>com.apple.security.cs.allow-unsigned-executable-memory</key><true/>
<key>com.apple.security.cs.disable-library-validation</key><true/>
<key>com.apple.security.network.client</key><true/>
<key>com.apple.security.files.user-selected.read-write</key><true/>
<key>com.apple.security.inherit</key><true/>
```

**CI:** `playwright.config.tauri.ts` spins up the example app on `depot-ubuntu-latest`, runs a smoke suite (open file → grid render → scroll → expect rows).

### Layer 7 — Documentation

**New file:** `docs/embedding/tauri.md`. Sections:

1. *What this is* — pitch + B2 architecture diagram.
2. *Quick start* — Cargo dep, npm deps, mount call, `tauri.conf.json` config, entitlements.plist, `pip install buckaroo`.
3. *Customizing* — implementing `IModel` directly if you want a non-Tauri shell.
4. *Sidecar contract* — `__main__.py` flags + handshake + endpoints. Lets non-Tauri shells reuse the Python side.
5. *Building & shipping* — externalBin layout, code-signing notes per OS (macOS hardenedRuntime + Windows trusted-signing-cli + auto-update via Tauri updater plugin).

Cross-link from README's "Embeddings" list.

## Suggested ordering

0. **B2 spike** (~3–5 days). Adapt existing spike-app to IPC pattern. Validates emit-relay throughput for binary parquet frames.
1. **Layer 2** — `IModel` interface + retype `WebSocketModel`. Pure refactor, no behavior change.
2. **Layer 3** — finalize sidecar contract changes (suppress ipython warning, add protocol_version, `--stdio-control`, server-mint sessionId).
3. **Layer 1** + shared `standaloneApp.tsx` refactor + `TauriIPCModel` skeleton.
4. **Layer 5 + Layer 4b** — Rust plugin commands (~30) + JS adapter. The slog.
5. **Layer 6** — example app + Playwright CI.
6. **Layer 7** — docs.

Each step is independently shippable. Stop anywhere and downstream apps can hand-roll the rest.

## Open questions

- **30+ IPC handlers — auto-generate from WS message types?** WebSocketModel today handles a small set of message types in `WebSocketModel.ts`. If we enumerate them, we could generate the matching Rust handlers via macros or build-time codegen. Not required for v1; nteract hand-rolled theirs.
- **Streaming binary frames over IPC.** Tauri's `invoke` returns a single response; `emit` carries one payload at a time. Buckaroo's parquet streaming is multi-frame. Probably fine via emit-with-sequence-id, but the spike validates this.
- **Find-Python heuristics on Linux/Windows.** macOS `which python3` is unambiguous-ish; Linux varies (system, pyenv, conda, uv); Windows is wild west. Document the resolution order and let `BuckarooConfig::venv(path)` override.
- **Code-signing automation.** Adopt nteract's macOS hardened-runtime + Windows trusted-signing-cli stack when v1.1 hits Windows. v1 macOS-only is OK without dev signing for internal use, signing-required for public.
- **Auto-update.** Adopt Tauri's updater plugin + minisign + GitHub Releases endpoint (nteract's pattern). Probably v1.1.
- **Multi-window.** Each `mountBuckarooTauri` is one webview = one Rust-side session = one internal WS connection. Rust manages a session map; JS-side `TauriIPCModel` includes a `sessionId` in every invoke. Architecture composes; UX patterns deferred.

## Out-of-scope follow-ups (after v1 ships)

- Bundle a `python-build-standalone` interpreter for hosts that want zero-Python-config distribution.
- Additional `BuckarooConfig` profiles: `pandas()`, `polars()`, `xorq_duckdb()`.
- Windows + aarch64-linux platforms.
- Auto-update wired into the GH release flow.
- `buckaroo init-tauri` CLI helper for scaffolding integrations.
- Code-generated IPC handlers from WS message-type enums (drives consistency between Python protocol and Rust handlers).

## Decision log (changes from v1 → v2 → v3)

| Topic | v1 | v2 | v3 (this) |
|---|---|---|---|
| Scope | Drop-in lib for any integrator | Same, but recognized one consumer (xorq) | Same |
| Cut | Implicit full | Cut γ + Phase 0 spike | Same |
| Architecture | Webview-direct WS to Python | Same | **Webview ↔ Rust IPC ↔ Python (B2 / nteract)** |
| Python distribution | "TBD, probably PyInstaller" | PyInstaller onedir, 200 MB sidecar, 5-target CI matrix | **User-supplied Python; no freeze, no matrix** |
| Layer 4 (freeze) | Big TODO | Big slog | **Collapses to ~nothing** |
| Layer 5 (Rust) | Thin spawn-and-supervise | Same | **Grows to ~30 #[tauri::command] handlers + emit relay** |
| Layer 2 (IModel) | Maybe | Deferred | **Mandatory, load-bearing** |
| Spawn API | std::process::Command | std::process::Command | **`tauri-plugin-shell` .shell().command()** |
| Bundle convention | externalBin | bundle.resources + onedir | **externalBin (Rust binary only)** |
| CSP | "one-line entry" | strict CSP question deferred to spike | **`csp: null` in production (nteract precedent)** |
| Code signing | Punt to host | N1 unsigned signing-ready | **Adopt nteract's hardened-runtime entitlements + trusted-signing-cli + Tauri updater patterns** |
| Session handshake | `BUCKAROO_PORT` + `BUCKAROO_SESSION` | `BUCKAROO_PORT` only; sessionId from `/load` | **`BUCKAROO_PORT` only; `LoadHandler` mints sessionId server-side** |
| Versioning | Not specified | Lockstep across 4 artifacts | **Lockstep across 3 artifacts (no sidecar binary)** |
| Platforms v1 | 5 | 3 (macos×2 + linux×1) | Same as v2 |
| Backend choice | Bundle the world | xorq + datafusion only, slim | **Choice via BuckarooConfig — moot for distribution since user supplies Python** |
| Phase 0 spike | None | WS+CSP existential check on macOS | **TauriIPCModel + emit-relay throughput check** |
