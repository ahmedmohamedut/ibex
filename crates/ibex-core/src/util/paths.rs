//! File IDs and physical paths inside a backup folder.

use std::path::{Path, PathBuf};

use sha1::{Digest, Sha1};

/// The manifest key for a file: lowercase SHA-1 hex of `"<domain>-<relativePath>"`.
///
/// ```
/// use ibex_core::util::paths::file_id;
///
/// // The canonical example: the SMS database.
/// assert_eq!(
///     file_id("HomeDomain", "Library/SMS/sms.db"),
///     "3d0d7e5fb2ce288813306e4d4636395e047a3d28"
/// );
/// ```
pub fn file_id(domain: &str, relative_path: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(domain.as_bytes());
    hasher.update(b"-");
    hasher.update(relative_path.as_bytes());
    hex::encode(hasher.finalize())
}

/// Where a file's bytes live on disk: `<root>/<first two hex chars>/<fileID>`.
///
/// Returns `None` for a `file_id` that is too short to have a shard prefix,
/// which only happens with corrupt manifests.
pub fn physical_path(root: &Path, file_id: &str) -> Option<PathBuf> {
    let shard = file_id.get(..2)?;
    Some(root.join(shard).join(file_id))
}

/// Split a relative path into (parent, file name), using `/` as the separator
/// regardless of host platform — these are iOS paths, not local ones.
pub fn split_relative(relative_path: &str) -> (Option<&str>, &str) {
    match relative_path.rsplit_once('/') {
        Some((parent, name)) => (Some(parent), name),
        None => (None, relative_path),
    }
}

/// Normalize a directory path for prefix matching: no leading or trailing `/`.
pub fn normalize_dir(dir: &str) -> &str {
    dir.trim_matches('/')
}

/// Mask the identifier in a WhatsApp-style path so it can be logged.
///
/// WhatsApp stores media under `Media/<number>@s.whatsapp.net/...` or the newer
/// `Media/<number>@lid/...`, and group media under `<number>-<number>@g.us`.
/// Those are phone numbers, which hard rule 3 forbids putting in a log or an
/// error message — even though the same rule permits "paths within the backup",
/// because here they are the same string.
///
/// The structure is kept so a path stays recognisable and debuggable.
///
/// ```
/// use ibex_core::util::paths::redact_jid;
///
/// assert_eq!(
///     redact_jid("Media/447700900123@lid/a/b/photo.jpg"),
///     "Media/<jid>@lid/a/b/photo.jpg"
/// );
/// ```
pub fn redact_jid(path: &str) -> String {
    path.split('/')
        .map(|segment| match segment.split_once('@') {
            // Keep the domain part: @lid, @s.whatsapp.net and @g.us say what
            // kind of conversation it was, and carry no personal data.
            Some((_, domain)) => format!("<jid>@{domain}"),
            None => segment.to_string(),
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_file_ids() {
        // Verified against the published hash for sms.db, and recomputed here
        // for other well-known paths so a refactor of the hashing cannot
        // silently change the layout.
        assert_eq!(
            file_id("HomeDomain", "Library/SMS/sms.db"),
            "3d0d7e5fb2ce288813306e4d4636395e047a3d28"
        );
        assert_eq!(
            file_id("CameraRollDomain", "Media/PhotoData/Photos.sqlite"),
            "12b144c0bd44f2b3dffd9186d3f9c05b917cee25"
        );
        assert_eq!(
            file_id("HomeDomain", "Library/AddressBook/AddressBook.sqlitedb"),
            "31bb7ba8914766d4ba40d6dfb6113c8b614be442"
        );
    }

    #[test]
    fn the_separator_is_ambiguous_by_design() {
        // Apple joins domain and path with a bare "-" and escapes nothing, so
        // ("AppDomain", "com.foo-x") and ("AppDomain-com.foo", "x") hash to the
        // same fileID. We cannot fix that without breaking compatibility, and
        // it is why `Manifest.find` treats the computed ID as a fast path only
        // and falls back to querying domain + relativePath, which are stored
        // separately and unambiguously in the Files table.
        assert_eq!(file_id("A", "B-C"), file_id("A-B", "C"));
    }

    #[test]
    fn physical_path_is_sharded() {
        let p = physical_path(Path::new("/b"), "3d0d7e5fb2ce288813306e4d4636395e047a3d28");
        assert_eq!(
            p.unwrap(),
            Path::new("/b/3d/3d0d7e5fb2ce288813306e4d4636395e047a3d28")
        );
        assert_eq!(physical_path(Path::new("/b"), "a"), None);
    }

    #[test]
    fn splitting_uses_forward_slashes() {
        assert_eq!(
            split_relative("Library/SMS/sms.db"),
            (Some("Library/SMS"), "sms.db")
        );
        assert_eq!(split_relative("sms.db"), (None, "sms.db"));
        assert_eq!(split_relative(""), (None, ""));
    }

    #[test]
    fn jids_are_masked_but_paths_stay_recognisable() {
        // The newer linked-identity form, which is what a current backup uses.
        assert_eq!(
            redact_jid("Media/447700900123@lid/a/b/photo.jpg"),
            "Media/<jid>@lid/a/b/photo.jpg"
        );
        // The classic form.
        assert_eq!(
            redact_jid("Media/15550001234@s.whatsapp.net/7/4/clip.mp4"),
            "Media/<jid>@s.whatsapp.net/7/4/clip.mp4"
        );
        // A group, whose JID embeds two numbers.
        assert_eq!(
            redact_jid("Message/Media/15550001234-1545900921@g.us/a/2/v.mp4"),
            "Message/Media/<jid>@g.us/a/2/v.mp4"
        );
        // Paths with no JID are untouched, so SMS and Raw paths stay readable.
        assert_eq!(redact_jid("Library/SMS/sms.db"), "Library/SMS/sms.db");
        assert_eq!(redact_jid(""), "");
        // No digits survive from the identifier itself.
        let masked = redact_jid("Media/447700900123@lid/x.jpg");
        assert!(!masked.contains("447700900123"), "{masked}");
    }

    #[test]
    fn normalizing_dirs() {
        assert_eq!(normalize_dir("/Library/SMS/"), "Library/SMS");
        assert_eq!(normalize_dir(""), "");
        assert_eq!(normalize_dir("/"), "");
    }
}
