//! Original-files export: the attachments themselves, with their own names.
//!
//! For when the point is the photos rather than the conversation. Names come
//! from message data, so every one goes through [`naming`], and mtimes are
//! restored through [`Backup::copy_file_to`], the same primitive "Save as…"
//! uses.

use std::path::Path;

use crate::Backup;
use crate::error::{IoResultExt, Result};
use crate::export::naming::{self, NameAllocator};
use crate::export::{
    CancelToken, ChatSource, ConversationReport, ExportFormat, ExportOptions, ExportReport,
    Exporter, Progress, Selection, conversations_for,
};

/// Copies attachments out with their original names.
pub struct FilesExporter;

impl Exporter for FilesExporter {
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
        let mut report = ExportReport::new(ExportFormat::Files, source.id(), out);

        let total = conversations.len() as u64;
        let mut cancelled = false;
        let mut bytes = 0u64;
        // One allocator per conversation folder, since names only have to be
        // unique within a directory.
        let mut folders = NameAllocator::new();

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

            let folder_name = folders.allocate(&conversation.label());
            let folder = out.join(&folder_name);
            let mut names = NameAllocator::new();
            let mut copied = 0u64;
            let mut missing = 0u64;
            let mut created = false;

            for message in source.messages_chronological(&conversation.id)? {
                if cancel.is_cancelled() {
                    cancelled = true;
                    break;
                }
                if !opts.in_range(&message) {
                    continue;
                }
                for attachment in &message.attachments {
                    if cancel.is_cancelled() {
                        cancelled = true;
                        break;
                    }
                    let Some(file_id) = attachment.file_id.as_deref() else {
                        missing += 1;
                        continue;
                    };
                    let Ok(Some(entry)) = backup.get(file_id) else {
                        missing += 1;
                        continue;
                    };
                    // Only create the folder once there is something to put in
                    // it, so a chat with no media leaves no empty directory.
                    if !created {
                        std::fs::create_dir_all(&folder).at(&folder)?;
                        created = true;
                    }
                    let name = names.allocate(&attachment.file_name);
                    let dest = naming::join_safe(&folder, &name);
                    match backup.copy_file_to(&entry, &dest) {
                        Ok((written, _mtime)) => {
                            bytes += written;
                            copied += 1;
                        }
                        Err(_) => missing += 1,
                    }
                }
                if cancelled {
                    break;
                }
            }

            report.add(ConversationReport {
                id: conversation.id.clone(),
                output_name: folder_name,
                messages: 0,
                attachments_copied: copied,
                attachments_missing: missing,
            });
            if cancelled {
                break;
            }
        }

        report.totals.bytes_written = bytes;
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
