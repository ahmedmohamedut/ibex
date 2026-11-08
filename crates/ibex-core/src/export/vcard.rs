//! Exporting Contacts: one `.vcf` (vCard 3.0, RFC 2426) file per contact.
//!
//! One shape, like Photos/Voice/Documents — see `export::run`'s dispatch.

use crate::Backup;
use crate::error::{IoResultExt, Result};
use crate::export::naming::{self, NameAllocator};
use crate::export::{
    CancelToken, ConversationReport, ExportFormat, ExportOptions, ExportReport, Exporter, Progress,
    Selection,
};
use crate::extractors::{Contact, ContactsDb};
use crate::page::Page;
use std::path::Path;

/// Copies every selected contact out as `<display name>.vcf`.
pub struct ContactsExporter;

impl Exporter for ContactsExporter {
    fn export(
        &self,
        backup: &Backup,
        sel: &Selection,
        out: &Path,
        _opts: &ExportOptions,
        progress: &dyn Fn(Progress),
        cancel: &CancelToken,
    ) -> Result<ExportReport> {
        std::fs::create_dir_all(out).at(out)?;
        let db = ContactsDb::open(backup)?;
        let contacts = selected_contacts(&db, sel)?;

        let mut report = ExportReport::new(ExportFormat::Files, "contacts", out);
        let total = contacts.len() as u64;
        let mut cancelled = false;
        let mut bytes = 0u64;
        let mut written = 0u64;
        let mut names = NameAllocator::new();

        for (index, contact) in contacts.iter().enumerate() {
            if cancel.is_cancelled() {
                cancelled = true;
                break;
            }
            progress(Progress {
                done: index as u64,
                total: Some(total),
                message: format!("Contact {} of {total}", index + 1),
            });

            let label = contact
                .display_name
                .clone()
                .unwrap_or_else(|| "Unnamed contact".to_string());
            let base = naming::safe_component(&format!("{label}.vcf"));
            let allocated = names.allocate(&base);
            let dest = naming::join_safe(out, &allocated);

            let vcard = render_vcard(contact);
            std::fs::write(&dest, vcard.as_bytes()).at(&dest)?;
            bytes += vcard.len() as u64;
            written += 1;
        }

        report.add(ConversationReport {
            id: "contacts".to_string(),
            output_name: out.display().to_string(),
            messages: written,
            attachments_copied: written,
            attachments_missing: 0,
        });
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

fn selected_contacts(db: &ContactsDb<'_>, sel: &Selection) -> Result<Vec<Contact>> {
    let mut all = Vec::new();
    let mut offset = 0u64;
    loop {
        let page = db.list(Page::new(offset, Page::MAX_LIMIT))?;
        let count = page.items.len() as u64;
        all.extend(page.items);
        match page.next_offset {
            Some(next) if count > 0 => offset = next,
            _ => break,
        }
    }

    if !sel.conversation_ids.is_empty() {
        let wanted: std::collections::HashSet<&String> = sel.conversation_ids.iter().collect();
        all.retain(|c| wanted.contains(&c.id));
    }
    Ok(all)
}

/// Render one contact as a vCard 3.0 record.
fn render_vcard(contact: &Contact) -> String {
    let mut out = String::new();
    out.push_str("BEGIN:VCARD\r\n");
    out.push_str("VERSION:3.0\r\n");
    let fn_value = contact
        .display_name
        .clone()
        .unwrap_or_else(|| "Unnamed contact".to_string());
    out.push_str(&format!("FN:{}\r\n", escape(&fn_value)));
    out.push_str(&format!(
        "N:{};{};;;\r\n",
        escape(contact.last_name.as_deref().unwrap_or("")),
        escape(contact.first_name.as_deref().unwrap_or("")),
    ));
    if let Some(org) = &contact.organization {
        out.push_str(&format!("ORG:{}\r\n", escape(org)));
    }
    if let Some(nickname) = &contact.nickname {
        out.push_str(&format!("NICKNAME:{}\r\n", escape(nickname)));
    }
    for phone in &contact.phones {
        out.push_str(&format!("TEL;TYPE=CELL:{}\r\n", escape(phone)));
    }
    for email in &contact.emails {
        out.push_str(&format!("EMAIL:{}\r\n", escape(email)));
    }
    out.push_str("END:VCARD\r\n");
    out
}

/// Escape `,;\` and drop/fold newlines, per RFC 2426 §5.
fn escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace(',', "\\,")
        .replace(';', "\\;")
        .replace('\n', "\\n")
        .replace('\r', "")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::FixtureBackup;
    use crate::{CacheDir, Selection};

    fn open_fixture(tmp: &std::path::Path) -> Backup {
        let root = FixtureBackup::new().with_address_book().build(tmp).unwrap();
        Backup::open_with_cache(&root, None, CacheDir::at(tmp.join("cache"))).unwrap()
    }

    #[test]
    fn vcard_escapes_special_characters() {
        let contact = Contact {
            id: "1".to_string(),
            display_name: Some("A, B; C\\D".to_string()),
            first_name: Some("A, B; C\\D".to_string()),
            last_name: None,
            nickname: None,
            organization: None,
            phones: vec![],
            emails: vec![],
        };
        let vcard = render_vcard(&contact);
        assert!(vcard.contains("FN:A\\, B\\; C\\\\D\r\n"), "got {vcard:?}");
        assert!(
            !vcard.contains("A, B; C\\D\r\n"),
            "unescaped text leaked through"
        );
    }

    #[test]
    fn exporting_every_contact_writes_one_vcf_file_each() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let out = tmp.path().join("out");

        let report = ContactsExporter
            .export(
                &backup,
                &Selection::all("contacts"),
                &out,
                &ExportOptions::default(),
                &|_| {},
                &CancelToken::new(),
            )
            .unwrap();

        assert_eq!(report.totals.conversations, 1);
        assert_eq!(report.totals.messages, 5, "five contacts in the fixture");
        assert!(!report.cancelled);

        let files: Vec<_> = std::fs::read_dir(&out)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.ends_with(".vcf"))
            .collect();
        assert_eq!(files.len(), 5, "got {files:?}");
        assert!(
            files.contains(&"Alan Turing.vcf".to_string()),
            "got {files:?}"
        );
        let turing = std::fs::read_to_string(out.join("Alan Turing.vcf")).unwrap();
        assert!(turing.contains("ORG:Bletchley Park"));
        assert!(turing.contains("TEL;TYPE=CELL:+1 555 000 1004"));
    }

    #[test]
    fn a_single_contact_can_be_selected() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let out = tmp.path().join("out");
        let db = crate::extractors::ContactsDb::open(&backup).unwrap();
        let turing_id = db
            .list(Page::default())
            .unwrap()
            .items
            .into_iter()
            .find(|c| c.last_name.as_deref() == Some("Turing"))
            .unwrap()
            .id;

        let report = ContactsExporter
            .export(
                &backup,
                &Selection::one("contacts", &turing_id),
                &out,
                &ExportOptions::default(),
                &|_| {},
                &CancelToken::new(),
            )
            .unwrap();
        assert_eq!(report.totals.messages, 1);
    }
}
