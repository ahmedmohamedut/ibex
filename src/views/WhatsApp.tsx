import { MessageCircle, Users } from "lucide-react";
import { useMemo, useState } from "react";
import { useWaChats, useWaMessages } from "../api/whatsapp";
import type { ConversationSummary } from "../bindings/ConversationSummary";
import { ChatThread } from "../components/chat/ChatThread";
import { EmptyState, Spinner } from "../components/EmptyState";
import { ExportDialog, type ExportScope } from "../components/ExportDialog";
import { formatCount, formatDate } from "../components/format";
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

function label(chat: ConversationSummary): string {
  if (chat.displayName && chat.displayName.trim()) return chat.displayName;
  if (chat.participants.length === 0) return "Unknown";
  return chat.participants.map((p) => p.displayName ?? p.id).join(", ");
}

export function WhatsApp() {
  const backup = useUi((s) => s.backup);
  const [selected, setSelected] = useState<string | null>(null);
  const [filter, setFilter] = useState("");
  const [exporting, setExporting] = useState<ExportScope | null>(null);
  /** Status, channel and broadcast feeds are hidden until asked for. */
  const [includeFeeds, setIncludeFeeds] = useState(false);

  const chats = useWaChats(backup?.handle, includeFeeds);
  const thread = useWaMessages(backup?.handle, selected);

  const items = useMemo(() => {
    const all = chats.data?.items ?? [];
    const needle = filter.trim().toLowerCase();
    if (!needle) return all;
    return all.filter((c) => label(c).toLowerCase().includes(needle));
  }, [chats.data, filter]);

  const current = chats.data?.items.find((c) => c.id === selected);
  const messages = useMemo(
    () => thread.data?.pages.flatMap((p) => p.messages) ?? [],
    [thread.data],
  );

  if (!backup) return null;

  return (
    <div className="panes">
      {exporting ? (
        <ExportDialog
          handle={backup.handle}
          source="whatsapp"
          scope={exporting}
          messageCount={
            exporting.kind === "conversation"
              ? Number(current?.messageCount ?? 0)
              : undefined
          }
          onClose={() => setExporting(null)}
        />
      ) : null}
      <div className="pane pane--list">
        <div className="pane__header">
          <input
            className="input"
            style={{ flex: 1 }}
            placeholder="Filter chats…"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            aria-label="Filter chats"
          />
        </div>
        <div className="pane__header" style={{ background: "var(--bg)" }}>
          <label style={{ display: "flex", alignItems: "center", gap: 6, fontSize: 12 }}>
            <input
              type="checkbox"
              checked={includeFeeds}
              onChange={(e) => setIncludeFeeds(e.target.checked)}
            />
            Show status and channels
          </label>
          <span className="topbar__spacer" />
          <span style={{ color: "var(--text-muted)", fontSize: 11 }}>
            {chats.data ? formatCount(chats.data.total ?? items.length) : ""}
          </span>
        </div>

        {chats.isPending ? (
          <Spinner label="Reading chats…" />
        ) : chats.isError ? (
          <EmptyState title="Could not read WhatsApp" detail={String(chats.error)} />
        ) : items.length === 0 ? (
          <EmptyState
            title={filter ? "Nothing matches" : "No chats"}
            detail={
              filter
                ? "Try a different filter."
                : "This backup's ChatStorage.sqlite has no chats in it."
            }
          />
        ) : (
          <div className="pane__body">
            {items.map((chat) => (
              <button
                key={chat.id}
                type="button"
                className="conv-row"
                aria-selected={chat.id === selected}
                onClick={() => setSelected(chat.id)}
              >
                <span className="avatar" aria-hidden="true">
                  {initials(label(chat))}
                </span>
                <span className="conv-row__main">
                  <span className="conv-row__name">
                    {chat.isGroup ? (
                      <Users size={11} aria-hidden="true" style={{ marginRight: 4 }} />
                    ) : null}
                    {label(chat)}
                  </span>
                  <span className="conv-row__preview">
                    {chat.lastMessagePreview ?? "No messages"}
                  </span>
                </span>
                <span className="conv-row__side">
                  {formatDate(chat.lastMessageAt)}
                  <br />
                  {formatCount(chat.messageCount)}
                </span>
              </button>
            ))}
          </div>
        )}
      </div>

      <div className="pane">
        {!selected || !current ? (
          <EmptyState
            title="Pick a chat"
            detail="Threads open at the newest message; scroll up for older ones."
          />
        ) : (
          <>
            <div className="pane__header">
              <MessageCircle size={14} aria-hidden="true" />
              <span className="pane__title" title={label(current)}>
                {label(current)}
              </span>
              <span className="topbar__spacer" />
              <span style={{ color: "var(--text-muted)", fontSize: 11 }}>
                {formatCount(current.messageCount)} messages
                {current.isGroup
                  ? ` · ${formatCount(current.participants.length)} participants`
                  : ""}
              </span>
              <button
                type="button"
                className="button"
                onClick={() =>
                  setExporting({
                    kind: "conversation",
                    id: current.id,
                    label: label(current),
                  })
                }
                title="Export this conversation"
              >
                Export
              </button>
            </div>

            {thread.isPending ? (
              <Spinner label="Reading messages…" />
            ) : thread.isError ? (
              <EmptyState title="Could not read this chat" detail={String(thread.error)} />
            ) : (
              <ChatThread
                handle={backup.handle}
                messages={messages}
                isGroup={current.isGroup}
                hasOlder={Boolean(thread.hasNextPage)}
                isLoadingOlder={thread.isFetchingNextPage}
                onLoadOlder={() => void thread.fetchNextPage()}
                palette="whatsapp"
              />
            )}
          </>
        )}
      </div>
    </div>
  );
}
