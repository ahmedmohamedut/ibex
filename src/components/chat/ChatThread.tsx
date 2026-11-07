import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { Message } from "../../bindings/Message";
import { EmptyState, Spinner } from "../EmptyState";
import { MessageBubble } from "./MessageBubble";

/** A date separator, e.g. "Tuesday, 3 March 2026". */
function dayLabel(iso: string): string {
  return new Date(iso).toLocaleDateString(undefined, {
    weekday: "long",
    day: "numeric",
    month: "long",
    year: "numeric",
  });
}

function dayKey(iso: string | null): string {
  if (!iso) return "unknown";
  return new Date(iso).toDateString();
}

/**
 * A conversation thread.
 *
 * Messages arrive newest-first and render oldest-at-top, with older pages
 * loaded by scrolling up. Prepending content normally yanks the viewport, so
 * the scroll offset is restored from the height change after each prepend.
 */
export function ChatThread(props: {
  handle: string;
  /** Newest first, as the API returns them. */
  messages: Message[];
  isGroup: boolean;
  hasOlder: boolean;
  isLoadingOlder: boolean;
  onLoadOlder: () => void;
  palette?: "imessage" | "whatsapp";
}) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const [lightbox, setLightbox] = useState<{ url: string; name: string } | null>(null);

  // Oldest first for display.
  const ordered = useMemo(() => [...props.messages].reverse(), [props.messages]);
  const byId = useMemo(() => {
    const map = new Map<string, Message>();
    for (const m of ordered) map.set(m.id, m);
    return map;
  }, [ordered]);

  // Keep the viewport steady when an older page is prepended.
  const previous = useRef({ height: 0, count: 0, top: 0 });
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const grew = ordered.length > previous.current.count;
    const prependedAtTop = grew && previous.current.count > 0 && previous.current.top < 80;
    if (prependedAtTop) {
      // Restore the distance from the old top, so the message the user was
      // looking at stays put.
      el.scrollTop = el.scrollHeight - previous.current.height + previous.current.top;
    } else if (previous.current.count === 0) {
      // First render: a thread opens at its newest message.
      el.scrollTop = el.scrollHeight;
    }
    previous.current = { height: el.scrollHeight, count: ordered.length, top: el.scrollTop };
  }, [ordered]);

  // Load older pages as the top comes into view.
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const onScroll = () => {
      previous.current.top = el.scrollTop;
      previous.current.height = el.scrollHeight;
      if (el.scrollTop < 120 && props.hasOlder && !props.isLoadingOlder) {
        props.onLoadOlder();
      }
    };
    el.addEventListener("scroll", onScroll, { passive: true });
    return () => el.removeEventListener("scroll", onScroll);
  }, [props.hasOlder, props.isLoadingOlder, props.onLoadOlder]);

  const jumpTo = (id: string) => {
    document.getElementById(`message-${id}`)?.scrollIntoView({ block: "center" });
  };

  if (ordered.length === 0) {
    return <EmptyState title="No messages" detail="This conversation is empty." />;
  }

  let lastDay = "";
  return (
    <>
      <div className="pane__body" ref={scrollRef}>
        {props.isLoadingOlder ? (
          <div className="thread__loading">Loading older messages…</div>
        ) : props.hasOlder ? (
          <div className="thread__loading">
            <button
              type="button"
              className="button button--quiet"
              onClick={props.onLoadOlder}
            >
              Load older messages
            </button>
          </div>
        ) : (
          <div className="thread__loading">Start of the conversation</div>
        )}

        <div className="thread">
          {ordered.map((m) => {
            const key = dayKey(m.sentAt);
            const separator = key !== lastDay && m.sentAt ? dayLabel(m.sentAt) : null;
            lastDay = key;
            return (
              <div key={m.id} style={{ display: "contents" }}>
                {separator ? <div className="day">{separator}</div> : null}
                <MessageBubble
                  handle={props.handle}
                  message={m}
                  showSender={props.isGroup}
                  parent={m.replyTo ? byId.get(m.replyTo) : undefined}
                  palette={props.palette}
                  onOpenImage={(url, name) => setLightbox({ url, name })}
                  onJumpTo={jumpTo}
                />
              </div>
            );
          })}
        </div>
      </div>

      {lightbox ? (
        <div
          className="lightbox"
          role="dialog"
          aria-modal="true"
          aria-label={lightbox.name}
          onClick={() => setLightbox(null)}
          onKeyDown={(e) => {
            if (e.key === "Escape") setLightbox(null);
          }}
        >
          <img src={lightbox.url} alt={lightbox.name} />
        </div>
      ) : null}
    </>
  );
}

export { Spinner };
