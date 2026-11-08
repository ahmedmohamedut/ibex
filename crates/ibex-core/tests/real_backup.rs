//! Integration tests against a real backup.
//!
//! Gated by the `IBEX_TEST_BACKUP` environment variable, pointing at the folder
//! that contains `Manifest.db`. When it is unset — as it is in CI and on any
//! machine without a backup — every test here returns early and passes.
//!
//! An encrypted backup also needs `IBEX_TEST_PASSWORD`. Set it in your own
//! shell so it never lands in a command line or in this repository.
//!
//! These assert **structure and counts only**. They never print or assert on
//! message text, contact names, phone numbers or file contents (AGENTS.md hard
//! rules 2 and 3).

use std::path::PathBuf;

use ibex_core::{Backup, EntryKind, Page};

/// The backup under test, or `None` when the variable is unset.
fn test_backup() -> Option<PathBuf> {
    let path = std::env::var_os("IBEX_TEST_BACKUP")?;
    if path.is_empty() {
        return None;
    }
    Some(PathBuf::from(path))
}

/// The backup password, if one was provided.
fn test_password() -> Option<String> {
    std::env::var("IBEX_TEST_PASSWORD")
        .ok()
        .filter(|p| !p.is_empty())
}

/// Open the backup, or skip.
macro_rules! backup_or_skip {
    () => {
        match test_backup() {
            Some(path) => match Backup::open(&path, test_password().as_deref()) {
                Ok(backup) => backup,
                Err(e) if e.code() == "password_required" => {
                    println!(
                        "skipping: IBEX_TEST_BACKUP is encrypted. \
                             Set IBEX_TEST_PASSWORD to run these assertions."
                    );
                    return;
                }
                Err(e) if e.code() == "bad_password" => {
                    panic!("IBEX_TEST_PASSWORD did not unlock IBEX_TEST_BACKUP");
                }
                Err(e) => panic!("could not open IBEX_TEST_BACKUP: {e}"),
            },
            None => {
                println!("skipping: IBEX_TEST_BACKUP is not set");
                return;
            }
        }
    };
}

#[test]
fn opens_and_describes_the_device() {
    let backup = backup_or_skip!();
    let info = backup.info();

    // Structure, not values: we assert that the fields are *there*, and print
    // only the non-identifying ones.
    assert!(
        info.product_type.is_some() || info.product_name.is_some(),
        "a real backup should identify its device model"
    );
    assert!(
        info.product_version.is_some(),
        "a real backup should record its iOS version"
    );
    assert_eq!(backup.id().len(), 32);
    println!(
        "device model: {:?}, iOS: {:?}, encrypted: {}",
        info.product_type, info.product_version, info.encrypted
    );
}

#[test]
fn the_manifest_has_domains_and_files() {
    let backup = backup_or_skip!();

    let total = backup.manifest().count_files().unwrap();
    assert!(total > 0, "a real backup has files");

    let domains = backup.domains().unwrap();
    assert!(!domains.is_empty(), "a real backup has domains");
    assert!(
        domains.iter().any(|d| d.domain == "HomeDomain"),
        "every iOS backup has a HomeDomain"
    );

    // Per-domain counts must add up to the manifest total.
    let summed: u64 = domains.iter().map(|d| d.file_count).sum();
    assert!(
        summed <= total,
        "per-domain counts ({summed}) cannot exceed the total ({total})"
    );

    println!("{} files across {} domains", total, domains.len());
    for domain in domains.iter().take(15) {
        println!("  {} ({} entries)", domain.domain, domain.file_count);
    }
}

