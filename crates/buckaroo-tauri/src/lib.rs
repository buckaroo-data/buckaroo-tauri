//! buckaroo-tauri — Tauri 2.x plugin for embedding buckaroo's grid in a desktop app.
//!
//! Architecture (B2): the webview never opens a WebSocket. Instead, it talks to
//! Rust via Tauri IPC; Rust talks to a user-supplied Python sidecar via an
//! internal localhost WebSocket. The sidecar runs `python -m buckaroo.server`.
//!
//! Quick start (host app's `src-tauri/src/main.rs`):
//!
//! ```ignore
//! fn main() {
//!     tauri::Builder::default()
//!         .plugin(tauri_plugin_shell::init())
//!         .plugin(buckaroo_tauri::init(buckaroo_tauri::BuckarooConfig::xorq()))
//!         .run(tauri::generate_context!())
//!         .expect("error while running tauri application");
//! }
//! ```
//!
//! See `examples/tauri-app/` and the project plan at TAURI_EMBEDDING_PLAN.md.

mod commands;
mod config;
mod state;
mod supervisor;

pub use config::{BackendKind, BuckarooConfig};

use tauri::{
    plugin::{Builder, TauriPlugin},
    Manager, Runtime,
};

/// Initialize the buckaroo Tauri plugin.
///
/// Call once when building your Tauri app. Requires `tauri-plugin-shell` to be
/// registered first (used to spawn the Python sidecar).
///
/// # Example
///
/// ```ignore
/// tauri::Builder::default()
///     .plugin(tauri_plugin_shell::init())
///     .plugin(buckaroo_tauri::init(buckaroo_tauri::BuckarooConfig::xorq()))
///     .run(tauri::generate_context!())
///     .unwrap();
/// ```
pub fn init<R: Runtime>(config: BuckarooConfig) -> TauriPlugin<R> {
    Builder::new("buckaroo-tauri")
        .invoke_handler(tauri::generate_handler![
            commands::buckaroo_health,
            commands::buckaroo_load_path,
            commands::buckaroo_send,
            commands::buckaroo_pick_file,
        ])
        .setup(move |app, _api| {
            let app_handle = app.app_handle().clone();
            let cfg = config.clone();
            app.manage(state::SidecarState::new());
            tauri::async_runtime::spawn(async move {
                if let Err(e) = supervisor::start(app_handle, cfg).await {
                    log::error!("buckaroo-tauri sidecar startup failed: {}", e);
                }
            });
            Ok(())
        })
        .build()
}
