/// Auto-generates Tauri 2 permission descriptors for the plugin's commands.
/// Each command listed here gets `allow-<command>` and `deny-<command>`
/// permissions, plus a `default` permission set that allows all of them.
///
/// Hosts opt in via `"buckaroo:default"` in their capability file.
const COMMANDS: &[&str] = &[
    "buckaroo_health",
    "buckaroo_diagnostics",
    "buckaroo_load_path",
    "buckaroo_load_expr",
    "buckaroo_load_compare",
    "buckaroo_send",
    "buckaroo_pick_file",
];

fn main() {
    tauri_plugin::Builder::new(COMMANDS).build();
}
