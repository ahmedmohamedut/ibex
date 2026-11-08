//! Turning a file in the backup into something the webview can display.
//!
//! The camera roll's grid does **not** come through here. Apple leaves a
//! 360 px JPEG derivative in the backup for every asset, so the grid loads
//! those directly and decodes nothing — see [`crate::extractors::photos`].
//! This module exists for the one case that derivative cannot serve: the
//! full-screen lightbox, where 53% of a real library is HEIC and WebKitGTK
//! cannot decode a single one of them.
//!
//! Three properties matter:
//!
//! - **Pass-through when possible.** A JPEG, PNG, GIF or WebP is handed over
//!   untouched, at full quality. Only HEIC/HEIF is re-encoded, so nothing is
//!   needlessly recompressed.
//! - **Cached.** A render lands in the existing cache layout at
//!   `thumbs/<max_edge>/<file_id>.jpg`, written to a `.partial` file and
//!   renamed, so a half-written JPEG is never served.
//! - **Bounded.** Decoding a 48 MP HEIC costs real CPU and memory. Holding
//!   arrow-down in the lightbox would otherwise start a decode per keypress,
//!   so concurrent renders are limited to the machine's parallelism.

use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, OnceLock};

use image::codecs::jpeg::JpegEncoder;
use image::{ExtendedColorType, ImageEncoder, RgbImage, imageops};
use libheif_rs::{ColorSpace, HeifContext, LibHeif, RgbChroma};

use crate::error::{Error, ExtractError, IoResultExt, Result};
use crate::util::mime::PreviewKind;
use crate::{Backup, ManifestEntry};

/// Quality for re-encoded JPEGs. 85 is visually lossless at display sizes and
/// roughly a third the bytes of 95.
const JPEG_QUALITY: u8 = 85;

/// The largest edge a render is allowed to ask for, so a hostile or buggy
/// caller cannot ask for a 60,000 px image.
pub const MAX_EDGE_LIMIT: u32 = 8_192;

/// What [`render`] produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Render {
    /// The file to serve.
    pub path: PathBuf,
    /// Its content type.
    pub mime: String,
    /// Whether this is the original file rather than a re-encode.
    pub original: bool,
}

/// Prepare a displayable version of one file.
///
/// `max_edge` caps the long edge of a re-encoded image; `0` means full size.
/// Files the webview can already display are returned as-is and `max_edge` is
/// ignored for them, because re-encoding a JPEG to shrink it would lose
/// quality for no benefit the lightbox can see.
pub fn render(backup: &Backup, entry: &ManifestEntry, max_edge: u32) -> Result<Render> {
    let path = backup.materialize(entry)?;
    let sniffed = backup.sniff(entry)?;

    if !is_heif(&sniffed.mime, &entry.relative_path) {
        // Everything the webview handles itself.
        return Ok(Render {
            path,
            mime: sniffed.mime,
            original: true,
        });
    }

    let max_edge = if max_edge == 0 {
        0
    } else {
        max_edge.min(MAX_EDGE_LIMIT)
    };
    let cached = backup.cache().thumb_path(max_edge, &entry.file_id);
    if cached.is_file() {
        return Ok(Render {
            path: cached,
            mime: "image/jpeg".to_string(),
            original: false,
        });
    }

    let _permit = limiter().acquire();
    // Another thread may have finished the same render while we waited.
    if cached.is_file() {
        return Ok(Render {
            path: cached,
            mime: "image/jpeg".to_string(),
            original: false,
        });
    }

    heif_to_jpeg(&path, &cached, max_edge)?;
    Ok(Render {
        path: cached,
        mime: "image/jpeg".to_string(),
        original: false,
    })
}

/// Whether a file is HEIF/HEIC, and so needs converting.
///
/// Sniffed type first; the extension only decides when the bytes gave nothing,
/// because a real library had 578 assets whose recorded type contradicted
/// their contents.
fn is_heif(mime: &str, relative_path: &str) -> bool {
    if mime.starts_with("image/hei") || mime == "image/avif" {
        return true;
    }
    if mime != "application/octet-stream" && !mime.is_empty() {
        return false;
    }
    let lower = relative_path.to_ascii_lowercase();
    lower.ends_with(".heic") || lower.ends_with(".heif")
}

