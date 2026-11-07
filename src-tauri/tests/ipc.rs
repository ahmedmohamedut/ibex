//! End-to-end tests over the real IPC boundary.
//!
//! These drive the commands exactly as the frontend does — through
//! `invoke`, with JSON arguments and camelCase keys — against a synthetic
//! backup. That catches the mistakes unit tests cannot: a wrong argument name,
//! a serde rename that does not match the TypeScript, a command missing from
//! the handler.

use ibex_core::testutil::FixtureBackup;
use ibex_lib::state::AppState;
use serde_json::{Value, json};
use tauri::test::{INVOKE_KEY, mock_builder, mock_context, noop_assets};
use tauri::webview::InvokeRequest;
use tauri::{Manager, WebviewWindow, ipc};

/// Build a mock app with the real command handler and state.
fn app() -> tauri::App<tauri::test::MockRuntime> {
    mock_builder()
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            ibex_lib::commands::open_backup,
            ibex_lib::commands::close_backup,
            ibex_lib::commands::recent_backups,
            ibex_lib::commands::forget_recent,
            ibex_lib::commands::list_domains,
            ibex_lib::commands::list_dir,
            ibex_lib::commands::file_info,
            ibex_lib::commands::read_text_preview,
            ibex_lib::commands::read_hex_preview,
            ibex_lib::commands::backup_stats,
            ibex_lib::commands::save_file_as,
            ibex_lib::commands::clear_cache,
            ibex_lib::commands::total_cache_size,
        ])
        .build(mock_context(noop_assets()))
        .expect("failed to build the mock app")
}

/// Invoke a command, returning either its JSON value or the error JSON.
fn invoke(
    webview: &WebviewWindow<tauri::test::MockRuntime>,
    cmd: &str,
    args: Value,
) -> Result<Value, Value> {
    let body = ipc::InvokeBody::Json(args);
    tauri::test::get_ipc_response(
        webview,
        InvokeRequest {
            cmd: cmd.into(),
            callback: ipc::CallbackFn(0),
            error: ipc::CallbackFn(1),
            url: "tauri://localhost".parse().unwrap(),
            body,
            headers: Default::default(),
            invoke_key: INVOKE_KEY.to_string(),
        },
    )
    .map(|response| response.deserialize::<Value>().expect("response is JSON"))
}

/// A mock app plus a fixture backup opened through `open_backup`.
struct Harness {
    _tmp: tempfile::TempDir,
    _app: tauri::App<tauri::test::MockRuntime>,
    webview: WebviewWindow<tauri::test::MockRuntime>,
    handle: String,
    root: std::path::PathBuf,
}

/// Open a fixture through the IPC layer, with the cache redirected into the
/// tempdir so the suite never touches the user's data directory.
fn harness() -> Harness {
    let tmp = tempfile::tempdir().unwrap();
    let root = FixtureBackup::new()
        .with_device_name("Fixture iPhone")
        .with_sample_media()
        .with_db(
            "HomeDomain",
            "Library/SMS/sms.db",
            "CREATE TABLE message (ROWID INTEGER PRIMARY KEY);",
        )
        .with_phantom_file("MediaDomain", "Library/SMS/Attachments/gone.jpg", 4242)
        .build(tmp.path())
        .unwrap();

    let app = app();
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();

    // `open_backup` uses the default cache location, so register the backup
    // directly with a redirected cache and then exercise the other commands.
    let backup = ibex_core::Backup::open_with_cache(
        &root,
        None,
        ibex_core::CacheDir::at(tmp.path().join("cache")),
    )
    .unwrap();
    let handle = app.state::<AppState>().insert(backup);

    Harness {
        _tmp: tmp,
        _app: app,
        webview,
        handle,
        root,
    }
}

