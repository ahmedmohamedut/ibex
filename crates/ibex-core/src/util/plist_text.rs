//! Rendering property lists as readable text.
//!
//! A backup is full of binary plists, which are unreadable as bytes. Converting
//! to XML gives the Raw browser (and the Files view later) something to show,
//! and goes through the `plist` dependency we already have.

use crate::error::{BackupError, Result};

/// Convert a property list — binary or XML — into pretty-printed XML.
pub fn to_xml(bytes: &[u8]) -> Result<String> {
    let value = plist::Value::from_reader(std::io::Cursor::new(bytes)).map_err(|source| {
        BackupError::Plist {
            file: "plist preview".to_string(),
            source,
        }
    })?;

    let mut out = Vec::new();
    value
        .to_writer_xml(&mut out)
        .map_err(|source| BackupError::Plist {
            file: "plist preview".to_string(),
            source,
        })?;

    // `to_writer_xml` emits UTF-8; a plist cannot contain invalid sequences
    // once parsed, so this is lossless in practice.
    Ok(String::from_utf8_lossy(&out).into_owned())
}

/// Convert if it parses, otherwise fall back to the raw bytes as text.
///
/// Used by the preview path, where a plist that will not parse should still show
/// something rather than an error.
pub fn to_xml_or_text(bytes: &[u8]) -> (String, bool) {
    match to_xml(bytes) {
        Ok(xml) => (xml, true),
        Err(_) => (String::from_utf8_lossy(bytes).into_owned(), false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use plist::{Dictionary, Value};

    fn sample() -> Value {
        let mut dict = Dictionary::new();
        dict.insert("Name".into(), Value::String("fixture".into()));
        dict.insert("Count".into(), Value::Integer(3.into()));
        dict.insert("Enabled".into(), Value::Boolean(true));
        dict.insert(
            "Items".into(),
            Value::Array(vec![Value::String("a".into()), Value::String("b".into())]),
        );
        Value::Dictionary(dict)
    }

    #[test]
    fn binary_plists_become_readable_xml() {
        let mut binary = Vec::new();
        sample().to_writer_binary(&mut binary).unwrap();
        assert!(binary.starts_with(b"bplist"));

        let xml = to_xml(&binary).unwrap();
        assert!(xml.contains("<key>Name</key>"), "got: {xml}");
        assert!(xml.contains("<string>fixture</string>"));
        assert!(xml.contains("<integer>3</integer>"));
        assert!(xml.contains("<true"));
    }

    #[test]
    fn xml_plists_pass_through() {
        let mut original = Vec::new();
        sample().to_writer_xml(&mut original).unwrap();
        let xml = to_xml(&original).unwrap();
        assert!(xml.contains("<key>Count</key>"));
    }

    #[test]
    fn nonsense_is_an_error() {
        assert!(to_xml(b"this is not a plist").is_err());
    }

    #[test]
    fn the_fallback_shows_something_regardless() {
        let (text, parsed) = to_xml_or_text(b"not a plist, just words");
        assert!(!parsed);
        assert_eq!(text, "not a plist, just words");

        let mut binary = Vec::new();
        sample().to_writer_binary(&mut binary).unwrap();
        let (text, parsed) = to_xml_or_text(&binary);
        assert!(parsed);
        assert!(text.contains("<plist"));
    }
}
