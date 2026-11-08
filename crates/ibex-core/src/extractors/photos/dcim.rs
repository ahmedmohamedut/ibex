//! Listing the camera roll straight from `Media/DCIM`, when the library is not
//! readable.
//!
//! `Photos.sqlite` is a 440 MiB Core Data store; if it is missing from a
//! backup, truncated, or in a schema shape we cannot read, the pictures are
//! still all there on disk. This module walks the manifest instead and reads
//! capture dates out of each file's EXIF header, so a degraded backup still
//! shows a browsable camera roll rather than an error.
//!
//! It is **degraded on purpose** and says so: no albums, no favourites, no
//! Live Photo pairing beyond matching names, and no hidden or trashed state,
//! because all of that lives only in the database.
//!
//! The real backup's library parses, so this path is exercised by fixtures
//! rather than by real data. That is recorded in ROADMAP.md rather than
//! implied away.

use std::io::Cursor;

use crate::error::Result;
use crate::extractors::photos::{
    DOMAIN, PhotoAsset, PhotoAvailability, PhotoCursor, PhotoKind, PhotoPage,
};
use crate::page::Page;
use crate::util::mime::{self, PreviewKind};
use crate::util::time::from_apple;
use crate::{Backup, ManifestEntry};

/// Where the camera writes.
const DCIM: &str = "Media/DCIM";

/// How much of a file to read when looking for an EXIF date.
///
/// EXIF sits in the first APP1 segment of a JPEG and in the `meta` box of a
/// HEIC, both near the start. 128 KiB covers every file measured in the real
/// backup, and on an encrypted backup only those blocks get decrypted.
const EXIF_PREFIX: usize = 128 * 1024;

/// File extensions the camera roll holds, lowercase.
const MEDIA_EXTENSIONS: &[&str] = &[
    "heic", "heif", "jpg", "jpeg", "png", "webp", "mov", "mp4", "m4v", "gif", "dng", "tif", "tiff",
];

/// Whether this backup has a camera roll on disk at all.
pub fn has_any(backup: &Backup) -> bool {
    backup
        .list_dir(DOMAIN, DCIM, Page::first(4))
        .map(|page| !page.items.is_empty())
        .unwrap_or(false)
}

/// Every `1xxAPPLE` directory the camera has created, in name order.
fn roll_directories(backup: &Backup) -> Result<Vec<String>> {
    let mut dirs = Vec::new();
    let mut offset = 0u64;
    loop {
        let page = backup.list_dir(DOMAIN, DCIM, Page::new(offset, Page::MAX_LIMIT))?;
        let count = page.items.len() as u64;
        for entry in &page.items {
            if entry.kind.is_dir() {
                dirs.push(entry.relative_path.clone());
            }
        }
        match page.next_offset {
            Some(next) if count > 0 => offset = next,
            _ => break,
        }
    }
    dirs.sort();
    Ok(dirs)
}

/// List the camera roll without the library.
///
/// Ordered newest first by capture date, like the database-backed grid. The
/// whole roll has to be read to sort it, so the cursor pages a sorted vector
/// rather than a SQL keyset; at ~10,000 entries that is one manifest scan.
pub fn list(backup: &Backup, cursor: PhotoCursor) -> Result<PhotoPage> {
    let mut assets = collect(backup)?;
    // Newest first, undated last, Z_PK-less ties broken by name so paging is
    // stable across calls.
    assets.sort_by(|a, b| {
        b.created_at
            .cmp(&a.created_at)
            .then_with(|| b.file_name.cmp(&a.file_name))
    });

    let limit = cursor.clamped_limit() as usize;
    let start = cursor
        .before_pk
        .and_then(|pk| usize::try_from(pk).ok())
        .unwrap_or(0)
        .min(assets.len());
    let end = (start + limit).min(assets.len());
    let page: Vec<PhotoAsset> = assets[start..end].to_vec();
    let older = (end < assets.len()).then(|| PhotoCursor {
        before_date: None,
        before_pk: Some(end as i64),
        limit: cursor.clamped_limit(),
    });
    Ok(PhotoPage {
        assets: page,
        older,
    })
}

/// How many media files the roll holds.
pub fn count(backup: &Backup) -> Result<u64> {
    Ok(collect(backup)?.len() as u64)
}

/// Walk every roll directory and build an asset per media file.
fn collect(backup: &Backup) -> Result<Vec<PhotoAsset>> {
    let mut assets = Vec::new();
    for dir in roll_directories(backup)? {
        let mut offset = 0u64;
        loop {
            let page = backup.list_dir(DOMAIN, &dir, Page::new(offset, Page::MAX_LIMIT))?;
            let count = page.items.len() as u64;
            // Live Photo companions are not assets of their own, exactly as in
            // the library: a .MOV beside a still of the same name is its video.
            let names: std::collections::HashSet<&str> = page
                .items
                .iter()
                .map(|e| e.relative_path.as_str())
                .collect();
            for entry in &page.items {
                if entry.kind.is_dir() {
                    continue;
                }
                let name = file_name(&entry.relative_path);
                let Some(extension) = extension(name) else {
                    continue;
                };
                if !MEDIA_EXTENSIONS.contains(&extension.as_str()) {
                    continue;
                }
                let is_video = matches!(extension.as_str(), "mov" | "mp4" | "m4v");
                // A .MOV whose still exists is a companion, not an asset.
                if is_video && has_still_companion(&entry.relative_path, &names) {
                    continue;
                }
                assets.push(asset_from_entry(backup, entry, is_video, &names));
            }
            match page.next_offset {
                Some(next) if count > 0 => offset = next,
                _ => break,
            }
        }
    }
    Ok(assets)
}