#[test]
fn file_ids_match_the_hash_of_domain_and_path() {
    let backup = backup_or_skip!();

    // Sample rows and recompute their IDs. This is the single assumption the
    // whole resolver rests on, so it is worth checking against real data.
    let rows: Vec<(String, String, String)> = backup.manifest().with_connection(|conn| {
        let mut stmt = conn
            .prepare("SELECT fileID, domain, relativePath FROM Files LIMIT 500")
            .unwrap();
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    });
    assert!(!rows.is_empty());

    let mut checked = 0;
    for (file_id, domain, relative_path) in &rows {
        if relative_path.is_empty() {
            continue;
        }
        assert_eq!(
            &ibex_core::util::paths::file_id(domain, relative_path),
            file_id,
            "fileID should be SHA1(domain-relativePath); mismatch in domain {domain}"
        );
        checked += 1;
    }
    println!("recomputed {checked} file IDs, all matching");
}

#[test]
fn listings_walk_one_level_at_a_time_and_paginate() {
    let backup = backup_or_skip!();

    let root = backup.list_dir("HomeDomain", "", Page::default()).unwrap();
    assert!(!root.is_empty(), "HomeDomain should have a root listing");
    for entry in &root.items {
        assert!(
            !entry.relative_path.contains('/'),
            "a root listing must not contain nested paths"
        );
    }

    // Walk down into the first directory we find, if any.
    if let Some(dir) = root.items.iter().find(|e| e.kind.is_dir()) {
        let children = backup
            .list_dir("HomeDomain", &dir.relative_path, Page::default())
            .unwrap();
        for entry in &children.items {
            assert_eq!(
                entry.parent(),
                Some(dir.relative_path.as_str()),
                "children must be exactly one level below their parent"
            );
        }
        println!(
            "{} has {} direct children",
            dir.relative_path,
            children.total.unwrap_or(0)
        );

        // Paging must cover the listing exactly once.
        if children.total.unwrap_or(0) > 1 {
            let mut seen = Vec::new();
            let mut offset = Some(0);
            while let Some(o) = offset {
                let page = backup
                    .list_dir("HomeDomain", &dir.relative_path, Page::new(o, 1))
                    .unwrap();
                seen.extend(page.items.iter().map(|e| e.file_id.clone()));
                offset = page.next_offset;
            }
            let unique: std::collections::HashSet<_> = seen.iter().collect();
            assert_eq!(
                seen.len(),
                unique.len(),
                "paging returned the same entry twice"
            );
            assert_eq!(
                seen.len() as u64,
                children.total.unwrap(),
                "paging missed entries"
            );
        }
    }
}

#[test]
fn the_sms_database_is_where_we_expect_it() {
    let backup = backup_or_skip!();

    match backup.find("HomeDomain", "Library/SMS/sms.db").unwrap() {
        Some(entry) => {
            assert_eq!(entry.kind, EntryKind::File);
            assert_eq!(entry.file_id, "3d0d7e5fb2ce288813306e4d4636395e047a3d28");
            let present = backup.has_data(&entry);
            println!(
                "sms.db: {} bytes of metadata, data present: {present}",
                entry.size()
            );

            if present {
                // Opening it must go through a working copy in the cache, and
                // the structure-only query below prints no content.
                let conn = backup.open_db("HomeDomain", "Library/SMS/sms.db").unwrap();
                let tables: i64 = conn
                    .query_one(
                        "SELECT count(*) FROM sqlite_master WHERE type = 'table'",
                        [],
                        |r| r.get(0),
                    )
                    .unwrap();
                assert!(tables > 0, "sms.db should contain tables");
                let messages: i64 = conn
                    .query_one("SELECT count(*) FROM message", [], |r| r.get(0))
                    .unwrap();
                println!("sms.db: {tables} tables, {messages} message rows");
            }
        }
        None => println!("no sms.db in this backup"),
    }
}

