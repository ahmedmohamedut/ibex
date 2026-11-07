import { useVirtualizer } from "@tanstack/react-virtual";
import { File, FileQuestion, Folder, Link2 } from "lucide-react";
import { useEffect, useRef } from "react";
import type { ManifestEntry } from "../bindings/ManifestEntry";
import { formatBytes, formatDateTime } from "./format";

const ROW_HEIGHT = 26;

function name(entry: ManifestEntry): string {
  const at = entry.relativePath.lastIndexOf("/");
  return at === -1 ? entry.relativePath : entry.relativePath.slice(at + 1);
}

function KindIcon(props: { entry: ManifestEntry }) {
  const { kind } = props.entry;
  if (kind === "directory") return <Folder size={14} aria-label="Folder" />;
  if (kind === "symlink") return <Link2 size={14} aria-label="Symlink" />;
  if (kind === "file") return <File size={14} aria-label="File" />;
  return <FileQuestion size={14} aria-label="Unknown kind" />;
}

/**
 * A virtualized file listing.
 *
 * A domain can hold tens of thousands of entries, so only the visible rows are
 * rendered and further pages are fetched as the user nears the end.
 */
export function FileTable(props: {
  entries: ManifestEntry[];
  selectedId: string | null;
  onSelect: (entry: ManifestEntry) => void;
  onOpenDir: (entry: ManifestEntry) => void;
  hasMore: boolean;
  isFetchingMore: boolean;
  onLoadMore: () => void;
}) {
  const parentRef = useRef<HTMLDivElement>(null);

  const virtualizer = useVirtualizer({
    count: props.entries.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 12,
  });

  const items = virtualizer.getVirtualItems();
  const lastIndex = items.at(-1)?.index;
  const { hasMore, isFetchingMore, onLoadMore } = props;
  const count = props.entries.length;

  // Pull the next page once the viewport approaches the end of what we have.
  // Depending on the individual values, not on `props`, keeps this from
  // re-running on every render.
  useEffect(() => {
    if (lastIndex === undefined) return;
    if (hasMore && !isFetchingMore && lastIndex >= count - 20) {
      onLoadMore();
    }
  }, [lastIndex, count, hasMore, isFetchingMore, onLoadMore]);

  return (
    <div className="pane__body" ref={parentRef}>
      <div className="table__head">
        <div>Name</div>
        <div style={{ textAlign: "right" }}>Size</div>
        <div>Modified</div>
      </div>
      <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
        {items.map((item) => {
          const entry = props.entries[item.index];
          const isDir = entry.kind === "directory";
          return (
            <button
              type="button"
              key={entry.fileId}
              className="row"
              aria-selected={entry.fileId === props.selectedId}
              style={{
                position: "absolute",
                top: 0,
                left: 0,
                height: `${item.size}px`,
                transform: `translateY(${item.start}px)`,
              }}
              onClick={() => props.onSelect(entry)}
              onDoubleClick={() => {
                if (isDir) props.onOpenDir(entry);
              }}
              title={entry.relativePath}
            >
              <span className="row__name">
                <KindIcon entry={entry} />
                {name(entry)}
              </span>
              <span className="row__size">
                {isDir ? "—" : formatBytes(entry.meta?.size ?? null)}
              </span>
              <span className="row__date">{formatDateTime(entry.meta?.mtime)}</span>
            </button>
          );
        })}
      </div>
      {props.isFetchingMore ? (
        <div style={{ padding: "8px 10px", color: "var(--text-muted)" }}>
          Loading more…
        </div>
      ) : null}
    </div>
  );
}
