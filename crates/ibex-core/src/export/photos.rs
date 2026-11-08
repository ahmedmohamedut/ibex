//! Exporting the camera roll: copy the files out with sensible names.
//!
//! Unlike the chat sources, there is no HTML/CSV/JSON shape for a photo — the
//! point of exporting one is the file itself. So this is the one exporter
//! selected by [`Selection::source`] rather than by [`ExportFormat`]; see
//! [`crate::export::run`].
//!
//! Two decisions, both made with the project's owner before this was written:
//!
//! - **The edited version is the photo**, exactly as Photos itself shows it.
//!   [`crate::extractors::PhotoAsset::display_file_id`] already encodes this,
//!   so exporting is "copy whatever the lightbox would show".
//! - **A Live Photo's companion video travels with its still**, under the
//!   same base name, the way Photos keeps them paired.
//!
//! Dates and names come from the library, so both go through [`naming`] —
//! nothing here assumes a file name is safe to use as-is.

use std::path::Path;

use chrono::Datelike;

use crate::Backup;
use crate::error::{IoResultExt, Result};
use crate::export::naming::{self, NameAllocator};
use crate::export::{
    CancelToken, ConversationReport, ExportFormat, ExportOptions, ExportReport, Exporter, Progress,
    Selection,
};
use crate::extractors::{PhotoAsset, PhotoCursor, PhotosDb};

/// Copies camera-roll assets out as `Photos/<YYYY>/<YYYY-MM-DD> <name>.<ext>`.
pub struct PhotosExporter;

impl Exporter for PhotosExporter {
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
        let db = PhotosDb::open(backup)?;
        let assets = selected_assets(&db, sel, opts)?;

        let mut report = ExportReport::new(ExportFormat::Files, "photos", out);
        let total = assets.len() as u64;
        let mut cancelled = false;
        let mut bytes = 0u64;

        // One folder per year, allocated once and reused by every asset in
        // it — `NameAllocator::allocate` hands out a *new* name on every
        // call, so the year label itself is only ever passed to it once. Each
        // year also tracks its own copied/missing counts, so every asset
        // (resolved or not) is attributed to exactly one folder and, through
        // `ExportReport::add`, to the totals exactly once — never both
        // directly and through `add`, which is what double-counted a
        // selection that resolved to nothing.
        struct YearBucket {
            folder_name: String,
            names: NameAllocator,
            copied: u64,
            missing: u64,
        }
        let mut year_dirs: std::collections::HashMap<i32, YearBucket> =
            std::collections::HashMap::new();
        let mut year_folder_names = NameAllocator::new();

        for (index, asset) in assets.iter().enumerate() {
            if cancel.is_cancelled() {
                cancelled = true;
                break;
            }
            progress(Progress {
                done: index as u64,
                total: Some(total),
                message: format!("Asset {} of {total}", index + 1),
            });

            let year = asset.created_at.map(|d| d.year()).unwrap_or(0);
            let bucket = year_dirs.entry(year).or_insert_with(|| {
                let label = if year == 0 {
                    "Unknown date".to_string()
                } else {
                    year.to_string()
                };
                YearBucket {
                    folder_name: year_folder_names.allocate(&label),
                    names: NameAllocator::new(),
                    copied: 0,
                    missing: 0,
                }
            });

            let resolved = asset
                .display_file_id()
                .and_then(|id| backup.get(id).ok().flatten());
            let Some(entry) = resolved else {
                bucket.missing += 1;
                continue;
            };

            let year_dir = out.join(&bucket.folder_name);
            std::fs::create_dir_all(&year_dir).at(&year_dir)?;

            let base = naming::safe_component(&display_name(asset));
            let stamped = match asset.created_at {
                Some(d) => format!("{} {base}", d.format("%Y-%m-%d")),
                None => base,
            };
            let allocated = bucket.names.allocate(&stamped);
            let dest = naming::join_safe(&year_dir, &allocated);

            match backup.copy_file_to(&entry, &dest) {
                Ok((written, _mtime)) => {
                    bytes += written;
                    bucket.copied += 1;
                }
                Err(_) => {
                    bucket.missing += 1;
                    continue;
                }
            }

            // The Live Photo companion travels under the same base name, so
            // the pair survives being moved around after export.
            if let Some(live_id) = asset.display_live_file_id()
                && let Ok(Some(live_entry)) = backup.get(live_id)
            {
                let live_name = with_extension(&allocated, "mov");
                let live_dest = naming::join_safe(&year_dir, &live_name);
                if let Ok((written, _)) = backup.copy_file_to(&live_entry, &live_dest) {
                    bytes += written;
                }
            }
        }

