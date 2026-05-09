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

    let body = match &args.session {
        Some(s) => serde_json::json!({
            "session": s,
            "path": args.path,
            "mode": "viewer",
            "no_browser": true,
        }),
        // Server mints when omitted (Layer 3 contract).
        None => serde_json::json!({
            "path": args.path,
            "mode": "viewer",
            "no_browser": true,
        }),
    };

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
    tx.send(json_str).map_err(|e| format!("send failed: {}", e))?;
    Ok(())
}

/// Open a native file picker. Returns the selected path or null if cancelled.
///
/// v1 stub: not yet wired to tauri-plugin-dialog. The host app can implement
/// its own dialog and call buckaroo_load_path directly until this lands.
#[tauri::command]
pub(crate) async fn buckaroo_pick_file<R: Runtime>(_app: AppHandle<R>) -> Result<Option<String>, String> {
    Err("buckaroo_pick_file not yet implemented — open a path with buckaroo_load_path \
         after using your own file dialog (e.g. tauri-plugin-dialog).".to_string())
}
