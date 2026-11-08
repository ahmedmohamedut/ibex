//! Browsable "Files" sources: On My iPhone, and every app's `Documents/`.
//!
//! Unlike every other extractor in this module, there is no database here —
//! browsing reuses `Backup::list_dir` exactly as the Raw browser does. What's
//! missing without this module is just *naming* the starting points, which
//! turned out to need more than `docs/DATA_SOURCES.md` first assumed: see
//! [`app_name`] for what inspecting the real backup's `Info.plist` found.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::Backup;
use crate::error::Result;

/// The Files app's "On My iPhone" container.
const ON_MY_IPHONE_DOMAIN: &str = "AppDomainGroup-group.com.apple.FileProvider.LocalStorage";
const ON_MY_IPHONE_ROOT: &str = "File Provider Storage";

/// One browsable root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DocSource {
    pub domain: String,
    /// Relative path within the domain that this source starts browsing at.
    /// Empty for a source that starts at the domain's root.
    pub root: String,
    pub label: String,
    /// `None` for "On My iPhone", which isn't an app.
    pub bundle_id: Option<String>,
}

/// Every browsable Files source this backup has.
///
/// "On My iPhone" first, when it has anything in it; then one entry per
/// `AppDomain-<bundle>` with a non-empty `Documents/`, found with a single
/// grouped query rather than a `list_dir` per app domain — the real backup
/// this was built against has well over a hundred app domains, most with an
/// empty `Documents/`.
pub fn list_document_sources(backup: &Backup) -> Result<Vec<DocSource>> {
    let mut sources = Vec::new();

    let on_my_iphone_present = backup
        .find(ON_MY_IPHONE_DOMAIN, ON_MY_IPHONE_ROOT)
        .ok()
        .flatten()
        .is_some();
    if on_my_iphone_present {
        sources.push(DocSource {
            domain: ON_MY_IPHONE_DOMAIN.to_string(),
            root: ON_MY_IPHONE_ROOT.to_string(),
            label: "On My iPhone".to_string(),
            bundle_id: None,
        });
    }

    let app_domains = backup.manifest().with_connection(|conn| {
        let mut stmt = conn.prepare(
            "SELECT domain, count(*) FROM Files \
             WHERE domain LIKE 'AppDomain-%' \
               AND relativePath LIKE 'Documents/%' \
               AND flags = 1 \
             GROUP BY domain HAVING count(*) > 0 ORDER BY domain",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
    })?;

    let app_names = &backup.info().app_names;
    for domain in app_domains {
        let bundle_id = domain
            .strip_prefix("AppDomain-")
            .unwrap_or(&domain)
            .to_string();
        sources.push(DocSource {
            domain: domain.clone(),
            root: "Documents".to_string(),
            label: app_name(&bundle_id, app_names),
            bundle_id: Some(bundle_id),
        });
    }

    Ok(sources)
}

/// Free first-party Apple apps with no App Store purchase record, so no
/// `iTunesMetadata` — confirmed against the real backup this was built
/// against, which has these seven in `Installed Applications` but not in
/// `Applications`. Not exhaustive; anything else falls through to
/// [`prettify_bundle_id`].
const FIRST_PARTY_APP_NAMES: &[(&str, &str)] = &[
    ("com.apple.Keynote", "Keynote"),
    ("com.apple.Numbers", "Numbers"),
    ("com.apple.Pages", "Pages"),
    ("com.apple.iMovie", "iMovie"),
    ("com.apple.mobilegarageband", "GarageBand"),
    ("com.apple.sports", "Sports"),
    ("com.apple.Music", "Music"),
    ("com.apple.podcasts", "Podcasts"),
    ("com.apple.mobilenotes", "Notes"),
    ("com.apple.MobileSMS", "Messages"),
    ("com.apple.mobilecal", "Calendar"),
    ("com.apple.mobilesafari", "Safari"),
    ("com.apple.VoiceMemos", "Voice Memos"),
    ("com.apple.news", "News"),
    ("com.apple.freeform", "Freeform"),
];

/// Resolve a bundle id to the best name we have: the real App Store title
/// when `Info.plist` had one, a small built-in table for free first-party
/// apps (which have no App Store purchase record at all), or the bundle id
/// itself, prettified, as a last resort that is never wrong, just plain.
pub fn app_name(bundle_id: &str, resolved: &std::collections::HashMap<String, String>) -> String {
    if let Some(name) = resolved.get(bundle_id) {
        return name.clone();
    }
    if let Some((_, name)) = FIRST_PARTY_APP_NAMES
        .iter()
        .find(|(id, _)| *id == bundle_id)
    {
        return name.to_string();
    }
    prettify_bundle_id(bundle_id)
}

/// `com.example.myCoolApp` -> `MyCoolApp`. Never fails; worst case the result
/// is just the bundle id's last segment, unchanged.
fn prettify_bundle_id(bundle_id: &str) -> String {
    let last = bundle_id.rsplit('.').next().unwrap_or(bundle_id);
    let mut chars = last.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => bundle_id.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::FixtureBackup;
    use crate::{Backup, CacheDir};

    fn open_fixture(tmp: &std::path::Path) -> Backup {
        let root = FixtureBackup::new()
            .with_installed_app_named("com.example.good", "Good App")
            .with_installed_app("com.apple.Keynote")
            .with_installed_app("com.unknown.mystery_app")
            .with_file(
                "AppDomain-com.example.good",
                "Documents/report.pdf",
                b"%PDF-1.4",
            )
            .with_file(
                "AppDomain-com.apple.Keynote",
                "Documents/deck.key",
                b"keynote",
            )
            // A domain with files outside Documents/ only — must not appear.
            .with_file(
                "AppDomain-com.unknown.mystery_app",
                "Library/cache.db",
                b"x",
            )
            .with_file(
                "AppDomainGroup-group.com.apple.FileProvider.LocalStorage",
                "File Provider Storage/Downloads/a.pdf",
                b"%PDF-1.4",
            )
            .build(tmp)
            .unwrap();
        Backup::open_with_cache(&root, None, CacheDir::at(tmp.join("cache"))).unwrap()
    }

    #[test]
    fn on_my_iphone_and_populated_app_domains_are_listed() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let sources = list_document_sources(&backup).unwrap();

        assert!(
            sources
                .iter()
                .any(|s| s.label == "On My iPhone" && s.bundle_id.is_none())
        );
        assert!(
            sources
                .iter()
                .any(|s| s.bundle_id.as_deref() == Some("com.example.good"))
        );
        assert!(
            sources
                .iter()
                .any(|s| s.bundle_id.as_deref() == Some("com.apple.Keynote"))
        );
        // Files outside Documents/ must not make a domain count as a source.
        assert!(
            !sources
                .iter()
                .any(|s| s.bundle_id.as_deref() == Some("com.unknown.mystery_app")),
            "an app with no files under Documents/ must not be listed"
        );
    }

    #[test]
    fn an_empty_on_my_iphone_is_not_listed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = FixtureBackup::new()
            .with_file("HomeDomain", "a.txt", b"x")
            .build(tmp.path())
            .unwrap();
        let backup =
            Backup::open_with_cache(&root, None, CacheDir::at(tmp.path().join("cache"))).unwrap();
        let sources = list_document_sources(&backup).unwrap();
        assert!(sources.is_empty());
    }

    #[test]
    fn app_names_prefer_the_real_title_then_the_first_party_table_then_prettifying() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let sources = list_document_sources(&backup).unwrap();

        let good = sources
            .iter()
            .find(|s| s.bundle_id.as_deref() == Some("com.example.good"))
            .unwrap();
        assert_eq!(good.label, "Good App");

        let keynote = sources
            .iter()
            .find(|s| s.bundle_id.as_deref() == Some("com.apple.Keynote"))
            .unwrap();
        assert_eq!(keynote.label, "Keynote");
    }

    #[test]
    fn prettifying_capitalizes_the_last_bundle_segment() {
        assert_eq!(prettify_bundle_id("com.example.myCoolApp"), "MyCoolApp");
        assert_eq!(prettify_bundle_id("single"), "Single");
        assert_eq!(prettify_bundle_id(""), "");
    }
}
