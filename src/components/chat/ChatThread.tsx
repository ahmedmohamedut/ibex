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
/** How many older pages to load while hunting for a not-yet-loaded message,
 * before giving up rather than paging forever. */
const MAX_JUMP_PAGES = 20;

export function ChatThread(props: {
  handle: string;
  /** Newest first, as the API returns them. */
  messages: Message[];
  isGroup: boolean;
  hasOlder: boolean;
  isLoadingOlder: boolean;
  onLoadOlder: () => void;
  palette?: "imessage" | "whatsapp";
  /** A message to scroll to and briefly highlight, e.g. from a search
   * result — paged in with `onLoadOlder` if not already loaded. */
  highlightMessageId?: string | null;
  /** Called once the jump either lands or gives up, so the caller can clear
   * its pending-jump state. */
  onHighlightSettled?: () => void;
}) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const [lightbox, setLightbox] = useState<{ url: string; name: string } | null>(null);
  const [flashId, setFlashId] = useState<string | null>(null);
  const [jumpGaveUp, setJumpGaveUp] = useState(false);
  const jumpAttempts = useRef(0);

  // Oldest first for display.
  const ordered = useMemo(() => [...props.messages].reverse(), [props.messages]);
  const byId = useMemo(() => {
    const map = new Map<string, Message>();
    for (const m of ordered) map.set(m.id, m);
    return map;
  }, [ordered]);

  // Reset the hunt whenever a new target arrives.
  useEffect(() => {
    jumpAttempts.current = 0;
    setJumpGaveUp(false);
  }, [props.highlightMessageId]);

  // Hunt for a message that might not be in the loaded range yet: keep
  // asking for older pages until it shows up, or until MAX_JUMP_PAGES says
  // it isn't going to.
  useEffect(() => {
    const target = props.highlightMessageId;
    if (!target) return;
    if (byId.has(target)) return;
    if (jumpGaveUp) return;
    if (props.isLoadingOlder) return;
    if (!props.hasOlder || jumpAttempts.current >= MAX_JUMP_PAGES) {
      setJumpGaveUp(true);
      props.onHighlightSettled?.();
      return;
    }
    jumpAttempts.current += 1;
    props.onLoadOlder();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [props.highlightMessageId, byId, props.hasOlder, props.isLoadingOlder, jumpGaveUp]);

  // Found it: scroll to it and flash it briefly.
  useEffect(() => {
    const target = props.highlightMessageId;
    if (!target || !byId.has(target)) return;
    document.getElementById(`message-${target}`)?.scrollIntoView({ block: "center" });
    setFlashId(target);
    const timer = setTimeout(() => {
      setFlashId(null);
      props.onHighlightSettled?.();
    }, 2000);
    return () => clearTimeout(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [props.highlightMessageId, byId]);

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
        {jumpGaveUp && props.highlightMessageId && !byId.has(props.highlightMessageId) ? (
          <div className="thread__loading thread__loading--warn">
            Couldn't find this message — it may be outside the loaded range.
          </div>
        ) : null}
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
                  highlighted={m.id === flashId}
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
