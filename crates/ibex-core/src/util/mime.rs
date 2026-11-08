//! Content-type sniffing.
//!
//! Files in a backup have hashed names with no extension, so the content has to
//! speak for itself. The order is: magic bytes (`infer`), then the plist magics
//! `infer` does not know, then the *original* relative path's extension
//! (`mime_guess`), then a UTF-8 text heuristic.
//!
//! The decision of *how to show* a file is made here rather than in the
//! frontend, so the Raw browser, the media protocol and the exporters all agree.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// How the UI should present a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PreviewKind {
    /// Something the webview can decode and show inline.
    Image,
    /// An image format the webview cannot decode, such as HEIC.
    ImageUnsupported,
    /// Plain text, JSON, source, CSV.
    Text,
    /// A property list, binary or XML. Shown as pretty-printed XML.
    Plist,
    Audio,
    Video,
    Pdf,
    /// Anything else: offer the hex view.
    Binary,
}

/// A sniffed content type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Sniffed {
    /// The media type, e.g. `image/png`.
    pub mime: String,
    pub preview: PreviewKind,
}

/// How many bytes are enough to sniff. All magic numbers are far shorter; the
/// extra room is for the UTF-8 heuristic.
pub const SNIFF_BYTES: usize = 4096;

/// Image formats a WebKitGTK webview cannot decode, so previewing them needs a
/// conversion step (milestone M5 for HEIC).
const UNSUPPORTED_IMAGE_MIMES: &[&str] = &[
    "image/heic",
    "image/heif",
    "image/heic-sequence",
    "image/heif-sequence",
    "image/x-canon-cr2",
    "image/vnd.adobe.photoshop",
    "image/x-icon",
    "image/tiff",
];

/// Sniff a content type from a prefix of the file's bytes plus its name.
///
/// `file_name` is the name from the manifest (`entry.name()`), not the hashed
/// physical name, which carries no extension.
pub fn sniff(bytes: &[u8], file_name: &str) -> Sniffed {
    // 1. Property lists, which `infer` does not recognise and which are very
    //    common in a backup.
    if is_binary_plist(bytes) || is_xml_plist(bytes) {
        return Sniffed {
            mime: "application/x-plist".to_string(),
            preview: PreviewKind::Plist,
        };
    }

    // 2. Magic bytes.
    if let Some(kind) = infer::get(bytes) {
        let mime = normalize_mime(kind.mime_type());
        return Sniffed {
            preview: preview_for(&mime),
            mime,
        };
    }

    // 3. The original name's extension.
    if let Some(mime) = mime_guess::from_path(file_name).first_raw() {
        return Sniffed {
            mime: mime.to_string(),
            preview: preview_for(mime),
        };
    }

    // 4. Does it read as text?
    if looks_like_text(bytes) {
        return Sniffed {
            mime: "text/plain".to_string(),
            preview: PreviewKind::Text,
        };
    }

    Sniffed {
        mime: "application/octet-stream".to_string(),
        preview: PreviewKind::Binary,
    }
}

/// Correct a magic-byte match for what the webview's `<audio>`/`<video>`
/// element actually needs in `Content-Type`, when it differs from the type
/// name a codec-sniffing library hands back.
///
/// `infer` reports a WhatsApp voice note's Ogg container as `audio/opus`
/// (the RTP payload type, reused loosely), but WebKitGTK's media backend
/// does not recognise that as a playable container and the element errors
/// out instead of decoding — even though the same bytes play fine labelled
/// `audio/ogg`, which is what RFC 7845 §9 actually registers for an
/// Ogg-encapsulated Opus file.
fn normalize_mime(mime: &str) -> String {
    if mime.eq_ignore_ascii_case("audio/opus") {
        return "audio/ogg".to_string();
    }
    mime.to_string()
}

