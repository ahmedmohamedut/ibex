//! HTML export: a conversation you can read in any browser, forever.
//!
//! One folder per conversation holding `index.html` and a `media/` directory,
//! plus a top-level `index.html` listing them all.
//!
//! Two rules make this safe and durable:
//!
//! - **No JavaScript.** It opens from a `file://` URL with no server and no
//!   runtime, so it will still work when nothing else about this program does.
//! - **Everything from the backup is escaped**, through the single
//!   [`escape_html`] below. Message bodies, contact names and file names are
//!   all attacker-chosen in the sense that whoever sent the message picked
//!   them, and this is the one place where getting that wrong turns a message
//!   into markup.

use std::fmt::Write as _;
use std::path::Path;

use crate::Backup;
use crate::error::{IoResultExt, Result};
use crate::export::naming::{self, NameAllocator};
use crate::export::{
    CancelToken, ChatSource, ConversationReport, ExportFormat, ExportOptions, ExportReport,
    Exporter, Progress, Selection, conversations_for,
};
use crate::extractors::{ConversationSummary, Message, MessageKind};
use crate::util::mime::PreviewKind;

/// Escape text for HTML.
///
/// Covers the five characters that matter in both element text and quoted
/// attribute values, so one function serves every context this exporter emits.
/// Deliberately not clever: no allow-list, no partial escaping.
pub fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

/// The export's stylesheet.
///
/// The values mirror `src/styles/tokens.css`; keep the two in step by hand,
/// since the export must not depend on the frontend build. Inlined so a
/// conversation folder is self-contained.
const STYLES: &str = r#"
:root {
  --bg: #ffffff; --bg-elev: #f6f6f7; --border: #e2e2e6;
  --text: #1c1c1e; --text-muted: #6b6b73;
  --bubble-me: #0a7cff; --bubble-me-text: #ffffff;
  --bubble-them: #e9e9eb; --bubble-them-text: #1c1c1e;
  --bubble-wa-me: #d9fdd3; --bubble-wa-me-text: #111b21;
  --warning: #b26a00; --radius-bubble: 18px;
}
@media (prefers-color-scheme: dark) {
  :root {
    --bg: #1c1c1e; --bg-elev: #2c2c2e; --border: #3a3a3c;
    --text: #f2f2f7; --text-muted: #9a9aa1;
    --bubble-them: #2c2c2e; --bubble-them-text: #f2f2f7;
    --bubble-wa-me: #005c4b; --bubble-wa-me-text: #e9edef;
  }
}
* { box-sizing: border-box; }
body {
  margin: 0; padding: 24px 16px; background: var(--bg); color: var(--text);
  font: 14px/1.45 system-ui, -apple-system, "Segoe UI", Cantarell, "Noto Sans", sans-serif;
}
.wrap { max-width: 760px; margin: 0 auto; }
h1 { font-size: 20px; margin: 0 0 4px; }
.meta { color: var(--text-muted); font-size: 12px; margin-bottom: 20px; }
.day {
  text-align: center; margin: 20px 0 10px; font-size: 11px; font-weight: 600;
  color: var(--text-muted);
}
.row { display: flex; flex-direction: column; margin-bottom: 8px; max-width: 76%; }
.row.me { align-self: flex-end; align-items: flex-end; margin-left: auto; }
.row.them { align-items: flex-start; }
.row.system { align-items: center; max-width: 100%; margin: 12px auto; }
.sender { font-size: 11px; font-weight: 600; color: var(--text-muted); margin: 0 8px 2px; }
.bubble {
  padding: 6px 11px; border-radius: var(--radius-bubble); white-space: pre-wrap;
  overflow-wrap: anywhere;
}
.me .bubble { background: var(--bubble-me); color: var(--bubble-me-text); }
.me.wa .bubble { background: var(--bubble-wa-me); color: var(--bubble-wa-me-text); }
.them .bubble { background: var(--bubble-them); color: var(--bubble-them-text); }
.system .bubble {
  background: none; color: var(--text-muted); font-size: 12px; text-align: center;
}
.unsent .bubble {
  background: none; border: 1px dashed var(--border); color: var(--text-muted);
  font-style: italic;
}
.time { font-size: 10px; color: var(--text-muted); margin: 2px 8px 0; }
.quote {
  border-left: 2px solid currentColor; padding-left: 6px; margin-bottom: 4px;
  font-size: 12px; opacity: 0.75;
}
.reactions { margin: -4px 8px 0; }
.reaction {
  display: inline-block; padding: 0 6px; border: 1px solid var(--border);
  border-radius: 999px; background: var(--bg); font-size: 11px;
}
.att { margin-top: 4px; }
.att img { display: block; max-width: 260px; max-height: 300px; border-radius: 10px; }
.att audio, .att video { max-width: 260px; }
.missing {
  display: inline-block; padding: 4px 8px; border: 1px dashed var(--border);
  border-radius: 8px; color: var(--text-muted); font-size: 12px;
}
.card {
  display: inline-block; padding: 4px 8px; border: 1px solid var(--border);
  border-radius: 8px; color: inherit;
}
ul.index { list-style: none; padding: 0; }
ul.index li { border-bottom: 1px solid var(--border); padding: 8px 0; }
ul.index a { color: inherit; text-decoration: none; font-weight: 600; }
ul.index .sub { color: var(--text-muted); font-size: 12px; font-weight: 400; }
.note {
  border-left: 3px solid var(--warning); padding: 6px 10px; margin: 16px 0;
  color: var(--text-muted); font-size: 12px;
}
"#;

