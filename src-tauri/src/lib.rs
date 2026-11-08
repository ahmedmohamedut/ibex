//! The ibex Tauri application.
//!
//! A thin shell over `ibex-core`: typed commands that run blocking work off the
//! UI thread, and the `ibex://` scheme that streams file bytes to the webview.
//! See the contract in `docs/ARCHITECTURE.md`.

pub mod commands;
pub mod dto;
pub mod error;
pub mod jobs;
pub mod protocol;
pub mod recents;
pub mod state;

use state::AppState;

/// The command handler, as a macro so the real app and the integration tests
/// cannot drift apart. Adding a command in one place adds it in both.
#[macro_export]
macro_rules! ibex_invoke_handler {
    () => {
        tauri::generate_handler![
            $crate::commands::peek_backup,
            $crate::commands::open_backup,
            $crate::commands::close_backup,
            $crate::commands::recent_backups,
            $crate::commands::forget_recent,
            $crate::commands::list_domains,
            $crate::commands::list_dir,
            $crate::commands::file_info,
            $crate::commands::read_text_preview,
            $crate::commands::read_hex_preview,
            $crate::commands::sms_conversations,
            $crate::commands::sms_messages,
            $crate::commands::wa_chats,
            $crate::commands::wa_messages,
            $crate::commands::photos_list,
            $crate::commands::photos_timeline,
            $crate::commands::photos_albums,
            $crate::commands::photo_detail,
            $crate::commands::backup_stats,
            $crate::commands::save_file_as,
            $crate::commands::clear_cache,
            $crate::commands::start_export,
            $crate::commands::cancel_job,
            $crate::commands::list_jobs,
            $crate::commands::total_cache_size,
        ]
    };
}

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
        .invoke_handler(ibex_invoke_handler!())
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
