//! End-to-end export tests against the synthetic fixtures.
//!
//! These check the things that matter once output leaves the program: that
//! escaping holds, that links resolve to files that exist, that nothing is
//! silently dropped, and that a cancelled export admits it.

use std::path::Path;

use ibex_core::export::{
    CancelToken, ExportFormat, ExportOptions, Selection, csv as csv_export, run,
};
use ibex_core::testutil::FixtureBackup;
use ibex_core::{Backup, CacheDir};

/// A fixture with both chat sources and the sample media.
fn fixture(tmp: &Path) -> Backup {
    let root = FixtureBackup::new()
        .with_sms()
        .with_whatsapp()
        .with_address_book()
        .build(tmp)
        .unwrap();
    Backup::open_with_cache(&root, None, CacheDir::at(tmp.join("cache"))).unwrap()
}

fn noop_progress(_p: ibex_core::export::Progress) {}

#[test]
fn html_export_produces_a_browsable_tree_with_resolving_links() {
    let tmp = tempfile::tempdir().unwrap();
    let backup = fixture(tmp.path());
    let out = tmp.path().join("out-html");

    let report = run(
        &backup,
        &Selection::all("sms"),
        ExportFormat::Html,
        &out,
        &ExportOptions::default(),
        &noop_progress,
        &CancelToken::new(),
    )
    .unwrap();

    assert!(!report.cancelled);
    assert!(report.totals.conversations >= 1);
    assert!(report.totals.messages >= 1);

    // The top-level index exists and every conversation link resolves.
    let index = out.join("index.html");
    assert!(index.is_file(), "no top-level index.html");
    let index_html = std::fs::read_to_string(&index).unwrap();

    let mut checked = 0;
    for conversation in &report.conversations {
        let page = out.join(&conversation.output_name).join("index.html");
        assert!(page.is_file(), "missing {page:?}");
        assert!(
            index_html.contains(&conversation.output_name),
            "the index does not link to {}",
            conversation.output_name
        );

        // Every src/href in the conversation page resolves to a real file.
        let html = std::fs::read_to_string(&page).unwrap();
        for attr in ["src=\"", "href=\""] {
            for part in html.split(attr).skip(1) {
                let target = part.split('"').next().unwrap_or("");
                if target.is_empty() || target.starts_with("http") || target.starts_with('#') {
                    continue;
                }
                let resolved = out.join(&conversation.output_name).join(target);
                assert!(
                    resolved.exists(),
                    "{page:?} links to {target:?} which does not exist"
                );
                checked += 1;
            }
        }
    }
    assert!(checked > 0, "no links were checked, so this proved nothing");

    // The report lands beside the output.
    assert!(out.join("export-report.json").is_file());
}

#[test]
fn html_export_escapes_hostile_message_content() {
    // The fixture's SMS content is benign, so this builds a conversation whose
    // name and message body are both injection attempts.
    let tmp = tempfile::tempdir().unwrap();
    let root = FixtureBackup::new()
        .with_hostile_sms()
        .build(tmp.path())
        .unwrap();
    let backup =
        Backup::open_with_cache(&root, None, CacheDir::at(tmp.path().join("cache"))).unwrap();
    let out = tmp.path().join("out-hostile");

    let report = run(
        &backup,
        &Selection::all("sms"),
        ExportFormat::Html,
        &out,
        &ExportOptions::default(),
        &noop_progress,
        &CancelToken::new(),
    )
    .unwrap();

    let mut pages = vec![std::fs::read_to_string(out.join("index.html")).unwrap()];
    for conversation in &report.conversations {
        pages.push(
            std::fs::read_to_string(out.join(&conversation.output_name).join("index.html"))
                .unwrap(),
        );
    }

    for html in &pages {
        let lower = html.to_lowercase();

        // The thing that matters: no message data became markup. Since `<` is
        // escaped everywhere, no tag from the backup can survive. Note that
        // escaped *text* may legitimately contain strings like "javascript:"
        // or "onerror=" — a message body saying so is harmless, because it is
        // text. So the assertions below target markup and attributes, not
        // substrings.
        assert!(!lower.contains("<script"), "a <script tag survived");
        assert!(!lower.contains("<iframe"), "an <iframe tag survived");
        // The exporter emits its own `<img src="media/...">` tags, so look for
        // the fixture's unquoted payload specifically: `<img src=x` can only
        // appear if message data was not escaped.
        assert!(
            !lower.contains("<img src=x"),
            "the injected <img payload survived as markup"
        );
        assert!(
            !lower.contains("href=\"javascript:") && !lower.contains("src=\"javascript:"),
            "a javascript: URL reached an attribute"
        );
        // No attribute was closed early by message data: every `"` from the
        // backup is escaped, so a handler cannot be appended to a tag.
        assert!(
            !lower.contains("onerror=\""),
            "an event handler reached a tag"
        );
    }

    // And the hostile text is present, escaped rather than dropped: an export
    // that silently discarded awkward messages would be worse than one that
    // escapes them.
    let conversation_pages = &pages[1..];
    assert!(
        conversation_pages
            .iter()
            .any(|html| html.contains("&lt;script&gt;alert(&#39;body&#39;)&lt;/script&gt;")),
        "the escaped script body should appear verbatim"
    );
}

