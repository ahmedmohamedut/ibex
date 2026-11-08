import { CheckSquare, Share, X } from "lucide-react";
import { useMemo, useState } from "react";
import {
  DEFAULT_PHOTO_FILTER,
  usePhotoAlbums,
  usePhotoDetail,
  usePhotos,
} from "../api/photos";
import type { PhotoAsset } from "../bindings/PhotoAsset";
import type { PhotoFilter } from "../bindings/PhotoFilter";
import { EmptyState, Spinner } from "../components/EmptyState";
import { ExportDialog, type ExportScope } from "../components/ExportDialog";
import { formatCount } from "../components/format";
import { Lightbox } from "../components/photos/Lightbox";
import { PhotoGrid } from "../components/photos/PhotoGrid";
import { useUi } from "../store/ui";

type QuickFilter = "all" | "photos" | "videos" | "favorites" | "screenshots" | "deleted";

function filterFor(quick: QuickFilter, albumId: string | null): PhotoFilter {
  const base = { ...DEFAULT_PHOTO_FILTER, albumId };
  switch (quick) {
    case "photos":
      return { ...base, kind: "photo" };
    case "videos":
      return { ...base, kind: "video" };
    case "favorites":
      return { ...base, favoritesOnly: true };
    case "screenshots":
      return { ...base, screenshotsOnly: true };
    case "deleted":
      return { ...base, trashedOnly: true, albumId: null };
    default:
      return base;
  }
}

const QUICK_FILTERS: { value: QuickFilter; label: string }[] = [
  { value: "all", label: "All" },
  { value: "photos", label: "Photos" },
  { value: "videos", label: "Videos" },
  { value: "favorites", label: "Favorites" },
  { value: "screenshots", label: "Screenshots" },
  { value: "deleted", label: "Recently Deleted" },
];

export function Photos() {
  const backup = useUi((s) => s.backup);
  const [quick, setQuick] = useState<QuickFilter>("all");
  const [albumId, setAlbumId] = useState<string | null>(null);
  const [selectMode, setSelectMode] = useState(false);
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  const [openId, setOpenId] = useState<string | null>(null);
  const [exporting, setExporting] = useState<ExportScope | null>(null);

  const filter = useMemo(() => filterFor(quick, albumId), [quick, albumId]);

  const photos = usePhotos(backup?.handle, filter);
  const albums = usePhotoAlbums(backup?.handle);
  const detail = usePhotoDetail(backup?.handle, openId);

  const assets = useMemo(
    () => photos.data?.pages.flatMap((p) => p.assets) ?? [],
    [photos.data],
  );
  const openIndex = assets.findIndex((a) => a.id === openId);
  const openAsset: PhotoAsset | undefined =
    detail.data ?? (openIndex >= 0 ? assets[openIndex] : undefined);

  if (!backup) return null;

  const toggleSelect = (id: string) => {
    setSelectedIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  return (
    <div className="panes">
      {exporting ? (
        <ExportDialog
          handle={backup.handle}
          source="photos"
          scope={exporting}
          messageCount={
            exporting.kind === "photos" ? exporting.assetIds?.length : undefined
          }
          onClose={() => setExporting(null)}
        />
      ) : null}

      {openAsset ? (
        <Lightbox
          handle={backup.handle}
          asset={openAsset}
          hasPrev={openIndex > 0}
          hasNext={openIndex >= 0 && openIndex < assets.length - 1}
          onPrev={() => setOpenId(assets[openIndex - 1]?.id ?? null)}
          onNext={() => setOpenId(assets[openIndex + 1]?.id ?? null)}
          onClose={() => setOpenId(null)}
          onExportOne={() =>
            setExporting({
              kind: "photos",
              filter,
              label: openAsset.fileName,
              assetIds: [openAsset.id],
            })
          }
        />
      ) : null}

      <div className="pane">
        <div className="pane__header" style={{ gap: 10, flexWrap: "wrap" }}>
          {QUICK_FILTERS.map((f) => (
            <button
              key={f.value}
              type="button"
              className="button button--quiet"
              aria-current={quick === f.value && !albumId}
              onClick={() => {
                setQuick(f.value);
                setAlbumId(null);
              }}
            >
              {f.label}
            </button>
          ))}

          {albums.data && albums.data.length > 0 ? (
            <select
              className="input"
              value={albumId ?? ""}
              onChange={(e) => {
                setAlbumId(e.target.value || null);
                setQuick("all");
              }}
              aria-label="Album"
            >
              <option value="">All albums</option>
              {albums.data.map((album) => (
                <option key={album.id} value={album.id}>
                  {album.title} ({formatCount(album.count)})
                </option>
              ))}
            </select>
          ) : null}

          <span className="topbar__spacer" />

          <button
            type="button"
            className="button button--quiet"
            onClick={() => {
              setSelectMode((on) => !on);
              setSelectedIds(new Set());
            }}
          >
            {selectMode ? (
              <>
                <X size={14} aria-hidden="true" /> Cancel
              </>
            ) : (
              <>
                <CheckSquare size={14} aria-hidden="true" /> Select
              </>
            )}
          </button>

          {selectMode && selectedIds.size > 0 ? (
            <button
              type="button"
              className="button button--primary"
              onClick={() =>
                setExporting({
                  kind: "photos",
                  filter,
                  label: `${formatCount(selectedIds.size)} selected item(s)`,
                  assetIds: Array.from(selectedIds),
                })
              }
            >
              <Share size={14} aria-hidden="true" /> Export {selectedIds.size} item
              {selectedIds.size === 1 ? "" : "s"}
            </button>
          ) : !selectMode ? (
            <button
              type="button"
              className="button"
              onClick={() =>
                setExporting({
                  kind: "photos",
                  filter,
                  label:
                    albumId && albums.data
                      ? (albums.data.find((a) => a.id === albumId)?.title ?? "this view")
                      : QUICK_FILTERS.find((f) => f.value === quick)?.label.toLowerCase() ??
                        "everything",
                })
              }
            >
              <Share size={14} aria-hidden="true" /> Export view
            </button>
          ) : null}
        </div>

        {photos.isPending ? (
          <Spinner label="Reading the camera roll…" />
        ) : photos.isError ? (
          <EmptyState title="Could not read Photos" detail={String(photos.error)} />
        ) : assets.length === 0 ? (
          <EmptyState
            title="No photos"
            detail="Nothing matches this filter in this backup."
          />
        ) : (
          <PhotoGrid
            handle={backup.handle}
            assets={assets}
            selectedIds={selectedIds}
            onToggleSelect={(id) => {
              if (!selectMode) setSelectMode(true);
              toggleSelect(id);
            }}
            onOpen={(asset) => {
              if (selectMode) toggleSelect(asset.id);
              else setOpenId(asset.id);
            }}
            hasMore={Boolean(photos.hasNextPage)}
            isFetchingMore={photos.isFetchingNextPage}
            onLoadMore={() => void photos.fetchNextPage()}
          />
        )}

        <div
          style={{
            padding: "6px 10px",
            color: "var(--text-muted)",
            fontSize: 11,
            borderTop: "1px solid var(--border)",
          }}
        >
          {formatCount(assets.length)} item{assets.length === 1 ? "" : "s"} loaded
          {photos.hasNextPage ? " (scroll for more)" : ""}
        </div>
      </div>
    </div>
  );
}