/// Writes browsable HTML.
pub struct HtmlExporter;

impl Exporter for HtmlExporter {
    fn export(
        &self,
        backup: &Backup,
        sel: &Selection,
        out: &Path,
        opts: &ExportOptions,
        progress: &dyn Fn(Progress),
        cancel: &CancelToken,
    ) -> Result<ExportReport> {
        std::fs::create_dir_all(out).at(out)?;
        let source = ChatSource::open(backup, &sel.source)?;
        let conversations = conversations_for(&source, sel, opts)?;
        let mut report = ExportReport::new(ExportFormat::Html, source.id(), out);

        let total = conversations.len() as u64;
        let mut cancelled = false;
        let mut bytes = 0u64;
        let mut folders = NameAllocator::new();
        // (folder, conversation) for the top-level index.
        let mut written: Vec<(String, &ConversationSummary, u64)> = Vec::new();

        for (index, conversation) in conversations.iter().enumerate() {
            if cancel.is_cancelled() {
                cancelled = true;
                break;
            }
            progress(Progress {
                done: index as u64,
                total: Some(total),
                message: format!("Conversation {} of {total}", index + 1),
            });

            let folder_name = folders.allocate(&conversation.label());
            let folder = out.join(&folder_name);
            std::fs::create_dir_all(&folder).at(&folder)?;

            let messages: Vec<Message> = source
                .messages_chronological(&conversation.id)?
                .into_iter()
                .filter(|m| opts.in_range(m))
                .collect();

            // Copy media first, so the page can link to real files.
            let mut media_names: std::collections::HashMap<String, String> =
                std::collections::HashMap::new();
            let mut copied = 0u64;
            let mut missing = 0u64;
            if opts.include_attachments {
                let media_dir = folder.join("media");
                let mut names = NameAllocator::new();
                for message in &messages {
                    for attachment in &message.attachments {
                        if cancel.is_cancelled() {
                            cancelled = true;
                            break;
                        }
                        let Some(file_id) = attachment.file_id.as_deref() else {
                            missing += 1;
                            continue;
                        };
                        if media_names.contains_key(file_id) {
                            continue;
                        }
                        let Ok(Some(entry)) = backup.get(file_id) else {
                            missing += 1;
                            continue;
                        };
                        std::fs::create_dir_all(&media_dir).at(&media_dir)?;
                        let name = names.allocate(&attachment.file_name);
                        let dest = naming::join_safe(&media_dir, &name);
                        match backup.copy_file_to(&entry, &dest) {
                            Ok((n, _)) => {
                                bytes += n;
                                copied += 1;
                                media_names.insert(file_id.to_string(), name);
                            }
                            Err(_) => missing += 1,
                        }
                    }
                    if cancelled {
                        break;
                    }
                }
            } else {
                missing = messages
                    .iter()
                    .flat_map(|m| &m.attachments)
                    .filter(|a| !a.is_present())
                    .count() as u64;
            }

            let html = render_conversation(
                conversation,
                &messages,
                &media_names,
                source.id(),
                opts.include_attachments,
            );
            let path = folder.join("index.html");
            std::fs::write(&path, &html).at(&path)?;
            bytes += html.len() as u64;

            report.add(ConversationReport {
                id: conversation.id.clone(),
                output_name: folder_name.clone(),
                messages: messages.len() as u64,
                attachments_copied: copied,
                attachments_missing: missing,
            });
            written.push((folder_name, conversation, messages.len() as u64));
            if cancelled {
                break;
            }
        }

        let index = render_index(source.label(), &written, cancelled);
        let index_path = out.join("index.html");
        std::fs::write(&index_path, &index).at(&index_path)?;
        bytes += index.len() as u64;

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

/// The top-level index listing every exported conversation.
fn render_index(
    label: &str,
    conversations: &[(String, &ConversationSummary, u64)],
    cancelled: bool,
) -> String {
    let mut out = String::new();
    out.push_str(&page_head(&format!("{label} export")));
    let _ = write!(
        out,
        "<h1>{} export</h1><p class=\"meta\">{} conversation(s)</p>",
        escape_html(label),
        conversations.len()
    );
    if cancelled {
        out.push_str(
            "<p class=\"note\">This export was cancelled before it finished, so it is \
             incomplete.</p>",
        );
    }
    out.push_str("<ul class=\"index\">");
    for (folder, conversation, messages) in conversations {
        let range = match (conversation.last_message_at, messages) {
            (Some(last), _) => format!("last activity {}", last.format("%Y-%m-%d")),
            _ => String::new(),
        };
        let _ = write!(
            out,
            "<li><a href=\"{}/index.html\">{}</a><div class=\"sub\">{} message(s){}</div></li>",
            escape_html(folder),
            escape_html(&conversation.label()),
            messages,
            if range.is_empty() {
                String::new()
            } else {
                format!(" · {}", escape_html(&range))
            }
        );
    }
    out.push_str("</ul>");
    out.push_str(page_tail());
    out
}

/// One conversation's page.
fn render_conversation(
    conversation: &ConversationSummary,
    messages: &[Message],
    media: &std::collections::HashMap<String, String>,
    source_id: &str,
    include_attachments: bool,
) -> String {
    let wa = source_id == "whatsapp";
    let mut out = String::new();
    out.push_str(&page_head(&conversation.label()));
    let _ = write!(out, "<h1>{}</h1>", escape_html(&conversation.label()));

    let participants = conversation
        .participants
        .iter()
        .map(|p| escape_html(p.label()))
        .collect::<Vec<_>>()
        .join(", ");
    let _ = write!(
        out,
        "<p class=\"meta\">{} message(s){}{}</p>",
        messages.len(),
        if participants.is_empty() {
            String::new()
        } else {
            format!(" · {participants}")
        },
        match (
            messages.first().and_then(|m| m.sent_at),
            messages.last().and_then(|m| m.sent_at)
        ) {
            (Some(first), Some(last)) => format!(
                " · {} to {}",
                first.format("%Y-%m-%d"),
                last.format("%Y-%m-%d")
            ),
            _ => String::new(),
        }
    );

    if !include_attachments {
        out.push_str(
            "<p class=\"note\">Attachments were not included in this export, so media is \
             referenced by name only.</p>",
        );
    }

    // Reply quotes need to find their parent.
    let by_id: std::collections::HashMap<&str, &Message> =
        messages.iter().map(|m| (m.id.as_str(), m)).collect();

    let mut last_day = String::new();
    for message in messages {
        if let Some(sent_at) = message.sent_at {
            let day = sent_at.format("%Y-%m-%d").to_string();
            if day != last_day {
                let _ = write!(
                    out,
                    "<div class=\"day\">{}</div>",
                    escape_html(&sent_at.format("%A, %-d %B %Y").to_string())
                );
                last_day = day;
            }
        }
        out.push_str(&render_message(
            message,
            &by_id,
            media,
            wa,
            include_attachments,
        ));
    }

    out.push_str(page_tail());
    out
}

/// One message bubble.
fn render_message(
    message: &Message,
    by_id: &std::collections::HashMap<&str, &Message>,
    media: &std::collections::HashMap<String, String>,
    wa: bool,
    include_attachments: bool,
) -> String {
    let mut out = String::new();

    // A system event that is not an unsent message sits centred and muted.
    if message.kind == MessageKind::System && !message.unsent {
        let _ = write!(
            out,
            "<div class=\"row system\"><div class=\"bubble\">{}</div></div>",
            escape_html(message.body.as_deref().unwrap_or("System message"))
        );
        return out;
    }

    let side = if message.from_me { "me" } else { "them" };
    let classes = format!(
        "row {side}{}{}",
        if wa && message.from_me { " wa" } else { "" },
        if message.unsent { " unsent" } else { "" }
    );
    let _ = write!(out, "<div class=\"{classes}\">");

    if !message.from_me
        && let Some(sender) = &message.sender
    {
        let _ = write!(
            out,
            "<div class=\"sender\">{}</div>",
            escape_html(sender.label())
        );
    }

    out.push_str("<div class=\"bubble\">");

    if let Some(reply_to) = &message.reply_to {
        let quoted = by_id
            .get(reply_to.as_str())
            .and_then(|m| m.body.as_deref())
            .unwrap_or("earlier message");
        let _ = write!(out, "<div class=\"quote\">{}</div>", escape_html(quoted));
    }

    if message.unsent {
        out.push_str(if wa {
            "<em>Message deleted</em>"
        } else {
            "<em>Message unsent</em>"
        });
    } else if let Some(body) = &message.body {
        out.push_str(&escape_html(body));
    }

    for attachment in &message.attachments {
        let copied = attachment
            .file_id
            .as_deref()
            .and_then(|id| media.get(id))
            .filter(|_| include_attachments);
        let Some(name) = copied else {
            // Either not in the backup, or attachments were left out. Either
            // way the export says so rather than linking to nothing.
            let _ = write!(
                out,
                "<div class=\"att\"><span class=\"missing\">{} — {}</span></div>",
                escape_html(&attachment.file_name),
                if attachment.is_present() {
                    "not exported"
                } else {
                    "not in backup"
                }
            );
            continue;
        };
        let href = format!("media/{}", escape_html(name));
        let alt = escape_html(&attachment.file_name);
        let _ = match attachment.preview {
            Some(PreviewKind::Image) => write!(
                out,
                "<div class=\"att\"><a href=\"{href}\"><img src=\"{href}\" alt=\"{alt}\"></a></div>"
            ),
            Some(PreviewKind::Audio) => write!(
                out,
                "<div class=\"att\"><audio controls src=\"{href}\"></audio></div>"
            ),
            Some(PreviewKind::Video) => write!(
                out,
                "<div class=\"att\"><video controls src=\"{href}\"></video></div>"
            ),
            _ => write!(
                out,
                "<div class=\"att\"><a class=\"card\" href=\"{href}\">{alt}</a></div>"
            ),
        };
    }

    out.push_str("</div>");

    let visible: Vec<&crate::extractors::Reaction> =
        message.reactions.iter().filter(|r| !r.removed).collect();
    if !visible.is_empty() {
        out.push_str("<div class=\"reactions\">");
        for reaction in visible {
            let _ = write!(
                out,
                "<span class=\"reaction\">{}</span>",
                escape_html(&reaction_label(&reaction.kind))
            );
        }
        out.push_str("</div>");
    }

    if let Some(sent_at) = message.sent_at {
        let _ = write!(
            out,
            "<div class=\"time\">{}{}</div>",
            escape_html(&sent_at.format("%H:%M").to_string()),
            if message.edited { " · edited" } else { "" }
        );
    }

    out.push_str("</div>");
    out
}

/// A reaction as a short glyph.
fn reaction_label(kind: &crate::extractors::ReactionKind) -> String {
    use crate::extractors::ReactionKind as K;
    match kind {
        K::Loved => "❤️".to_string(),
        K::Liked => "👍".to_string(),
        K::Disliked => "👎".to_string(),
        K::Laughed => "😂".to_string(),
        K::Emphasized => "‼️".to_string(),
        K::Questioned => "❓".to_string(),
        K::Emoji(emoji) => emoji.clone(),
        K::Unknown(_) => "•".to_string(),
    }
}

fn page_head(title: &str) -> String {
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>{}</title>\n<style>{STYLES}</style>\n</head>\n<body>\n<div class=\"wrap\">\n",
        escape_html(title)
    )
}

fn page_tail() -> &'static str {
    "\n</div>\n</body>\n</html>\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escaping_covers_every_dangerous_character() {
        assert_eq!(
            escape_html("<script>alert('x')</script>"),
            "&lt;script&gt;alert(&#39;x&#39;)&lt;/script&gt;"
        );
        // Ampersands must be escaped first, or the others double-escape.
        assert_eq!(escape_html("a & b"), "a &amp; b");
        assert_eq!(escape_html("&lt;"), "&amp;lt;");
        // Quotes matter because the same function escapes attribute values.
        assert_eq!(escape_html(r#"say "hi""#), "say &quot;hi&quot;");
        assert_eq!(escape_html("it's"), "it&#39;s");
        // Ordinary text, including non-ASCII, is untouched.
        assert_eq!(escape_html("café 日本語 🙂"), "café 日本語 🙂");
        assert_eq!(escape_html(""), "");
    }

    #[test]
    fn an_attribute_injection_attempt_cannot_break_out() {
        // A file name chosen to close the attribute and add a handler.
        let hostile = r#"x" onerror="alert(1)"#;
        let escaped = escape_html(hostile);
        assert!(!escaped.contains('"'), "{escaped}");
        assert!(
            !escaped.contains("onerror=\""),
            "the handler must not survive as markup: {escaped}"
        );
    }

    #[test]
    fn reaction_labels_pass_unknown_codes_through_safely() {
        use crate::extractors::ReactionKind as K;
        assert_eq!(reaction_label(&K::Loved), "❤️");
        assert_eq!(reaction_label(&K::Emoji("🎉".into())), "🎉");
        assert_eq!(reaction_label(&K::Unknown(1234)), "•");
        // A hostile custom emoji is still escaped by the caller.
        let label = reaction_label(&K::Emoji("<img>".into()));
        assert_eq!(escape_html(&label), "&lt;img&gt;");
    }

    #[test]
    fn the_stylesheet_carries_no_javascript() {
        // The export's durability rests on there being no script anywhere.
        let lower = STYLES.to_lowercase();
        assert!(!lower.contains("<script"));
        assert!(!lower.contains("javascript:"));
        assert!(!lower.contains("expression("));
        // And it defines both themes.
        assert!(STYLES.contains("prefers-color-scheme"));
    }

    #[test]
    fn a_page_is_a_complete_standalone_document() {
        let head = page_head("Title");
        assert!(head.starts_with("<!doctype html>"));
        assert!(head.contains("<meta charset=\"utf-8\">"));
        assert!(head.contains("<style>"));
        assert!(page_tail().contains("</html>"));
        // The title is escaped like everything else.
        assert!(page_head("<b>").contains("&lt;b&gt;"));
    }
}