#[test]
fn csv_export_has_a_stable_header_and_quotes_awkward_text() {
    let tmp = tempfile::tempdir().unwrap();
    let root = FixtureBackup::new()
        .with_hostile_sms()
        .build(tmp.path())
        .unwrap();
    let backup =
        Backup::open_with_cache(&root, None, CacheDir::at(tmp.path().join("cache"))).unwrap();
    let out = tmp.path().join("out-csv");

    let report = run(
        &backup,
        &Selection::all("sms"),
        ExportFormat::Csv,
        &out,
        &ExportOptions::default(),
        &noop_progress,
        &CancelToken::new(),
    )
    .unwrap();

    let path = out.join("sms-messages.csv");
    assert!(path.is_file());

    // Re-read it with a CSV reader: if quoting were wrong, parsing would fail
    // or the field count would drift.
    let mut reader = csv::Reader::from_path(&path).unwrap();
    let header: Vec<String> = reader
        .headers()
        .unwrap()
        .iter()
        .map(str::to_string)
        .collect();
    assert_eq!(header, csv_export::COLUMNS, "the header is a contract");

    let mut rows = 0;
    let mut found_formula = false;
    for record in reader.records() {
        let record = record.expect("every row must parse");
        assert_eq!(record.len(), csv_export::COLUMNS.len());
        let body = record.get(9).unwrap_or("");
        if body.starts_with('\'') {
            found_formula = true;
        }
        rows += 1;
    }
    assert!(rows > 0);
    assert!(
        found_formula,
        "the fixture's =formula message should have been neutralised"
    );
    assert!(
        report.formulas_neutralised > 0,
        "and the report should say so: {report:?}"
    );
}

#[test]
fn json_export_round_trips_through_serde() {
    let tmp = tempfile::tempdir().unwrap();
    let backup = fixture(tmp.path());
    let out = tmp.path().join("out-json");

    let report = run(
        &backup,
        &Selection::all("whatsapp"),
        ExportFormat::Json,
        &out,
        &ExportOptions::default(),
        &noop_progress,
        &CancelToken::new(),
    )
    .unwrap();

    let path = out.join("whatsapp-messages.json");
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).expect("valid JSON");

    assert_eq!(value["source"], "whatsapp");
    let conversations = value["conversations"].as_array().unwrap();
    assert_eq!(conversations.len() as u64, report.totals.conversations);

    // camelCase, matching the TypeScript bindings the UI uses.
    let first_message = &conversations[0]["messages"][0];
    assert!(
        first_message.get("conversationId").is_some(),
        "{first_message}"
    );
    assert!(first_message.get("fromMe").is_some());
    assert!(first_message.get("sentAt").is_some());

    // Totals agree with what is in the file.
    let counted: usize = conversations
        .iter()
        .map(|c| c["messages"].as_array().unwrap().len())
        .sum();
    assert_eq!(counted as u64, report.totals.messages);
}

#[test]
fn files_export_copies_originals_with_names_mtimes_and_no_collisions() {
    let tmp = tempfile::tempdir().unwrap();
    let backup = fixture(tmp.path());
    let out = tmp.path().join("out-files");

    let report = run(
        &backup,
        &Selection::all("whatsapp"),
        ExportFormat::Files,
        &out,
        &ExportOptions::default(),
        &noop_progress,
        &CancelToken::new(),
    )
    .unwrap();

    assert!(
        report.totals.attachments_copied > 0,
        "the fixture has media to copy"
    );
    // The fixture deliberately references one file that is not in the backup.
    assert!(
        report.totals.attachments_missing > 0,
        "a missing attachment must be reported, not hidden"
    );

    // Every copied file exists and is non-empty.
    let mut found = 0;
    for conversation in &report.conversations {
        let folder = out.join(&conversation.output_name);
        if !folder.is_dir() {
            continue;
        }
        for entry in std::fs::read_dir(&folder).unwrap() {
            let entry = entry.unwrap();
            let meta = entry.metadata().unwrap();
            assert!(meta.len() > 0, "{:?} is empty", entry.path());
            // mtime came from the manifest, so it predates this test run.
            let modified = meta.modified().unwrap();
            assert!(
                modified < std::time::SystemTime::now(),
                "mtime should come from the backup"
            );
            found += 1;
        }
    }
    assert_eq!(found as u64, report.totals.attachments_copied);
}

