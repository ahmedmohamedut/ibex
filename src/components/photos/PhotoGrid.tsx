import { useVirtualizer } from "@tanstack/react-virtual";
import { Clapperboard, Pencil, Sparkles } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import type { PhotoAsset } from "../../bindings/PhotoAsset";
import { thumbUrl } from "../../api/media";

const TILE_SIZE = 160;
const GAP = 4;
const HEADER_HEIGHT = 32;

/** A grid row: either a month header or one strip of tiles. */
type Row =
  | { kind: "header"; label: string; key: string }
  | { kind: "tiles"; assets: PhotoAsset[]; key: string };

/**
 * Group assets by calendar month (already newest-first) into grid rows that
 * fit `columns` tiles per strip.
 */
function buildRows(assets: PhotoAsset[], columns: number): Row[] {
  const rows: Row[] = [];
  let currentMonth = "";
  let strip: PhotoAsset[] = [];

  const flushStrip = () => {
    if (strip.length === 0) return;
    rows.push({ kind: "tiles", assets: strip, key: `${rows.length}-tiles` });
    strip = [];
  };

  for (const asset of assets) {
    const month = asset.createdAt ? asset.createdAt.slice(0, 7) : "unknown";
    if (month !== currentMonth) {
      flushStrip();
      currentMonth = month;
      const label = asset.createdAt
        ? new Date(asset.createdAt).toLocaleDateString(undefined, {
            year: "numeric",
            month: "long",
          })
        : "Unknown date";
      rows.push({ kind: "header", label, key: `${rows.length}-header-${month}` });
    }
    strip.push(asset);
    if (strip.length === columns) flushStrip();
  }
  flushStrip();
  return rows;
}

function formatDuration(seconds: number): string {
  const m = Math.floor(seconds / 60);
  const s = Math.round(seconds % 60);
  return `${m}:${s.toString().padStart(2, "0")}`;
}

function Tile(props: {
  handle: string;
  asset: PhotoAsset;
  selected: boolean;
  onOpen: () => void;
  onToggleSelect: () => void;
}) {
  const { asset } = props;
  const thumbId = asset.thumbFileId ?? asset.fileId;
  return (
    <button
      type="button"
      className="photo-tile"
      aria-current={props.selected}
      onClick={(event) => {
        if (event.shiftKey || event.ctrlKey || event.metaKey) {
          props.onToggleSelect();
        } else {
          props.onOpen();
        }
      }}
      title={asset.fileName}
      style={{ width: TILE_SIZE, height: TILE_SIZE }}
    >
      {thumbId ? (
        <img
          src={thumbUrl(props.handle, thumbId, 256)}
          alt=""
          loading="lazy"
          className="photo-tile__img"
        />
      ) : (
        <span className="photo-tile__placeholder">Not in backup</span>
      )}
      <span className="photo-tile__badges">
        {asset.kind === "video" ? (
          <span className="photo-tile__badge">
            <Clapperboard size={11} aria-hidden="true" />
            {asset.duration ? formatDuration(asset.duration) : ""}
          </span>
        ) : null}
        {asset.live ? (
          <span className="photo-tile__badge">
            <Sparkles size={11} aria-hidden="true" /> Live
          </span>
        ) : null}
        {asset.edited ? (
          <span className="photo-tile__badge">
            <Pencil size={11} aria-hidden="true" />
          </span>
        ) : null}
      </span>
      {props.selected ? <span className="photo-tile__check" aria-hidden="true" /> : null}
    </button>
  );
}

/** How many tiles fit per row, recomputed when the pane is resized. */
function useColumnCount(ref: React.RefObject<HTMLDivElement | null>): number {
  const [columns, setColumns] = useState(4);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const compute = () => {
      const width = el.clientWidth;
      const perRow = Math.max(1, Math.floor((width + GAP) / (TILE_SIZE + GAP)));
      setColumns(perRow);
    };
    compute();
    const observer = new ResizeObserver(compute);
    observer.observe(el);
    return () => observer.disconnect();
  }, [ref]);
  return columns;
}

/**
 * The virtualized camera-roll grid.
 *
 * Month headers and tile strips are flattened into one list of rows so
 * `@tanstack/react-virtual` stays in its ordinary single-axis mode — the same
 * approach `FileTable.tsx` uses for the Raw browser's listing.
 */
export function PhotoGrid(props: {
  handle: string;
  assets: PhotoAsset[];
  selectedIds: Set<string>;
  onToggleSelect: (id: string) => void;
  onOpen: (asset: PhotoAsset) => void;
  hasMore: boolean;
  isFetchingMore: boolean;
  onLoadMore: () => void;
}) {
  const parentRef = useRef<HTMLDivElement>(null);
  const columns = useColumnCount(parentRef);

  const rows = useMemo(() => buildRows(props.assets, columns), [props.assets, columns]);

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => parentRef.current,
    estimateSize: (index) =>
      rows[index]?.kind === "header" ? HEADER_HEIGHT : TILE_SIZE + GAP,
    overscan: 4,
  });

  const items = virtualizer.getVirtualItems();
  const lastIndex = items.at(-1)?.index;
  const { hasMore, isFetchingMore, onLoadMore } = props;
  const rowCount = rows.length;

  // Pull the next page once the viewport nears the end of what is loaded.
  useEffect(() => {
    if (lastIndex === undefined) return;
    if (lastIndex >= rowCount - 3 && hasMore && !isFetchingMore) {
      onLoadMore();
    }
  }, [lastIndex, rowCount, hasMore, isFetchingMore, onLoadMore]);

  return (
    <div ref={parentRef} className="photo-grid">
      <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
        {items.map((item) => {
          const row = rows[item.index];
          if (!row) return null;
          return (
            <div
              key={row.key}
              style={{
                position: "absolute",
                top: 0,
                left: 0,
                width: "100%",
                height: item.size,
                transform: `translateY(${item.start}px)`,
              }}
            >
              {row.kind === "header" ? (
                <div className="photo-grid__header">{row.label}</div>
              ) : (
                <div className="photo-grid__strip" style={{ gap: GAP }}>
                  {row.assets.map((asset) => (
                    <Tile
                      key={asset.id}
                      handle={props.handle}
                      asset={asset}
                      selected={props.selectedIds.has(asset.id)}
                      onOpen={() => props.onOpen(asset)}
                      onToggleSelect={() => props.onToggleSelect(asset.id)}
                    />
                  ))}
                </div>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}
