# Spike notes — alternatives explored

Chronological log of design alternatives considered or prototyped during the Tauri embedding spike. The user asked me to "try alternatives... take notes on different approaches, then we'll discuss the right path with knowledge."

This file is messy on purpose. The polished decisions live in `TAURI_EMBEDDING_PLAN.md`'s decision log.

---

## What's working as of 2026-05-09 morning

End-to-end verifiable from logs alone, no UI interaction:

```
cd examples/tauri-app
BUCKAROO_PYTHON=/path/to/python \
BUCKAROO_AUTOLOAD_PARQUET=/abs/path/to/file.parquet \
RUST_LOG=info pnpm tauri dev
```

Logs show every stage:
```
[example] autoload from env: /.../citibike-trips-2016-04.parquet
[buckaroo-tauri] sidecar start attempt=1
[buckaroo-tauri] sidecar listening on 127.0.0.1:58276
[buckaroo-tauri] autoload starting: path=...
[buckaroo-tauri] autoload /load ok: session=c0eb... rows=100000
[buckaroo-tauri] internal WS open to ws://127.0.0.1:58276/ws/c0eb...
[buckaroo-tauri] relay msg type=initial_state bytes=3236
```

This headless-verification path lets me iterate without manual clicking.

---

## Alternative 1 — Plugin namespace: `buckaroo` vs `buckaroo-tauri`

**Issue hit:** Tauri 2 derives the permission namespace from the *crate name*, not the `Builder::new("name")` runtime name. With crate `buckaroo-tauri` and runtime `Builder::new("buckaroo")`, capabilities couldn't reference any permissions — there was a `buckaroo-tauri:default` (auto-emitted) but no `buckaroo:default`, and the runtime invoke names used the wrong prefix.

**Fixed by:** matching them: crate `buckaroo-tauri` + `Builder::new("buckaroo-tauri")` + capability `buckaroo-tauri:default` + JS invokes `plugin:buckaroo-tauri|...`.

