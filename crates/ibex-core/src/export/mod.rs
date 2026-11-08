//! Exporting conversations out of a backup.
//!
//! Both chat extractors emit the same [`crate::extractors::Message`], so one
//! set of exporters serves SMS, iMessage and WhatsApp without knowing which it
//! is looking at.
//!
//! This is the first part of ibex whose output leaves the machine: a folder
//! somebody opens in a browser, mails to a lawyer, or loads into a spreadsheet.
//! Three things follow from that, and each is enforced here rather than left to
//! the individual exporters:
//!
//! - **Every name is sanitized** through [`naming`], because names come from
//!   message data and a hostile one must not escape the output folder.
//! - **Everything skipped is reported.** An export that silently dropped twelve
//!   photos would look identical to a complete one, so [`ExportReport`] counts
//!   what was missing.
//! - **Cancellation is honest.** A cancelled export says so in its report
//!   rather than passing off a partial folder as finished.

pub mod csv;
pub mod files;
pub mod html;
pub mod json;
pub mod naming;
pub mod photos;
pub mod source;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::Result;
use crate::extractors::Message;
use crate::{Backup, Page};

pub use source::ChatSource;

/// Which export format to write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ExportFormat {
    /// Readable offline in any browser, with media copied alongside.
    Html,
    Csv,
    Json,
    /// The original attachment files, with their original names and mtimes.
    Files,
}

impl ExportFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            ExportFormat::Html => "html",
            ExportFormat::Csv => "csv",
            ExportFormat::Json => "json",
            ExportFormat::Files => "files",
        }
    }
}

/// What to export.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Selection {
    /// Source id: `sms`, `whatsapp` or `photos`.
    pub source: String,
    /// For a chat source: conversation ids, or empty for every conversation.
    /// For `photos`: explicit asset ids, or empty to use `photo_filter`.
    pub conversation_ids: Vec<String>,
    /// `photos` only. Which assets `conversation_ids` being empty means:
    /// defaults to the same view the grid shows (trashed and hidden
    /// excluded). Ignored, and absent from every existing caller, for a chat
    /// source — `#[serde(default)]` keeps this an additive, backward
    /// compatible change to a struct the IPC layer already shares.
    #[serde(default)]
    pub photo_filter: Option<crate::extractors::PhotoFilter>,
}

impl Selection {
    /// One conversation.
    pub fn one(source: &str, id: &str) -> Self {
        Self {
            source: source.to_string(),
            conversation_ids: vec![id.to_string()],
            photo_filter: None,
        }
    }

    /// Every conversation in a source.
    pub fn all(source: &str) -> Self {
        Self {
            source: source.to_string(),
            conversation_ids: Vec::new(),
            photo_filter: None,
        }
    }

    pub fn is_everything(&self) -> bool {
        self.conversation_ids.is_empty()
    }
}

/// How to export.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ExportOptions {
    /// Copy attachment files alongside the export.
    pub include_attachments: bool,
    /// Only messages at or after this instant.
    pub from: Option<DateTime<Utc>>,
    /// Only messages at or before this instant.
    pub to: Option<DateTime<Utc>>,
    /// Include the status and channel feeds WhatsApp hides by default.
    pub include_broadcasts: bool,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            include_attachments: true,
            from: None,
            to: None,
            include_broadcasts: false,
        }
    }
}

impl ExportOptions {
    /// Whether a message falls inside the requested date range.
    ///
    /// A message with no date is kept: dropping it would lose data over a
    /// field that is legitimately absent.
    pub fn in_range(&self, message: &Message) -> bool {
        let Some(sent_at) = message.sent_at else {
            return true;
        };
        if let Some(from) = self.from
            && sent_at < from
        {
            return false;
        }
        if let Some(to) = self.to
            && sent_at > to
        {
            return false;
        }
        true
    }
}

/// Progress of a running export.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Progress {
    pub done: u64,
    /// `None` until the total is known.
    pub total: Option<u64>,
    /// What is happening, with no message content in it.
    pub message: String,
}

/// Cooperative cancellation.
///
/// Checked between messages and before each media copy, so cancelling an export
/// of a 45 GB backup stops within one file rather than at the end.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// What one conversation contributed to an export.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ConversationReport {
    pub id: String,
    /// The folder or file name written, already sanitized.
    pub output_name: String,
    pub messages: u64,
    pub attachments_copied: u64,
    pub attachments_missing: u64,
}

/// Totals across an export.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ExportTotals {
    pub conversations: u64,
    pub messages: u64,
    pub attachments_copied: u64,
    /// Attachments referenced by a message but absent from the backup. Normal,
    /// and reported so an incomplete export cannot look complete.
    pub attachments_missing: u64,
    pub bytes_written: u64,
}

/// What an export did. Written to `export-report.json` beside the output.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ExportReport {
    pub ibex_version: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub format: String,
    pub source: String,
    /// True when the export stopped early. The folder is kept, and partial.
    pub cancelled: bool,
    pub output_dir: String,
    pub conversations: Vec<ConversationReport>,
    pub totals: ExportTotals,
    /// Counts of CSV cells rewritten to stop a spreadsheet executing them.
    pub formulas_neutralised: u64,
    /// Anything the user should know, with no message content in it.
    pub warnings: Vec<String>,
}

