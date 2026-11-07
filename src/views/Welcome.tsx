import { open } from "@tauri-apps/plugin-dialog";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { FolderOpen, Lock, X } from "lucide-react";
import { useEffect, useState } from "react";
import { useForgetRecent, useOpenBackup, useRecentBackups } from "../api/backup";
import { errorCode } from "../api/client";
import { formatBytes, formatDate, formatDateTime } from "../components/format";
import { useUi } from "../store/ui";

/** Plain-words explanation for the errors opening can fail with. */
function explain(error: unknown): { title: string; detail: string } {
  const code = errorCode(error);
  const message = error instanceof Error ? error.message : String(error);
  switch (code) {
    case "not_a_backup":
      return {
        title: "That folder is not an iPhone backup",
        detail:
          "It has no Manifest.db. Pick the folder named after the device's UDID — the one containing Manifest.db, Info.plist and the two-character hex folders.",
      };
    case "not_a_directory":
      return {
        title: "That is not a folder",
        detail: "Choose the backup folder itself, not a file inside it.",
      };
    case "password_required":
    case "encryption_unsupported":
      return {
        title: "This backup is encrypted",
        detail:
          "Encrypted backups need the backup password, which ibex can use from milestone M7. Until then, only unencrypted backups open.",
      };
    case "missing_metadata":
      return {
        title: "This backup is missing its metadata",
        detail: `${message} The folder may be an incomplete copy.`,
      };
    default:
      return { title: "Could not open that backup", detail: message };
  }
}

export function Welcome() {
  const openBackup = useOpenBackup();
  const recents = useRecentBackups();
  const forget = useForgetRecent();
  const openedBackup = useUi((s) => s.openedBackup);
  const [failure, setFailure] = useState<{ title: string; detail: string } | null>(null);
  const [dropping, setDropping] = useState(false);

  const openPath = (path: string) => {
    setFailure(null);
    openBackup.mutate(
      { path },
      {
        onSuccess: openedBackup,
        onError: (error) => setFailure(explain(error)),
      },
    );
  };

  const pickFolder = async () => {
    const picked = await open({ directory: true, multiple: false });
    if (typeof picked === "string") openPath(picked);
  };

  // Dropping a folder on the window opens it, per docs/UI_DESIGN.md.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void getCurrentWebview()
      .onDragDropEvent((event) => {
        if (event.payload.type === "over" || event.payload.type === "enter") {
          setDropping(true);
        } else if (event.payload.type === "leave") {
          setDropping(false);
        } else if (event.payload.type === "drop") {
          setDropping(false);
          const first = event.payload.paths[0];
          if (first) openPath(first);
        }
      })
      .then((fn) => {
        unlisten = fn;
      });
    return () => unlisten?.();
    // openPath closes over stable setters only.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <div className={dropping ? "welcome welcome--dropping" : "welcome"}>
      <div style={{ textAlign: "center" }}>
        <h1 className="welcome__title">ibex</h1>
        <p className="welcome__sub">
          Browse and export the contents of an iPhone backup.
        </p>
      </div>

      <button
        type="button"
        className="button button--primary button--large"
        onClick={pickFolder}
        disabled={openBackup.isPending}
      >
        <FolderOpen size={18} aria-hidden="true" />
        {openBackup.isPending ? "Opening…" : "Open backup folder…"}
      </button>
      <p className="welcome__sub" style={{ marginTop: -12 }}>
        or drop the folder onto this window
      </p>

      {failure ? (
        <div className="card" style={{ width: "min(620px, 100%)", borderColor: "var(--danger)" }}>
          <p className="card__title" style={{ color: "var(--danger)" }}>
            {failure.title}
          </p>
          <p style={{ margin: 0 }}>{failure.detail}</p>
        </div>
      ) : null}

      {recents.data && recents.data.length > 0 ? (
        <div className="recents">
          <p className="card__title">Recent</p>
          {recents.data.map((recent) => (
            <div key={recent.path} style={{ display: "flex", gap: 6, alignItems: "stretch" }}>
              <button
                type="button"
                className="recent"
                onClick={() => openPath(recent.path)}
                disabled={!recent.exists || openBackup.isPending}
                title={recent.exists ? recent.path : "This folder has moved or been deleted"}
              >
                <span>
                  <strong>
                    {recent.deviceName ?? recent.productName ?? "Unknown device"}
                  </strong>
                  {recent.productVersion ? ` · iOS ${recent.productVersion}` : ""}
                  {recent.encrypted ? (
                    <>
                      {" "}
                      <span className="badge badge--warn">
                        <Lock size={10} aria-hidden="true" /> Encrypted
                      </span>
                    </>
                  ) : null}
                  <br />
                  <span className="recent__path">{recent.path}</span>
                </span>
                <span style={{ color: "var(--text-muted)", fontSize: 12, textAlign: "right" }}>
                  {formatDate(recent.backupDate)}
                  <br />
                  {formatBytes(recent.totalBytes)}
                </span>
                <span style={{ color: "var(--text-muted)", fontSize: 11 }}>
                  {recent.exists ? formatDateTime(recent.lastOpened) : "missing"}
                </span>
              </button>
              <button
                type="button"
                className="button button--quiet"
                onClick={() => forget.mutate(recent.path)}
                title="Remove from this list"
                aria-label={`Remove ${recent.path} from the recent list`}
              >
                <X size={14} aria-hidden="true" />
              </button>
            </div>
          ))}
        </div>
      ) : null}
    </div>
  );
}