/// Map a media type to the way it should be shown.
pub fn preview_for(mime: &str) -> PreviewKind {
    let lower = mime.to_ascii_lowercase();
    let essence = lower.split(';').next().unwrap_or(&lower).trim();

    if essence == "application/pdf" {
        return PreviewKind::Pdf;
    }
    if essence == "application/x-plist" || essence == "application/x-bplist" {
        return PreviewKind::Plist;
    }
    if let Some(subtype) = essence.strip_prefix("image/") {
        return if UNSUPPORTED_IMAGE_MIMES.contains(&essence) {
            PreviewKind::ImageUnsupported
        } else if matches!(
            subtype,
            "png" | "jpeg" | "gif" | "webp" | "bmp" | "svg+xml" | "apng" | "avif"
        ) {
            PreviewKind::Image
        } else {
            // Unknown image subtype: let the webview try rather than refuse.
            PreviewKind::Image
        };
    }
    if essence.starts_with("audio/") {
        return PreviewKind::Audio;
    }
    if essence.starts_with("video/") {
        return PreviewKind::Video;
    }
    if essence.starts_with("text/")
        || matches!(
            essence,
            "application/json"
                | "application/xml"
                | "application/javascript"
                | "application/x-sh"
                | "application/sql"
                | "application/x-yaml"
        )
    {
        return PreviewKind::Text;
    }
    PreviewKind::Binary
}

/// A binary property list starts with `bplist` and a two-digit version.
pub fn is_binary_plist(bytes: &[u8]) -> bool {
    bytes.starts_with(b"bplist")
}

/// An XML property list: an XML declaration or doctype mentioning `plist`.
fn is_xml_plist(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(512)];
    let Ok(text) = std::str::from_utf8(head) else {
        // A truncated multi-byte character at the boundary is fine to ignore;
        // the markers we look for are ASCII and appear early.
        return match std::str::from_utf8(&head[..head.len().saturating_sub(3)]) {
            Ok(text) => mentions_plist(text),
            Err(_) => false,
        };
    };
    mentions_plist(text)
}

fn mentions_plist(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    (lower.contains("<?xml") || lower.contains("<!doctype"))
        && (lower.contains("plist") || lower.contains("propertylist"))
}

