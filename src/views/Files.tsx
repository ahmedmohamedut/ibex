import { Share } from "lucide-react";
import { useMemo, useState } from "react";
import { useDirListing } from "../api/raw";
import { useDocumentSources } from "../api/voice";
import type { DocSource } from "../bindings/DocSource";
import { Breadcrumb } from "../components/Breadcrumb";
import { EmptyState, Spinner } from "../components/EmptyState";
import { ExportDialog, type ExportScope } from "../components/ExportDialog";
import { FileDetail } from "../components/FileDetail";
import { FileTable } from "../components/FileTable";
import { formatCount } from "../components/format";
import { useUi } from "../store/ui";

/** The manifest path for `dir` (relative to the source's own root) within
 * `source`, joining the two only when `dir` is non-empty. */
function fullPath(source: DocSource, dir: string): string {
  return dir ? `${source.root}/${dir}` : source.root;
}

/** The reverse of `fullPath`: a manifest-relative path (as every
 * `ManifestEntry.relativePath` already is) back to one relative to the
 * source's own root, for `dir` state and the breadcrumb. */
function relativeToSource(source: DocSource, manifestPath: string): string {
  const prefix = `${source.root}/`;
  return manifestPath.startsWith(prefix) ? manifestPath.slice(prefix.length) : manifestPath;
}

export function Files() {
  const backup = useUi((s) => s.backup);
  const [sourceId, setSourceId] = useState<string | null>(null);
  const [dir, setDir] = useState("");
  const [fileId, setFileId] = useState<string | null>(null);
  const [exporting, setExporting] = useState<ExportScope | null>(null);

  const sources = useDocumentSources(backup?.handle);
  const source = sources.data?.find((s) => s.domain === sourceId) ?? null;

  const listing = useDirListing(backup?.handle, source?.domain ?? null, source ? fullPath(source, dir) : "");
  const entries = useMemo(
    () => listing.data?.pages.flatMap((page) => page.items) ?? [],
    [listing.data],
  );
  const total = listing.data?.pages[0]?.total ?? null;

  if (!backup) return null;

  const selectSource = (next: DocSource | null) => {
    setSourceId(next?.domain ?? null);
    setDir("");
    setFileId(null);
  };

  if (!source) {
    return (
      <div className="panes">
        <div className="pane pane--list">
          <div className="pane__header">
            <span className="pane__title">Sources</span>
          </div>
          {sources.isPending ? (
            <Spinner label="Reading sources…" />
          ) : sources.isError ? (
            <EmptyState title="Could not list Files sources" detail={String(sources.error)} />
          ) : (sources.data ?? []).length === 0 ? (
            <EmptyState
              title="Nothing here"
              detail="No app has documents in this backup."
            />
          ) : (
            <div className="pane__body">
              {(sources.data ?? []).map((candidate) => (
                <button
                  key={candidate.domain}
                  type="button"
                  className="row"
                  style={{ gridTemplateColumns: "1fr", height: 28 }}
                  onClick={() => selectSource(candidate)}
                  title={candidate.bundleId ?? undefined}
                >
                  <span className="row__name">{candidate.label}</span>
                </button>
              ))}
            </div>
          )}
        </div>
        <div className="pane">
          <EmptyState
            title="Pick a source"
            detail="On My iPhone, or one of the apps that stores documents in this backup."
          />
        </div>
      </div>
    );
  }

  return (
    <div className="panes">
      <div className="pane pane--list">
        <div className="pane__header">
          <button
            type="button"
            className="button button--quiet"
            onClick={() => selectSource(null)}
            title="Back to sources"
          >
            ← Sources
          </button>
          <span className="pane__title" title={source.bundleId ?? source.label}>
            {source.label}
          </span>
        </div>
        <div className="pane__header" style={{ background: "var(--bg)" }}>
          <Breadcrumb root={source.label} path={dir} onNavigate={setDir} />
          <span className="topbar__spacer" />
          <span style={{ color: "var(--text-muted)", fontSize: 11 }}>
            {total === null ? "" : `${formatCount(Number(total))} entries`}
          </span>
          <button
            type="button"
            className="button button--quiet"
            onClick={() =>
              setExporting({
                kind: "documents",
                label: source.label,
                root: { domain: source.domain, root: source.root },
              })
            }
            title="Export everything under this folder"
          >
            <Share size={13} aria-hidden="true" /> Export
          </button>
        </div>

        {listing.isPending ? (
          <Spinner label="Listing…" />
        ) : listing.isError ? (
          <EmptyState title="Could not list this folder" detail={String(listing.error)} />
        ) : entries.length === 0 ? (
          <EmptyState title="Empty" detail="This folder has no entries." />
        ) : (
          <FileTable
            entries={entries}
            selectedId={fileId}
            onSelect={(entry) => {
              if (entry.kind === "directory") {
                setDir(relativeToSource(source, entry.relativePath));
              } else {
                setFileId(entry.fileId);
              }
            }}
            onOpenDir={(entry) => setDir(relativeToSource(source, entry.relativePath))}
            hasMore={Boolean(listing.hasNextPage)}
            isFetchingMore={listing.isFetchingNextPage}
            onLoadMore={() => void listing.fetchNextPage()}
          />
        )}
      </div>

      {exporting ? (
        <ExportDialog
          handle={backup.handle}
          source="documents"
          scope={exporting}
          onClose={() => setExporting(null)}
        />
      ) : null}

      <FileDetail handle={backup.handle} fileId={fileId} />
    </div>
  );
}
