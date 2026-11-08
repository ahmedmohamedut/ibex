//! Full-text search over SMS and WhatsApp message bodies.
//!
//! Builds a SQLite FTS5 index in `cache/<backup-id>/search.db`, walking the
//! same `SmsDb`/`WhatsAppDb` pagination the Messages and WhatsApp views use —
//! so the SMS `attributedBody` decode and the WhatsApp opaque-reference
//! preview filtering are reused rather than re-derived. See
//! `docs/ARCHITECTURE.md`.
//!
//! The index always rebuilds from scratch: a backup is a read-only
//! point-in-time snapshot opened fresh each session, so there is nothing to
//! detect as "stale" within one session — simpler and correct, not a cut
//! corner.

use chrono::{DateTime, Utc};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::export::{CancelToken, Progress};
use crate::extractors::sms::SmsDb;
use crate::extractors::whatsapp::WhatsAppDb;
use crate::{Backup, CacheDir, MessageCursor, Page, Paged, Service};

/// Which source(s) to restrict a search to.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SearchFilters {
    /// `"sms"` | `"whatsapp"`; `None` searches both.
    pub source: Option<String>,
    pub conversation_id: Option<String>,
}

/// One matching message.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SearchHit {
    pub source: String,
    pub service: Service,
    pub conversation_id: String,
    pub message_id: String,
    pub sent_at: Option<DateTime<Utc>>,
    /// The matched text, with `\u{1}`/`\u{2}` marking the start/end of each
    /// highlighted span, for the frontend to turn into `<mark>`.
    pub snippet: String,
}

/// What building the index did.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SearchIndexReport {
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    /// True when the build stopped early. The index keeps what it had
    /// already written, the same philosophy a cancelled export follows.
    pub cancelled: bool,
    pub conversations_indexed: u64,
    pub messages_indexed: u64,
}

/// Whether a usable index already exists, for the "first use prompts to
/// build the index" UI flow.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct IndexStatus {
    pub built: bool,
    pub messages_indexed: u64,
    pub built_at: Option<DateTime<Utc>>,
}

const SCHEMA: &str = "
    DROP TABLE IF EXISTS messages_fts;
    CREATE VIRTUAL TABLE messages_fts USING fts5(
        body,
        source UNINDEXED,
        service UNINDEXED,
        conversation_id UNINDEXED,
        message_id UNINDEXED,
        sent_at UNINDEXED,
        tokenize = 'unicode61'
    );
    CREATE TABLE IF NOT EXISTS index_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
";

fn open_search_db(cache: &CacheDir) -> crate::error::Result<Connection> {
    let path = cache.search_db();
    if let Some(parent) = path.parent() {
        CacheDir::ensure(parent)?;
    }
    Ok(Connection::open(path)?)
}

/// A conversation to walk while indexing, with enough to estimate progress.
struct ConvRef {
    source: &'static str,
    id: String,
    message_count: u64,
}

/// Every SMS + WhatsApp conversation (status/channel feeds excluded — they
/// are feeds, not message threads, and are hidden by default in the
/// WhatsApp view too).
fn all_conversations(backup: &Backup) -> crate::error::Result<Vec<ConvRef>> {
    let mut out = Vec::new();

    let sms = SmsDb::open(backup)?;
    let mut offset = 0u64;
    loop {
        let page = sms.list_conversations(Page::new(offset, Page::MAX_LIMIT))?;
        if page.items.is_empty() {
            break;
        }
        out.extend(page.items.iter().map(|c| ConvRef {
            source: "sms",
            id: c.id.clone(),
            message_count: c.message_count,
        }));
        match page.next_offset {
            Some(next) => offset = next,
            None => break,
        }
    }

    let wa = WhatsAppDb::open(backup)?;
    let mut offset = 0u64;
    loop {
        let page = wa.list_conversations(Page::new(offset, Page::MAX_LIMIT), false)?;
        if page.items.is_empty() {
            break;
        }
        out.extend(page.items.iter().map(|c| ConvRef {
            source: "whatsapp",
            id: c.id.clone(),
            message_count: c.message_count,
        }));
        match page.next_offset {
            Some(next) => offset = next,
            None => break,
        }
    }

    Ok(out)
}