**Alternative not taken:** rename crate to `buckaroo` (would conflict with the existing `buckaroo` Python package's mental model — buckaroo-the-Python-thing vs buckaroo-the-Tauri-plugin would both be called "buckaroo"). Long-term that's worse than the slightly-verbose `buckaroo-tauri` namespace.

**Decision:** keep `buckaroo-tauri` everywhere. Already wired.

---

## Alternative 2 — Headless verification mechanism

Three approaches considered:

**(a) Env-var-driven autoload in the plugin** *(adopted)*
- `BuckarooConfig::with_autoload_path(path)` triggers a /load + WS-open after handshake.
- Example app reads `BUCKAROO_AUTOLOAD_PARQUET` env var and forwards.
- Pros: one env var → full chain runs → all stages logged → autonomous verification.
- Cons: a "test mode" feature in production code. The `with_autoload_path` is publicly visible on `BuckarooConfig` even though only test scenarios use it.

**(b) Separate `buckaroo-tauri-test-utils` crate**
- Same logic, but in a `cfg(test)`-only or behind a `test-utils` feature flag.
- Pros: keeps prod API clean.
- Cons: more crate scaffolding, harder to use from real example apps.

**(c) JS-side auto-click after sidecar:ready**
- `if (env.includes("AUTOLOAD")) clickLoadButton()` in the example app's index.html.
- Pros: zero plugin changes.
- Cons: requires the webview to fully load and execute JS, which means I can't verify with `tauri dev` running in true headless mode (it still opens a window).

**Decision:** (a). Cleanest end-to-end, the `with_autoload_path` builder method is also useful for production scenarios like "open the file the user double-clicked" (file association launches).

**Possible refinement later:** make `with_autoload_path` smart about whether the path is from a CLI arg vs env var vs file association — wire to Tauri's `Single Instance` plugin and re-trigger autoload on second-instance launches.

---

## Alternative 3 — Binary parquet frames over IPC ✅ DONE

The buckaroo WS protocol pairs an `infinite_resp` JSON frame with a following binary parquet frame. Originally deferred to v1.1; **now implemented in main path**.

### Tried (a) Base64-in-JSON emit payload

Implementation:
- Rust supervisor stashes the latest `infinite_resp` text frame in `pending_infinite_resp: Option<serde_json::Value>`.
- When the next binary frame arrives, base64-encodes it, inserts into the JSON object as `data_b64`, emits the combined value as one `buckaroo:msg` event.
- JS-side `TauriIPCModel` decodes the base64, builds a `DataView`, and emits `"msg:custom"` with `(msgClean, [DataView])` — matching `WebSocketModel`'s contract on the standalone path.

Verified end-to-end with `BUCKAROO_TEST_INFINITE_REQUEST=1`:
```
[buckaroo-tauri] firing test infinite_request
[buckaroo-tauri] relay msg type=initial_state bytes=3236
[buckaroo-tauri] stash infinite_resp (134 bytes), awaiting binary
[buckaroo-tauri] relay infinite_resp + binary (13568 parquet bytes)
```

**Decision:** ship (a). 33% base64 overhead is acceptable for buckaroo's scroll-page sizes (typically <100 KB per page). Revisit (b)/(c) only if perf complaints arrive.

### Untried but documented

**(b) Tauri 2 `Channel` API** — Tauri 2's `tauri::ipc::Channel<T>` for typed streaming. Native binary support, no encoding overhead. More invasive refactor; less battle-tested. Right answer if (a) hits perf walls.

**(c) Local HTTP service via custom URI scheme** — Rust serves binary blobs at `tauri://localhost/api/parquet/<id>`; JS does `fetch()`. Reintroduces an HTTP surface JS sees; defeats the point.

### Architectural question still open

Does Tauri's `emit()` serialize the entire payload through JSON every time, or does it have a fast-path for `Vec<u8>`? Untested. If JSON serializes fine, we could skip base64 and emit raw byte arrays — but the JS side would receive arrays-of-numbers, which are 4–8x larger in memory than `Uint8Array`. Base64 is probably right anyway.

---

## Alternative 4 — Python interpreter discovery ✅ (a) DONE

Updated resolution order:
1. `BuckarooConfig::with_python(path)` — explicit override
2. `BUCKAROO_PYTHON` env var
3. **`which buckaroo-server` — sibling python in same bin/** (NEW)
4. **`which buckaroo-table` — sibling python in same bin/** (NEW; older buckaroo console script)
5. `python3` on PATH

### Tried (a) `which`-based discovery

Implementation:
- Added `buckaroo-server = "buckaroo.server.__main__:main"` to `pyproject.toml`'s `[project.scripts]`.
- Plugin's `resolve_python` runs `which buckaroo-server`; if found, returns the sibling `python`/`python3` in the same bin dir.

Verified by running the example app **without** `BUCKAROO_PYTHON` set, only `PATH=/path/to/venv/bin:$PATH`:
```
[buckaroo-tauri] resolve_python: derived from `which buckaroo-server`: /Users/paddy/buckaroo/.venv/bin/python
[buckaroo-tauri] sidecar listening on 127.0.0.1:58304
```

Auto-discovery works. xorq desktop only needs to ensure the user's venv bin is on PATH — no `BUCKAROO_PYTHON` plumbing required.

### Untried but documented

**(b) Walk known venv locations** (`~/.virtualenvs`, `~/.cache/uv/`, `~/.pyenv/`) — punt unless someone complains.
**(c) UI picker** — host's responsibility, not the plugin's.
**(d) Bundled `python-build-standalone`** — explicitly out of scope per the plan; reconsider for a "buckaroo Desktop for non-Python users" follow-up.

---

## Alternative 5 — Sidecar lifecycle: WS reconnect on crash

The supervisor currently restarts the **process** on crash but doesn't auto-reconnect the **internal WS** when it does. After restart:
- New port (random)
- Old WS connection is dead
- Webview's TauriIPCModel keeps listening for `buckaroo:msg` events that never come

Options:

**(a) Re-emit `sidecar:ready` after restart, expect JS to re-load**
- JS-side TauriIPCModel listens for sidecar:ready and re-fetches state.
- Cons: requires JS to remember what file was loaded.

**(b) Server-side session persistence + auto-reconnect**
- Sessions persist on disk across restarts; supervisor reconnects to the same session ID.
- Pros: fully transparent to JS.
- Cons: requires session serialization in buckaroo's server.

**(c) Just don't try — surface the failure**
- Restart loop bottoms out with `sidecar:failed`; user has to restart the app.
- Pros: simplest.
- Cons: bad UX.

**Recommendation:** (a) for v1.1. Document that JS hosts should listen for `sidecar:ready` re-emissions and re-trigger their own load logic.

---

## Alternative 6 — Native file dialog wiring

`buckaroo_pick_file` is a stub. Three options for v1.1:

**(a) `tauri-plugin-dialog`**
- Standard Tauri dialog plugin.
- Pros: maintained, file-type filter support, multi-platform.
- Cons: extra peer dep; user has to register in their `Cargo.toml` *and* in capabilities.

**(b) Native via objc2/win32/gtk**
- Roll our own per-platform.
- Pros: zero extra deps.
- Cons: a lot of code per OS, easy to get wrong.

**(c) Skip — make hosts use their own dialog**
- Document in README; hosts pass paths to `buckaroo_load_path` directly.
- Pros: no work.
- Cons: every integrator has to wire this themselves.

**Recommendation:** (a). Tauri's plugin ecosystem expects plugin composition; making `tauri-plugin-dialog` a peer dep is fine.

---

## Alternative 7 — Multi-window support

Architecture composes naturally — each `mountBuckaroo` should be one webview ↔ one session. But the current Rust supervisor has a single `ws_tx` in state, meaning all webviews share one WS connection.

To properly multi-window, the state needs:
- `Map<webview_id, ws_tx>`
- `buckaroo_load_path` becomes per-webview
- emit() needs to target the right webview (`app.emit_to(WebviewLabel, ...)`)

Not started. v1.1+ feature — same priority as binary frames probably.

---

## Alternative 8 — `csp: null` vs strict CSP

Currently example uses `csp: null` (matching nteract's production precedent). The architecture doesn't *need* CSP because the webview only talks to Rust IPC, not external origins. So strict CSP would be overkill ceremony.

But strict CSP catches XSS bugs in user-supplied content (e.g., DataFrames with crafted HTML in cell values that buckaroo's React might inadvertently render). The grid components should sanitize, but defense-in-depth is real.

**Recommendation:** document `csp: null` as the simplest path; add a separate guide on "tightening CSP for hosts that need it" as a v1.x cookbook entry. Probably:
```
"csp": "default-src 'self' tauri:; script-src 'self' tauri:; style-src 'self' 'unsafe-inline' tauri:"
```
since buckaroo bundles `style-src 'unsafe-inline'`-style React + AG Grid styles.

---

## Alternative 9 — Plugin name in invoke path

Tauri 2 uses `plugin:<name>|<command>` for invokes. With `Builder::new("buckaroo-tauri")` we get `plugin:buckaroo-tauri|buckaroo_load_path` — verbose and the `buckaroo` prefix is doubled.

Considered:
- Rename invokes from `buckaroo_load_path` → `load_path` (drop the prefix). Final string: `plugin:buckaroo-tauri|load_path`. Cleaner.
- Trade-off: command names without prefix are more likely to collide with hypothetical other plugins (e.g., `tauri-plugin-load`?).

**Decision:** keep `buckaroo_*` prefix for now; it's grep-friendly when triaging issues.

---

## Open spike questions worth more time

- Does the sidecar's `--stdio-control` work correctly when the parent dies hard (kill -9)? Should test with `kill -9 $TAURI_PID` and verify Python exits.
- Cross-platform: Linux WebKitGTK CSP enforcement quirks (would need a Linux box / VM).
- Performance: how many `app.emit("buckaroo:msg", ...)` per second can Tauri handle before it backs up? Buckaroo's grid sends scroll-driven `infinite_request`s in bursts.
- Does the buckaroo server's existing TTL eviction logic trip when the same Tauri app stays open for hours? (5-minute eviction interval, 1-hour TTL — should be fine, but worth verifying.)

These are all worth a 30-minute spike each.
