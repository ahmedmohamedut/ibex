import { Lock, Monitor, Moon, Search, Share, Sun, X } from "lucide-react";
import { useCloseBackup } from "../api/backup";
import { formatDate } from "./format";
import { useUi, type Theme } from "../store/ui";

const THEMES: { value: Theme; label: string; Icon: typeof Sun }[] = [
  { value: "system", label: "Match the system theme", Icon: Monitor },
  { value: "light", label: "Light theme", Icon: Sun },
  { value: "dark", label: "Dark theme", Icon: Moon },
];

export function TopBar() {
  const backup = useUi((s) => s.backup);
  const closedBackup = useUi((s) => s.closedBackup);
  const theme = useUi((s) => s.theme);
  const setTheme = useUi((s) => s.setTheme);
  const openSearch = useUi((s) => s.openSearch);
  const closeBackup = useCloseBackup();

  const next = THEMES[(THEMES.findIndex((t) => t.value === theme) + 1) % THEMES.length];
  const current = THEMES.find((t) => t.value === theme) ?? THEMES[0];
  const { Icon } = current;

  const close = () => {
    if (backup) closeBackup.mutate(backup.handle);
    closedBackup();
  };

  return (
    <header className="topbar">
      {backup ? (
        <>
          <span className="topbar__device">
            {backup.info.productName ?? backup.info.deviceName ?? "Unknown device"}
            {backup.info.productVersion ? ` · iOS ${backup.info.productVersion}` : ""}
            {` · ${formatDate(backup.info.lastBackupDate ?? backup.info.backupDate)}`}
          </span>
          {backup.encrypted ? (
            <span className="badge badge--warn">
              <Lock size={11} aria-hidden="true" /> Encrypted
            </span>
          ) : null}
          <button type="button" className="button button--quiet" onClick={close}>
            <X size={14} aria-hidden="true" /> Close
          </button>
        </>
      ) : (
        <span className="topbar__device">ibex</span>
      )}

      <span className="topbar__spacer" />

      <button
        type="button"
        className="button"
        onClick={openSearch}
        disabled={!backup}
        title="Search (Ctrl+K)"
      >
        <Search size={14} aria-hidden="true" /> Search
      </button>
      {/* Export already exists per conversation/selection/section; a
          top-bar "export everything" trigger is a later polish item. */}
      <button type="button" className="button" disabled title="Export arrives in milestone M4">
        <Share size={14} aria-hidden="true" /> Export
      </button>
      <button
        type="button"
        className="button button--quiet"
        onClick={() => setTheme(next.value)}
        title={`${current.label}. Click for: ${next.label}`}
        aria-label={current.label}
      >
        <Icon size={14} aria-hidden="true" />
      </button>
    </header>
  );
}
