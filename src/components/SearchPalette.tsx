import { Search, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useJobs } from "../api/jobs";
import { useIndexStatus, useSearch, useStartIndex } from "../api/search";
import { useSmsConversations } from "../api/sms";
import { useWaChats } from "../api/whatsapp";
import type { ConversationSummary } from "../bindings/ConversationSummary";
import type { SearchHit } from "../bindings/SearchHit";
import { useUi } from "../store/ui";
import { toastError } from "./Toasts";

function conversationLabel(conv: ConversationSummary | undefined, fallbackId: string): string {
  if (!conv) return fallbackId;
  if (conv.displayName && conv.displayName.trim()) return conv.displayName;
  if (conv.participants.length === 0) return "Unknown";
  return conv.participants.map((p) => p.displayName ?? p.id).join(", ");
}

/** `\u0001`/`\u0002` bracket each matched span (see `search::search` on the
 * Rust side); turn them into `<mark>`. */
function Snippet(props: { text: string }) {
  const parts = props.text.split(/[\u0001\u0002]/);
  return (
    <>
      {parts.map((part, i) => (i % 2 === 1 ? <mark key={i}>{part}</mark> : <span key={i}>{part}</span>))}
    </>
  );
}

/** The Ctrl+K / ⌘K command palette: search SMS + WhatsApp, jump to a result. */
export function SearchPalette() {
  const backup = useUi((s) => s.backup);
  const closeSearch = useUi((s) => s.closeSearch);
  const jumpTo = useUi((s) => s.jumpTo);
  const handle = backup?.handle;

  const [query, setQuery] = useState("");
  const [debounced, setDebounced] = useState("");
  const [activeIndex, setActiveIndex] = useState(0);
  const [indexJobId, setIndexJobId] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    const timer = setTimeout(() => setDebounced(query), 200);
    return () => clearTimeout(timer);
  }, [query]);

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  const status = useIndexStatus(handle);
  const startIndex = useStartIndex();
  const jobs = useJobs();
  // Warm the conversation-name caches — the same queries Messages/WhatsApp
  // use — so results show real names instead of raw conversation ids.
  const smsConvs = useSmsConversations(handle);
  const waConvs = useWaChats(handle, false);

  const results = useSearch(handle, debounced);
  const hits = results.data?.items ?? [];
  useEffect(() => setActiveIndex(0), [hits.length, debounced]);

  const indexJob = jobs.data?.find((j) => j.id === indexJobId);
  const building = indexJob?.state === "running";
  useEffect(() => {
    if (indexJob?.state === "finished") void status.refetch();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [indexJob?.state]);

  const labelFor = (hit: SearchHit): string => {
    const conv =
      hit.source === "sms"
        ? smsConvs.data?.items.find((c) => c.id === hit.conversationId)
        : waConvs.data?.items.find((c) => c.id === hit.conversationId);
    return conversationLabel(conv, hit.conversationId);
  };

  const open = (hit: SearchHit) => {
    jumpTo({
      source: hit.source === "whatsapp" ? "whatsapp" : "sms",
      conversationId: hit.conversationId,
      messageId: hit.messageId,
    });
  };

  if (!backup) return null;

  return (
    <div
      className="modal__backdrop"
      role="dialog"
      aria-modal="true"
      aria-label="Search"
      onClick={closeSearch}
      onKeyDown={(event) => {
        if (event.key === "Escape") {
          closeSearch();
        } else if (event.key === "ArrowDown") {
          event.preventDefault();
          setActiveIndex((i) => Math.min(i + 1, Math.max(hits.length - 1, 0)));
        } else if (event.key === "ArrowUp") {
          event.preventDefault();
          setActiveIndex((i) => Math.max(i - 1, 0));
        } else if (event.key === "Enter") {
          const hit = hits[activeIndex];
          if (hit) open(hit);
        }
      }}
    >
      <div
        className="modal palette"
        style={{ width: "min(560px, 100%)" }}
        onClick={(event) => event.stopPropagation()}
      >
        {status.data && !status.data.built ? (
          <div className="palette__prompt">
            <p>Search needs a one-time index of your SMS and WhatsApp messages.</p>
            <button
              type="button"
              className="button button--primary"
              disabled={building}
              onClick={() => {
                if (!handle) return;
                startIndex.mutate(handle, {
                  onSuccess: (jobId) => setIndexJobId(jobId),
                  onError: (error) => toastError("Could not build the search index", error),
                });
              }}
            >
              {building ? (
                <>
                  <span className="spinner" style={{ width: 12, height: 12 }} /> Building index…
                </>
              ) : (
                "Build search index"
              )}
            </button>
          </div>
        ) : (
          <>
            <div className="palette__input">
              <Search size={14} aria-hidden="true" />
              <input
                ref={inputRef}
                className="palette__field"
                placeholder="Search messages…"
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                aria-label="Search messages"
              />
              <button
                type="button"
                className="button button--quiet"
                onClick={closeSearch}
                aria-label="Close search"
              >
                <X size={14} aria-hidden="true" />
              </button>
            </div>

            <div className="palette__results">
              {!debounced ? (
                <div className="palette__empty">Type to search SMS and WhatsApp messages.</div>
              ) : results.isFetching ? (
                <div className="palette__empty">Searching…</div>
              ) : hits.length === 0 ? (
                <div className="palette__empty">No matches</div>
              ) : (
                hits.map((hit, i) => (
                  <button
                    key={`${hit.source}-${hit.messageId}`}
                    type="button"
                    className={
                      "palette__result" + (i === activeIndex ? " palette__result--active" : "")
                    }
                    onMouseEnter={() => setActiveIndex(i)}
                    onClick={() => open(hit)}
                  >
                    <span className="palette__result-service">
                      {hit.source === "whatsapp" ? "WhatsApp" : "Messages"}
                    </span>
                    <span className="palette__result-conv">{labelFor(hit)}</span>
                    <span className="palette__result-snippet">
                      <Snippet text={hit.snippet} />
                    </span>
                  </button>
                ))
              )}
            </div>
          </>
        )}
      </div>
    </div>
  );
}
