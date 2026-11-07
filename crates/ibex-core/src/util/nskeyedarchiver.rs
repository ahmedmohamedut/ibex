//! Decoder for `NSKeyedArchiver` property lists.
//!
//! The `file` column of `Manifest.db` holds an archived `MBFile` object. An
//! archive is a flat `$objects` array plus `$top`, with every cross-reference
//! expressed as a UID index into that array:
//!
//! ```text
//! { "$archiver": "NSKeyedArchiver",
//!   "$top":     { "root": Uid(1) },
//!   "$objects": [ "$null",
//!                 { "$class": Uid(3), "Size": 1024, "EncryptionKey": Uid(2) },
//!                 { "$class": Uid(4), "NS.data": <data> },
//!                 ... ] }
//! ```
//!
//! [`resolve`] inlines those references generically — by following UIDs, never
//! by assuming an object sits at a particular index — and unwraps the standard
//! `NS.*` container encodings so callers see plain plist values.

use std::collections::HashSet;

use plist::{Dictionary, Value};

use crate::error::{BackupError, Result};

/// How deep a reference chain may go before we stop following it.
const MAX_DEPTH: usize = 64;

/// Parse an archived plist (binary or XML) and inline its UID references.
///
/// A plist that is *not* an archive is returned unchanged, so this is safe to
/// call on any plist blob.
pub fn resolve_bytes(bytes: &[u8]) -> Result<Value> {
    let value =
        Value::from_reader(std::io::Cursor::new(bytes)).map_err(|source| BackupError::Plist {
            file: "NSKeyedArchiver blob".to_string(),
            source,
        })?;
    resolve(value)
}

/// Inline the UID references in an already-parsed archive.
pub fn resolve(value: Value) -> Result<Value> {
    let Some(dict) = value.as_dictionary() else {
        return Ok(value);
    };
    let Some(objects) = dict.get("$objects").and_then(Value::as_array) else {
        // Not an archive. Hand it back as-is.
        return Ok(value);
    };

    let top = dict
        .get("$top")
        .and_then(Value::as_dictionary)
        .ok_or_else(|| BackupError::MalformedMetadata {
            file: "NSKeyedArchiver blob".to_string(),
            detail: "$objects present but $top is missing or not a dictionary".to_string(),
        })?;

    let mut seen = HashSet::new();

    // The common case is a single "root"; archives with several top-level keys
    // keep all of them.
    if let Some(root) = top.get("root") {
        return Ok(
            walk(root, objects, &mut seen, 0).unwrap_or(Value::Dictionary(Dictionary::new()))
        );
    }

    let mut out = Dictionary::new();
    for (key, child) in top {
        if let Some(resolved) = walk(child, objects, &mut seen, 0) {
            out.insert(key.clone(), resolved);
        }
    }
    Ok(Value::Dictionary(out))
}

/// Resolve one value against the `$objects` table.
///
/// Returns `None` for anything that represents nil: the `$null` placeholder, a
/// reference that points outside the table, a cycle, or a chain deeper than
/// [`MAX_DEPTH`]. Callers drop the key rather than inventing a value, because
/// plist has no null type.
fn walk(value: &Value, objects: &[Value], seen: &mut HashSet<u64>, depth: usize) -> Option<Value> {
    if depth > MAX_DEPTH {
        return None;
    }

    match value {
        // Binary plists carry real UID values.
        Value::Uid(uid) => follow(uid.get(), objects, seen, depth),
        Value::Dictionary(dict) => {
            // XML archives spell a UID as { "CF$UID": n }.
            if dict.len() == 1
                && let Some(n) = dict.get("CF$UID").and_then(Value::as_unsigned_integer)
            {
                return follow(n, objects, seen, depth);
            }
            Some(resolve_dictionary(dict, objects, seen, depth))
        }
        Value::Array(items) => Some(Value::Array(
            items
                .iter()
                .filter_map(|item| walk(item, objects, seen, depth + 1))
                .collect(),
        )),
        // `$objects[0]` is the string "$null", meaning nil.
        Value::String(s) if s == "$null" => None,
        other => Some(other.clone()),
    }
}

