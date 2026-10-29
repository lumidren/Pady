/// Every command in `generate_handler!` (src/main.rs). Listing them makes each
/// one need a permission (`allow-<command>`), which only the main window is
/// granted (capabilities/default.json); the overlay window can just listen.
const COMMANDS: &[&str] = &[
    "get_state",
    "save_config",
    "set_enabled",
    "reset_config",
    "set_autostart",
    "test_action",
    "set_live",
    "open_config_folder",
    "open_touchpad_settings",
    "detect_app",
    "fix_permissions",
    "apply_preset",
    "check_update",
    "install_update",
    "pick_program",
    "record_shape",
];

fn main() {
    let manifest = tauri_build::AppManifest::new().commands(COMMANDS);
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(manifest)).expect("failed to run tauri-build");
}
