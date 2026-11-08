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
///
/// The handler comes from `ibex_invoke_handler!`, the same macro the real app
/// uses, so a command can never be registered in one and missing from the other.
fn app(data_root: &std::path::Path) -> tauri::App<tauri::test::MockRuntime> {
    mock_builder()
        // An isolated data root: no test may write a decrypted cache or a fake
        // recents entry into the real one.
        .manage(AppState::with_data_root(data_root))
        .invoke_handler(ibex_lib::ibex_invoke_handler!())
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

    let app = app(tmp.path());
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
    let app = app(tmp.path());
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
fn an_encrypted_backup_asks_for_a_password_then_opens_with_it() {
    let tmp = tempfile::tempdir().unwrap();
    let root = FixtureBackup::new()
        .encrypted_with_password("hunter2")
        .with_device_name("Locked iPhone")
        .with_file("HomeDomain", "a.txt", b"secret contents")
        .build(tmp.path())
        .unwrap();
    let path = root.to_str().unwrap();

    let app = app(tmp.path());
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();

    // No password: the frontend switches on this code to show the modal.
    let err = invoke(
        &webview,
        "open_backup",
        json!({ "path": path, "password": null }),
    )
    .expect_err("an encrypted backup needs a password");
    assert_eq!(err["code"], "password_required");

    // Peeking names the device for the modal's title, without any password.
    let peek = invoke(&webview, "peek_backup", json!({ "path": path })).unwrap();
    assert_eq!(peek["encrypted"], true);
    assert_eq!(peek["info"]["deviceName"], "Locked iPhone");

    // A wrong password gets its own code, so the modal can keep the field and
    // show an inline error rather than bailing out.
    let err = invoke(
        &webview,
        "open_backup",
        json!({ "path": path, "password": "wrong" }),
    )
    .expect_err("a wrong password must be rejected");
    assert_eq!(err["code"], "bad_password");

    // And the right one opens it.
    let summary = invoke(
        &webview,
        "open_backup",
        json!({ "path": path, "password": "hunter2" }),
    )
    .unwrap();
    assert_eq!(summary["encrypted"], true);
    assert!(!summary["handle"].as_str().unwrap().is_empty());
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

#[test]
fn an_encrypted_backup_serves_decrypted_bytes_through_the_commands() {
    let tmp = tempfile::tempdir().unwrap();
    let root = FixtureBackup::new()
        .encrypted_with_password("pw")
        .with_sample_media()
        .build(tmp.path())
        .unwrap();

    let app = app(tmp.path());
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let backup = ibex_core::Backup::open_with_cache(
        &root,
        Some("pw"),
        ibex_core::CacheDir::at(tmp.path().join("cache")),
    )
    .unwrap();
    let handle = app.state::<AppState>().insert(backup);

    let listing = invoke(
        &webview,
        "list_dir",
        json!({ "handle": handle, "domain": "HomeDomain", "path": "Library/IbexSamples" }),
    )
    .unwrap();
    let id = |name: &str| -> String {
        listing["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["relativePath"].as_str().unwrap().ends_with(name))
            .unwrap_or_else(|| panic!("{name} missing"))["fileId"]
            .as_str()
            .unwrap()
            .to_string()
    };

    // A preview of an encrypted file must show the plaintext, not ciphertext.
    let text = invoke(
        &webview,
        "read_text_preview",
        json!({ "handle": handle, "fileId": id("notes.txt") }),
    )
    .unwrap();
    assert_eq!(text["kind"], "text");
    assert!(
        text["text"]
            .as_str()
            .unwrap()
            .contains("ibex synthetic fixture"),
        "expected decrypted text, got {}",
        text["text"]
    );

    // And file_info sniffs the decrypted content, so previews pick the right
    // view rather than falling back to hex for everything.
    let info = invoke(
        &webview,
        "file_info",
        json!({ "handle": handle, "fileId": id("tone.wav") }),
    )
    .unwrap();
    assert_eq!(info["sniffed"]["preview"], "audio");
    assert_eq!(info["dataBytes"], 44 + 8_000 * 5, "the plaintext length");

    // The hex view shows plaintext bytes too.
    let hex = invoke(
        &webview,
        "read_hex_preview",
        json!({ "handle": handle, "fileId": id("tone.wav"), "offset": 0, "len": 4 }),
    )
    .unwrap();
    assert_eq!(hex["hex"], "52494646", "\"RIFF\", decrypted");
}

#[test]
fn sms_conversations_and_messages_come_back_over_ipc() {
    let tmp = tempfile::tempdir().unwrap();
    let root = FixtureBackup::new()
        .with_sms()
        .with_address_book()
        .build(tmp.path())
        .unwrap();

    let app = app(tmp.path());
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let backup = ibex_core::Backup::open_with_cache(
        &root,
        None,
        ibex_core::CacheDir::at(tmp.path().join("cache")),
    )
    .unwrap();
    let handle = app.state::<AppState>().insert(backup);

    // Conversations, in the camelCase shape the generated bindings declare.
    let convs = invoke(&webview, "sms_conversations", json!({ "handle": handle })).unwrap();
    let items = convs["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert!(items[0].get("isGroup").is_some(), "got {:?}", items[0]);
    assert!(items[0].get("messageCount").is_some());
    assert!(items[0].get("lastMessageAt").is_some());

    // One thread, newest first, with the fiddly bits intact.
    let page = invoke(
        &webview,
        "sms_messages",
        json!({ "handle": handle, "conversationId": "1" }),
    )
    .unwrap();
    let messages = page["messages"].as_array().unwrap();
    // Eight: the tapback is excluded, but the `associated_message_type` 2
    // message is a message and must be listed.
    assert_eq!(messages.len(), 8, "the tapback must not be listed");
    assert!(
        messages.iter().any(|m| m["id"] == "FIXTURE-MSG-0009"),
        "a code-2 message must survive the IPC boundary too"
    );

    let find = |id: &str| {
        messages
            .iter()
            .find(|m| m["id"] == id)
            .unwrap_or_else(|| panic!("{id} missing"))
    };

    // Text decoded out of attributedBody survives the IPC boundary.
    assert_eq!(
        find("FIXTURE-ATTRIBUTED-0001")["body"],
        "Text that lives only in attributedBody"
    );
    // A reaction, with its camelCase kind.
    let reacted = find("FIXTURE-MSG-0002");
    assert_eq!(reacted["reactions"].as_array().unwrap().len(), 1);
    assert_eq!(reacted["reactions"][0]["kind"], "loved");
    // A reply link.
    assert_eq!(find("FIXTURE-MSG-0004")["replyTo"], "FIXTURE-MSG-0001");
    // An attachment that is present and one that is not.
    let with_attachments = find("FIXTURE-MSG-0007");
    let attachments = with_attachments["attachments"].as_array().unwrap();
    assert_eq!(attachments.len(), 2);
    assert!(attachments[0]["fileId"].is_string());
    assert!(
        attachments[1]["fileId"].is_null(),
        "the missing attachment must report no file id"
    );

    // Paging: two at a time, and the cursor round-trips through JSON.
    let first = invoke(
        &webview,
        "sms_messages",
        json!({ "handle": handle, "conversationId": "1",
                "cursor": { "beforeDate": null, "beforeRowid": null, "limit": 2 } }),
    )
    .unwrap();
    assert_eq!(first["messages"].as_array().unwrap().len(), 2);
    let older = &first["older"];
    assert!(older.is_object(), "expected a cursor for the next page");

    let second = invoke(
        &webview,
        "sms_messages",
        json!({ "handle": handle, "conversationId": "1", "cursor": older }),
    )
    .unwrap();
    let second_ids: Vec<&str> = second["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap())
        .collect();
    let first_ids: Vec<&str> = first["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap())
        .collect();
    assert!(
        second_ids.iter().all(|id| !first_ids.contains(id)),
        "pages must not overlap"
    );
}

#[test]
fn the_sidebar_learns_that_messages_is_now_implemented() {
    let tmp = tempfile::tempdir().unwrap();
    let root = FixtureBackup::new().with_sms().build(tmp.path()).unwrap();
    let path = root.to_str().unwrap();

    let app = app(tmp.path());
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();

    let summary = invoke(
        &webview,
        "open_backup",
        json!({ "path": path, "password": null }),
    )
    .unwrap();
    let sources = summary["sources"].as_array().unwrap();
    let sms = sources.iter().find(|s| s["id"] == "sms").unwrap();
    assert_eq!(sms["available"], true);
    assert_eq!(sms["implemented"], true, "M2 ships the Messages view");

    // M3 ships WhatsApp, M5 ships Photos, M6 ships Voice Memos and
    // Documents too; the rest are still pending, so the sidebar stays greyed
    // there.
    let shipped = ["sms", "whatsapp", "photos", "voice", "documents"];
    for source in sources {
        let id = source["id"].as_str().unwrap();
        assert_eq!(
            source["implemented"],
            shipped.contains(&id),
            "{id} implemented flag"
        );
    }
}

#[test]
fn whatsapp_chats_and_messages_come_back_over_ipc() {
    let tmp = tempfile::tempdir().unwrap();
    let root = FixtureBackup::new()
        .with_whatsapp()
        .with_address_book()
        .build(tmp.path())
        .unwrap();

    let app = app(tmp.path());
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let backup = ibex_core::Backup::open_with_cache(
        &root,
        None,
        ibex_core::CacheDir::at(tmp.path().join("cache")),
    )
    .unwrap();
    let handle = app.state::<AppState>().insert(backup);

    // Conversations by default; status feeds only when asked for.
    let chats = invoke(&webview, "wa_chats", json!({ "handle": handle })).unwrap();
    assert_eq!(chats["total"], 3, "two visible chats plus one hidden");
    let with_feeds = invoke(
        &webview,
        "wa_chats",
        json!({ "handle": handle, "includeBroadcasts": true }),
    )
    .unwrap();
    assert_eq!(with_feeds["total"], 4, "the toggle adds the status feed");

    // camelCase shapes, as the generated bindings declare.
    let first = &chats["items"][0];
    assert!(first.get("isGroup").is_some(), "got {first:?}");
    assert_eq!(first["service"], "whatsApp");

    // One thread.
    let page = invoke(
        &webview,
        "wa_messages",
        json!({ "handle": handle, "chatId": "1" }),
    )
    .unwrap();
    let messages = page["messages"].as_array().unwrap();
    let find = |id: &str| {
        messages
            .iter()
            .find(|m| m["id"] == id)
            .unwrap_or_else(|| panic!("{id} missing"))
    };

    // A type-0 message with a media item is a link preview, not an image.
    let preview = find("FIXTUREWA0002");
    assert_eq!(preview["kind"], "text");
    assert_eq!(preview["attachments"].as_array().unwrap().len(), 1);

    // An ordinary message carrying ZGROUPEVENTTYPE 2 is not a system notice.
    assert_eq!(find("FIXTUREWA0015")["kind"], "text");

    // An Ogg/Opus attachment is a voice note, and resolves in the backup.
    let voice = find("FIXTUREWA0004");
    assert_eq!(voice["kind"], "voice");
    assert!(voice["attachments"][0]["fileId"].is_string());

    // Media absent from the backup reports no file id.
    assert!(find("FIXTUREWA0010")["attachments"][0]["fileId"].is_null());

    // A deleted message keeps no text.
    let deleted = find("FIXTUREWA0006");
    assert_eq!(deleted["unsent"], true);
    assert!(deleted["body"].is_null());

    // Paging round-trips its cursor through JSON without overlap.
    let first_page = invoke(
        &webview,
        "wa_messages",
        json!({ "handle": handle, "chatId": "1",
                "cursor": { "beforeDate": null, "beforeRowid": null, "limit": 3 } }),
    )
    .unwrap();
    assert_eq!(first_page["messages"].as_array().unwrap().len(), 3);
    let older = &first_page["older"];
    assert!(older.is_object(), "expected a cursor for the next page");
    let second = invoke(
        &webview,
        "wa_messages",
        json!({ "handle": handle, "chatId": "1", "cursor": older }),
    )
    .unwrap();
    let ids = |v: &serde_json::Value| -> Vec<String> {
        v["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["id"].as_str().unwrap().to_string())
            .collect()
    };
    let (a, b) = (ids(&first_page), ids(&second));
    assert!(b.iter().all(|id| !a.contains(id)), "pages must not overlap");
}

#[test]
fn an_export_runs_as_a_job_and_reports_what_it_did() {
    let tmp = tempfile::tempdir().unwrap();
    let root = FixtureBackup::new()
        .with_sms()
        .with_address_book()
        .build(tmp.path())
        .unwrap();

    let app = app(tmp.path());
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let backup = ibex_core::Backup::open_with_cache(
        &root,
        None,
        ibex_core::CacheDir::at(tmp.path().join("cache")),
    )
    .unwrap();
    let handle = app.state::<AppState>().insert(backup);
    let out = tmp.path().join("export-out");

    // Starting returns a job id immediately rather than blocking.
    let job_id = invoke(
        &webview,
        "start_export",
        json!({
            "handle": handle,
            "selection": { "source": "sms", "conversationIds": [] },
            "format": "html",
            "outDir": out.to_str().unwrap(),
            "options": null
        }),
    )
    .unwrap();
    let job_id = job_id.as_str().expect("a job id").to_string();
    assert!(job_id.starts_with("job-"), "got {job_id}");

    // The job appears in the registry straight away, so the Exports view can
    // show it while it runs.
    let jobs = invoke(&webview, "list_jobs", json!({})).unwrap();
    let listed = jobs.as_array().unwrap();
    assert!(listed.iter().any(|j| j["id"] == job_id.as_str()));
    assert_eq!(listed[0]["kind"], "export");

    // Wait for it to finish. The work runs on a blocking worker, so poll the
    // registry rather than guessing at a sleep.
    let mut report = None;
    for _ in 0..200 {
        let jobs = invoke(&webview, "list_jobs", json!({})).unwrap();
        let job = jobs
            .as_array()
            .unwrap()
            .iter()
            .find(|j| j["id"] == job_id.as_str())
            .cloned()
            .unwrap();
        if job["state"] != "running" {
            report = Some(job);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let job = report.expect("the export should finish");
    assert_eq!(job["state"], "finished", "got {job}");

    // The report crossed IPC in camelCase, as the bindings declare.
    let report = &job["report"];
    assert_eq!(report["format"], "html");
    assert_eq!(report["cancelled"], false);
    assert!(report["totals"]["messages"].as_u64().unwrap() > 0);
    assert!(report.get("ibexVersion").is_some(), "{report}");
    assert!(report.get("formulasNeutralised").is_some());

    // And the files really exist where the report says.
    assert!(out.join("index.html").is_file());
    assert!(out.join("export-report.json").is_file());
}

#[test]
fn cancelling_an_unknown_job_is_false_not_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let app = app(tmp.path());
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();

    let cancelled = invoke(&webview, "cancel_job", json!({ "jobId": "job-nope" })).unwrap();
    assert_eq!(cancelled, false);
}