#[test]
fn opening_a_backup_does_not_modify_it() {
    let Some(path) = test_backup() else {
        println!("skipping: IBEX_TEST_BACKUP is not set");
        return;
    };

    // Snapshot the top level (not the whole tree: a real backup is huge) plus
    // the shard directory holding sms.db.
    fn snapshot(dir: &std::path::Path) -> Vec<(PathBuf, u64, Option<std::time::SystemTime>)> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut out: Vec<_> = entries
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let meta = e.metadata().ok()?;
                Some((e.path(), meta.len(), meta.modified().ok()))
            })
            .collect();
        out.sort();
        out
    }

    let before = snapshot(&path);
    let before_shard = snapshot(&path.join("3d"));

    let backup = match Backup::open(&path, test_password().as_deref()) {
        Ok(backup) => backup,
        Err(e) if e.code() == "password_required" => {
            println!("skipping: encrypted backup, IBEX_TEST_PASSWORD not set");
            return;
        }
        Err(e) => panic!("{e}"),
    };
    let _ = backup.domains().unwrap();
    let _ = backup.list_dir("HomeDomain", "", Page::default()).unwrap();
    if let Some(entry) = backup.find("HomeDomain", "Library/SMS/sms.db").unwrap()
        && backup.has_data(&entry)
    {
        let _ = backup.open_db("HomeDomain", "Library/SMS/sms.db");
    }

    assert_eq!(
        before,
        snapshot(&path),
        "hard rule 1: the backup folder must not be modified"
    );
    assert_eq!(
        before_shard,
        snapshot(&path.join("3d")),
        "hard rule 1: backup file shards must not be modified"
    );
    println!("backup folder unchanged after open, listing and a database read");
}

#[test]
fn file_dates_are_plausible_which_settles_the_mbfile_epoch_question() {
    let backup = backup_or_skip!();

    // `MBFile`'s LastModified is read as a Unix timestamp, not an Apple-epoch
    // one. If that is wrong, every date is out by 31 years and lands in the
    // 2050s. This is the check that settles it against real data.
    let listing = backup
        .list_dir("HomeDomain", "Library", Page::first(200))
        .unwrap();

    let mut checked = 0;
    let mut earliest: Option<chrono::DateTime<chrono::Utc>> = None;
    let mut latest: Option<chrono::DateTime<chrono::Utc>> = None;
    for entry in &listing.items {
        let Some(mtime) = entry.meta.as_ref().and_then(|m| m.mtime) else {
            continue;
        };
        checked += 1;
        earliest = Some(earliest.map_or(mtime, |e| e.min(mtime)));
        latest = Some(latest.map_or(mtime, |l| l.max(mtime)));
    }

    assert!(checked > 0, "no entry carried a modification time");
    let earliest = earliest.unwrap();
    let latest = latest.unwrap();
    println!("{checked} dates, from {earliest} to {latest}");

    // An iPhone's files cannot predate the iPhone, and cannot be from the
    // future. Reading Apple-epoch seconds as Unix would put these past 2050.
    let year_of = |d: chrono::DateTime<chrono::Utc>| {
        use chrono::Datelike;
        d.year()
    };
    assert!(
        year_of(earliest) >= 2007,
        "earliest file date is {earliest}, before the iPhone existed - \
         the epoch handling is wrong"
    );
    assert!(
        year_of(latest) <= 2030,
        "latest file date is {latest}, in the future - MBFile times are \
         probably being read as Apple-epoch rather than Unix seconds"
    );
}

#[test]
fn files_decrypt_to_plausible_content() {
    let backup = backup_or_skip!();
    if !backup.is_encrypted() {
        println!("skipping: this backup is not encrypted");
        return;
    }

    // Sniff a sample of files. On a real device most of HomeDomain's Library
    // is plists, databases and images; if decryption were broken, everything
    // would sniff as opaque binary instead.
    let listing = backup
        .list_dir("HomeDomain", "Library/Preferences", Page::first(60))
        .unwrap();

    let mut sniffed = 0;
    let mut recognised = 0;
    for entry in listing.items.iter().filter(|e| e.kind.is_file()) {
        if !backup.has_data(entry) {
            continue;
        }
        let Ok(s) = backup.sniff(entry) else { continue };
        sniffed += 1;
        if s.mime != "application/octet-stream" {
            recognised += 1;
        }
    }

    if sniffed == 0 {
        println!("no readable files in Library/Preferences to sample");
        return;
    }
    println!("{recognised} of {sniffed} sampled files have a recognisable type");
    assert!(
        recognised * 2 >= sniffed,
        "only {recognised} of {sniffed} decrypted files look like known formats; \
         decryption is probably producing garbage"
    );
}

