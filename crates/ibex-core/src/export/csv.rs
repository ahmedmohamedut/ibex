//! CSV export: one row per message.
//!
//! Message text is full of commas, quotes and newlines, so quoting goes through
//! the `csv` crate rather than being hand-rolled.
//!
//! **Formula neutralisation.** A spreadsheet treats a cell beginning `=`, `+`,
//! `-` or `@` as a formula and evaluates it on open, which would let whoever
//! sent a message run something on the machine of whoever reads the export —
//! including fetching remote content, breaking ibex's offline promise at one
//! remove. Such cells get a leading apostrophe, which spreadsheets read as
//! "this is text". The count goes in the report so it is never a silent edit.

use std::path::Path;

use crate::Backup;
use crate::error::{IoResultExt, Result};
use crate::export::{
    CancelToken, ChatSource, ConversationReport, ExportFormat, ExportOptions, ExportReport,
    Exporter, Progress, Selection, conversations_for,
};
use crate::extractors::{ConversationSummary, Message};

/// The header row. Column order is part of the output contract, so it is
/// asserted by a golden test.
pub const COLUMNS: &[&str] = &[
    "conversation_id",
    "conversation_name",
    "message_id",
    "sent_at",
    "direction",
    "sender_id",
    "sender_name",
    "service",
    "kind",
    "body",
    "attachments",
    "reply_to",
    "reactions",
    "edited",
    "unsent",
];

/// Characters that start a formula in Excel, LibreOffice and Sheets.
const FORMULA_STARTS: &[char] = &['=', '+', '-', '@'];

/// Prefix a cell that a spreadsheet would evaluate.
///
/// Returns the cell and whether it was changed, so the caller can count.
pub fn neutralise_formula(cell: &str) -> (String, bool) {
    match cell.chars().next() {
        Some(first) if FORMULA_STARTS.contains(&first) => (format!("'{cell}"), true),
        _ => (cell.to_string(), false),
    }
}

/// Writes one CSV file per export.
pub struct CsvExporter;

impl Exporter for CsvExporter {
    fn export(
        &self,
        backup: &Backup,
        sel: &Selection,
        out: &Path,
        opts: &ExportOptions,
        progress: &dyn Fn(Progress),
        cancel: &CancelToken,
    ) -> Result<ExportReport> {
        std::fs::create_dir_all(out).at(out)?;
        let source = ChatSource::open(backup, &sel.source)?;
        let conversations = conversations_for(&source, sel, opts)?;
        let mut report = ExportReport::new(ExportFormat::Csv, source.id(), out);

        let path = out.join(format!("{}-messages.csv", source.id()));
        let mut writer = csv::Writer::from_path(&path).map_err(|e| crate::error::Error::Io {
            path: path.clone(),
            source: std::io::Error::other(e),
        })?;
        writer.write_record(COLUMNS).map_err(csv_io(&path))?;

        let total = conversations.len() as u64;
        let mut cancelled = false;
        let mut neutralised = 0u64;

        for (index, conversation) in conversations.iter().enumerate() {
            if cancel.is_cancelled() {
                cancelled = true;
                break;
            }
            progress(Progress {
                done: index as u64,
                total: Some(total),
                message: format!("Conversation {} of {total}", index + 1),
            });

            let mut written = 0u64;
            let mut missing = 0u64;
            for message in source.messages_chronological(&conversation.id)? {
                if cancel.is_cancelled() {
                    cancelled = true;
                    break;
                }
                if !opts.in_range(&message) {
                    continue;
                }
                missing += message
                    .attachments
                    .iter()
                    .filter(|a| !a.is_present())
                    .count() as u64;
                let row = row_for(conversation, &message, &mut neutralised);
                writer.write_record(&row).map_err(csv_io(&path))?;
                written += 1;
            }

            report.add(ConversationReport {
                id: conversation.id.clone(),
                output_name: path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                messages: written,
                // CSV copies no files; it records what is referenced.
                attachments_copied: 0,
                attachments_missing: missing,
            });
            if cancelled {
                break;
            }
        }

        writer.flush().at(&path)?;
        drop(writer);

        report.formulas_neutralised = neutralised;
        report.totals.bytes_written = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if neutralised > 0 {
            report.warnings.push(format!(
                "{neutralised} cell(s) began with =, + , - or @ and were prefixed with an \
                 apostrophe so a spreadsheet shows them as text instead of running them."
            ));
        }
        report.finish(cancelled);
        report.write(out)?;
        progress(Progress {
            done: total,
            total: Some(total),
            message: "Finished".to_string(),
        });
        Ok(report)
    }
}