impl ExportReport {
    /// A finished report for an export that covered nothing.
    ///
    /// Public because it is the honest representation of an export that found
    /// no conversations, and because callers outside this crate — the job
    /// registry, tests — need a way to name that state without reaching into
    /// the lifecycle methods below.
    pub fn empty(format: ExportFormat, source: &str, out: &Path, cancelled: bool) -> Self {
        let mut report = Self::new(format, source, out);
        report.finish(cancelled);
        report
    }

    pub(crate) fn new(format: ExportFormat, source: &str, out: &Path) -> Self {
        Self {
            ibex_version: env!("CARGO_PKG_VERSION").to_string(),
            started_at: Utc::now(),
            finished_at: Utc::now(),
            format: format.as_str().to_string(),
            source: source.to_string(),
            cancelled: false,
            output_dir: out.display().to_string(),
            conversations: Vec::new(),
            totals: ExportTotals::default(),
            formulas_neutralised: 0,
            warnings: Vec::new(),
        }
    }

    /// Fold one conversation's figures into the totals.
    pub(crate) fn add(&mut self, conversation: ConversationReport) {
        self.totals.conversations += 1;
        self.totals.messages += conversation.messages;
        self.totals.attachments_copied += conversation.attachments_copied;
        self.totals.attachments_missing += conversation.attachments_missing;
        self.conversations.push(conversation);
    }

    pub(crate) fn finish(&mut self, cancelled: bool) {
        self.finished_at = Utc::now();
        self.cancelled = cancelled;
        if cancelled {
            self.warnings
                .push("The export was cancelled, so this folder is incomplete.".to_string());
        }
        if self.totals.attachments_missing > 0 {
            self.warnings.push(format!(
                "{} attachment(s) are referenced by messages but their contents are not in the \
                 backup, so they could not be exported.",
                self.totals.attachments_missing
            ));
        }
    }

    /// Write the report next to the export.
    pub(crate) fn write(&self, out: &Path) -> Result<PathBuf> {
        let path = out.join("export-report.json");
        let json = serde_json::to_vec_pretty(self).map_err(|e| crate::error::Error::Io {
            path: path.clone(),
            source: std::io::Error::other(e),
        })?;
        crate::error::IoResultExt::at(std::fs::write(&path, json), &path)?;
        Ok(path)
    }
}

/// One export format.
pub trait Exporter {
    /// Write `sel` into `out`.
    ///
    /// `progress` is called as work completes; callers that forward it over IPC
    /// are expected to throttle. `cancel` is checked often enough that stopping
    /// feels immediate.
    fn export(
        &self,
        backup: &Backup,
        sel: &Selection,
        out: &Path,
        opts: &ExportOptions,
        progress: &dyn Fn(Progress),
        cancel: &CancelToken,
    ) -> Result<ExportReport>;
}

/// Run an export in the chosen format.
pub fn run(
    backup: &Backup,
    sel: &Selection,
    format: ExportFormat,
    out: &Path,
    opts: &ExportOptions,
    progress: &dyn Fn(Progress),
    cancel: &CancelToken,
) -> Result<ExportReport> {
    // Photos has one export shape — original files, named and dated — rather
    // than the four formats a chat source offers, so it is selected by
    // source id and the requested `format` is not consulted.
    if sel.source == "photos" {
        return photos::PhotosExporter.export(backup, sel, out, opts, progress, cancel);
    }
    match format {
        ExportFormat::Html => html::HtmlExporter.export(backup, sel, out, opts, progress, cancel),
        ExportFormat::Csv => csv::CsvExporter.export(backup, sel, out, opts, progress, cancel),
        ExportFormat::Json => json::JsonExporter.export(backup, sel, out, opts, progress, cancel),
        ExportFormat::Files => {
            files::FilesExporter.export(backup, sel, out, opts, progress, cancel)
        }
    }
}

