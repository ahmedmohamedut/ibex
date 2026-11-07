import { useMemo } from "react";
import { useDirListing, useDomains } from "../api/raw";
import { Breadcrumb } from "../components/Breadcrumb";
import { EmptyState, Spinner } from "../components/EmptyState";
import { FileDetail } from "../components/FileDetail";
import { FileTable } from "../components/FileTable";
import { formatCount } from "../components/format";
import { useUi } from "../store/ui";

function DomainList() {
  const backup = useUi((s) => s.backup);
  const domain = useUi((s) => s.domain);
  const selectDomain = useUi((s) => s.selectDomain);
  const domains = useDomains(backup?.handle);

  if (domains.isPending) return <Spinner label="Reading the manifest…" />;
  if (domains.isError) {
    return (
      <EmptyState title="Could not list domains" detail={String(domains.error)} />
    );
  }

  return (
    <div className="pane__body">
      {(domains.data ?? []).map((summary) => (
        <button
          key={summary.domain}
          type="button"
          className="row"
          style={{ gridTemplateColumns: "1fr auto", height: 26 }}
          aria-selected={summary.domain === domain}
          onClick={() => selectDomain(summary.domain)}
          title={summary.domain}
        >
          <span className="row__name">{summary.domain}</span>
          <span className="row__size">{formatCount(Number(summary.fileCount))}</span>
        </button>
      ))}
    </div>
  );
}

export function Raw() {
  const backup = useUi((s) => s.backup);
  const domain = useUi((s) => s.domain);
  const dir = useUi((s) => s.dir);
  const fileId = useUi((s) => s.fileId);
  const setDir = useUi((s) => s.setDir);
  const selectFile = useUi((s) => s.selectFile);
  const selectDomain = useUi((s) => s.selectDomain);

  const listing = useDirListing(backup?.handle, domain, dir);
  const entries = useMemo(
    () => listing.data?.pages.flatMap((page) => page.items) ?? [],
    [listing.data],
  );
  const total = listing.data?.pages[0]?.total ?? null;

  if (!backup) return null;

  // No domain picked yet: the list pane shows domains full width.
  if (!domain) {
    return (
      <div className="panes">
        <div className="pane pane--list">
          <div className="pane__header">
            <span className="pane__title">Domains</span>
          </div>
          <DomainList />
        </div>
        <div className="pane">
          <EmptyState
            title="Pick a domain"
            detail="Every file in the backup belongs to one. HomeDomain holds most app and system data; CameraRollDomain holds photos."
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
            onClick={() => selectDomain(null)}
            title="Back to the domain list"
          >
            ← Domains
          </button>
          <span className="pane__title" title={domain}>
            {domain}
          </span>
        </div>
        <div className="pane__header" style={{ background: "var(--bg)" }}>
          <Breadcrumb root={domain} path={dir} onNavigate={setDir} />
          <span className="topbar__spacer" />
          <span style={{ color: "var(--text-muted)", fontSize: 11 }}>
            {total === null ? "" : `${formatCount(Number(total))} entries`}
          </span>
        </div>

        {listing.isPending ? (
          <Spinner label="Listing…" />
        ) : listing.isError ? (
          <EmptyState title="Could not list this folder" detail={String(listing.error)} />
        ) : entries.length === 0 ? (
          <EmptyState
            title="Nothing here"
            detail="This folder has no entries in the manifest."
          />
        ) : (
          <FileTable
            entries={entries}
            selectedId={fileId}
            onSelect={(entry) => {
              if (entry.kind === "directory") setDir(entry.relativePath);
              else selectFile(entry.fileId);
            }}
            onOpenDir={(entry) => setDir(entry.relativePath)}
            hasMore={Boolean(listing.hasNextPage)}
            isFetchingMore={listing.isFetchingNextPage}
            onLoadMore={() => void listing.fetchNextPage()}
          />
        )}
      </div>

      <FileDetail handle={backup.handle} fileId={fileId} />
    </div>
  );
}