/// Build (or rebuild) the search index for `backup`.
pub fn build_index(
    backup: &Backup,
    progress: &dyn Fn(Progress),
    cancel: &CancelToken,
) -> crate::error::Result<SearchIndexReport> {
    let started_at = Utc::now();

    let conversations = all_conversations(backup)?;
    // Over-counts (tapbacks, system messages and attachment-only rows have
    // no body to index) but is cheap and close enough for a progress bar.
    let estimated_total = conversations
        .iter()
        .map(|c| c.message_count)
        .sum::<u64>()
        .max(1);

    let mut conn = open_search_db(backup.cache())?;
    conn.execute_batch(SCHEMA)?;

    progress(Progress {
        done: 0,
        total: Some(estimated_total),
        message: "Indexing messages…".to_string(),
    });

    let sms = SmsDb::open(backup)?;
    let wa = WhatsAppDb::open(backup)?;

    let mut messages_indexed = 0u64;
    let mut conversations_indexed = 0u64;
    let mut cancelled = false;

    {
        let tx = conn.transaction()?;
        {
            let mut insert = tx.prepare(
                "INSERT INTO messages_fts
                    (body, source, service, conversation_id, message_id, sent_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;

            'convs: for conv in &conversations {
                if cancel.is_cancelled() {
                    cancelled = true;
                    break;
                }

                let mut cursor = MessageCursor::first(MessageCursor::MAX_LIMIT);
                loop {
                    let page = match conv.source {
                        "sms" => sms.list_messages(&conv.id, cursor)?,
                        _ => wa.list_messages(&conv.id, cursor)?,
                    };

                    for m in &page.messages {
                        let Some(body) = m.body.as_deref().filter(|b| !b.is_empty()) else {
                            continue;
                        };
                        let service_json = serde_json::to_string(&m.service)
                            .expect("Service is a plain enum; serializing it cannot fail");
                        insert.execute(rusqlite::params![
                            body,
                            conv.source,
                            service_json,
                            m.conversation_id,
                            m.id,
                            m.sent_at.map(|d| d.to_rfc3339()),
                        ])?;
                        messages_indexed += 1;
                    }
                    progress(Progress {
                        done: messages_indexed,
                        total: Some(estimated_total),
                        message: "Indexing messages…".to_string(),
                    });

                    match page.older {
                        Some(next) => cursor = next,
                        None => break,
                    }
                    if cancel.is_cancelled() {
                        cancelled = true;
                        break 'convs;
                    }
                }
                conversations_indexed += 1;
            }
        }
        tx.commit()?;
    }

    let finished_at = Utc::now();
    conn.execute(
        "INSERT INTO index_meta (key, value) VALUES ('built_at', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![finished_at.to_rfc3339()],
    )?;
    conn.execute(
        "INSERT INTO index_meta (key, value) VALUES ('message_count', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![messages_indexed.to_string()],
    )?;

    Ok(SearchIndexReport {
        started_at,
        finished_at,
        cancelled,
        conversations_indexed,
        messages_indexed,
    })
}

/// Whether `backup` already has a usable index, without building one.
pub fn index_status(backup: &Backup) -> crate::error::Result<IndexStatus> {
    let path = backup.cache().search_db();
    if !path.exists() {
        return Ok(IndexStatus {
            built: false,
            messages_indexed: 0,
            built_at: None,
        });
    }

    let conn = Connection::open(&path)?;
    // A missing index_meta table means an earlier build never finished, or
    // the file predates this schema — either way, "not built" is the right
    // answer, not an error.
    let row = conn.query_row(
        "SELECT (SELECT value FROM index_meta WHERE key = 'message_count'),
                (SELECT value FROM index_meta WHERE key = 'built_at')",
        [],
        |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
            ))
        },
    );

    let Ok((count, built_at)) = row else {
        return Ok(IndexStatus {
            built: false,
            messages_indexed: 0,
            built_at: None,
        });
    };

    Ok(IndexStatus {
        built: count.is_some(),
        messages_indexed: count.and_then(|c| c.parse().ok()).unwrap_or(0),
        built_at: built_at
            .and_then(|t| DateTime::parse_from_rfc3339(&t).ok())
            .map(|d| d.with_timezone(&Utc)),
    })
}