/// The conversations an export should cover, in listing order.
pub(crate) fn conversations_for(
    source: &ChatSource<'_>,
    sel: &Selection,
    opts: &ExportOptions,
) -> Result<Vec<crate::extractors::ConversationSummary>> {
    let all = source
        .conversations(Page::first(Page::MAX_LIMIT), opts.include_broadcasts)?
        .items;
    if sel.is_everything() {
        return Ok(all);
    }
    // Preserve the caller's order, and skip ids that are not in the listing
    // rather than failing the whole export over one stale id.
    let mut out = Vec::new();
    for id in &sel.conversation_ids {
        if let Some(found) = all.iter().find(|c| &c.id == id) {
            out.push(found.clone());
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extractors::{MessageKind, Service};

    fn message_at(iso: Option<&str>) -> Message {
        Message {
            id: "m".into(),
            conversation_id: "1".into(),
            sent_at: iso.map(|s| DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)),
            from_me: false,
            sender: None,
            body: None,
            kind: MessageKind::Text,
            attachments: Vec::new(),
            reply_to: None,
            reactions: Vec::new(),
            edited: false,
            unsent: false,
            service: Service::Sms,
        }
    }

    fn at(iso: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(iso)
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn a_date_range_filters_inclusively() {
        let opts = ExportOptions {
            from: Some(at("2026-11-01T00:00:00Z")),
            to: Some(at("2026-11-30T23:59:59Z")),
            ..Default::default()
        };
        assert!(opts.in_range(&message_at(Some("2026-11-15T12:00:00Z"))));
        // The bounds themselves are included.
        assert!(opts.in_range(&message_at(Some("2026-11-01T00:00:00Z"))));
        assert!(opts.in_range(&message_at(Some("2026-11-30T23:59:59Z"))));
        assert!(!opts.in_range(&message_at(Some("2026-10-31T23:59:59Z"))));
        assert!(!opts.in_range(&message_at(Some("2026-12-01T00:00:00Z"))));
    }

    #[test]
    fn a_message_with_no_date_is_kept() {
        // Dropping it would lose data over a field that is legitimately absent.
        let opts = ExportOptions {
            from: Some(at("2026-11-01T00:00:00Z")),
            ..Default::default()
        };
        assert!(opts.in_range(&message_at(None)));
    }

    #[test]
    fn an_open_ended_range_works_at_both_ends() {
        let from_only = ExportOptions {
            from: Some(at("2026-11-01T00:00:00Z")),
            ..Default::default()
        };
        assert!(from_only.in_range(&message_at(Some("2030-01-01T00:00:00Z"))));
        assert!(!from_only.in_range(&message_at(Some("2020-01-01T00:00:00Z"))));

        let to_only = ExportOptions {
            to: Some(at("2026-11-01T00:00:00Z")),
            ..Default::default()
        };
        assert!(to_only.in_range(&message_at(Some("2020-01-01T00:00:00Z"))));
        assert!(!to_only.in_range(&message_at(Some("2030-01-01T00:00:00Z"))));

        // No range at all keeps everything.
        let none = ExportOptions::default();
        assert!(none.in_range(&message_at(Some("1999-01-01T00:00:00Z"))));
    }

    #[test]
    fn selections_describe_one_or_everything() {
        let one = Selection::one("sms", "348");
        assert!(!one.is_everything());
        assert_eq!(one.conversation_ids, vec!["348"]);

        let all = Selection::all("whatsapp");
        assert!(all.is_everything());
    }

    #[test]
    fn cancellation_is_observable_through_a_clone() {
        // The job holds one handle and the exporter another.
        let token = CancelToken::new();
        let in_job = token.clone();
        assert!(!in_job.is_cancelled());
        token.cancel();
        assert!(in_job.is_cancelled());
    }

    #[test]
    fn the_report_totals_what_it_is_given_and_warns_about_gaps() {
        let mut report = ExportReport::new(ExportFormat::Html, "sms", Path::new("/out"));
        report.add(ConversationReport {
            id: "1".into(),
            output_name: "Chat".into(),
            messages: 10,
            attachments_copied: 3,
            attachments_missing: 1,
        });
        report.add(ConversationReport {
            id: "2".into(),
            output_name: "Other".into(),
            messages: 5,
            attachments_copied: 0,
            attachments_missing: 0,
        });
        report.finish(false);

        assert_eq!(report.totals.conversations, 2);
        assert_eq!(report.totals.messages, 15);
        assert_eq!(report.totals.attachments_copied, 3);
        assert_eq!(report.totals.attachments_missing, 1);
        assert!(!report.cancelled);
        // A missing attachment must produce a visible warning.
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.contains("not in the backup")),
            "{:?}",
            report.warnings
        );
    }

    #[test]
    fn a_cancelled_report_says_so() {
        let mut report = ExportReport::new(ExportFormat::Csv, "sms", Path::new("/out"));
        report.finish(true);
        assert!(report.cancelled);
        assert!(
            report.warnings.iter().any(|w| w.contains("cancelled")),
            "a cancelled export must not look complete: {:?}",
            report.warnings
        );
    }

    #[test]
    fn the_report_serializes_as_camel_case() {
        let mut report = ExportReport::new(ExportFormat::Json, "whatsapp", Path::new("/out"));
        report.finish(false);
        let json = serde_json::to_string(&report).unwrap();
        for key in [
            "ibexVersion",
            "startedAt",
            "finishedAt",
            "outputDir",
            "formulasNeutralised",
        ] {
            assert!(json.contains(key), "missing {key} in {json}");
        }
    }

    #[test]
    fn formats_have_stable_names() {
        // These end up in the report and in CLI arguments.
        assert_eq!(ExportFormat::Html.as_str(), "html");
        assert_eq!(ExportFormat::Csv.as_str(), "csv");
        assert_eq!(ExportFormat::Json.as_str(), "json");
        assert_eq!(ExportFormat::Files.as_str(), "files");
    }
}