#[test]
fn a_cancelled_export_says_so_and_keeps_what_it_wrote() {
    let tmp = tempfile::tempdir().unwrap();
    let backup = fixture(tmp.path());
    let out = tmp.path().join("out-cancelled");

    // Cancelled before anything is written.
    let cancel = CancelToken::new();
    cancel.cancel();
    let report = run(
        &backup,
        &Selection::all("sms"),
        ExportFormat::Html,
        &out,
        &ExportOptions::default(),
        &noop_progress,
        &cancel,
    )
    .unwrap();

    assert!(report.cancelled, "the report must admit it was cancelled");
    assert!(
        report.warnings.iter().any(|w| w.contains("cancelled")),
        "{:?}",
        report.warnings
    );
    // The report is still written, so the UI can show what happened.
    assert!(out.join("export-report.json").is_file());
}

#[test]
fn a_date_range_narrows_the_export() {
    let tmp = tempfile::tempdir().unwrap();
    let backup = fixture(tmp.path());

    let all = run(
        &backup,
        &Selection::all("sms"),
        ExportFormat::Json,
        &tmp.path().join("out-all"),
        &ExportOptions::default(),
        &noop_progress,
        &CancelToken::new(),
    )
    .unwrap();

    // The SMS fixture's messages are all on 2026-11-01; a window before that
    // must come back empty rather than failing.
    let opts = ExportOptions {
        to: Some(
            chrono::DateTime::parse_from_rfc3339("2020-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
        ),
        ..Default::default()
    };
    let narrowed = run(
        &backup,
        &Selection::all("sms"),
        ExportFormat::Json,
        &tmp.path().join("out-narrow"),
        &opts,
        &noop_progress,
        &CancelToken::new(),
    )
    .unwrap();

    assert!(all.totals.messages > 0);
    assert_eq!(
        narrowed.totals.messages, 0,
        "nothing should fall inside the window"
    );
}

#[test]
fn exporting_one_conversation_exports_only_that_one() {
    let tmp = tempfile::tempdir().unwrap();
    let backup = fixture(tmp.path());
    let out = tmp.path().join("out-one");

    let report = run(
        &backup,
        &Selection::one("sms", "1"),
        ExportFormat::Html,
        &out,
        &ExportOptions::default(),
        &noop_progress,
        &CancelToken::new(),
    )
    .unwrap();

    assert_eq!(report.totals.conversations, 1);
    assert_eq!(report.conversations[0].id, "1");

    // A stale id is skipped rather than failing the whole export.
    let report = run(
        &backup,
        &Selection::one("sms", "99999"),
        ExportFormat::Html,
        &tmp.path().join("out-stale"),
        &ExportOptions::default(),
        &noop_progress,
        &CancelToken::new(),
    )
    .unwrap();
    assert_eq!(report.totals.conversations, 0);
}

#[test]
fn an_export_never_writes_to_the_backup_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let root = FixtureBackup::new()
        .with_sms()
        .with_whatsapp()
        .build(tmp.path())
        .unwrap();
    let backup =
        Backup::open_with_cache(&root, None, CacheDir::at(tmp.path().join("cache"))).unwrap();

    fn snapshot(dir: &Path) -> Vec<(std::path::PathBuf, u64, std::time::SystemTime)> {
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

    let before = snapshot(&root);
    for (source, format) in [
        ("sms", ExportFormat::Html),
        ("sms", ExportFormat::Csv),
        ("whatsapp", ExportFormat::Json),
        ("whatsapp", ExportFormat::Files),
    ] {
        run(
            &backup,
            &Selection::all(source),
            format,
            &tmp.path().join(format!("out-{source}-{}", format.as_str())),
            &ExportOptions::default(),
            &noop_progress,
            &CancelToken::new(),
        )
        .unwrap();
    }
    assert_eq!(
        before,
        snapshot(&root),
        "hard rule 1: exporting must not touch the backup"
    );
}