/// Decode a HEIF file and write it out as a JPEG.
fn heif_to_jpeg(source: &Path, destination: &Path, max_edge: u32) -> Result<()> {
    let name = source.to_str().ok_or_else(|| {
        Error::from(ExtractError::Unsupported {
            source_id: "thumbs",
            detail: "the cache path is not valid UTF-8".to_string(),
        })
    })?;

    let lib = LibHeif::new();
    let ctx = HeifContext::read_from_file(name).map_err(heif_error)?;
    let handle = ctx.primary_image_handle().map_err(heif_error)?;
    // `decode` applies the file's rotation, crop and mirror boxes, so the
    // result is already the right way up.
    let decoded = lib
        .decode(&handle, ColorSpace::Rgb(RgbChroma::Rgb), None)
        .map_err(heif_error)?;

    let planes = decoded.planes();
    let plane = planes.interleaved.ok_or_else(|| {
        Error::from(ExtractError::Unsupported {
            source_id: "thumbs",
            detail: "libheif returned no interleaved RGB plane".to_string(),
        })
    })?;
    let (width, height) = (plane.width, plane.height);
    if width == 0 || height == 0 {
        return Err(ExtractError::Unsupported {
            source_id: "thumbs",
            detail: "the image decoded to zero pixels".to_string(),
        }
        .into());
    }

    // `stride` is the bytes per row, which is >= width * 3 and usually padded,
    // so the rows have to be copied out one at a time rather than wholesale.
    let mut rgb = Vec::with_capacity(width as usize * height as usize * 3);
    let row_bytes = width as usize * 3;
    for y in 0..height as usize {
        let start = y * plane.stride;
        let end = start + row_bytes;
        let row = plane.data.get(start..end).ok_or_else(|| {
            Error::from(ExtractError::Unsupported {
                source_id: "thumbs",
                detail: "the decoded image is shorter than its own dimensions".to_string(),
            })
        })?;
        rgb.extend_from_slice(row);
    }

    let mut image = RgbImage::from_raw(width, height, rgb).ok_or_else(|| {
        Error::from(ExtractError::Unsupported {
            source_id: "thumbs",
            detail: "the decoded pixels do not match the image dimensions".to_string(),
        })
    })?;

    if max_edge > 0 && width.max(height) > max_edge {
        let (target_width, target_height) = fit(width, height, max_edge);
        // Triangle, not libheif's `scale`, which is nearest-neighbour and
        // leaves visible stair-stepping on a downscale.
        image = imageops::resize(&image, target_width, target_height, imageops::Triangle);
    }

    if let Some(parent) = destination.parent() {
        crate::CacheDir::ensure(parent)?;
    }
    // Write to a uniquely-named temporary file, then rename, so a crash or
    // a cancelled job cannot leave a truncated JPEG behind that later looks
    // like a valid cache hit — and so two concurrent renders of the same
    // photo (two lightbox opens, or a render racing a prefetch) cannot
    // truncate each other's file out from under them. A name shared between
    // callers is not made safe by writing the same bytes: `File::create`
    // truncates on open regardless of what the previous opener was doing.
    let partial = crate::cache::CacheDir::unique_temp_path(destination);
    {
        let file = std::fs::File::create(&partial).at(&partial)?;
        let mut writer = BufWriter::new(file);
        JpegEncoder::new_with_quality(&mut writer, JPEG_QUALITY)
            .write_image(
                image.as_raw(),
                image.width(),
                image.height(),
                ExtendedColorType::Rgb8,
            )
            .map_err(|e| {
                Error::from(ExtractError::Unsupported {
                    source_id: "thumbs",
                    detail: format!("could not encode the JPEG: {e}"),
                })
            })?;
    }
    std::fs::rename(&partial, destination).at(destination)?;
    Ok(())
}

/// Scale `width`x`height` to fit inside `max_edge`, keeping the aspect ratio
/// and never returning a zero dimension.
fn fit(width: u32, height: u32, max_edge: u32) -> (u32, u32) {
    let long = width.max(height) as f64;
    let scale = max_edge as f64 / long;
    (
        ((width as f64 * scale).round() as u32).max(1),
        ((height as f64 * scale).round() as u32).max(1),
    )
}

