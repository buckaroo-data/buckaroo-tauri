//! Supervisor: spawn `python -m buckaroo.server`, parse the BUCKAROO_PORT
//! handshake, open the internal WebSocket, and relay messages to the webview.
//!
//! Restart policy: on sidecar termination, restart up to `config.max_restarts`
//! times with exponential backoff. After exhaustion, emit `sidecar:failed` and
//! stop trying — the host app surfaces the error to the user.

use crate::config::BuckarooConfig;
use crate::state::SidecarState;
use futures_util::{SinkExt, StreamExt};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tauri_plugin_shell::{process::CommandEvent, ShellExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

const HANDSHAKE_TIMEOUT_S: u64 = 10;

pub(crate) async fn start<R: Runtime>(
    app: AppHandle<R>,
    config: BuckarooConfig,
) -> Result<(), String> {
    let mut attempt: u32 = 0;
    let max = config.max_restarts;

    loop {
        attempt += 1;
        log::info!("[buckaroo-tauri] sidecar start attempt={}", attempt);
        match spawn_once(app.clone(), &config).await {
            Ok(()) => {
                // Spawn finished cleanly (sidecar exited). Reset attempt counter
                // because it ran successfully for some time.
                attempt = 0;
                log::warn!("[buckaroo-tauri] sidecar exited cleanly — restarting");
            }
            Err(e) => {
                log::error!("[buckaroo-tauri] sidecar attempt {} failed: {}", attempt, e);
                if attempt > max {
                    let _ = app.emit("buckaroo:sidecar_failed", e.clone());
                    return Err(e);
                }
                // Exponential backoff: 1s, 2s, 4s, 8s...
                let backoff_s = 1u64 << (attempt - 1).min(4);
                tokio::time::sleep(Duration::from_secs(backoff_s)).await;
            }
        }
    }
}

async fn spawn_once<R: Runtime>(
    app: AppHandle<R>,
    config: &BuckarooConfig,
) -> Result<(), String> {
    let autoload_path = config.autoload_path.clone();
    let python = config
        .resolve_python()
        .map_err(|e| format!("failed to resolve python: {}", e))?;

    let mut cmd = app
        .shell()
        .command(python.to_string_lossy().into_owned())
        .args([
            "-m",
            "buckaroo.server",
            "--port",
            &config.port.to_string(),
            "--no-browser",
            "--stdio-control",
        ]);
    if let Some(dir) = &config.working_dir {
        cmd = cmd.current_dir(dir.clone());
    }
    for (k, v) in &config.env {
        cmd = cmd.env(k, v);
    }

    let (mut rx, _child) = cmd
        .spawn()
        .map_err(|e| format!("spawn failed: {}", e))?;

    let app_handle = app.clone();
    let mut got_port = false;

    while let Some(event) = rx.recv().await {
        match event {
            CommandEvent::Stdout(line) => {
                let s = String::from_utf8_lossy(&line);
                let trimmed = s.trim_end();
                log::debug!("[buckaroo-sidecar:stdout] {}", trimmed);
                if !got_port {
                    if let Some(rest) = trimmed.strip_prefix("BUCKAROO_PORT=") {
                        if let Ok(port) = rest.trim().parse::<u16>() {
                            got_port = true;
                            on_port_discovered(&app_handle, port).await?;
                            if let Some(path) = autoload_path.clone() {
                                let app_for_autoload = app_handle.clone();
                                tokio::spawn(async move {
                                    if let Err(e) = autoload(&app_for_autoload, port, &path).await
                                    {
                                        log::error!(
                                            "[buckaroo-tauri] autoload failed: {}",
                                            e
                                        );
                                    }
                                });
                            }
                        }
                    }
                }
            }
            CommandEvent::Stderr(line) => {
                log::debug!(
                    "[buckaroo-sidecar:stderr] {}",
                    String::from_utf8_lossy(&line).trim_end()
                );
            }
            CommandEvent::Terminated(status) => {
                log::warn!("[buckaroo-sidecar] terminated: {:?}", status);
                if !got_port {
                    return Err(format!(
                        "sidecar exited before handshake (status={:?}); is buckaroo installed in {}? \
                         Try: `{} -m buckaroo.server --no-browser`",
                        status,
                        python.display(),
                        python.display()
                    ));
                }
                return Ok(());
            }
            _ => {}
        }
    }

    if !got_port {
        return Err(format!(
            "sidecar handshake did not arrive within stdout stream (timeout={}s)",
            HANDSHAKE_TIMEOUT_S
        ));
    }
    Ok(())
}

async fn on_port_discovered<R: Runtime>(app: &AppHandle<R>, port: u16) -> Result<(), String> {
    let state: tauri::State<SidecarState> = app.state();
    *state.port.lock().unwrap() = Some(port);
    log::info!("[buckaroo-tauri] sidecar listening on 127.0.0.1:{}", port);
    let _ = app.emit("buckaroo:sidecar_ready", port);
    Ok(())
}

/// Headless-verification path: POST /load, mint a session, open the internal WS.
/// Triggered by `BuckarooConfig::with_autoload_path(...)`. Equivalent to the
/// host calling `buckaroo_load_path` via invoke, but doesn't require a webview.
async fn autoload<R: Runtime>(
    app: &AppHandle<R>,
    port: u16,
    path: &std::path::Path,
) -> Result<(), String> {
    // Default to "buckaroo" mode — the full experience with stats panel,
    // command bar, dataflow operations. Override with BUCKAROO_AUTOLOAD_MODE
    // env var ("viewer" for read-only grid, "buckaroo" for full UI, "lazy"
    // for polars-streaming).
    let mode = std::env::var("BUCKAROO_AUTOLOAD_MODE").unwrap_or_else(|_| "buckaroo".to_string());
    log::info!(
        "[buckaroo-tauri] autoload starting: mode={} path={}",
        mode,
        path.display()
    );
    let body = serde_json::json!({
        "path": path.to_string_lossy(),
        "mode": mode,
        "no_browser": true,
    });
    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/load", port))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("autoload POST /load failed: {}", e))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("autoload /load returned {}: {}", status, text));
    }
    let metadata: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("autoload bad JSON: {}", e))?;
    let session_id = metadata
        .get("session")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "autoload: response missing 'session' field".to_string())?
        .to_string();
    let rows = metadata.get("rows").and_then(|v| v.as_u64()).unwrap_or(0);
    log::info!(
        "[buckaroo-tauri] autoload /load ok: session={} rows={}",
        session_id,
        rows
    );
    connect_internal_ws(app, port, &session_id).await?;

    // Optional: fire a sample infinite_request to exercise the binary-frame
    // pairing path. Triggered by BUCKAROO_TEST_INFINITE_REQUEST=1 — useful for
    // headless smoke tests of the parquet relay.
    if std::env::var("BUCKAROO_TEST_INFINITE_REQUEST")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
    {
        log::info!("[buckaroo-tauri] firing test infinite_request");
        let test_msg = serde_json::json!({
            "type": "infinite_request",
            "payload_args": {
                "key": "main",
                "start": 0,
                "end": 100,
                "request_time": chrono_now_ms(),
            }
        });
        let state: tauri::State<SidecarState> = app.state();
        let tx_guard = state.ws_tx.lock().await;
        if let Some(tx) = tx_guard.as_ref() {
            let json_str = serde_json::to_string(&test_msg).unwrap_or_default();
            if let Err(e) = tx.send(json_str) {
                log::warn!("[buckaroo-tauri] test infinite_request send failed: {}", e);
            }
        }
    }
    Ok(())
}

