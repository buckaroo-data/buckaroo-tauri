//! IPC commands the plugin exposes to the webview.
//!
//! buckaroo's WS protocol is small (only `infinite_request` and
//! `buckaroo_state_change` flow client→server), so a passthrough
//! `buckaroo_send` is enough — we don't enumerate one command per message
//! type. Adding strongly-typed commands later is additive.

use crate::state::SidecarState;
use crate::supervisor;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Runtime};

#[derive(Serialize)]
pub(crate) struct LoadResult {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub rows: Option<u64>,
    /// Raw metadata blob from the server's /load response, for diagnostics.
    pub metadata: serde_json::Value,
}

#[derive(Serialize)]
pub(crate) struct HealthInfo {
    pub port: Option<u16>,
    pub session_id: Option<String>,
    pub ws_open: bool,
}

#[derive(Deserialize)]
pub(crate) struct LoadPathArgs {
    pub path: String,
    /// Optional: pre-supply a session id. If omitted, the buckaroo server
    /// mints one server-side and returns it in the response.
    #[serde(default)]
    pub session: Option<String>,
    /// Optional: override the server-side widget mode. `"viewer"` (default)
    /// loads the lightweight `DFViewerInfiniteDS`; `"buckaroo"` loads the
    /// full widget (summary stats, low-code commands, post-processing);
    /// `"lazy"` loads via polars LazyFrame for constant-memory scrolling.
    #[serde(default)]
    pub mode: Option<String>,
    /// Optional: when `mode == "buckaroo"`, swap the in-server execution
    /// backend. `"pandas"` (default) materialises via `pd.read_parquet`
    /// and computes summary stats up front. `"xorq"` wraps the file in
    /// `xo.deferred_read_parquet(...)` and serves each scroll via the
    /// XorqServerDataflow push-down path — no upfront materialisation,
    /// constant memory footprint over arbitrarily large parquets.
    /// Requires a `buckaroo[xorq]` install on the sidecar.
    #[serde(default)]
    pub backend: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct LoadExprArgs {
    /// Path to a `xorq build` / `xo.build_expr` output directory. The server
    /// reads expr.yaml, requirements.txt, the wheel, etc. from here and
    /// rehydrates the expression via xorq, then serves it via the
    /// XorqServerDataflow — each grid scroll fires
    /// `handle_infinite_request_xorq` against the live expression
    /// (push-down query execution against the underlying backend, no
    /// upfront materialisation).
    pub build_dir: String,
    /// Optional: pre-supply a session id. If omitted, the server mints one
    /// server-side and returns it in the response.
    #[serde(default)]
    pub session: Option<String>,
    /// Optional: project root used by the server to discover stat /
    /// post-processing klass extensions (`load_project_stat_klasses` and
    /// `load_project_post_processing_klasses`). Pass the catalog repo when
    /// the entries should pick up project-side extensions.
    #[serde(default)]
    pub project_root: Option<String>,
    /// Optional: prompt string echoed back in the initial_state payload,
    /// used by some hosts for breadcrumb / title chrome.
    #[serde(default)]
    pub prompt: Option<String>,
    /// Optional: server-side component_config override forwarded verbatim
    /// into the session.
    #[serde(default)]
    pub component_config: Option<serde_json::Value>,
}

/// Diagnostic info about the running sidecar.
#[tauri::command]
pub(crate) async fn buckaroo_health<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, SidecarState>,
) -> Result<HealthInfo, String> {
    let port = *state.port.lock().unwrap();
    let session_id = state.session_id.lock().unwrap().clone();
    let ws_open = state.ws_tx.lock().await.is_some();
    Ok(HealthInfo {
        port,
        session_id,
        ws_open,
    })
}

/// Load a file via the sidecar's HTTP /load endpoint, then open the internal
/// WS to that session if not already open.
#[tauri::command]
pub(crate) async fn buckaroo_load_path<R: Runtime>(
    args: LoadPathArgs,
    app: AppHandle<R>,
    state: tauri::State<'_, SidecarState>,
) -> Result<LoadResult, String> {
    let port = state
        .port
        .lock()
        .unwrap()
        .ok_or_else(|| "sidecar not yet ready".to_string())?;

    let mode = args.mode.as_deref().unwrap_or("viewer");
    let mut body = serde_json::json!({
        "path": args.path,
        "mode": mode,
        "no_browser": true,
    });
    if let Some(s) = &args.session {
        body.as_object_mut().unwrap().insert("session".into(), serde_json::Value::String(s.clone()));
    }
    if let Some(b) = &args.backend {
        body.as_object_mut().unwrap().insert("backend".into(), serde_json::Value::String(b.clone()));
    }

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/load", port))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("POST /load failed: {}", e))?;

    let status = resp.status();
    let text = resp.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("POST /load returned {}: {}", status, text));
    }
    let metadata: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("invalid JSON from /load: {}", e))?;

    let session_id = metadata
        .get("session")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "load response missing 'session' field".to_string())?;
    let rows = metadata.get("rows").and_then(|v| v.as_u64());

    supervisor::connect_internal_ws(&app, port, &session_id).await?;

    Ok(LoadResult {
        session_id,
        rows,
        metadata,
    })
}

