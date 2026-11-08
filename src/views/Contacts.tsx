import { Mail, Phone as PhoneIcon, Share, User } from "lucide-react";
import { useMemo, useState } from "react";
import { useContacts } from "../api/contacts";
import { EmptyState, Spinner } from "../components/EmptyState";
import { ExportDialog, type ExportScope } from "../components/ExportDialog";
import { formatCount } from "../components/format";
import { useUi } from "../store/ui";

/** Initials for the avatar, mirroring the Rust side's rule. */
function initials(label: string): string {
  const out = label
    .split(/\s+/)
    .slice(0, 2)
    .map((word) => [...word].find((c) => /[\p{L}\p{N}]/u.test(c)) ?? "")
    .join("")
    .toUpperCase();
  return out || "?";
}

export function Contacts() {
  const backup = useUi((s) => s.backup);
  const [selected, setSelected] = useState<string | null>(null);
  const [filter, setFilter] = useState("");
  const [exporting, setExporting] = useState<ExportScope | null>(null);

  const contacts = useContacts(backup?.handle);

  const items = useMemo(() => {
    const all = contacts.data?.items ?? [];
    const needle = filter.trim().toLowerCase();
    if (!needle) return all;
    return all.filter(
      (c) =>
        (c.displayName ?? "").toLowerCase().includes(needle) ||
        (c.organization ?? "").toLowerCase().includes(needle),
    );
  }, [contacts.data, filter]);

  const current = contacts.data?.items.find((c) => c.id === selected);

  if (!backup) return null;

  return (
    <div className="panes">
      {exporting ? (
        <ExportDialog
          handle={backup.handle}
          source="contacts"
          scope={exporting}
          messageCount={exporting.kind === "contacts" ? exporting.contactIds?.length : undefined}
          onClose={() => setExporting(null)}
        />
      ) : null}
      <div className="pane pane--list">
        <div className="pane__header">
          <input
            className="input"
            style={{ flex: 1 }}
            placeholder="Filter contacts…"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            aria-label="Filter contacts"
          />
          <button
            type="button"
            className="button"
            onClick={() => setExporting({ kind: "contacts", label: "every contact" })}
            title="Export every contact as vCard"
          >
            <Share size={14} aria-hidden="true" /> Export all
          </button>
        </div>

        {contacts.isPending ? (
          <Spinner label="Reading contacts…" />
        ) : contacts.isError ? (
          <EmptyState title="Could not read Contacts" detail={String(contacts.error)} />
        ) : items.length === 0 ? (
          <EmptyState
            title={filter ? "Nothing matches" : "No contacts"}
            detail={
              filter ? "Try a different filter." : "This backup's address book is empty."
            }
          />
        ) : (
          <div className="pane__body">
            {items.map((contact) => (
              <button
                key={contact.id}
                type="button"
                className="conv-row"
                aria-selected={contact.id === selected}
                onClick={() => setSelected(contact.id)}
              >
                <span className="avatar" aria-hidden="true">
                  {initials(contact.displayName ?? "?")}
                </span>
                <span className="conv-row__main">
                  <span className="conv-row__name" dir="auto">
                    {contact.displayName ?? "Unnamed contact"}
                  </span>
                  <span className="conv-row__preview" dir="auto">
                    {contact.organization ?? contact.phones[0] ?? contact.emails[0] ?? ""}
                  </span>
                </span>
              </button>
            ))}
            <div style={{ padding: "8px 10px", color: "var(--text-muted)", fontSize: 11 }}>
              {formatCount(contacts.data?.total ?? items.length)} contacts
            </div>
          </div>
        )}
      </div>

      <div className="pane">
        {!selected || !current ? (
          <EmptyState
            title="Pick a contact"
            detail="Select someone from the list to see their details."
          />
        ) : (
          <>
            <div className="pane__header">
              <User size={14} aria-hidden="true" />
              <span className="pane__title" title={current.displayName ?? undefined}>
                {current.displayName ?? "Unnamed contact"}
              </span>
              <span className="topbar__spacer" />
              <button
                type="button"
                className="button"
                onClick={() =>
                  setExporting({
                    kind: "contacts",
                    label: current.displayName ?? "this contact",
                    contactIds: [current.id],
                  })
                }
                title="Export this contact as vCard"
              >
                <Share size={14} aria-hidden="true" /> Export vCard
              </button>
            </div>
            <div className="pane__body" style={{ padding: 16 }}>
              {current.organization ? (
                <p style={{ marginTop: 0 }}>{current.organization}</p>
              ) : null}
              {current.nickname ? (
                <p style={{ color: "var(--text-muted)" }}>&ldquo;{current.nickname}&rdquo;</p>
              ) : null}
              {current.phones.length > 0 ? (
                <div style={{ marginBottom: 12 }}>
                  <h3
                    style={{
                      fontSize: 11,
                      textTransform: "uppercase",
                      color: "var(--text-muted)",
                      margin: "0 0 4px",
                    }}
                  >
                    Phone
                  </h3>
                  {current.phones.map((phone) => (
                    <div
                      key={phone}
                      style={{ display: "flex", alignItems: "center", gap: 6, padding: "2px 0" }}
                    >
                      <PhoneIcon size={12} aria-hidden="true" /> {phone}
                    </div>
                  ))}
                </div>
              ) : null}
              {current.emails.length > 0 ? (
                <div>
                  <h3
                    style={{
                      fontSize: 11,
                      textTransform: "uppercase",
                      color: "var(--text-muted)",
                      margin: "0 0 4px",
                    }}
                  >
                    Email
                  </h3>
                  {current.emails.map((email) => (
                    <div
                      key={email}
                      style={{ display: "flex", alignItems: "center", gap: 6, padding: "2px 0" }}
                    >
                      <Mail size={12} aria-hidden="true" /> {email}
                    </div>
                  ))}
                </div>
              ) : null}
              {current.phones.length === 0 && current.emails.length === 0 ? (
                <p style={{ color: "var(--text-muted)" }}>No phone number or email on file.</p>
              ) : null}
            </div>
          </>
        )}
      </div>
    </div>
  );
}
