//! Exporting Voice Memos: copy each recording out, titled.
//!
//! One shape, like Photos — see `export::run`'s dispatch.

use std::path::Path;

use crate::Backup;
use crate::error::{IoResultExt, Result};
use crate::export::naming::{self, NameAllocator};
use crate::export::{
    CancelToken, ConversationReport, ExportFormat, ExportOptions, ExportReport, Exporter, Progress,
    Selection,
};
use crate::extractors::{RecordingAvailability, VoiceMemosDb, VoiceRecording};
use crate::page::Page;

/// Copies recordings out as `<YYYY-MM-DD HHmm> <title>.<ext>`.
pub struct VoiceExporter;

impl Exporter for VoiceExporter {
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
        let db = VoiceMemosDb::open(backup)?;
        let recordings = selected_recordings(&db, sel, opts)?;

        let mut report = ExportReport::new(ExportFormat::Files, "voice", out);
        let total = recordings.len() as u64;
        let mut cancelled = false;
        let mut bytes = 0u64;
        let mut copied = 0u64;
        let mut missing = 0u64;
        let mut names = NameAllocator::new();

        for (index, recording) in recordings.iter().enumerate() {
            if cancel.is_cancelled() {
                cancelled = true;
                break;
            }
            progress(Progress {
                done: index as u64,
                total: Some(total),
                message: format!("Recording {} of {total}", index + 1),
            });

            let resolved = recording
                .availability
                .eq(&RecordingAvailability::InBackup)
                .then_some(recording.file_id.as_deref())
                .flatten()
                .and_then(|id| backup.get(id).ok().flatten());
            let Some(entry) = resolved else {
                missing += 1;
                continue;
            };

            let extension = entry
                .name()
                .rsplit_once('.')
                .map(|(_, ext)| ext)
                .unwrap_or("m4a");
            let stamped = match recording.created_at {
                Some(d) => format!(
                    "{} {}",
                    d.format("%Y-%m-%d %H%M"),
                    recording.title.as_deref().unwrap_or("Recording")
                ),
                None => recording
                    .title
                    .clone()
                    .unwrap_or_else(|| "Recording".to_string()),
            };
            let base = naming::safe_component(&format!("{stamped}.{extension}"));
            let allocated = names.allocate(&base);
            let dest = naming::join_safe(out, &allocated);

            match backup.copy_file_to(&entry, &dest) {
                Ok((written, _mtime)) => {
                    bytes += written;
                    copied += 1;
                }
                Err(_) => missing += 1,
            }
        }

        report.add(ConversationReport {
            id: "voice".to_string(),
            output_name: out.display().to_string(),
            messages: copied,
            attachments_copied: copied,
            attachments_missing: missing,
        });
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

fn selected_recordings(
    db: &VoiceMemosDb<'_>,
    sel: &Selection,
    opts: &ExportOptions,
) -> Result<Vec<VoiceRecording>> {
    let mut all = Vec::new();
    let mut offset = 0u64;
    loop {
        let page = db.list(Page::new(offset, Page::MAX_LIMIT))?;
        let count = page.items.len() as u64;
        all.extend(page.items);
        match page.next_offset {
            Some(next) if count > 0 => offset = next,
            _ => break,
        }
    }

    if !sel.conversation_ids.is_empty() {
        let wanted: std::collections::HashSet<&String> = sel.conversation_ids.iter().collect();
        all.retain(|r| wanted.contains(&r.id));
    }

    if opts.from.is_some() || opts.to.is_some() {
        all.retain(|r| match r.created_at {
            None => true,
            Some(when) => {
                opts.from.is_none_or(|from| when >= from) && opts.to.is_none_or(|to| when <= to)
            }
        });
    }
    Ok(all)
}
