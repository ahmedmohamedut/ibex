import { MessageSquare, Users } from "lucide-react";
import { useMemo, useState } from "react";
import { useSmsConversations, useSmsMessages } from "../api/sms";
import type { ConversationSummary } from "../bindings/ConversationSummary";
import { ChatThread } from "../components/chat/ChatThread";
import { EmptyState, Spinner } from "../components/EmptyState";
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

function label(conv: ConversationSummary): string {
  if (conv.displayName && conv.displayName.trim()) return conv.displayName;
  if (conv.participants.length === 0) return "Unknown";
  return conv.participants.map((p) => p.displayName ?? p.id).join(", ");
}

export function Messages() {
  const backup = useUi((s) => s.backup);
  const [selected, setSelected] = useState<string | null>(null);
  const [filter, setFilter] = useState("");

  const conversations = useSmsConversations(backup?.handle);
  const thread = useSmsMessages(backup?.handle, selected);

  const items = useMemo(() => {
    const all = conversations.data?.items ?? [];
    const needle = filter.trim().toLowerCase();
    if (!needle) return all;
    // Filters the list only; searching message text is milestone M8.
    return all.filter((c) => label(c).toLowerCase().includes(needle));
  }, [conversations.data, filter]);

  const current = items.find((c) => c.id === selected)
    ?? conversations.data?.items.find((c) => c.id === selected);

  const messages = useMemo(
    () => thread.data?.pages.flatMap((p) => p.messages) ?? [],
    [thread.data],
  );

  if (!backup) return null;

  return (
    <div className="panes">
      <div className="pane pane--list">
        <div className="pane__header">
          <input
            className="input"
            style={{ flex: 1 }}
            placeholder="Filter conversations…"
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            aria-label="Filter conversations"
          />
        </div>

        {conversations.isPending ? (
          <Spinner label="Reading conversations…" />
        ) : conversations.isError ? (
          <EmptyState
            title="Could not read Messages"
            detail={String(conversations.error)}
          />
        ) : items.length === 0 ? (
          <EmptyState
            title={filter ? "Nothing matches" : "No conversations"}
            detail={
              filter
                ? "Try a different filter."
                : "This backup's sms.db has no conversations in it."
            }
          />
        ) : (
          <div className="pane__body">
            {items.map((conv) => (
              <button
                key={conv.id}
                type="button"
                className="conv-row"
                aria-selected={conv.id === selected}
                onClick={() => setSelected(conv.id)}
              >
                <span className="avatar" aria-hidden="true">
                  {initials(label(conv))}
                </span>
                <span className="conv-row__main">
                  <span className="conv-row__name">
                    {conv.isGroup ? (
                      <Users size={11} aria-hidden="true" style={{ marginRight: 4 }} />
                    ) : null}
                    {label(conv)}
                  </span>
                  <span className="conv-row__preview">
                    {conv.lastMessagePreview ?? "No messages"}
                  </span>
                </span>
                <span className="conv-row__side">
                  {formatDate(conv.lastMessageAt)}
                  <br />
                  {formatCount(conv.messageCount)}
                </span>
              </button>
            ))}
            <div style={{ padding: "8px 10px", color: "var(--text-muted)", fontSize: 11 }}>
              {formatCount(conversations.data?.total ?? items.length)} conversations
            </div>
          </div>
        )}
      </div>

      <div className="pane">
        {!selected || !current ? (
          <EmptyState
            title="Pick a conversation"
            detail="Threads open at the newest message; scroll up for older ones."
          />
        ) : (
          <>
            <div className="pane__header">
              <MessageSquare size={14} aria-hidden="true" />
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
                disabled
                title="Export arrives in milestone M4"
              >
                Export
              </button>
            </div>

            {thread.isPending ? (
              <Spinner label="Reading messages…" />
            ) : thread.isError ? (
              <EmptyState
                title="Could not read this conversation"
                detail={String(thread.error)}
              />
            ) : (
              <ChatThread
                handle={backup.handle}
                messages={messages}
                isGroup={current.isGroup}
                hasOlder={Boolean(thread.hasNextPage)}
                isLoadingOlder={thread.isFetchingNextPage}
                onLoadOlder={() => void thread.fetchNextPage()}
              />
            )}
          </>
        )}
      </div>
    </div>
  );
}
