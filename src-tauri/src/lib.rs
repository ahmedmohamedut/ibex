//! The ibex Tauri application.
//!
//! A thin shell over `ibex-core`: typed commands that run blocking work off the
//! UI thread, and the `ibex://` scheme that streams file bytes to the webview.
//! See the contract in `docs/ARCHITECTURE.md`.

pub mod commands;
pub mod dto;
pub mod error;
pub mod protocol;
pub mod recents;
pub mod state;

use state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::new())
        // Asynchronous so reads happen on a worker thread rather than blocking
        // the webview while a large file is served.
        .register_asynchronous_uri_scheme_protocol("ibex", |ctx, request, responder| {
            let app = ctx.app_handle().clone();
            std::thread::spawn(move || {
                let state = tauri::Manager::state::<AppState>(&app);
                responder.respond(protocol::respond(&state, &request));
            });
        })
        .invoke_handler(tauri::generate_handler![
            commands::open_backup,
            commands::close_backup,
            commands::recent_backups,
            commands::forget_recent,
            commands::list_domains,
            commands::list_dir,
            commands::file_info,
            commands::read_text_preview,
            commands::read_hex_preview,
            commands::backup_stats,
            commands::save_file_as,
            commands::clear_cache,
            commands::total_cache_size,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