#[test]
fn sms_conversation_counts_reconcile_with_the_database() {
    let backup = backup_or_skip!();
    use ibex_core::extractors::Extractor;
    use ibex_core::extractors::sms::{Sms, SmsDb};

    if !Sms::detect(&backup).is_present() {
        println!("skipping: no sms.db in this backup");
        return;
    }
    let db = SmsDb::open(&backup).unwrap();

    // Every conversation, and the sum of what each claims to hold.
    let page = db
        .list_conversations(ibex_core::Page::first(ibex_core::Page::MAX_LIMIT))
        .unwrap();
    let claimed: u64 = page.items.iter().map(|c| c.message_count).sum();

    // What the database says, counting only rows that are messages rather than
    // tapbacks. The ranges matter: a real backup carries ordinary messages with
    // associated_message_type 2 and 3, so "non-zero" is not "tapback".
    let actual: i64 = backup
        .open_db("HomeDomain", "Library/SMS/sms.db")
        .unwrap()
        .query_one(
            "SELECT count(*) FROM chat_message_join j
             JOIN message m ON m.ROWID = j.message_id
             WHERE NOT (coalesce(m.associated_message_type, 0) = 1000
                 OR coalesce(m.associated_message_type, 0) BETWEEN 2000 AND 2007
                 OR coalesce(m.associated_message_type, 0) BETWEEN 3000 AND 3007)",
            [],
            |r| r.get(0),
        )
        .unwrap();

    println!(
        "{} conversations claiming {claimed} messages; database has {actual}",
        page.items.len()
    );
    assert_eq!(
        claimed, actual as u64,
        "the conversation list must account for every message exactly once"
    );
}

#[test]
fn text_only_in_attributedbody_is_recovered_on_real_data() {
    let backup = backup_or_skip!();
    use ibex_core::MessageCursor;
    use ibex_core::extractors::Extractor;
    use ibex_core::extractors::sms::{Sms, SmsDb};

    if !Sms::detect(&backup).is_present() {
        println!("skipping: no sms.db in this backup");
        return;
    }

    // Find the chats holding messages whose `text` column is NULL. On iOS 16+
    // their text lives only in the attributedBody typedstream, so these are
    // exactly the messages that would render blank without decoding it.
    let conn = backup.open_db("HomeDomain", "Library/SMS/sms.db").unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT j.chat_id, count(*) FROM chat_message_join j
             JOIN message m ON m.ROWID = j.message_id
             WHERE m.text IS NULL AND m.attributedBody IS NOT NULL
             GROUP BY j.chat_id ORDER BY count(*) DESC LIMIT 5",
        )
        .unwrap();
    let targets: Vec<(i64, i64)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();

    if targets.is_empty() {
        println!("no text-NULL messages in this backup; nothing to check");
        return;
    }

    let db = SmsDb::open(&backup).unwrap();
    let mut recovered = 0usize;
    let mut checked = 0usize;
    for (chat_id, _) in &targets {
        let page = db
            .list_messages(&chat_id.to_string(), MessageCursor::first(500))
            .unwrap();
        for message in &page.messages {
            checked += 1;
            if message
                .body
                .as_deref()
                .is_some_and(|b| !b.trim().is_empty())
            {
                recovered += 1;
            }
        }
    }

    println!(
        "{recovered} of {checked} messages across {} chats have body text",
        targets.len()
    );
    // Not all messages have text — attachment-only ones legitimately do not —
    // but a chat full of blanks would mean decoding had failed outright.
    assert!(
        recovered * 2 > checked,
        "only {recovered} of {checked} messages had text; attributedBody \
         decoding is probably not working"
    );
}

