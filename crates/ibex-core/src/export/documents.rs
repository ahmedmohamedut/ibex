//! Exporting Files sources: copy files out, preserving their folder
//! structure under the chosen root.
//!
//! One shape, like Photos and Voice Memos — see `export::run`'s dispatch.

use std::path::{Path, PathBuf};

use crate::Backup;
use crate::ManifestEntry;
use crate::error::{IoResultExt, Result};
use crate::export::naming;
use crate::export::{
    CancelToken, ConversationReport, DocumentRoot, ExportFormat, ExportOptions, ExportReport,
    Exporter, Progress, Selection,
};
use crate::page::Page;

/// Copies files out of one Files source, preserving relative structure.
pub struct DocumentsExporter;

impl Exporter for DocumentsExporter {
    fn export(
        &self,
        backup: &Backup,
        sel: &Selection,
        out: &Path,
        _opts: &ExportOptions,
        progress: &dyn Fn(Progress),
        cancel: &CancelToken,
    ) -> Result<ExportReport> {
        std::fs::create_dir_all(out).at(out)?;
        let root =
            sel.document_root
                .clone()
                .ok_or_else(|| crate::error::ExtractError::Unsupported {
                    source_id: "documents",
                    detail: "no document_root given".to_string(),
                })?;

        let files = selected_files(backup, &root, sel)?;
        let mut report = ExportReport::new(ExportFormat::Files, "documents", out);
        let total = files.len() as u64;
        let mut cancelled = false;
        let mut bytes = 0u64;
        let mut copied = 0u64;
        let mut missing = 0u64;

        for (index, (relative, entry)) in files.iter().enumerate() {
            if cancel.is_cancelled() {
                cancelled = true;
                break;
            }
            progress(Progress {
                done: index as u64,
                total: Some(total),
                message: format!("File {} of {total}", index + 1),
            });

            let dest = sanitized_dest(out, relative);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).at(parent)?;
            }
            match backup.copy_file_to(entry, &dest) {
                Ok((written, _mtime)) => {
                    bytes += written;
                    copied += 1;
                }
                Err(_) => missing += 1,
            }
        }

        report.add(ConversationReport {
            id: "documents".to_string(),
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

/// Every file to export, paired with its path relative to the source root —
/// the relative path is what gets sanitized and reproduced in the output.
fn selected_files(
    backup: &Backup,
    root: &DocumentRoot,
    sel: &Selection,
) -> Result<Vec<(String, ManifestEntry)>> {
    let mut out = Vec::new();
    if sel.conversation_ids.is_empty() {
        walk(backup, &root.domain, &root.root, "", &mut out)?;
    } else {
        for relative in &sel.conversation_ids {
            let full = join_backup_path(&root.root, relative);
            let Some(entry) = backup.find(&root.domain, &full).ok().flatten() else {
                continue;
            };
            if entry.kind.is_dir() {
                walk(backup, &root.domain, &full, relative, &mut out)?;
            } else if entry.kind.is_file() {
                out.push((relative.clone(), entry));
            }
        }
    }
    Ok(out)
}

/// Recursively collect every file under `dir`, pairing each with its path
/// relative to the original root (`prefix`) rather than to the backup.
fn walk(
    backup: &Backup,
    domain: &str,
    dir: &str,
    prefix: &str,
    out: &mut Vec<(String, ManifestEntry)>,
) -> Result<()> {
    let mut offset = 0u64;
    loop {
        let page = backup.list_dir(domain, dir, Page::new(offset, Page::MAX_LIMIT))?;
        let count = page.items.len() as u64;
        for entry in page.items {
            let relative = if prefix.is_empty() {
                entry.name().to_string()
            } else {
                format!("{prefix}/{}", entry.name())
            };
            if entry.kind.is_dir() {
                walk(backup, domain, &entry.relative_path, &relative, out)?;
            } else if entry.kind.is_file() {
                out.push((relative, entry));
            }
        }
        match page.next_offset {
            Some(next) if count > 0 => offset = next,
            _ => break,
        }
    }
    Ok(())
}

fn join_backup_path(root: &str, relative: &str) -> String {
    if root.is_empty() {
        relative.to_string()
    } else {
        format!("{root}/{relative}")
    }
}

/// Reproduce a relative path under `out`, sanitizing every segment
/// individually. Manifest paths are real historical device paths, not
/// guaranteed filesystem-safe on this OS, so no segment is ever trusted as
/// pre-sanitized — the same rule `naming::join_safe` enforces for one
/// component at a time.
fn sanitized_dest(out: &Path, relative: &str) -> PathBuf {
    let mut dest = out.to_path_buf();
    for segment in relative.split('/').filter(|s| !s.is_empty()) {
        dest = naming::join_safe(&dest, segment);
    }
    dest
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::Selection;
    use crate::testutil::FixtureBackup;
    use crate::{Backup, CacheDir};

    fn fixture_backup(tmp: &std::path::Path) -> Backup {
        let root = FixtureBackup::new()
            .with_file(
                "AppDomain-com.example.good",
                "Documents/report.pdf",
                b"%PDF-1.4 a",
            )
            .with_file(
                "AppDomain-com.example.good",
                "Documents/Sub/notes.txt",
                b"hello",
            )
            .with_file(
                "AppDomainGroup-group.com.apple.FileProvider.LocalStorage",
                "File Provider Storage/Downloads/a.pdf",
                b"%PDF-1.4 b",
            )
            .build(tmp)
            .unwrap();
        Backup::open_with_cache(&root, None, CacheDir::at(tmp.join("cache"))).unwrap()
    }

    fn doc_root() -> DocumentRoot {
        DocumentRoot {
            domain: "AppDomain-com.example.good".to_string(),
            root: "Documents".to_string(),
        }
    }

    fn noop_progress(_: Progress) {}

    #[test]
    fn exporting_everything_preserves_the_folder_structure() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = fixture_backup(tmp.path());
        let out = tmp.path().join("out");

        let mut sel = Selection::all("documents");
        sel.document_root = Some(doc_root());
        let report = DocumentsExporter
            .export(
                &backup,
                &sel,
                &out,
                &ExportOptions::default(),
                &noop_progress,
                &CancelToken::new(),
            )
            .unwrap();

        assert_eq!(report.totals.messages, 2);
        assert!(out.join("report.pdf").is_file());
        assert!(out.join("Sub").join("notes.txt").is_file());
        assert_eq!(
            std::fs::read(out.join("Sub").join("notes.txt")).unwrap(),
            b"hello"
        );
    }

    #[test]
    fn an_explicit_selection_can_be_a_single_file_or_a_whole_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = fixture_backup(tmp.path());
        let out = tmp.path().join("out");

        let mut sel = Selection::one("documents", "Sub");
        sel.document_root = Some(doc_root());
        let report = DocumentsExporter
            .export(
                &backup,
                &sel,
                &out,
                &ExportOptions::default(),
                &noop_progress,
                &CancelToken::new(),
            )
            .unwrap();

        assert_eq!(report.totals.messages, 1, "only the folder's one file");
        assert!(out.join("Sub").join("notes.txt").is_file());
        assert!(!out.join("report.pdf").exists());
    }

    #[test]
    fn a_path_traversal_attempt_in_a_relative_path_cannot_escape_the_output() {
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("out");

        // Proves the *naming* layer independently of any particular
        // selection: every segment is sanitized, so this can never resolve
        // outside `out` regardless of what a manifest path contained.
        let dest = sanitized_dest(&out, "../../etc/passwd");
        assert!(
            dest.starts_with(&out),
            "escaped the output folder: {dest:?}"
        );
    }

    #[test]
    fn a_different_source_root_exports_a_different_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = fixture_backup(tmp.path());
        let out = tmp.path().join("out");

        let mut sel = Selection::all("documents");
        sel.document_root = Some(DocumentRoot {
            domain: "AppDomainGroup-group.com.apple.FileProvider.LocalStorage".to_string(),
            root: "File Provider Storage".to_string(),
        });
        let report = DocumentsExporter
            .export(
                &backup,
                &sel,
                &out,
                &ExportOptions::default(),
                &noop_progress,
                &CancelToken::new(),
            )
            .unwrap();

        assert_eq!(report.totals.messages, 1);
        assert!(out.join("Downloads").join("a.pdf").is_file());
    }
}
