//! JSON export: one document per export, messages as objects.
//!
//! The normalized types already derive `Serialize`, so the shape here is the
//! same one the UI's generated TypeScript bindings describe. That is
//! deliberate: an export and the app agree about what a message is, and there
//! is no second schema to drift.

use std::path::Path;

use serde::Serialize;

use crate::Backup;
use crate::error::{IoResultExt, Result};
use crate::export::{
    CancelToken, ChatSource, ConversationReport, ExportFormat, ExportOptions, ExportReport,
    Exporter, Progress, Selection, conversations_for,
};
use crate::extractors::{ConversationSummary, Message};

/// One conversation and its messages.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConversationDocument<'a> {
    conversation: &'a ConversationSummary,
    messages: Vec<Message>,
}

/// The whole export.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportDocument<'a> {
    ibex_version: &'a str,
    source: &'a str,
    exported_at: String,
    conversations: Vec<ConversationDocument<'a>>,
}

/// Writes one JSON file per export.
pub struct JsonExporter;

impl Exporter for JsonExporter {
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
        let mut report = ExportReport::new(ExportFormat::Json, source.id(), out);

        let total = conversations.len() as u64;
        let mut cancelled = false;
        let mut documents = Vec::new();

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

            let mut messages = Vec::new();
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
                messages.push(message);
            }

            report.add(ConversationReport {
                id: conversation.id.clone(),
                output_name: format!("{}-messages.json", source.id()),
                messages: messages.len() as u64,
                attachments_copied: 0,
                attachments_missing: missing,
            });
            documents.push(ConversationDocument {
                conversation,
                messages,
            });
            if cancelled {
                break;
            }
        }

        let document = ExportDocument {
            ibex_version: env!("CARGO_PKG_VERSION"),
            source: source.id(),
            exported_at: chrono::Utc::now().to_rfc3339(),
            conversations: documents,
        };

        let path = out.join(format!("{}-messages.json", source.id()));
        let file = std::fs::File::create(&path).at(&path)?;
        let mut file = std::io::BufWriter::new(file);
        serde_json::to_writer_pretty(&mut file, &document).map_err(|e| {
            crate::error::Error::Io {
                path: path.clone(),
                source: std::io::Error::other(e),
            }
        })?;
        std::io::Write::flush(&mut file).at(&path)?;
        drop(file);

        report.totals.bytes_written = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
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
