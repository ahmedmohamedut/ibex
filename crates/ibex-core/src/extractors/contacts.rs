//! Resolving phone numbers and emails to contact names.
//!
//! Source: `HomeDomain` / `Library/AddressBook/AddressBook.sqlitedb`, where
//! `ABPerson` holds the names and `ABMultiValue` the numbers and addresses
//! (`property` 3 = phone, 4 = email).
//!
//! Matching numbers is the fiddly part. The same person can appear as
//! `+1 (555) 000-1234` in the address book and `+15550001234` in `sms.db`, or
//! as a local `5550001234` with no country code. So every number is reduced to
//! digits, and a secondary index on the last nine digits catches the
//! country-code mismatches. Nine, not ten: it is long enough that collisions
//! between real contacts are vanishingly unlikely, and short enough to survive
//! differing national trunk prefixes.

use std::collections::HashMap;

use crate::error::Result;
use crate::extractors::{Availability, Participant};

/// How many trailing digits the fallback index keys on.
const SUFFIX_LEN: usize = 9;

/// Address-book lookups for one backup.
///
/// Built once and shared: `sms` and `whatsapp` both use it, and it holds the
/// whole address book in memory, which is a few thousand rows at most.
#[derive(Debug, Default)]
pub struct ContactResolver {
    /// Exact normalized key (digits, or a lowercased email) to display name.
    exact: HashMap<String, String>,
    /// Last [`SUFFIX_LEN`] digits to display name, for country-code mismatches.
    suffix: HashMap<String, String>,
    availability: Option<Availability>,
}

impl ContactResolver {
    /// An empty resolver: everything stays unresolved.
    ///
    /// Used when a backup has no address book, which is not an error — numbers
    /// simply show as numbers.
    pub fn empty() -> Self {
        Self {
            exact: HashMap::new(),
            suffix: HashMap::new(),
            availability: Some(Availability::Absent),
        }
    }