/// Load a xorq expression (from a `xorq build` directory) via the sidecar's
/// HTTP /load_expr endpoint, then open the internal WS to that session if
/// not already open. Counterpart to `buckaroo_load_path` for the xorq
/// push-down path: the server keeps the expression live and answers each
/// `infinite_request` from the grid via xorq query execution, instead of
/// paging over a materialised parquet.
#[tauri::command]
pub(crate) async fn buckaroo_load_expr<R: Runtime>(
    args: LoadExprArgs,
    app: AppHandle<R>,
    state: tauri::State<'_, SidecarState>,
) -> Result<LoadResult, String> {
    let port = state
        .port
        .lock()
        .unwrap()
        .ok_or_else(|| "sidecar not yet ready".to_string())?;

    let mut body = serde_json::Map::new();
    body.insert("build_dir".into(), serde_json::Value::String(args.build_dir));
    body.insert("no_browser".into(), serde_json::Value::Bool(true));
    if let Some(s) = args.session {
        body.insert("session".into(), serde_json::Value::String(s));
    }
    if let Some(p) = args.project_root {
        body.insert("project_root".into(), serde_json::Value::String(p));
    }
    if let Some(p) = args.prompt {
        body.insert("prompt".into(), serde_json::Value::String(p));
    }
    if let Some(c) = args.component_config {
        body.insert("component_config".into(), c);
    }

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/load_expr", port))
        .json(&serde_json::Value::Object(body))
        .send()
        .await
        .map_err(|e| format!("POST /load_expr failed: {}", e))?;

    let status = resp.status();
    let text = resp.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("POST /load_expr returned {}: {}", status, text));
    }
    let metadata: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("invalid JSON from /load_expr: {}", e))?;

    let session_id = metadata
        .get("session")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "load_expr response missing 'session' field".to_string())?;
    let rows = metadata.get("rows").and_then(|v| v.as_u64());

    supervisor::connect_internal_ws(&app, port, &session_id).await?;

    Ok(LoadResult {
        session_id,
        rows,
        metadata,
    })
}

/// Fetch the sidecar's server-side diagnostics blob (`GET /diagnostics`).
/// Useful for host UIs that want to surface server version, uptime,
/// installed extras, etc.
#[tauri::command]
pub(crate) async fn buckaroo_diagnostics<R: Runtime>(
    _app: AppHandle<R>,
    state: tauri::State<'_, SidecarState>,
) -> Result<serde_json::Value, String> {
    let port = state
        .port
        .lock()
        .unwrap()
        .ok_or_else(|| "sidecar not yet ready".to_string())?;

    let resp = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{}/diagnostics", port))
        .send()
        .await
        .map_err(|e| format!("GET /diagnostics failed: {}", e))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("GET /diagnostics returned {}: {}", status, text));
    }
    serde_json::from_str(&text)
        .map_err(|e| format!("invalid JSON from /diagnostics: {}", e))
}

/// Forward a JSON message to the buckaroo server over the internal WS.
/// Used for `infinite_request` (scroll/data fetch) and `buckaroo_state_change`.
#[tauri::command]
pub(crate) async fn buckaroo_send<R: Runtime>(
    msg: serde_json::Value,
    _app: AppHandle<R>,
    state: tauri::State<'_, SidecarState>,
) -> Result<(), String> {
    let tx_guard = state.ws_tx.lock().await;
    let tx = tx_guard
        .as_ref()
        .ok_or_else(|| "internal WS not connected — call buckaroo_load_path first".to_string())?;
    let json_str = serde_json::to_string(&msg).map_err(|e| e.to_string())?;
    tx.send(json_str)
        .map_err(|e| format!("send failed: {}", e))?;
    Ok(())
}

/// Open a native file picker. Returns the selected path or null if cancelled.
///
/// v1 stub: not yet wired to tauri-plugin-dialog. The host app can implement
/// its own dialog and call buckaroo_load_path directly until this lands.
#[tauri::command]
pub(crate) async fn buckaroo_pick_file<R: Runtime>(
    _app: AppHandle<R>,
) -> Result<Option<String>, String> {
    Err(
        "buckaroo_pick_file not yet implemented — open a path with buckaroo_load_path \
         after using your own file dialog (e.g. tauri-plugin-dialog)."
            .to_string(),
    )
}