        // One report entry per year, in order, each with its own real counts.
        let mut years: Vec<_> = year_dirs.into_iter().collect();
        years.sort_by_key(|(year, _)| *year);
        for (year, bucket) in years {
            report.add(ConversationReport {
                id: year.to_string(),
                output_name: bucket.folder_name,
                // Reusing the chat-shaped field for a count of exported
                // assets: the IPC contract is shared with the chat exporters,
                // and the UI already renders this as a plain count — see
                // JobToasts.tsx and Exports.tsx, which switch to "item(s)" /
                // "year(s)" for a photos report rather than "message(s)" /
                // "conversation(s)".
                messages: bucket.copied,
                attachments_copied: bucket.copied,
                attachments_missing: bucket.missing,
            });
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

/// Resolve a selection to the actual assets to export, honouring the date
/// range in `opts` the same way a chat export does.
fn selected_assets(
    db: &PhotosDb<'_>,
    sel: &Selection,
    opts: &ExportOptions,
) -> Result<Vec<PhotoAsset>> {
    let mut assets = if sel.conversation_ids.is_empty() {
        let filter = sel.photo_filter.clone().unwrap_or_default();
        let mut out = Vec::new();
        let mut cursor = Some(PhotoCursor::first(PhotoCursor::MAX_LIMIT));
        while let Some(c) = cursor {
            let page = db.list(&filter, c)?;
            out.extend(page.assets);
            cursor = page.older;
        }
        out
    } else {
        sel.conversation_ids
            .iter()
            .filter_map(|id| db.asset(id).ok().flatten())
            .collect()
    };

    if opts.from.is_some() || opts.to.is_some() {
        assets.retain(|a| match a.created_at {
            None => true,
            Some(when) => {
                opts.from.is_none_or(|from| when >= from) && opts.to.is_none_or(|to| when <= to)
            }
        });
    }
    Ok(assets)
}

/// What `safe_component` sees before sanitizing: the original import name
/// when it differs from the on-disk name (which is sometimes a bare UUID),
/// else the on-disk name.
fn display_name(asset: &PhotoAsset) -> String {
    asset
        .original_file_name
        .clone()
        .unwrap_or_else(|| asset.file_name.clone())
}

/// Swap a (de-duplicated, already-sanitized) file name's extension.
fn with_extension(name: &str, extension: &str) -> String {
    match name.rfind('.') {
        Some(at) if at > 0 => format!("{}.{extension}", &name[..at]),
        _ => format!("{name}.{extension}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::{ExportOptions, Selection};
    use crate::testutil::{FixtureBackup, photos as fixture};
    use crate::{Backup, CacheDir};

    fn fixture_backup(tmp: &std::path::Path) -> Backup {
        let root = FixtureBackup::new().with_photos().build(tmp).unwrap();
        Backup::open_with_cache(&root, None, CacheDir::at(tmp.join("cache"))).unwrap()
    }

    fn noop_progress(_: Progress) {}

    #[test]
    fn exporting_everything_copies_every_resolvable_asset() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = fixture_backup(tmp.path());
        let out = tmp.path().join("out");

        let report = PhotosExporter
            .export(
                &backup,
                &Selection::all("photos"),
                &out,
                &ExportOptions::default(),
                &noop_progress,
                &CancelToken::new(),
            )
            .unwrap();

        // Default view excludes trashed and hidden only: 8 of the fixture's
        // 10 assets. The cloud-only asset is still one of them — it is shown
        // as a placeholder, not filtered out — so it resolves to 7 copied and
        // 1 missing rather than 8 and 0.
        assert_eq!(report.totals.messages, 7);
        assert_eq!(report.totals.attachments_missing, 1);
        assert!(!report.cancelled);

        let written: Vec<_> = walk(&out);
        // The edited asset's render, plus the Live Photo's companion, both
        // travel alongside the originals.
        assert!(
            written
                .iter()
                .any(|p| p.extension().is_some_and(|e| e == "mov")),
            "the Live Photo's companion should have been copied: {written:?}"
        );
    }

    #[test]
    fn exporting_an_explicit_selection_ignores_the_default_filter() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = fixture_backup(tmp.path());
        let out = tmp.path().join("out");

        // The trashed asset would never appear in the default view, but an
        // explicit id picks it anyway — the same rule a chat export follows.
        let report = PhotosExporter
            .export(
                &backup,
                &Selection::one("photos", &fixture::TRASHED_PK.to_string()),
                &out,
                &ExportOptions::default(),
                &noop_progress,
                &CancelToken::new(),
            )
            .unwrap();

        assert_eq!(report.totals.messages, 1);
    }

    #[test]
    fn the_edited_render_is_exported_not_the_original() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = fixture_backup(tmp.path());
        let out = tmp.path().join("out");

        PhotosExporter
            .export(
                &backup,
                &Selection::one("photos", &fixture::EDITED_PK.to_string()),
                &out,
                &ExportOptions::default(),
                &noop_progress,
                &CancelToken::new(),
            )
            .unwrap();

        let db = PhotosDb::open(&backup).unwrap();
        let asset = db.asset(&fixture::EDITED_PK.to_string()).unwrap().unwrap();
        let render_entry = backup
            .get(asset.edited_file_id.as_deref().unwrap())
            .unwrap()
            .unwrap();
        let expected = std::fs::read(backup.materialize(&render_entry).unwrap()).unwrap();

        let written = walk(&out);
        let copied = written
            .iter()
            .find(|p| p.is_file())
            .expect("exactly one file should have been written");
        assert_eq!(std::fs::read(copied).unwrap(), expected);
    }

    #[test]
    fn a_missing_asset_is_counted_not_silently_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = fixture_backup(tmp.path());
        let out = tmp.path().join("out");

        let report = PhotosExporter
            .export(
                &backup,
                &Selection::one("photos", &fixture::CLOUD_ONLY_PK.to_string()),
                &out,
                &ExportOptions::default(),
                &noop_progress,
                &CancelToken::new(),
            )
            .unwrap();

        assert_eq!(report.totals.messages, 0);
        assert_eq!(report.totals.attachments_missing, 1);
        assert!(
            report.warnings.iter().any(|w| w.contains('1')),
            "the report should say something was missing: {:?}",
            report.warnings
        );
    }

