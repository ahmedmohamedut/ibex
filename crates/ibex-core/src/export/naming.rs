//! Turning conversation names and attachment names into filenames.
//!
//! This is the most dangerous code in the export path. Names come from message
//! data — contact names, group titles, attachment filenames — so they are
//! attacker-influenced in the sense that whoever sent you a message chose them.
//! A name that escapes the output directory, overwrites a sibling, or is
//! silently mangled by Windows is a real failure, so the rules here are strict
//! and the tests try to break them.
//!
//! Exports are meant to be portable, so the Windows restrictions apply on every
//! platform: an export made on Linux should still open on Windows.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Characters Windows forbids in a filename, plus the separators.
const FORBIDDEN: &[char] = &['<', '>', ':', '"', '|', '?', '*', '/', '\\'];

/// Device names Windows reserves, with or without an extension.
const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Longest a single path component may be, in bytes.
///
/// Well under the usual 255-byte limit, leaving room for a de-duplication
/// suffix and for the export's own directory nesting.
const MAX_COMPONENT: usize = 200;

/// What a name becomes when nothing usable survives sanitizing.
const FALLBACK: &str = "unnamed";

/// Make one path component safe to write.
///
/// Never returns a name containing a separator, so the result cannot climb out
/// of its directory however the caller joins it.
pub fn safe_component(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if c == '\0' || c.is_control() {
            // Control characters, including newlines a sender can put in a
            // group title, are dropped rather than replaced: they have no
            // useful rendering in a filename.
            continue;
        }
        out.push(if FORBIDDEN.contains(&c) { '_' } else { c });
    }

    // `.` and `..` are directory references, not names.
    let trimmed = out.trim();
    if trimmed.is_empty() || trimmed.chars().all(|c| c == '.') {
        return FALLBACK.to_string();
    }

    // Windows silently strips trailing dots and spaces, so a name ending in one
    // would not round-trip. Do it here, visibly.
    let mut out = trimmed.trim_end_matches([' ', '.']).to_string();
    if out.is_empty() {
        return FALLBACK.to_string();
    }

    // A reserved device name is refused even with an extension: `CON.txt` is
    // still the console on Windows.
    let stem = out.split('.').next().unwrap_or(&out).to_ascii_uppercase();
    if RESERVED.contains(&stem.as_str()) {
        out = format!("_{out}");
    }

    truncate_keeping_extension(&out, MAX_COMPONENT)
}

/// Truncate to `max` bytes without splitting a character, keeping any extension.
fn truncate_keeping_extension(name: &str, max: usize) -> String {
    if name.len() <= max {
        return name.to_string();
    }
    // Keep a short extension so the file still opens with the right program.
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !ext.is_empty() && ext.len() <= 16 && !stem.is_empty() => {
            (stem, Some(ext))
        }
        _ => (name, None),
    };
    let room = match ext {
        // +1 for the dot.
        Some(ext) => max.saturating_sub(ext.len() + 1),
        None => max,
    };
    let mut cut = room.min(stem.len());
    // Step back to a character boundary rather than slicing mid-UTF-8.
    while cut > 0 && !stem.is_char_boundary(cut) {
        cut -= 1;
    }
    let kept = &stem[..cut];
    let kept = kept.trim_end_matches([' ', '.']);
    let kept = if kept.is_empty() { FALLBACK } else { kept };
    match ext {
        Some(ext) => format!("{kept}.{ext}"),
        None => kept.to_string(),
    }
}

/// Hands out unique names within one directory.
///
/// Collisions are common: two photos called `IMG_0001.HEIC` from different
/// conversations, or two contacts with the same display name.
#[derive(Debug, Default)]
pub struct NameAllocator {
    /// Lowercased name to the number of times it has been handed out, because
    /// Windows and macOS filesystems are case-insensitive.
    used: HashMap<String, usize>,
}

impl NameAllocator {
    pub fn new() -> Self {
        Self::default()
    }

