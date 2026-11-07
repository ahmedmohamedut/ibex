//! Tauri application entry point.
//!
//! Commands, the `ibex://` media protocol and the job system arrive in
//! milestone M1; see `docs/ARCHITECTURE.md`.

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