#[test]
fn opening_a_folder_that_is_not_a_backup_returns_a_typed_error() {
    let tmp = tempfile::tempdir().unwrap();
    let app = app();
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();

    let err = invoke(
        &webview,
        "open_backup",
        json!({ "path": tmp.path().to_str().unwrap(), "password": null }),
    )
    .expect_err("an empty folder is not a backup");

    // The frontend's Welcome screen switches on this code to show its
    // plain-words explanation.
    assert_eq!(err["code"], "not_a_backup");
    assert!(
        err["message"].as_str().unwrap().contains("Manifest.db"),
        "got {err}"
    );
}

#[test]
fn an_encrypted_backup_asks_for_a_password_over_ipc() {
    let tmp = tempfile::tempdir().unwrap();
    let root = FixtureBackup::new()
        .marked_encrypted()
        .with_file("HomeDomain", "a.txt", b"x")
        .build(tmp.path())
        .unwrap();

    let app = app();
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();

    let err = invoke(
        &webview,
        "open_backup",
        json!({ "path": root.to_str().unwrap(), "password": null }),
    )
    .expect_err("an encrypted backup cannot open yet");
    assert_eq!(err["code"], "password_required");
}

#[test]
fn an_unknown_handle_is_rejected_by_every_command() {
    let h = harness();
    for (cmd, args) in [
        ("list_domains", json!({ "handle": "bogus" })),
        (
            "list_dir",
            json!({ "handle": "bogus", "domain": "HomeDomain", "path": "" }),
        ),
        ("file_info", json!({ "handle": "bogus", "fileId": "abc" })),
        ("backup_stats", json!({ "handle": "bogus" })),
    ] {
        let err = invoke(&h.webview, cmd, args).expect_err("{cmd} should reject");
        assert_eq!(err["code"], "unknown_handle", "{cmd}");
    }
}