    #[test]
    fn a_date_range_narrows_the_export() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = fixture_backup(tmp.path());
        let out = tmp.path().join("out");

        // The fixture's FAVORITE_PK asset is the one in a different month.
        let db = PhotosDb::open(&backup).unwrap();
        let favorite = db
            .asset(&fixture::FAVORITE_PK.to_string())
            .unwrap()
            .unwrap();
        let when = favorite.created_at.unwrap();

        let report = PhotosExporter
            .export(
                &backup,
                &Selection::all("photos"),
                &out,
                &ExportOptions {
                    from: Some(when - chrono::Duration::days(1)),
                    to: Some(when + chrono::Duration::days(1)),
                    ..Default::default()
                },
                &noop_progress,
                &CancelToken::new(),
            )
            .unwrap();

        assert_eq!(report.totals.messages, 1);
    }

    #[test]
    fn cancelling_leaves_a_report_marked_cancelled() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = fixture_backup(tmp.path());
        let out = tmp.path().join("out");
        let cancel = CancelToken::new();
        cancel.cancel();

        let report = PhotosExporter
            .export(
                &backup,
                &Selection::all("photos"),
                &out,
                &ExportOptions::default(),
                &noop_progress,
                &cancel,
            )
            .unwrap();
        assert!(report.cancelled);
        assert_eq!(report.totals.messages, 0);
    }

    fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        fn go(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    go(&path, out);
                } else if path.file_name().and_then(|n| n.to_str()) != Some("export-report.json") {
                    out.push(path);
                }
            }
        }
        go(dir, &mut out);
        out
    }
}