/// Map a csv error onto ours, with the file it happened to.
fn csv_io(path: &Path) -> impl Fn(csv::Error) -> crate::error::Error + '_ {
    move |e| crate::error::Error::Io {
        path: path.to_path_buf(),
        source: std::io::Error::other(e),
    }
}

/// One message as a row, counting any cells that had to be neutralised.
fn row_for(
    conversation: &ConversationSummary,
    message: &Message,
    neutralised: &mut u64,
) -> Vec<String> {
    let attachments = if message.attachments.is_empty() {
        String::new()
    } else {
        message
            .attachments
            .iter()
            .map(|a| {
                if a.is_present() {
                    a.file_name.clone()
                } else {
                    format!("{} (not in backup)", a.file_name)
                }
            })
            .collect::<Vec<_>>()
            .join("; ")
    };

    let reactions = message
        .reactions
        .iter()
        .filter(|r| !r.removed)
        .map(|r| format!("{:?}", r.kind))
        .collect::<Vec<_>>()
        .join("; ");

    let cells = vec![
        conversation.id.clone(),
        conversation.label(),
        message.id.clone(),
        message.sent_at.map(|d| d.to_rfc3339()).unwrap_or_default(),
        if message.from_me { "out" } else { "in" }.to_string(),
        message
            .sender
            .as_ref()
            .map(|s| s.id.clone())
            .unwrap_or_default(),
        message
            .sender
            .as_ref()
            .and_then(|s| s.display_name.clone())
            .unwrap_or_default(),
        format!("{:?}", message.service),
        format!("{:?}", message.kind),
        message.body.clone().unwrap_or_default(),
        attachments,
        message.reply_to.clone().unwrap_or_default(),
        reactions,
        message.edited.to_string(),
        message.unsent.to_string(),
    ];

    cells
        .into_iter()
        .map(|cell| {
            let (cell, changed) = neutralise_formula(&cell);
            if changed {
                *neutralised += 1;
            }
            cell
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formula_starts_are_neutralised() {
        for hostile in [
            "=1+1",
            "+1",
            "-1",
            "@SUM(A1)",
            "=HYPERLINK(\"http://evil.invalid\")",
            "=cmd|'/c calc'!A1",
        ] {
            let (cell, changed) = neutralise_formula(hostile);
            assert!(changed, "{hostile:?} should be neutralised");
            assert!(cell.starts_with('\''), "{cell:?}");
            // The original text survives after the apostrophe, so nothing is
            // lost — only how a spreadsheet interprets it changes.
            assert_eq!(&cell[1..], hostile);
        }
    }

    #[test]
    fn ordinary_text_is_untouched() {
        for benign in [
            "hello",
            "",
            "1+1",
            "a=b",
            " =not at the start",
            "tel: +1 555",
        ] {
            let (cell, changed) = neutralise_formula(benign);
            assert!(!changed, "{benign:?} should not be changed");
            assert_eq!(cell, benign);
        }
    }

    #[test]
    fn the_column_list_is_stable() {
        // The header is part of the output contract: anything parsing an ibex
        // CSV depends on it.
        assert_eq!(COLUMNS.len(), 15);
        assert_eq!(COLUMNS[0], "conversation_id");
        assert_eq!(COLUMNS[3], "sent_at");
        assert_eq!(COLUMNS[9], "body");
        assert_eq!(COLUMNS[14], "unsent");
    }
}