#[test]
fn whatsapp_chat_counts_reconcile_and_the_toggle_adds_feeds() {
    let backup = backup_or_skip!();
    use ibex_core::extractors::Extractor;
    use ibex_core::extractors::whatsapp::{WhatsApp, WhatsAppDb};

    if !WhatsApp::detect(&backup).is_present() {
        println!("skipping: no WhatsApp container in this backup");
        return;
    }
    let db = WhatsAppDb::open(&backup).unwrap();
    let page = ibex_core::Page::first(ibex_core::Page::MAX_LIMIT);

    let conversations = db.list_conversations(page, false).unwrap();
    let everything = db.list_conversations(page, true).unwrap();
    println!(
        "{} conversations, {} sessions in all",
        conversations.total.unwrap_or(0),
        everything.total.unwrap_or(0)
    );

    // Conversations are session types 0 and 1, hidden ones included: real data
    // showed hidden sessions hold genuine messages.
    let expected: i64 = db
        .connection()
        .query_one(
            "SELECT count(*) FROM ZWACHATSESSION WHERE coalesce(ZSESSIONTYPE, 0) IN (0, 1)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(conversations.total, Some(expected as u64));
    assert!(
        everything.total >= conversations.total,
        "the toggle must only ever add sessions"
    );

    // Each chat's count must be the rows actually present, not WhatsApp's own
    // ZMESSAGECOUNTER: in this backup that column disagreed with reality for
    // every single chat, which made the list contradict the thread.
    let mut checked = 0;
    for chat in conversations.items.iter().take(40) {
        let actual: i64 = db
            .connection()
            .query_one(
                "SELECT count(*) FROM ZWAMESSAGE WHERE ZCHATSESSION = ?1",
                [chat.id.parse::<i64>().unwrap()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            chat.message_count, actual as u64,
            "chat {} reports {} but holds {actual}",
            chat.id, chat.message_count
        );
        checked += 1;
    }
    println!("{checked} chat counts reconciled against the database");
}

#[test]
fn whatsapp_media_resolves_and_kinds_are_inferred_from_files() {
    let backup = backup_or_skip!();
    use ibex_core::MessageCursor;
    use ibex_core::extractors::whatsapp::{WhatsApp, WhatsAppDb};
    use ibex_core::extractors::{Extractor, MessageKind};
    use std::collections::BTreeMap;

    if !WhatsApp::detect(&backup).is_present() {
        println!("skipping: no WhatsApp container in this backup");
        return;
    }
    let db = WhatsAppDb::open(&backup).unwrap();

    // Sample the busiest chats, which is where the variety is.
    let chats = db
        .list_conversations(ibex_core::Page::first(12), false)
        .unwrap();

    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    let mut attachments = 0usize;
    let mut resolved = 0usize;
    for chat in &chats.items {
        let page = db
            .list_messages(&chat.id, MessageCursor::first(200))
            .unwrap();
        for message in &page.messages {
            let key = match &message.kind {
                MessageKind::Unknown(code) => format!("Unknown({code})"),
                other => format!("{other:?}"),
            };
            *kinds.entry(key).or_default() += 1;
            for a in &message.attachments {
                attachments += 1;
                if a.file_id.is_some() {
                    resolved += 1;
                }
            }
        }
    }

    let total: usize = kinds.values().sum();
    println!(
        "{total} messages sampled across {} chats",
        chats.items.len()
    );
    for (kind, n) in &kinds {
        println!("  {kind:<14} {n}");
    }
    println!("attachments: {resolved}/{attachments} resolved in the backup");

    assert!(total > 0, "the sample should not be empty");
    // If kind inference were broken, nearly everything would land in Unknown.
    let unknown: usize = kinds
        .iter()
        .filter(|(k, _)| k.starts_with("Unknown"))
        .map(|(_, n)| *n)
        .sum();
    assert!(
        unknown * 4 < total,
        "{unknown} of {total} messages had an unrecognised kind; inference is \
         probably not working"
    );
    // Media resolution measured at 97.5% when M3 was planned; assert a floor
    // well below it so a real regression shows up without flapping.
    if attachments > 50 {
        assert!(
            resolved * 10 >= attachments * 8,
            "only {resolved} of {attachments} attachments resolved; the \
             Message/ prefix may have changed"
        );
    }
}