    /// A unique, filesystem-safe version of `name`.
    ///
    /// The first caller gets the sanitized name; later ones get ` (2)`, ` (3)`
    /// inserted before the extension.
    pub fn allocate(&mut self, name: &str) -> String {
        let base = safe_component(name);
        let key = base.to_lowercase();

        let count = self.used.entry(key.clone()).or_insert(0);
        *count += 1;
        if *count == 1 {
            return base;
        }

        // Find a suffix that is genuinely free: a name like "x (2)" may already
        // have been handed out directly.
        let mut n = *count;
        loop {
            let candidate = with_suffix(&base, n);
            let candidate_key = candidate.to_lowercase();
            if let std::collections::hash_map::Entry::Vacant(slot) = self.used.entry(candidate_key)
            {
                slot.insert(1);
                return candidate;
            }
            n += 1;
        }
    }

    /// Whether a name has been handed out.
    pub fn contains(&self, name: &str) -> bool {
        self.used.contains_key(&name.to_lowercase())
    }
}

/// Insert ` (n)` before the extension.
fn with_suffix(name: &str, n: usize) -> String {
    let suffix = format!(" ({n})");
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !ext.is_empty() && ext.len() <= 16 && !stem.is_empty() => {
            (stem, Some(ext))
        }
        _ => (name, None),
    };
    // Keep the whole thing within the cap even after the suffix.
    let room = MAX_COMPONENT
        .saturating_sub(suffix.len())
        .saturating_sub(ext.map_or(0, |e| e.len() + 1));
    let stem = truncate_keeping_extension(stem, room);
    match ext {
        Some(ext) => format!("{stem}{suffix}.{ext}"),
        None => format!("{stem}{suffix}"),
    }
}

