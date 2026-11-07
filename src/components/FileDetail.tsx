import { save } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { Check, Copy, Download, FolderOpen } from "lucide-react";
import { useState } from "react";
import { useFileInfo, useSaveFileAs } from "../api/raw";
import { EmptyState, Spinner } from "./EmptyState";
import { formatBytes, formatDateTime, formatMode } from "./format";
import { PreviewPane } from "./preview/PreviewPane";
import { toastError, useToasts } from "./Toasts";

/** The protection class names iOS uses, for the metadata table. */
const PROTECTION_CLASSES: Record<number, string> = {
  1: "Complete (1)",
  2: "Complete unless open (2)",
  3: "Complete until first auth (3)",
  4: "Always (4)",
  5: "Complete unless open, shared (5)",
  10: "No protection (10)",
  11: "Always, shared (11)",
};

export function FileDetail(props: { handle: string; fileId: string | null }) {
  const query = useFileInfo(props.handle, props.fileId);
  const saveFile = useSaveFileAs();
  const pushToast = useToasts((s) => s.push);
  const [copied, setCopied] = useState(false);

  if (!props.fileId) {
    return <EmptyState title="Nothing selected" detail="Pick a file from the list." />;
  }
  if (query.isPending) return <Spinner label="Reading metadata…" />;
  if (query.isError || !query.data) {
    return <EmptyState title="Could not read this entry" detail={String(query.error)} />;
  }
  const info = query.data;

  const copyId = async () => {
    try {
      await navigator.clipboard.writeText(info.fileId);
      setCopied(true);
      setTimeout(() => setCopied(false), 1200);
    } catch (error) {
      toastError("Could not copy the file ID", error);
    }
  };

  const saveAs = async () => {
    try {
      const dest = await save({ defaultPath: info.name });
      if (!dest) return; // cancelled
      const report = await saveFile.mutateAsync({
        handle: props.handle,
        fileId: info.fileId,
        dest,
      });
      pushToast({
        kind: "info",
        title: `Saved ${formatBytes(report.bytes)}`,
        detail: report.mtimeRestored
          ? `${report.path} — original modification time kept`
          : `${report.path} — the backup recorded no modification time`,
      });
    } catch (error) {
      toastError("Could not save the file", error);
    }
  };

  const reveal = async () => {
    if (!info.physicalPath) return;
    try {
      await revealItemInDir(info.physicalPath);
    } catch (error) {
      toastError("Could not open the containing folder", error);
    }
  };

  return (
    <div className="pane">
      <div className="pane__header">
        <span className="pane__title" title={info.relativePath}>
          {info.name}
        </span>
        <span className="topbar__spacer" />
        <button type="button" className="button" onClick={copyId} title="Copy fileID">
          {copied ? <Check size={14} aria-hidden="true" /> : <Copy size={14} aria-hidden="true" />}
          {copied ? "Copied" : "Copy ID"}
        </button>
        {info.physicalPath && info.dataPresent ? (
          <button
            type="button"
            className="button"
            onClick={reveal}
            title="Open the containing folder in the backup"
          >
            <FolderOpen size={14} aria-hidden="true" />
          </button>
        ) : null}
        <button
          type="button"
          className="button button--primary"
          onClick={saveAs}
          disabled={!info.dataPresent || saveFile.isPending}
        >
          <Download size={14} aria-hidden="true" />
          {saveFile.isPending ? "Saving…" : "Save as…"}
        </button>
      </div>

      <div style={{ flex: 1, minHeight: 0, display: "flex", flexDirection: "column" }}>
        <div style={{ flex: 1, minHeight: 0 }}>
          <PreviewPane handle={props.handle} info={info} />
        </div>

        <details style={{ borderTop: "1px solid var(--border)", flex: "0 0 auto" }}>
          <summary
            style={{ padding: "6px 12px", cursor: "pointer", background: "var(--bg-elev)" }}
          >
            Details
          </summary>
          <dl className="kv" style={{ padding: "10px 12px", maxHeight: 220, overflow: "auto" }}>
            <dt>Path</dt>
            <dd className="mono">{info.relativePath}</dd>
            <dt>Domain</dt>
            <dd className="mono">{info.domain}</dd>
            <dt>File ID</dt>
            <dd className="mono">{info.fileId}</dd>
            <dt>Kind</dt>
            <dd>
              {typeof info.kind === "string"
                ? info.kind
                : `unknown flags (${info.kind.other})`}
            </dd>
            <dt>Type</dt>
            <dd className="mono">{info.sniffed?.mime ?? "—"}</dd>
            <dt>Size</dt>
            <dd>
              {formatBytes(info.meta?.size)}
              {info.dataBytes !== null &&
              info.meta &&
              info.dataBytes !== info.meta.size ? (
                <>
                  {" "}
                  <span className="badge badge--warn">
                    on disk {formatBytes(info.dataBytes)}
                  </span>
                </>
              ) : null}
            </dd>
            <dt>Modified</dt>
            <dd>{formatDateTime(info.meta?.mtime)}</dd>
            <dt>Created</dt>
            <dd>{formatDateTime(info.meta?.birth)}</dd>
            <dt>Mode</dt>
            <dd className="mono">{formatMode(info.meta?.mode)}</dd>
            <dt>Protection</dt>
            <dd>
              {info.meta?.protectionClass === null ||
              info.meta?.protectionClass === undefined
                ? "—"
                : (PROTECTION_CLASSES[info.meta.protectionClass] ??
                  String(info.meta.protectionClass))}
            </dd>
            <dt>In backup</dt>
            <dd>{info.dataPresent ? "yes" : "no — contents not captured"}</dd>
            <dt>Physical path</dt>
            <dd className="mono">{info.physicalPath ?? "—"}</dd>
          </dl>
        </details>
      </div>
    </div>
  );
}