fn heif_error(error: libheif_rs::HeifError) -> Error {
    // The message describes the format problem, not the image, so it is safe
    // to surface and to log.
    ExtractError::Unsupported {
        source_id: "thumbs",
        detail: format!("HEIF decode failed: {error}"),
    }
    .into()
}

/// Whether a preview kind would come through this module at all.
pub fn needs_render(preview: Option<PreviewKind>, mime: Option<&str>, path: &str) -> bool {
    if !matches!(preview, Some(PreviewKind::Image) | None) {
        return false;
    }
    is_heif(mime.unwrap_or(""), path)
}

/// A counting semaphore, so only so many decodes run at once.
struct Limiter {
    available: Mutex<usize>,
    freed: Condvar,
}

struct Permit<'a>(&'a Limiter);

impl Limiter {
    fn new(permits: usize) -> Self {
        Self {
            available: Mutex::new(permits.max(1)),
            freed: Condvar::new(),
        }
    }

    fn acquire(&self) -> Permit<'_> {
        // A poisoned lock means a previous holder panicked mid-decode. The
        // counter is only bookkeeping, so recovering beats failing every
        // later render.
        let mut available = self.available.lock().unwrap_or_else(|e| e.into_inner());
        while *available == 0 {
            available = self
                .freed
                .wait(available)
                .unwrap_or_else(|e| e.into_inner());
        }
        *available -= 1;
        Permit(self)
    }
}

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        let mut available = self.0.available.lock().unwrap_or_else(|e| e.into_inner());
        *available += 1;
        self.0.freed.notify_one();
    }
}