/// Join a sanitized component onto a base directory.
///
/// The one way the export code builds paths. Because the component is sanitized
/// first and contains no separator, the result is always inside `base`.
pub fn join_safe(base: &Path, component: &str) -> PathBuf {
    base.join(safe_component(component))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn separators_and_traversal_cannot_escape() {
        // The attack this whole module exists to prevent.
        for hostile in [
            "../../etc/passwd",
            "..",
            ".",
            "../",
            "/etc/passwd",
            "a/b",
            "a\\b",
            "....//....//etc",
        ] {
            let safe = safe_component(hostile);
            assert!(
                !safe.contains('/') && !safe.contains('\\'),
                "{hostile:?} produced {safe:?}"
            );
            assert_ne!(safe, "..", "{hostile:?}");
            assert_ne!(safe, ".", "{hostile:?}");

            // And joining it stays inside the base directory.
            let base = Path::new("/exports/run");
            let joined = join_safe(base, hostile);
            assert!(
                joined.starts_with(base),
                "{hostile:?} escaped to {joined:?}"
            );
            assert_eq!(
                joined.components().count(),
                base.components().count() + 1,
                "{hostile:?} added more than one component: {joined:?}"
            );
        }
    }

    #[test]
    fn windows_forbidden_characters_are_replaced() {
        assert_eq!(safe_component(r#"a<b>c:d"e|f?g*h"#), "a_b_c_d_e_f_g_h");
        // A colon matters: on Windows `C:name` is a drive-relative path.
        assert_eq!(safe_component("C:name"), "C_name");
    }

    #[test]
    fn control_characters_including_newlines_are_dropped() {
        assert_eq!(safe_component("line\nbreak"), "linebreak");
        assert_eq!(safe_component("tab\there"), "tabhere");
        assert_eq!(safe_component("nul\0byte"), "nulbyte");
        // A name made only of control characters has nothing left.
        assert_eq!(safe_component("\n\t\r"), FALLBACK);
    }

    #[test]
    fn reserved_device_names_are_defused() {
        for reserved in ["CON", "con", "NUL", "COM1", "LPT9", "AUX", "PRN"] {
            let safe = safe_component(reserved);
            assert!(safe.starts_with('_'), "{reserved} -> {safe}");
        }
        // Still reserved with an extension.
        assert_eq!(safe_component("CON.txt"), "_CON.txt");
        assert_eq!(safe_component("nul.csv"), "_nul.csv");
        // A name that merely starts with those letters is fine.
        assert_eq!(safe_component("console.txt"), "console.txt");
        assert_eq!(safe_component("COMMON"), "COMMON");
    }

    #[test]
    fn trailing_dots_and_spaces_are_trimmed() {
        // Windows drops these silently, so the export would not round-trip.
        assert_eq!(safe_component("name..."), "name");
        assert_eq!(safe_component("name   "), "name");
        assert_eq!(safe_component("name . . "), "name");
        assert_eq!(safe_component("  padded  "), "padded");
    }

    #[test]
    fn empty_and_whitespace_names_get_a_fallback() {
        assert_eq!(safe_component(""), FALLBACK);
        assert_eq!(safe_component("   "), FALLBACK);
        assert_eq!(safe_component("///"), "___");
    }

    #[test]
    fn long_names_are_capped_without_splitting_characters() {
        let long = "a".repeat(500);
        let safe = safe_component(&long);
        assert_eq!(safe.len(), MAX_COMPONENT);

        // Multi-byte characters must not be cut in half.
        let emoji = "\u{1f600}".repeat(200); // 4 bytes each
        let safe = safe_component(&emoji);
        assert!(safe.len() <= MAX_COMPONENT);
        assert!(
            std::str::from_utf8(safe.as_bytes()).is_ok(),
            "truncation split a character"
        );
        assert!(safe.chars().all(|c| c == '\u{1f600}'));

        // An extension survives truncation, so the file still opens.
        let long_name = format!("{}.heic", "b".repeat(500));
        let safe = safe_component(&long_name);
        assert!(safe.len() <= MAX_COMPONENT);
        assert!(safe.ends_with(".heic"), "{safe}");
    }

    #[test]
    fn collisions_get_numbered_suffixes() {
        let mut alloc = NameAllocator::new();
        assert_eq!(alloc.allocate("IMG_0001.HEIC"), "IMG_0001.HEIC");
        assert_eq!(alloc.allocate("IMG_0001.HEIC"), "IMG_0001 (2).HEIC");
        assert_eq!(alloc.allocate("IMG_0001.HEIC"), "IMG_0001 (3).HEIC");
        // A name with no extension.
        assert_eq!(alloc.allocate("Ada Lovelace"), "Ada Lovelace");
        assert_eq!(alloc.allocate("Ada Lovelace"), "Ada Lovelace (2)");
    }

    #[test]
    fn collisions_are_case_insensitive() {
        // An export written on Linux must still open on a case-insensitive
        // filesystem without two files colliding.
        let mut alloc = NameAllocator::new();
        assert_eq!(alloc.allocate("Photo.JPG"), "Photo.JPG");
        let second = alloc.allocate("photo.jpg");
        assert_ne!(second.to_lowercase(), "photo.jpg", "got {second}");
    }

    #[test]
    fn a_suffix_that_is_already_taken_is_skipped() {
        let mut alloc = NameAllocator::new();
        // Someone is genuinely called "chat (2)".
        assert_eq!(alloc.allocate("chat"), "chat");
        assert_eq!(alloc.allocate("chat (2)"), "chat (2)");
        // The next "chat" must not collide with it.
        let third = alloc.allocate("chat");
        assert_ne!(third, "chat (2)");
        assert_eq!(third, "chat (3)");
    }

    #[test]
    fn suffixed_names_still_respect_the_length_cap() {
        let mut alloc = NameAllocator::new();
        let long = format!("{}.heic", "c".repeat(400));
        let first = alloc.allocate(&long);
        let second = alloc.allocate(&long);
        assert!(first.len() <= MAX_COMPONENT);
        assert!(second.len() <= MAX_COMPONENT, "{} bytes", second.len());
        assert!(second.ends_with(".heic"));
        assert_ne!(first, second);
    }

    #[test]
    fn ordinary_names_pass_through_unchanged() {
        // Sanitizing must not mangle the common case.
        for name in [
            "Ada Lovelace",
            "Fixture Book Club",
            "IMG_0001.HEIC",
            "notes (draft).txt",
            "café",
            "日本語のグループ",
        ] {
            assert_eq!(safe_component(name), name, "{name} was altered");
        }
    }
}
