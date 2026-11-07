import { HardDrive, Lock, Smartphone, Trash2 } from "lucide-react";
import { useBackupStats, useClearCache } from "../api/backup";
import { useDomains } from "../api/raw";
import { Spinner } from "../components/EmptyState";
import { formatBytes, formatCount, formatDateTime } from "../components/format";
import { toastError, useToasts } from "../components/Toasts";
import { useUi } from "../store/ui";

export function Overview() {
  const backup = useUi((s) => s.backup);
  const setSection = useUi((s) => s.setSection);
  const stats = useBackupStats(backup?.handle);
  const domains = useDomains(backup?.handle);
  const clearCache = useClearCache();
  const pushToast = useToasts((s) => s.push);

  if (!backup) return null;
  const { info } = backup;

  const clear = () => {
    clearCache.mutate(backup.handle, {
      onSuccess: (freed) =>
        pushToast({
          kind: "info",
          title: `Cleared ${formatBytes(freed)} of cache`,
          detail:
            "Working copies will be made again the next time a database is opened.",
        }),
      onError: (error) => toastError("Could not clear the cache", error),
    });
  };

  return (
    <div className="pane__body">
      <div className="cards">
        <section className="card">
          <h2 className="card__title">
            <Smartphone size={12} aria-hidden="true" /> Device
          </h2>
          <dl className="kv">
            <dt>Name</dt>
            <dd>{info.deviceName ?? "—"}</dd>
            <dt>Model</dt>
            <dd>
              {info.productName ?? "—"}
              {info.productType ? ` (${info.productType})` : ""}
            </dd>
            <dt>iOS</dt>
            <dd>
              {info.productVersion ?? "—"}
              {info.buildVersion ? ` (${info.buildVersion})` : ""}
            </dd>
            <dt>Serial</dt>
            <dd className="mono">
              {/* Last four only: enough to tell devices apart without putting a
                  full identifier on screen. */}
              {info.serialNumber ? `…${info.serialNumber.slice(-4)}` : "—"}
            </dd>
            <dt>Apps</dt>
            <dd>{formatCount(info.installedApps.length)}</dd>
          </dl>
        </section>

        <section className="card">
          <h2 className="card__title">
            <HardDrive size={12} aria-hidden="true" /> Backup
          </h2>
          <dl className="kv">
            <dt>Taken</dt>
            <dd>{formatDateTime(info.lastBackupDate ?? info.backupDate)}</dd>
            <dt>Encrypted</dt>
            <dd>
              {info.encrypted ? (
                <span className="badge badge--warn">
                  <Lock size={10} aria-hidden="true" /> yes
                </span>
              ) : (
                "no"
              )}
            </dd>
            <dt>Full backup</dt>
            <dd>{info.isFullBackup === null ? "—" : info.isFullBackup ? "yes" : "no"}</dd>
            <dt>Total size</dt>
            <dd>{stats.data ? formatBytes(stats.data.totalBytes) : "…"}</dd>
            <dt>Entries</dt>
            <dd>{stats.data ? formatCount(stats.data.fileCount) : "…"}</dd>
            <dt>Domains</dt>
            <dd>{stats.data ? formatCount(stats.data.domainCount) : "…"}</dd>
            <dt>Path</dt>
            <dd className="mono">{backup.path}</dd>
          </dl>
        </section>

        <section className="card">
          <h2 className="card__title">Sources</h2>
          <dl className="kv">
            {backup.sources.map((source) => (
              <div key={source.id} style={{ display: "contents" }}>
                <dt>{source.label}</dt>
                <dd title={source.reason ?? undefined}>
                  {source.available ? "in this backup" : "not found"}
                  {source.available && !source.implemented ? (
                    <span className="badge" style={{ marginLeft: 6 }}>
                      viewer pending
                    </span>
                  ) : null}
                </dd>
              </div>
            ))}
          </dl>
          <p style={{ margin: "10px 0 0", color: "var(--text-muted)", fontSize: 12 }}>
            Every file is browsable now through{" "}
            <button
              type="button"
              className="crumbs__item"
              onClick={() => setSection("raw")}
            >
              Raw
            </button>
            . The dedicated views arrive in later milestones.
          </p>
        </section>

        <section className="card">
          <h2 className="card__title">Cache</h2>
          <dl className="kv">
            <dt>This backup</dt>
            <dd>{stats.data ? formatBytes(stats.data.cacheBytes) : "…"}</dd>
            <dt>Largest domain</dt>
            <dd>
              {domains.data && domains.data.length > 0
                ? `${[...domains.data].sort((a, b) => Number(b.fileCount) - Number(a.fileCount))[0].domain}`
                : "…"}
            </dd>
          </dl>
          <p style={{ margin: "10px 0", color: "var(--text-muted)", fontSize: 12 }}>
            ibex copies databases into <span className="mono">~/.local/share/ibex</span>{" "}
            to read them without touching the backup. Clearing the cache is always safe;
            copies are remade on demand.
          </p>
          {info.encrypted ? (
            <p
              style={{
                margin: "10px 0",
                fontSize: 12,
                color: "var(--warning)",
                borderLeft: "3px solid var(--warning)",
                paddingLeft: 8,
              }}
            >
              This backup is encrypted, so those cached copies are{" "}
              <strong>decrypted</strong> and readable by anything that can read your
              home directory. They stay there until you clear them.
            </p>
          ) : null}
          <button
            type="button"
            className="button"
            onClick={clear}
            disabled={clearCache.isPending}
          >
            <Trash2 size={14} aria-hidden="true" />
            {clearCache.isPending ? "Clearing…" : "Clear cache for this backup"}
          </button>
        </section>
      </div>

      {stats.isPending ? <Spinner label="Measuring the backup…" /> : null}
    </div>
  );
}