fn limiter() -> &'static Limiter {
    static LIMITER: OnceLock<Limiter> = OnceLock::new();
    LIMITER.get_or_init(|| {
        let parallelism = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(2);
        Limiter::new(parallelism)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fitting_preserves_the_aspect_ratio_and_never_hits_zero() {
        assert_eq!(fit(4000, 3000, 2048), (2048, 1536));
        assert_eq!(fit(3000, 4000, 2048), (1536, 2048));
        // A sliver of an image still has at least one pixel in each direction.
        assert_eq!(fit(10_000, 3, 100), (100, 1));
        // Square stays square.
        assert_eq!(fit(500, 500, 250), (250, 250));
    }

    #[test]
    fn heif_is_detected_by_bytes_first_then_by_name() {
        assert!(is_heif("image/heic", "x.bin"));
        assert!(is_heif("image/heif", "x.bin"));
        // A sniff that found nothing falls back to the extension.
        assert!(is_heif("application/octet-stream", "IMG_0001.HEIC"));
        assert!(is_heif("", "IMG_0001.heif"));
        // A confident sniff wins over a misleading name, which is the real
        // case: 578 assets claimed JPEG while holding PNG.
        assert!(!is_heif("image/png", "IMG_0001.HEIC"));
        assert!(!is_heif("image/jpeg", "IMG_0001.jpg"));
    }

    #[test]
    fn the_limiter_hands_out_no_more_permits_than_it_has() {
        let limiter = Limiter::new(2);
        let first = limiter.acquire();
        let second = limiter.acquire();
        assert_eq!(*limiter.available.lock().unwrap(), 0);
        drop(first);
        assert_eq!(*limiter.available.lock().unwrap(), 1);
        drop(second);
        assert_eq!(*limiter.available.lock().unwrap(), 2);
    }

    #[test]
    fn a_waiting_thread_gets_the_permit_the_first_one_releases() {
        use std::sync::Arc;
        let limiter = Arc::new(Limiter::new(1));
        let held = limiter.acquire();

        let other = Arc::clone(&limiter);
        let waiter = std::thread::spawn(move || {
            let _permit = other.acquire();
            true
        });
        // The waiter cannot proceed until the permit comes back.
        assert!(!waiter.is_finished());
        drop(held);
        assert!(waiter.join().unwrap());
    }

    #[test]
    fn a_web_safe_image_is_passed_through_untouched() {
        use crate::testutil::{FixtureBackup, photos as fixture};
        use crate::{Backup, CacheDir};

        let tmp = tempfile::tempdir().unwrap();
        let root = FixtureBackup::new()
            .with_file(
                "CameraRollDomain",
                "Media/DCIM/100APPLE/A.JPG",
                &fixture::jpeg(40, 30),
            )
            .build(tmp.path())
            .unwrap();
        let backup =
            Backup::open_with_cache(&root, None, CacheDir::at(tmp.path().join("cache"))).unwrap();
        let entry = backup
            .find("CameraRollDomain", "Media/DCIM/100APPLE/A.JPG")
            .unwrap()
            .unwrap();

        let small = render(&backup, &entry, 512).unwrap();
        assert!(small.original, "a JPEG must not be re-encoded");
        assert_eq!(small.mime, "image/jpeg");
        // The bytes are the backup's own, not a copy through the encoder.
        let served = std::fs::read(&small.path).unwrap();
        assert_eq!(served, fixture::jpeg(40, 30));
    }

    /// The decode path end to end: container in, JPEG of the right size out.
    ///
    /// Uses AVIF rather than HEIC because encoding HEIC needs an HEVC encoder
    /// plugin that most Linux machines lack, while the decode path through
    /// libheif — context, handle, interleaved RGB plane, stride — is the same
    /// one HEIC takes. The real backup's 4,211 HEICs cover the codec itself.
    #[test]
    fn a_heif_container_is_decoded_resized_and_cached() {
        use crate::testutil::{FixtureBackup, photos as fixture};
        use crate::{Backup, CacheDir};

        let Some(avif) = fixture::avif(120, 90) else {
            println!("skipping: this system's libheif has no AV1 encoder plugin");
            return;
        };

        let tmp = tempfile::tempdir().unwrap();
        let root = FixtureBackup::new()
            .with_file("CameraRollDomain", "Media/DCIM/100APPLE/A.HEIC", &avif)
            .build(tmp.path())
            .unwrap();
        let backup =
            Backup::open_with_cache(&root, None, CacheDir::at(tmp.path().join("cache"))).unwrap();
        let entry = backup
            .find("CameraRollDomain", "Media/DCIM/100APPLE/A.HEIC")
            .unwrap()
            .unwrap();

        let scaled = render(&backup, &entry, 60).unwrap();
        assert!(!scaled.original, "a HEIF container has to be converted");
        assert_eq!(scaled.mime, "image/jpeg");
        assert!(scaled.path.is_file());

        // The output is a real JPEG, scaled to fit the requested edge with the
        // aspect ratio kept.
        let bytes = std::fs::read(&scaled.path).unwrap();
        assert_eq!(&bytes[..2], &[0xFF, 0xD8], "not a JPEG");
        let (width, height) = jpeg_dimensions(&bytes).expect("should have an SOF marker");
        assert_eq!((width, height), (60, 45));

        // Second call is a cache hit: same path, and no leftover temp file
        // anywhere under the cache (the temp name is unique per call, so
        // there is no one fixed path to check).
        let again = render(&backup, &entry, 60).unwrap();
        assert_eq!(again.path, scaled.path);
        let thumbs_dir = scaled.path.parent().unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(thumbs_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name())
            .filter(|name| name.to_string_lossy() != format!("{}.jpg", entry.file_id))
            .collect();
        assert!(
            leftovers.is_empty(),
            "found leftover temp file(s): {leftovers:?}"
        );

        // A different size is a different cache entry, not an overwrite.
        let bigger = render(&backup, &entry, 0).unwrap();
        assert_ne!(bigger.path, scaled.path);
        let (full_width, full_height) =
            jpeg_dimensions(&std::fs::read(&bigger.path).unwrap()).unwrap();
        assert_eq!((full_width, full_height), (120, 90));
    }

    /// Width and height from a JPEG's SOF marker.
    #[cfg(test)]
    fn jpeg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
        let mut at = 2;
        while at + 9 < bytes.len() {
            if bytes[at] != 0xFF {
                at += 1;
                continue;
            }
            let marker = bytes[at + 1];
            if matches!(marker, 0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF) {
                let height = u16::from_be_bytes([bytes[at + 5], bytes[at + 6]]);
                let width = u16::from_be_bytes([bytes[at + 7], bytes[at + 8]]);
                return Some((u32::from(width), u32::from(height)));
            }
            let length = u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]) as usize;
            at += 2 + length;
        }
        None
    }
}