    /// Read the address book out of a backup.
    ///
    /// A missing or unreadable address book yields an empty resolver rather
    /// than an error: losing names is a degradation, not a failure.
    pub fn open(backup: &crate::Backup) -> Result<Self> {
        let entry = backup.find("HomeDomain", "Library/AddressBook/AddressBook.sqlitedb")?;
        let Some(entry) = entry else {
            return Ok(Self::empty());
        };
        if !backup.has_data(&entry) {
            return Ok(Self::empty());
        }

        let conn = match backup.open_db("HomeDomain", "Library/AddressBook/AddressBook.sqlitedb") {
            Ok(conn) => conn,
            Err(_) => return Ok(Self::empty()),
        };

        // Tolerant parsing (hard rule 6): check the tables and columns exist
        // before selecting them. Older and newer iOS both turn up here.
        let has = |table: &str| -> bool {
            conn.query_one(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [table],
                |row| row.get::<_, i64>(0),
            )
            .map(|n| n > 0)
            .unwrap_or(false)
        };
        if !has("ABPerson") || !has("ABMultiValue") {
            return Ok(Self {
                exact: HashMap::new(),
                suffix: HashMap::new(),
                availability: Some(Availability::Unsupported(
                    "AddressBook.sqlitedb has no ABPerson/ABMultiValue tables".to_string(),
                )),
            });
        }

        let columns: Vec<String> = {
            let mut stmt = conn.prepare("SELECT name FROM pragma_table_info('ABPerson')")?;
            stmt.query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        let has_col = |name: &str| columns.iter().any(|c| c == name);
        // Build the name expression from whichever columns exist.
        let mut parts: Vec<&str> = Vec::new();
        for candidate in ["First", "Last"] {
            if has_col(candidate) {
                parts.push(candidate);
            }
        }
        let fallbacks: Vec<&str> = ["Nickname", "Organization"]
            .into_iter()
            .filter(|c| has_col(c))
            .collect();

        let mut names: HashMap<i64, String> = HashMap::new();
        {
            let select = if parts.is_empty() {
                "SELECT ROWID, NULL, NULL FROM ABPerson".to_string()
            } else {
                format!(
                    "SELECT ROWID, {}, {} FROM ABPerson",
                    parts.first().copied().unwrap_or("NULL"),
                    parts.get(1).copied().unwrap_or("NULL")
                )
            };
            let mut stmt = conn.prepare(&select)?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })?;
            for row in rows {
                let (rowid, first, last) = row?;
                let joined = [first, last]
                    .into_iter()
                    .flatten()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");
                if !joined.is_empty() {
                    names.insert(rowid, joined);
                }
            }

            // Fill in anyone with no first or last name from a nickname or an
            // organization, so a business contact still gets a label.
            for column in fallbacks {
                let sql =
                    format!("SELECT ROWID, {column} FROM ABPerson WHERE {column} IS NOT NULL");
                let mut stmt = conn.prepare(&sql)?;
                let rows = stmt.query_map([], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?))
                })?;
                for row in rows {
                    let (rowid, value) = row?;
                    let Some(value) = value
                        .map(|v| v.trim().to_string())
                        .filter(|v| !v.is_empty())
                    else {
                        continue;
                    };
                    names.entry(rowid).or_insert(value);
                }
            }
        }

        let mut resolver = Self {
            exact: HashMap::new(),
            suffix: HashMap::new(),
            availability: Some(Availability::Present { version_hint: None }),
        };

        let mut stmt = conn.prepare(
            "SELECT record_id, property, value FROM ABMultiValue \
             WHERE value IS NOT NULL AND property IN (3, 4)",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (record_id, property, value) = row?;
            let Some(name) = names.get(&record_id) else {
                continue;
            };
            match property {
                // Phone.
                3 => resolver.insert_phone(&value, name),
                // Email.
                4 => {
                    let key = value.trim().to_ascii_lowercase();
                    if !key.is_empty() {
                        resolver.exact.entry(key).or_insert_with(|| name.clone());
                    }
                }
                _ => {}
            }
        }

        Ok(resolver)
    }

    fn insert_phone(&mut self, raw: &str, name: &str) {
        let digits = digits_of(raw);
        if digits.is_empty() {
            return;
        }
        self.exact
            .entry(digits.clone())
            .or_insert_with(|| name.to_string());
        if let Some(suffix) = suffix_key(&digits) {
            // `or_insert` rather than overwrite: on a suffix collision the
            // first contact wins deterministically instead of depending on row
            // order.
            self.suffix
                .entry(suffix)
                .or_insert_with(|| name.to_string());
        }
    }

    /// Look up a display name for a `sms.db` handle (a number or an email).
    pub fn display_name(&self, handle: &str) -> Option<&str> {
        let trimmed = handle.trim();
        if trimmed.is_empty() {
            return None;
        }
        // An email: match case-insensitively.
        if trimmed.contains('@') {
            return self
                .exact
                .get(&trimmed.to_ascii_lowercase())
                .map(String::as_str);
        }
        let digits = digits_of(trimmed);
        if digits.is_empty() {
            // Something like a short code or an alphanumeric sender.
            return self
                .exact
                .get(&trimmed.to_ascii_lowercase())
                .map(String::as_str);
        }
        if let Some(name) = self.exact.get(&digits) {
            return Some(name);
        }
        // Fall back to the last nine digits, for country-code mismatches.
        suffix_key(&digits).and_then(|key| self.suffix.get(&key).map(String::as_str))
    }

    /// Build a [`Participant`] for a handle, resolved if possible.
    pub fn participant(&self, handle: &str) -> Participant {
        match self.display_name(handle) {
            Some(name) => Participant {
                id: handle.to_string(),
                display_name: Some(name.to_string()),
                from_contacts: true,
            },
            None => Participant::unresolved(handle),
        }
    }

    /// How the address book was found, for diagnostics.
    pub fn availability(&self) -> Availability {
        self.availability.clone().unwrap_or(Availability::Absent)
    }

    /// Number of distinct identifiers that can be resolved.
    pub fn len(&self) -> usize {
        self.exact.len()
    }

    pub fn is_empty(&self) -> bool {
        self.exact.is_empty()
    }
}

/// Everything that is not a digit, removed.
fn digits_of(value: &str) -> String {
    value.chars().filter(char::is_ascii_digit).collect()
}