#[test]
fn domains_and_listings_come_back_in_the_shape_the_frontend_expects() {
    let h = harness();

    let domains = invoke(&h.webview, "list_domains", json!({ "handle": h.handle })).unwrap();
    let domains = domains.as_array().unwrap();
    assert!(!domains.is_empty());
    // camelCase keys, as the generated TypeScript declares.
    assert!(domains[0].get("fileCount").is_some(), "got {domains:?}");
    assert!(domains[0].get("domain").is_some());

    // The root of HomeDomain lists one directory: Library.
    let listing = invoke(
        &h.webview,
        "list_dir",
        json!({ "handle": h.handle, "domain": "HomeDomain", "path": "" }),
    )
    .unwrap();
    assert!(listing.get("items").is_some());
    assert!(listing.get("nextOffset").is_some());
    let items = listing["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["relativePath"], "Library");
    assert_eq!(items[0]["kind"], "directory");

    // Paging through a directory with an explicit page argument.
    let page = invoke(
        &h.webview,
        "list_dir",
        json!({
            "handle": h.handle,
            "domain": "HomeDomain",
            "path": "Library/IbexSamples",
            "page": { "offset": 0, "limit": 2 }
        }),
    )
    .unwrap();
    assert_eq!(page["items"].as_array().unwrap().len(), 2);
    assert_eq!(page["nextOffset"], 2);
    assert_eq!(page["total"], 6);
}

#[test]
fn file_info_describes_a_file_the_way_the_detail_pane_needs() {
    let h = harness();

    let listing = invoke(
        &h.webview,
        "list_dir",
        json!({ "handle": h.handle, "domain": "HomeDomain", "path": "Library/IbexSamples" }),
    )
    .unwrap();
    let wav = listing["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["relativePath"].as_str().unwrap().ends_with("tone.wav"))
        .expect("the fixture has a wav");

    let info = invoke(
        &h.webview,
        "file_info",
        json!({ "handle": h.handle, "fileId": wav["fileId"] }),
    )
    .unwrap();

    assert_eq!(info["name"], "tone.wav");
    assert_eq!(info["dataPresent"], true);
    assert_eq!(info["sniffed"]["preview"], "audio");
    assert!(
        info["sniffed"]["mime"]
            .as_str()
            .unwrap()
            .starts_with("audio/"),
        "got {}",
        info["sniffed"]["mime"]
    );
    assert_eq!(info["dataBytes"], 44 + 8_000 * 5);
    // Key material must never reach the frontend.
    assert!(
        info["meta"].get("encryptionKey").is_none(),
        "wrapped keys must not cross IPC: {}",
        info["meta"]
    );
}

#[test]
fn a_file_absent_from_the_backup_is_described_not_hidden() {
    let h = harness();

    let listing = invoke(
        &h.webview,
        "list_dir",
        json!({ "handle": h.handle, "domain": "MediaDomain", "path": "Library/SMS/Attachments" }),
    )
    .unwrap();
    let gone = &listing["items"].as_array().unwrap()[0];

    let info = invoke(
        &h.webview,
        "file_info",
        json!({ "handle": h.handle, "fileId": gone["fileId"] }),
    )
    .unwrap();

    assert_eq!(info["name"], "gone.jpg");
    assert_eq!(info["dataPresent"], false, "its bytes were never captured");
    assert!(info["sniffed"].is_null(), "nothing to sniff");
    assert_eq!(
        info["meta"]["size"], 4242,
        "the manifest still records a size"
    );
}

#[test]
fn previews_arrive_rendered_and_labelled() {
    let h = harness();

    let id = |path: &str| -> String {
        let listing = invoke(
            &h.webview,
            "list_dir",
            json!({ "handle": h.handle, "domain": "HomeDomain", "path": "Library/IbexSamples" }),
        )
        .unwrap();
        listing["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["relativePath"].as_str().unwrap().ends_with(path))
            .unwrap_or_else(|| panic!("{path} missing"))["fileId"]
            .as_str()
            .unwrap()
            .to_string()
    };

    // Plain text comes back verbatim.
    let text = invoke(
        &h.webview,
        "read_text_preview",
        json!({ "handle": h.handle, "fileId": id("notes.txt") }),
    )
    .unwrap();
    assert_eq!(text["kind"], "text");
    assert_eq!(text["truncated"], false);
    assert_eq!(text["lossy"], false);
    assert!(
        text["text"]
            .as_str()
            .unwrap()
            .contains("ibex synthetic fixture")
    );

    // A binary plist is rendered as XML rather than shown as noise.
    let plist = invoke(
        &h.webview,
        "read_text_preview",
        json!({ "handle": h.handle, "fileId": id("preferences.plist") }),
    )
    .unwrap();
    assert_eq!(plist["kind"], "plist");
    assert_eq!(plist["plistRendered"], true);
    assert!(plist["text"].as_str().unwrap().contains("<key>Label</key>"));

    // A bounded read reports itself as truncated, so the UI can say so.
    let truncated = invoke(
        &h.webview,
        "read_text_preview",
        json!({ "handle": h.handle, "fileId": id("notes.txt"), "maxBytes": 8 }),
    )
    .unwrap();
    assert_eq!(truncated["truncated"], true);
    assert_eq!(truncated["bytesRead"], 8);

    // The hex view gets the real leading bytes of the WAV.
    let hex = invoke(
        &h.webview,
        "read_hex_preview",
        json!({ "handle": h.handle, "fileId": id("tone.wav"), "offset": 0, "len": 4 }),
    )
    .unwrap();
    assert_eq!(hex["hex"], "52494646", "\"RIFF\"");
    assert_eq!(hex["offset"], 0);
    assert_eq!(hex["totalBytes"], 44 + 8_000 * 5);
}

#[test]
fn saving_a_file_out_through_ipc_keeps_the_bytes_and_the_mtime() {
    let h = harness();
    let listing = invoke(
        &h.webview,
        "list_dir",
        json!({ "handle": h.handle, "domain": "HomeDomain", "path": "Library/IbexSamples" }),
    )
    .unwrap();
    let png = listing["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["relativePath"].as_str().unwrap().ends_with("pixel.png"))
        .unwrap();

    let dest = h._tmp.path().join("out/saved.png");
    let report = invoke(
        &h.webview,
        "save_file_as",
        json!({
            "handle": h.handle,
            "fileId": png["fileId"],
            "dest": dest.to_str().unwrap()
        }),
    )
    .unwrap();

    assert_eq!(report["mtimeRestored"], true);
    assert!(dest.is_file(), "parent directories should be created");

    // Byte-identical to what is in the backup.
    let source_id = png["fileId"].as_str().unwrap();
    let in_backup = h.root.join(&source_id[..2]).join(source_id);
    assert_eq!(
        std::fs::read(&dest).unwrap(),
        std::fs::read(&in_backup).unwrap()
    );
    assert_eq!(report["bytes"], std::fs::metadata(&dest).unwrap().len());
}

#[test]
fn stats_and_cache_clearing_work_over_ipc() {
    let h = harness();

    // Open a database so the cache has something in it.
    let _ = invoke(&h.webview, "list_domains", json!({ "handle": h.handle })).unwrap();

    let stats = invoke(&h.webview, "backup_stats", json!({ "handle": h.handle })).unwrap();
    assert!(stats["totalBytes"].as_u64().unwrap() > 0);
    assert!(stats["fileCount"].as_u64().unwrap() > 0);
    assert_eq!(stats["domainCount"], 2, "HomeDomain and MediaDomain");
    assert!(
        stats["cacheBytes"].as_u64().unwrap() > 0,
        "opening copies Manifest.db into the cache"
    );

    let freed = invoke(&h.webview, "clear_cache", json!({ "handle": h.handle })).unwrap();
    assert!(freed.as_u64().unwrap() > 0, "clearing should free bytes");

    // And the backup still works afterwards: copies are remade on demand.
    let after = invoke(&h.webview, "backup_stats", json!({ "handle": h.handle })).unwrap();
    assert_eq!(after["fileCount"], stats["fileCount"]);
}

#[test]
fn closing_a_backup_invalidates_its_handle() {
    let h = harness();
    assert!(invoke(&h.webview, "list_domains", json!({ "handle": h.handle })).is_ok());

    invoke(&h.webview, "close_backup", json!({ "handle": h.handle })).unwrap();

    let err = invoke(&h.webview, "list_domains", json!({ "handle": h.handle })).unwrap_err();
    assert_eq!(err["code"], "unknown_handle");
}

#[test]
fn a_full_session_never_writes_to_the_backup_folder() {
    let h = harness();

    fn snapshot(dir: &std::path::Path) -> Vec<(std::path::PathBuf, u64, std::time::SystemTime)> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for entry in std::fs::read_dir(&d).unwrap() {
                let entry = entry.unwrap();
                let meta = entry.metadata().unwrap();
                if meta.is_dir() {
                    stack.push(entry.path());
                } else {
                    out.push((entry.path(), meta.len(), meta.modified().unwrap()));
                }
            }
        }
        out.sort();
        out
    }

    let before = snapshot(&h.root);

    // Everything the Raw browser does in a session.
    let _ = invoke(&h.webview, "list_domains", json!({ "handle": h.handle })).unwrap();
    let _ = invoke(&h.webview, "backup_stats", json!({ "handle": h.handle })).unwrap();
    let listing = invoke(
        &h.webview,
        "list_dir",
        json!({ "handle": h.handle, "domain": "HomeDomain", "path": "Library/IbexSamples" }),
    )
    .unwrap();
    for item in listing["items"].as_array().unwrap() {
        let id = &item["fileId"];
        let _ = invoke(
            &h.webview,
            "file_info",
            json!({ "handle": h.handle, "fileId": id }),
        );
        let _ = invoke(
            &h.webview,
            "read_text_preview",
            json!({ "handle": h.handle, "fileId": id }),
        );
        let _ = invoke(
            &h.webview,
            "read_hex_preview",
            json!({ "handle": h.handle, "fileId": id }),
        );
    }

    assert_eq!(
        before,
        snapshot(&h.root),
        "hard rule 1: the backup folder must not be modified"
    );
}
