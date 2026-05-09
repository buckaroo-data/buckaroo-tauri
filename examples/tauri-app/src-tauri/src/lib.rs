//! Minimal Tauri host app embedding buckaroo via the buckaroo-tauri plugin.
//!
//! Pattern (from the project's TAURI_EMBEDDING_PLAN.md):
//!   1. Register tauri-plugin-shell (used by buckaroo-tauri to spawn Python).
//!   2. Register buckaroo_tauri::init(BuckarooConfig::xorq()).
//!   3. The frontend imports buckaroo-tauri-adapter and constructs a
//!      TauriIPCModel after waitForInitialState() resolves.
//!
//! User must have buckaroo installed in their Python:
//!   pip install 'buckaroo[xorq]'

use buckaroo_tauri::BuckarooConfig;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Install a stderr logger so the buckaroo-tauri plugin's progress and
    // sidecar stdout/stderr are visible during development. Default level
    // surfaces info+; set RUST_LOG=debug for the full handshake trace.
    let _ = env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info"),
    )
    .try_init();

    let mut cfg = BuckarooConfig::xorq();

    // Headless verification convenience: if BUCKAROO_AUTOLOAD_PARQUET is set,
    // the plugin auto-calls /load with that path right after the sidecar
    // handshake. End-to-end behavior becomes verifiable from logs alone.
    if let Ok(autoload) = std::env::var("BUCKAROO_AUTOLOAD_PARQUET") {
        if !autoload.is_empty() {
            log::info!("[example] autoload from env: {}", autoload);
            cfg = cfg.with_autoload_path(autoload);
        }
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(buckaroo_tauri::init(cfg))
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