/// The last [`SUFFIX_LEN`] digits, if there are that many.
fn suffix_key(digits: &str) -> Option<String> {
    if digits.len() < SUFFIX_LEN {
        return None;
    }
    Some(digits[digits.len() - SUFFIX_LEN..].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::FixtureBackup;
    use crate::{Backup, CacheDir};

    fn resolver_with(pairs: &[(&str, i64, &str)]) -> ContactResolver {
        // (value, property, name) built directly, to test matching in isolation.
        let mut r = ContactResolver {
            exact: HashMap::new(),
            suffix: HashMap::new(),
            availability: Some(Availability::Present { version_hint: None }),
        };
        for (value, property, name) in pairs {
            match property {
                3 => r.insert_phone(value, name),
                _ => {
                    r.exact
                        .insert(value.trim().to_ascii_lowercase(), (*name).to_string());
                }
            }
        }
        r
    }

    #[test]
    fn digits_are_extracted_from_any_formatting() {
        assert_eq!(digits_of("+1 (555) 000-1234"), "15550001234");
        assert_eq!(digits_of("+44 7700 900123"), "447700900123");
        assert_eq!(digits_of("no digits here"), "");
    }

    #[test]
    fn exact_numbers_resolve_regardless_of_formatting() {
        let r = resolver_with(&[("+1 (555) 000-1234", 3, "Ada Lovelace")]);
        for form in [
            "+15550001234",
            "+1 555 000 1234",
            "(555) 000-1234 ",
            "15550001234",
        ] {
            assert_eq!(
                r.display_name(form),
                Some("Ada Lovelace"),
                "failed for {form:?}"
            );
        }
    }

    #[test]
    fn a_missing_country_code_still_resolves_through_the_suffix_index() {
        // Address book has the international form; the message has the local one.
        let r = resolver_with(&[("+44 7700 900123", 3, "Grace Hopper")]);
        assert_eq!(r.display_name("07700900123"), Some("Grace Hopper"));
        assert_eq!(r.display_name("+447700900123"), Some("Grace Hopper"));
    }

    #[test]
    fn short_numbers_do_not_match_through_the_suffix_index() {
        // Only 5 digits: no suffix key, so a short code cannot be mistaken for
        // a contact whose number happens to end the same way.
        let r = resolver_with(&[("+15550001234", 3, "Ada Lovelace")]);
        assert_eq!(r.display_name("51234"), None);
        assert_eq!(suffix_key("1234"), None);
    }

    #[test]
    fn emails_resolve_case_insensitively() {
        let r = resolver_with(&[("Ada@Example.COM", 4, "Ada Lovelace")]);
        assert_eq!(r.display_name("ada@example.com"), Some("Ada Lovelace"));
        assert_eq!(r.display_name("ADA@EXAMPLE.COM"), Some("Ada Lovelace"));
        assert_eq!(r.display_name("someone@else.com"), None);
    }

    #[test]
    fn unknown_and_empty_handles_resolve_to_nothing() {
        let r = resolver_with(&[("+15550001234", 3, "Ada Lovelace")]);
        assert_eq!(r.display_name("+15559999999"), None);
        assert_eq!(r.display_name(""), None);
        assert_eq!(r.display_name("   "), None);
    }

    #[test]
    fn participants_record_whether_the_name_came_from_contacts() {
        let r = resolver_with(&[("+15550001234", 3, "Ada Lovelace")]);

        let known = r.participant("+15550001234");
        assert_eq!(known.display_name.as_deref(), Some("Ada Lovelace"));
        assert!(known.from_contacts);
        assert_eq!(known.label(), "Ada Lovelace");

        let unknown = r.participant("+15559999999");
        assert_eq!(unknown.display_name, None);
        assert!(!unknown.from_contacts);
        assert_eq!(unknown.label(), "+15559999999", "falls back to the number");
    }

    #[test]
    fn an_empty_resolver_resolves_nothing_without_failing() {
        let r = ContactResolver::empty();
        assert_eq!(r.display_name("+15550001234"), None);
        assert!(r.is_empty());
        assert_eq!(r.availability(), Availability::Absent);
    }

    #[test]
    fn a_backup_without_an_address_book_yields_an_empty_resolver() {
        let tmp = tempfile::tempdir().unwrap();
        let root = FixtureBackup::new()
            .with_file("HomeDomain", "a.txt", b"x")
            .build(tmp.path())
            .unwrap();
        let backup =
            Backup::open_with_cache(&root, None, CacheDir::at(tmp.path().join("cache"))).unwrap();

        let r = ContactResolver::open(&backup).unwrap();
        assert!(r.is_empty());
        assert_eq!(r.availability(), Availability::Absent);
    }

    #[test]
    fn a_real_address_book_is_read_including_fallback_names() {
        let tmp = tempfile::tempdir().unwrap();
        let root = FixtureBackup::new()
            .with_address_book()
            .build(tmp.path())
            .unwrap();
        let backup =
            Backup::open_with_cache(&root, None, CacheDir::at(tmp.path().join("cache"))).unwrap();

        let r = ContactResolver::open(&backup).unwrap();
        assert!(r.availability().is_present());

        // First + Last.
        assert_eq!(r.display_name("+15550001001"), Some("Ada Lovelace"));
        // Email on the same contact.
        assert_eq!(r.display_name("ada@example.com"), Some("Ada Lovelace"));
        // Organization only, no personal name.
        assert_eq!(r.display_name("+15550001003"), Some("Example Dental"));
        // Local form of an international number.
        assert_eq!(r.display_name("07700900123"), Some("Grace Hopper"));
        // Not in the book.
        assert_eq!(r.display_name("+15559999999"), None);
    }

    #[test]
    fn an_address_book_missing_its_tables_is_reported_not_fatal() {
        let tmp = tempfile::tempdir().unwrap();
        let root = FixtureBackup::new()
            .with_db(
                "HomeDomain",
                "Library/AddressBook/AddressBook.sqlitedb",
                "CREATE TABLE Unrelated (a INTEGER);",
            )
            .build(tmp.path())
            .unwrap();
        let backup =
            Backup::open_with_cache(&root, None, CacheDir::at(tmp.path().join("cache"))).unwrap();

        let r = ContactResolver::open(&backup).unwrap();
        assert!(matches!(r.availability(), Availability::Unsupported(_)));
        assert!(r.is_empty());
    }
}