/// Follow a UID into `$objects`, guarding against cycles.
fn follow(index: u64, objects: &[Value], seen: &mut HashSet<u64>, depth: usize) -> Option<Value> {
    let target = objects.get(usize::try_from(index).ok()?)?;
    if !seen.insert(index) {
        // Already on the current resolution path: a cycle.
        return None;
    }
    let resolved = walk(target, objects, seen, depth + 1);
    seen.remove(&index);
    resolved
}

/// Resolve a dictionary's values, unwrapping the standard `NS.*` encodings that
/// Foundation uses for its container classes.
fn resolve_dictionary(
    dict: &Dictionary,
    objects: &[Value],
    seen: &mut HashSet<u64>,
    depth: usize,
) -> Value {
    // NSData / NSMutableData, NSString / NSMutableString, NSDate: each of
    // these wraps its payload under a single well-known key.
    for key in ["NS.data", "NS.string", "NS.time"] {
        if let Some(inner) = dict.get(key)
            && let Some(resolved) = walk(inner, objects, seen, depth + 1)
        {
            return resolved;
        }
    }

    let keys = dict.get("NS.keys").and_then(Value::as_array);
    let values = dict.get("NS.objects").and_then(Value::as_array);

    match (keys, values) {
        // NSDictionary: parallel key and value arrays.
        (Some(keys), Some(values)) => {
            let mut out = Dictionary::new();
            for (key, value) in keys.iter().zip(values) {
                let Some(key) = walk(key, objects, seen, depth + 1) else {
                    continue;
                };
                // Non-string keys can't be represented in a plist dictionary;
                // fall back to their debug-free string form.
                let key = match key {
                    Value::String(s) => s,
                    Value::Integer(i) => i.to_string(),
                    _ => continue,
                };
                if let Some(value) = walk(value, objects, seen, depth + 1) {
                    out.insert(key, value);
                }
            }
            Value::Dictionary(out)
        }
        // NSArray / NSSet.
        (None, Some(values)) => Value::Array(
            values
                .iter()
                .filter_map(|v| walk(v, objects, seen, depth + 1))
                .collect(),
        ),
        // A plain archived object: resolve every field, and reduce `$class` to
        // its class name so the output stays readable.
        _ => {
            let mut out = Dictionary::new();
            for (key, value) in dict {
                if key == "$class" {
                    if let Some(name) = walk(value, objects, seen, depth + 1)
                        .as_ref()
                        .and_then(Value::as_dictionary)
                        .and_then(|c| c.get("$classname"))
                        .and_then(Value::as_string)
                    {
                        out.insert("$class".to_string(), Value::String(name.to_string()));
                    }
                    continue;
                }
                if let Some(resolved) = walk(value, objects, seen, depth + 1) {
                    out.insert(key.clone(), resolved);
                }
            }
            Value::Dictionary(out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build `{ "CF$UID": n }`, the XML spelling of a UID.
    fn uid(n: u64) -> Value {
        let mut d = Dictionary::new();
        d.insert("CF$UID".to_string(), Value::Integer(n.into()));
        Value::Dictionary(d)
    }

    fn dict(pairs: Vec<(&str, Value)>) -> Value {
        let mut d = Dictionary::new();
        for (k, v) in pairs {
            d.insert(k.to_string(), v);
        }
        Value::Dictionary(d)
    }

    fn archive(objects: Vec<Value>) -> Value {
        dict(vec![
            ("$archiver", Value::String("NSKeyedArchiver".into())),
            ("$version", Value::Integer(100_000.into())),
            ("$top", dict(vec![("root", uid(1))])),
            ("$objects", Value::Array(objects)),
        ])
    }

    #[test]
    fn resolves_nested_references() {
        let archived = archive(vec![
            Value::String("$null".into()),
            dict(vec![
                ("$class", uid(4)),
                ("Size", Value::Integer(1024.into())),
                ("RelativePath", uid(2)),
                ("EncryptionKey", uid(3)),
            ]),
            Value::String("Library/SMS/sms.db".into()),
            dict(vec![
                ("$class", uid(5)),
                ("NS.data", Value::Data(vec![1, 2, 3])),
            ]),
            dict(vec![("$classname", Value::String("MBFile".into()))]),
            dict(vec![("$classname", Value::String("NSMutableData".into()))]),
        ]);

        let resolved = resolve(archived).unwrap();
        let d = resolved.as_dictionary().unwrap();

        assert_eq!(d.get("Size").unwrap().as_signed_integer(), Some(1024));
        assert_eq!(
            d.get("RelativePath").unwrap().as_string(),
            Some("Library/SMS/sms.db")
        );
        // NSMutableData unwrapped to the bytes themselves.
        assert_eq!(
            d.get("EncryptionKey").unwrap().as_data(),
            Some(&[1, 2, 3][..])
        );
        assert_eq!(d.get("$class").unwrap().as_string(), Some("MBFile"));
    }

    #[test]
    fn null_references_are_dropped() {
        let archived = archive(vec![
            Value::String("$null".into()),
            dict(vec![
                ("Size", Value::Integer(7.into())),
                // Points at $objects[0], i.e. nil.
                ("EncryptionKey", uid(0)),
            ]),
        ]);
        let resolved = resolve(archived).unwrap();
        let d = resolved.as_dictionary().unwrap();
        assert_eq!(d.get("Size").unwrap().as_signed_integer(), Some(7));
        assert!(
            !d.contains_key("EncryptionKey"),
            "a nil reference should leave the key out, not fabricate a value"
        );
    }

    #[test]
    fn a_cycle_does_not_hang() {
        // $objects[1] refers to [2], which refers back to [1].
        let archived = archive(vec![
            Value::String("$null".into()),
            dict(vec![("name", Value::String("a".into())), ("next", uid(2))]),
            dict(vec![("name", Value::String("b".into())), ("prev", uid(1))]),
        ]);
        let resolved = resolve(archived).unwrap();
        let d = resolved.as_dictionary().unwrap();
        assert_eq!(d.get("name").unwrap().as_string(), Some("a"));
        let next = d.get("next").unwrap().as_dictionary().unwrap();
        assert_eq!(next.get("name").unwrap().as_string(), Some("b"));
        // The back-reference is cut rather than followed forever.
        assert!(!next.contains_key("prev"));
    }

    #[test]
    fn out_of_range_reference_is_dropped() {
        let archived = archive(vec![
            Value::String("$null".into()),
            dict(vec![("dangling", uid(99))]),
        ]);
        let resolved = resolve(archived).unwrap();
        assert!(!resolved.as_dictionary().unwrap().contains_key("dangling"));
    }

    #[test]
    fn unwraps_ns_array_and_dictionary() {
        let archived = archive(vec![
            Value::String("$null".into()),
            dict(vec![("list", uid(2)), ("map", uid(3))]),
            dict(vec![(
                "NS.objects",
                Value::Array(vec![Value::Integer(1.into()), Value::Integer(2.into())]),
            )]),
            dict(vec![
                ("NS.keys", Value::Array(vec![Value::String("k".into())])),
                ("NS.objects", Value::Array(vec![Value::String("v".into())])),
            ]),
        ]);
        let resolved = resolve(archived).unwrap();
        let d = resolved.as_dictionary().unwrap();
        assert_eq!(d.get("list").unwrap().as_array().unwrap().len(), 2);
        assert_eq!(
            d.get("map")
                .unwrap()
                .as_dictionary()
                .unwrap()
                .get("k")
                .unwrap()
                .as_string(),
            Some("v")
        );
    }

    #[test]
    fn a_plain_plist_passes_through() {
        let plain = dict(vec![("Size", Value::Integer(5.into()))]);
        let resolved = resolve(plain.clone()).unwrap();
        assert_eq!(resolved, plain);
    }

    #[test]
    fn objects_without_top_is_an_error() {
        let bad = dict(vec![("$objects", Value::Array(vec![]))]);
        assert!(resolve(bad).is_err());
    }

    #[test]
    fn shared_references_resolve_more_than_once() {
        // Both fields point at the same object: the cycle guard must not treat
        // the second visit as a cycle.
        let archived = archive(vec![
            Value::String("$null".into()),
            dict(vec![("a", uid(2)), ("b", uid(2))]),
            Value::String("shared".into()),
        ]);
        let resolved = resolve(archived).unwrap();
        let d = resolved.as_dictionary().unwrap();
        assert_eq!(d.get("a").unwrap().as_string(), Some("shared"));
        assert_eq!(d.get("b").unwrap().as_string(), Some("shared"));
    }
}