fn chrono_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Connect the internal WS to a session and start the relay tasks. Called from
/// `commands::buckaroo_load_path` after a successful HTTP /load.
pub(crate) async fn connect_internal_ws<R: Runtime>(
    app: &AppHandle<R>,
    port: u16,
    session_id: &str,
) -> Result<(), String> {
    let state: tauri::State<SidecarState> = app.state();
    {
        let guard = state.ws_tx.lock().await;
        if guard.is_some() {
            return Ok(());
        }
    }

    let ws_url = format!("ws://127.0.0.1:{}/ws/{}", port, session_id);
    let (ws_stream, _) = tokio_tungstenite::connect_async(&ws_url)
        .await
        .map_err(|e| format!("WS connect to {} failed: {}", ws_url, e))?;
    let (mut ws_write, mut ws_read) = ws_stream.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();

    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if ws_write.send(Message::Text(msg.into())).await.is_err() {
                break;
            }
        }
    });

    let app_clone = app.clone();
    tokio::spawn(async move {
        // The buckaroo WS protocol pairs an `infinite_resp` JSON text frame
        // with a following binary frame (parquet bytes). We hold the text in
        // `pending_infinite_resp` until the binary arrives, then emit a single
        // combined event with `data_b64` injected.
        //
        // Mirrors WebSocketModel.ts on the standalone path:
        //   text "infinite_resp" stash → next binary → emit("msg:custom", msg, [DataView])
        let mut pending_infinite_resp: Option<serde_json::Value> = None;

        while let Some(msg) = ws_read.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    let parsed: serde_json::Value = match serde_json::from_str(&text) {
                        Ok(v) => v,
                        Err(e) => {
                            log::warn!("[buckaroo-tauri] non-JSON text frame: {}", e);
                            continue;
                        }
                    };
                    let msg_type = parsed
                        .get("type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("?")
                        .to_string();
                    if msg_type == "infinite_resp" {
                        log::debug!(
                            "[buckaroo-tauri] stash infinite_resp ({} bytes), awaiting binary",
                            text.len()
                        );
                        pending_infinite_resp = Some(parsed);
                        continue;
                    }
                    log::info!(
                        "[buckaroo-tauri] relay msg type={} bytes={}",
                        msg_type,
                        text.len()
                    );
                    let _ = app_clone.emit("buckaroo:msg", &parsed);
                }
                Ok(Message::Binary(bytes)) => {
                    use base64::Engine;
                    if let Some(mut combined) = pending_infinite_resp.take() {
                        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                        if let Some(map) = combined.as_object_mut() {
                            map.insert(
                                "data_b64".to_string(),
                                serde_json::Value::String(b64),
                            );
                        }
                        log::info!(
                            "[buckaroo-tauri] relay infinite_resp + binary ({} parquet bytes)",
                            bytes.len()
                        );
                        let _ = app_clone.emit("buckaroo:msg", &combined);
                    } else {
                        log::warn!(
                            "[buckaroo-tauri] orphan binary frame ({} bytes), no preceding \
                             infinite_resp",
                            bytes.len()
                        );
                    }
                }
                Ok(Message::Close(_)) | Err(_) => break,
                _ => {}
            }
        }
        log::warn!("[buckaroo-tauri] internal WS reader exited");
    });

    *state.ws_tx.lock().await = Some(tx);
    *state.session_id.lock().unwrap() = Some(session_id.to_string());
    log::info!("[buckaroo-tauri] internal WS open to {}", ws_url);
    Ok(())
}