/// Whether a video sits beside a still of the same base name.
fn has_still_companion(path: &str, siblings: &std::collections::HashSet<&str>) -> bool {
    const STILLS: &[&str] = &["HEIC", "heic", "JPG", "jpg", "JPEG", "jpeg", "PNG", "png"];
    STILLS
        .iter()
        .any(|ext| siblings.contains(swap_extension(path, ext).as_str()))
}

fn asset_from_entry(
    backup: &Backup,
    entry: &ManifestEntry,
    is_video: bool,
    siblings: &std::collections::HashSet<&str>,
) -> PhotoAsset {
    let name = file_name(&entry.relative_path).to_string();

    // A still with a .MOV beside it is a Live Photo.
    let live = ["MOV", "mov"]
        .iter()
        .map(|ext| swap_extension(&entry.relative_path, ext))
        .find(|candidate| siblings.contains(candidate.as_str()))
        .and_then(|candidate| {
            backup
                .find(DOMAIN, &candidate)
                .ok()
                .flatten()
                .filter(|e| backup.has_data(e))
        });

    let by_extension = mime_guess::from_path(&name).first_raw().map(str::to_string);
    let sniffed = backup.sniff(entry).ok();
    let mime = sniffed
        .as_ref()
        .map(|s| s.mime.clone())
        .or_else(|| by_extension.clone());
    let preview: Option<PreviewKind> = sniffed
        .as_ref()
        .map(|s| s.preview)
        .or_else(|| by_extension.as_deref().map(mime::preview_for));

    // EXIF first, because the backup's mtime is when the file was copied, not
    // when the picture was taken. Fall back to the filesystem time.
    let created = exif_date(backup, entry).or_else(|| entry.meta.as_ref().and_then(|m| m.mtime));
    let (width, height) = (0, 0);

    PhotoAsset {
        // Without the library there is no Z_PK; the file ID is the stable id.
        id: entry.file_id.clone(),
        uuid: None,
        kind: if is_video {
            PhotoKind::Video
        } else {
            PhotoKind::Photo
        },
        file_id: Some(entry.file_id.clone()),
        // No derivative without the library's directory naming, so the grid
        // falls back to the original for web-decodable types.
        thumb_file_id: None,
        live_file_id: live.as_ref().map(|e| e.file_id.clone()),
        edited_file_id: None,
        edited_live_file_id: None,
        file_name: name,
        original_file_name: None,
        mime,
        preview,
        created_at: created,
        added_at: None,
        modified_at: entry.meta.as_ref().and_then(|m| m.mtime),
        width,
        height,
        duration: None,
        size: entry.meta.as_ref().map(|m| m.size),
        favorite: false,
        screenshot: false,
        edited: false,
        live: live.is_some(),
        trashed: false,
        hidden: false,
        location: None,
        availability: PhotoAvailability::InBackup,
    }
}

/// `DateTimeOriginal`, then `DateTime`, from the file's EXIF header.
fn exif_date(backup: &Backup, entry: &ManifestEntry) -> Option<chrono::DateTime<chrono::Utc>> {
    let bytes = backup.read_prefix(entry, EXIF_PREFIX).ok()?;
    let mut cursor = Cursor::new(&bytes);
    let reader = exif::Reader::new().read_from_container(&mut cursor).ok()?;
    for tag in [exif::Tag::DateTimeOriginal, exif::Tag::DateTime] {
        let Some(field) = reader.get_field(tag, exif::In::PRIMARY) else {
            continue;
        };
        let text = field.display_value().to_string();
        // EXIF writes "YYYY-MM-DD HH:MM:SS" through this formatter.
        if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(&text, "%Y-%m-%d %H:%M:%S") {
            return Some(naive.and_utc());
        }
    }
    None
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn extension(name: &str) -> Option<String> {
    let at = name.rfind('.')?;
    if at + 1 >= name.len() {
        return None;
    }
    Some(name[at + 1..].to_ascii_lowercase())
}

fn swap_extension(path: &str, extension: &str) -> String {
    match path.rfind('.') {
        Some(at) if at > 0 => format!("{}.{extension}", &path[..at]),
        _ => format!("{path}.{extension}"),
    }
}

/// Unused today, kept so `from_apple` stays the only date path if a future
/// schema puts an Apple timestamp in a sidecar.
#[allow(dead_code)]
fn apple(value: f64) -> Option<chrono::DateTime<chrono::Utc>> {
    from_apple(value)
}