/// Build a safe FTS5 `MATCH` expression from free-form user input, rather
/// than passing it through.
///
/// Every whitespace-separated token is quoted (doubling any `"` inside it,
/// FTS5's own escape for a literal quote in a string) and space-joined,
/// which FTS5 treats as an implicit `AND` of exact-phrase tokens — so a
/// user's `"`, `-`, `NOT`, `OR`, a bare `:` and so on can never be read as
/// FTS5 query syntax. The last token gets a trailing `*` for prefix
/// matching, so a palette that searches as you type doesn't wait for a
/// whole word.
fn fts5_query(input: &str) -> String {
    let tokens: Vec<&str> = input.split_whitespace().collect();
    let last = tokens.len().saturating_sub(1);
    tokens
        .iter()
        .enumerate()
        .map(|(i, tok)| {
            let escaped = tok.replace('"', "\"\"");
            if i == last {
                format!("\"{escaped}\"*")
            } else {
                format!("\"{escaped}\"")
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Search the index built by [`build_index`].
pub fn search(
    backup: &Backup,
    query: &str,
    filters: &SearchFilters,
    page: Page,
) -> crate::error::Result<Paged<SearchHit>> {
    let match_expr = fts5_query(query);
    if match_expr.is_empty() {
        return Ok(Paged::new(Vec::new(), page, Some(0)));
    }

    let conn = open_search_db(backup.cache())?;
    let source = filters.source.as_deref();
    let conversation_id = filters.conversation_id.as_deref();

    // A missing messages_fts table (no index built yet) is "no results",
    // not an error — the caller checks `index_status` to tell those apart.
    let total: Option<i64> = conn
        .query_row(
            "SELECT count(*) FROM messages_fts
             WHERE messages_fts MATCH ?1
               AND (?2 IS NULL OR source = ?2)
               AND (?3 IS NULL OR conversation_id = ?3)",
            rusqlite::params![match_expr, source, conversation_id],
            |row| row.get(0),
        )
        .ok();
    let Some(total) = total else {
        return Ok(Paged::new(Vec::new(), page, Some(0)));
    };

    let mut stmt = conn.prepare(
        "SELECT message_id, conversation_id, source, service, sent_at,
                snippet(messages_fts, 0, ?4, ?5, '…', 12)
         FROM messages_fts
         WHERE messages_fts MATCH ?1
           AND (?2 IS NULL OR source = ?2)
           AND (?3 IS NULL OR conversation_id = ?3)
         ORDER BY rank
         LIMIT ?6 OFFSET ?7",
    )?;
    let rows = stmt.query_map(
        rusqlite::params![
            match_expr,
            source,
            conversation_id,
            "\u{1}",
            "\u{2}",
            page.limit,
            page.offset as i64
        ],
        |row| {
            let service_json: String = row.get(3)?;
            let sent_at_text: Option<String> = row.get(4)?;
            Ok(SearchHit {
                message_id: row.get(0)?,
                conversation_id: row.get(1)?,
                source: row.get(2)?,
                service: serde_json::from_str(&service_json).unwrap_or(Service::Other),
                sent_at: sent_at_text
                    .and_then(|t| DateTime::parse_from_rfc3339(&t).ok())
                    .map(|d| d.with_timezone(&Utc)),
                snippet: row.get(5)?,
            })
        },
    )?;
    let items = rows.collect::<rusqlite::Result<Vec<_>>>()?;

    Ok(Paged::new(items, page, Some(total.max(0) as u64)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CacheDir as CoreCacheDir;
    use crate::testutil::{FixtureBackup, whatsapp as wa_fixture};

    fn open_fixture(tmp: &std::path::Path) -> Backup {
        let root = FixtureBackup::new()
            .with_sms()
            .with_whatsapp()
            .build(tmp)
            .unwrap();
        Backup::open_with_cache(&root, None, CoreCacheDir::at(tmp.join("cache"))).unwrap()
    }

    fn build(backup: &Backup) -> SearchIndexReport {
        build_index(backup, &|_| {}, &CancelToken::new()).unwrap()
    }

    #[test]
    fn fts5_query_escapes_quotes_and_prefixes_the_last_token() {
        assert_eq!(fts5_query("hello world"), "\"hello\" \"world\"*");
        assert_eq!(fts5_query("say \"hi\""), "\"say\" \"\"\"hi\"\"\"*");
        assert_eq!(fts5_query(""), "");
        assert_eq!(fts5_query("   "), "");
    }

    #[test]
    fn finds_sms_text_that_lives_only_in_attributed_body() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        build(&backup);

        let hits = search(
            &backup,
            "attributedBody",
            &SearchFilters::default(),
            Page::default(),
        )
        .unwrap();
        assert!(
            hits.items.iter().any(|h| h.source == "sms"
                && h.snippet.contains('\u{1}')
                && h.snippet.to_lowercase().contains("attributedbody")),
            "expected the attributedBody-only message to be indexed and found, got {hits:?}"
        );
    }

    #[test]
    fn finds_whatsapp_text_and_the_source_filter_narrows_results() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        build(&backup);

        let both = search(
            &backup,
            "fixture",
            &SearchFilters::default(),
            Page::default(),
        )
        .unwrap();
        assert!(
            both.items.iter().any(|h| h.source == "whatsapp"),
            "got {both:?}"
        );

        let sms_only = search(
            &backup,
            "fixture",
            &SearchFilters {
                source: Some("sms".to_string()),
                conversation_id: None,
            },
            Page::default(),
        )
        .unwrap();
        assert!(sms_only.items.iter().all(|h| h.source == "sms"));
    }

    #[test]
    fn conversation_filter_narrows_to_one_thread() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        build(&backup);

        let hits = search(
            &backup,
            "fixture",
            &SearchFilters {
                source: Some("whatsapp".to_string()),
                conversation_id: Some(wa_fixture::DIRECT_SESSION.to_string()),
            },
            Page::default(),
        )
        .unwrap();
        assert!(
            hits.items
                .iter()
                .all(|h| h.conversation_id == wa_fixture::DIRECT_SESSION.to_string())
        );
    }

    #[test]
    fn rebuilding_does_not_duplicate_rows() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let first = build(&backup);
        let second = build(&backup);
        assert_eq!(first.messages_indexed, second.messages_indexed);

        let hits = search(
            &backup,
            "fixture",
            &SearchFilters::default(),
            Page::new(0, 2000),
        )
        .unwrap();
        let mut ids: Vec<_> = hits.items.iter().map(|h| h.message_id.clone()).collect();
        let before = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), before, "rebuild must not duplicate rows");
    }

    #[test]
    fn a_cancelled_build_reports_itself_as_cancelled_with_partial_counts() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());

        let cancel = CancelToken::new();
        cancel.cancel();
        let report = build_index(&backup, &|_| {}, &cancel).unwrap();
        assert!(report.cancelled);
        assert_eq!(report.messages_indexed, 0);
    }

    #[test]
    fn index_status_reflects_build_state() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());

        let before = index_status(&backup).unwrap();
        assert!(!before.built);

        let report = build(&backup);
        let after = index_status(&backup).unwrap();
        assert!(after.built);
        assert_eq!(after.messages_indexed, report.messages_indexed);
        assert!(after.built_at.is_some());
    }

    #[test]
    fn an_empty_query_returns_no_results_rather_than_erroring() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        build(&backup);

        let hits = search(&backup, "   ", &SearchFilters::default(), Page::default()).unwrap();
        assert!(hits.items.is_empty());
        assert_eq!(hits.total, Some(0));
    }
}