/// Whether a byte prefix reads as text.
///
/// Decodes as UTF-8 and contains no NUL and few control characters. A prefix
/// may end mid-character, so the last few bytes are allowed to be incomplete.
pub fn looks_like_text(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        // An empty file is harmless to show as text.
        return true;
    }
    if bytes.contains(&0) {
        return false;
    }
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(e) if e.error_len().is_none() && e.valid_up_to() > 0 => {
            // Truncated final character: judge the valid part.
            std::str::from_utf8(&bytes[..e.valid_up_to()]).unwrap_or("")
        }
        Err(_) => return false,
    };
    let control = text
        .chars()
        .filter(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
        .count();
    // Up to 1% stray control characters, for files with the odd form feed.
    control * 100 <= text.chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal valid PNG: signature plus an IHDR chunk.
    fn png() -> Vec<u8> {
        let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        bytes.extend_from_slice(&[0, 0, 0, 13]);
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0]);
        bytes.extend_from_slice(&[0x1f, 0x15, 0xc4, 0x89]);
        bytes
    }

    #[test]
    fn magic_bytes_win_over_the_name() {
        // A PNG misleadingly named .txt is still an image.
        let sniffed = sniff(&png(), "notes.txt");
        assert_eq!(sniffed.mime, "image/png");
        assert_eq!(sniffed.preview, PreviewKind::Image);
    }

    #[test]
    fn binary_plists_are_recognised() {
        let sniffed = sniff(b"bplist00\xd1\x01\x02", "com.apple.whatever");
        assert_eq!(sniffed.mime, "application/x-plist");
        assert_eq!(sniffed.preview, PreviewKind::Plist);
    }

    #[test]
    fn xml_plists_are_recognised() {
        let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict></dict></plist>"#;
        let sniffed = sniff(xml, "Info.plist");
        assert_eq!(sniffed.preview, PreviewKind::Plist);
    }

    #[test]
    fn plain_xml_is_not_a_plist() {
        let xml = br#"<?xml version="1.0"?><note><to>x</to></note>"#;
        let sniffed = sniff(xml, "note.xml");
        assert_eq!(sniffed.preview, PreviewKind::Text);
        assert_ne!(sniffed.mime, "application/x-plist");
    }

    #[test]
    fn the_extension_is_the_fallback() {
        // No magic bytes to go on, so the manifest name decides.
        let sniffed = sniff(b"col1,col2\n1,2\n", "export.csv");
        assert_eq!(sniffed.mime, "text/csv");
        assert_eq!(sniffed.preview, PreviewKind::Text);
    }

    #[test]
    fn unknown_text_falls_back_to_plain() {
        let sniffed = sniff(b"just some words\nand more\n", "nameless");
        assert_eq!(sniffed.mime, "text/plain");
        assert_eq!(sniffed.preview, PreviewKind::Text);
    }

    #[test]
    fn unknown_binary_is_offered_as_hex() {
        let sniffed = sniff(&[0x00, 0x01, 0x02, 0xff, 0xfe], "nameless");
        assert_eq!(sniffed.mime, "application/octet-stream");
        assert_eq!(sniffed.preview, PreviewKind::Binary);
    }

    #[test]
    fn heic_is_flagged_as_unviewable_rather_than_broken() {
        // WebKitGTK cannot decode HEIC; the UI says so instead of showing a
        // broken image. Milestone M5 adds conversion.
        assert_eq!(preview_for("image/heic"), PreviewKind::ImageUnsupported);
        assert_eq!(preview_for("image/heif"), PreviewKind::ImageUnsupported);
        assert_eq!(preview_for("image/png"), PreviewKind::Image);
    }

    #[test]
    fn an_ogg_opus_voice_note_is_served_as_audio_ogg_not_audio_opus() {
        // `infer`'s magic-byte match names this `audio/opus`, but WebKitGTK's
        // `<audio>` element fails to play that — see `normalize_mime`.
        let bytes = crate::testutil::whatsapp::ogg_opus();
        let sniffed = sniff(&bytes, "voice.opus");
        assert_eq!(sniffed.mime, "audio/ogg");
        assert_eq!(sniffed.preview, PreviewKind::Audio);
    }

    #[test]
    fn preview_kinds_cover_the_media_families() {
        assert_eq!(preview_for("audio/mpeg"), PreviewKind::Audio);
        assert_eq!(preview_for("audio/x-m4a"), PreviewKind::Audio);
        assert_eq!(preview_for("video/mp4"), PreviewKind::Video);
        assert_eq!(preview_for("application/pdf"), PreviewKind::Pdf);
        assert_eq!(preview_for("application/json"), PreviewKind::Text);
        assert_eq!(preview_for("text/html"), PreviewKind::Text);
        assert_eq!(preview_for("application/zip"), PreviewKind::Binary);
    }

    #[test]
    fn parameters_and_case_do_not_confuse_the_mapping() {
        assert_eq!(preview_for("TEXT/PLAIN; charset=utf-8"), PreviewKind::Text);
        assert_eq!(preview_for("Image/PNG"), PreviewKind::Image);
    }

    #[test]
    fn text_detection() {
        assert!(looks_like_text(b"hello\nworld\t!\r\n"));
        assert!(looks_like_text(b""), "an empty file is showable as text");
        assert!(looks_like_text("caf\u{e9} \u{1f600}".as_bytes()));
        assert!(!looks_like_text(b"has\0a nul"));
        assert!(!looks_like_text(&[0x80, 0x81, 0x82]), "invalid UTF-8");
    }

    #[test]
    fn a_prefix_cut_mid_character_still_reads_as_text() {
        // An emoji is 4 bytes; lop off the last one as a read of SNIFF_BYTES
        // would.
        let full = "hello \u{1f600}".as_bytes().to_vec();
        let truncated = &full[..full.len() - 1];
        assert!(std::str::from_utf8(truncated).is_err());
        assert!(looks_like_text(truncated));
    }

    #[test]
    fn sniffing_an_empty_file_does_not_panic() {
        let sniffed = sniff(&[], "empty");
        assert_eq!(sniffed.preview, PreviewKind::Text);
        let sniffed = sniff(&[], "empty.png");
        assert_eq!(sniffed.mime, "image/png", "the name is all we have");
    }
}
