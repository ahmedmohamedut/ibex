import { open } from "@tauri-apps/plugin-dialog";
import { FolderOpen, Share } from "lucide-react";
import { useState } from "react";
import { useStartExport } from "../api/jobs";
import type { ExportFormat } from "../bindings/ExportFormat";
import type { PhotoFilter } from "../bindings/PhotoFilter";
import { formatCount } from "./format";
import { toastError } from "./Toasts";

/** Which conversations — or, for photos, which assets — to export. */
export type ExportScope =
  | { kind: "conversation"; id: string; label: string }
  | { kind: "section" }
  | {
      kind: "photos";
      /** The grid's current filter: what "everything" means here. */
      filter: PhotoFilter;
      label: string;
      /** An explicit multi-selection, overriding the filter when present. */
      assetIds?: string[];
    };

const FORMATS: { value: ExportFormat; label: string; detail: string }[] = [
  {
    value: "html",
    label: "HTML",
    detail: "Readable in any browser, offline, with media copied alongside.",
  },
  { value: "csv", label: "CSV", detail: "One row per message, for a spreadsheet." },
  { value: "json", label: "JSON", detail: "One object per message, for scripting." },
  {
    value: "files",
    label: "Original files",
    detail: "Just the attachments, with their original names and dates.",
  },
];

export function ExportDialog(props: {
  handle: string;
  /** `sms` or `whatsapp`. */
  source: string;
  scope: ExportScope;
  /** Shown so it is clear how much is about to be written. */
  messageCount?: number;
  onClose: () => void;
}) {
  const isPhotos = props.source === "photos";
  const startExport = useStartExport();
  const [format, setFormat] = useState<ExportFormat>(isPhotos ? "files" : "html");
  const [includeAttachments, setIncludeAttachments] = useState(true);
  const [includeBroadcasts, setIncludeBroadcasts] = useState(false);
  const [from, setFrom] = useState("");
  const [to, setTo] = useState("");

  const scopeLabel =
    props.scope.kind === "conversation" || props.scope.kind === "photos"
      ? props.scope.label
      : props.source === "whatsapp"
        ? "every WhatsApp chat"
        : "every conversation";
  const itemWord = isPhotos ? "item" : "message";

  const submit = async () => {
    try {
      const dest = await open({ directory: true, multiple: false });
      if (typeof dest !== "string") return; // cancelled
      await startExport.mutateAsync({
        handle: props.handle,
        selection: {
          source: props.source,
          conversationIds:
            props.scope.kind === "conversation"
              ? [props.scope.id]
              : props.scope.kind === "photos" && props.scope.assetIds
                ? props.scope.assetIds
                : [],
          photoFilter:
            props.scope.kind === "photos" ? props.scope.filter : null,
        },
        format,
        outDir: dest,
        options: {
          includeAttachments,
          includeBroadcasts,
          // A date input gives a local day; widen it to cover the whole day.
          from: from ? new Date(`${from}T00:00:00`).toISOString() : null,
          to: to ? new Date(`${to}T23:59:59`).toISOString() : null,
        },
      });
      props.onClose();
    } catch (error) {
      toastError("Could not start the export", error);
    }
  };

  return (
    <div
      className="modal__backdrop"
      role="dialog"
      aria-modal="true"
      aria-labelledby="export-title"
      onKeyDown={(event) => {
        if (event.key === "Escape") props.onClose();
      }}
    >
      <div className="modal" style={{ width: "min(520px, 100%)" }}>
        <h2 className="modal__title" id="export-title">
          <Share size={14} aria-hidden="true" /> Export
        </h2>
        <p style={{ marginTop: 0, color: "var(--text-muted)" }}>
          Exporting {scopeLabel}
          {props.messageCount !== undefined
            ? ` · ${formatCount(props.messageCount)} ${itemWord}${props.messageCount === 1 ? "" : "s"}`
            : ""}
          .
        </p>

        {isPhotos ? null : (
          <fieldset style={{ border: 0, padding: 0, margin: "0 0 12px" }}>
            <legend className="card__title" style={{ padding: 0 }}>
              Format
            </legend>
            {FORMATS.map((option) => (
              <label
                key={option.value}
                style={{ display: "flex", gap: 8, alignItems: "flex-start", padding: "4px 0" }}
              >
                <input
                  type="radio"
                  name="format"
                  checked={format === option.value}
                  onChange={() => setFormat(option.value)}
                />
                <span>
                  <strong>{option.label}</strong>
                  <br />
                  <span style={{ color: "var(--text-muted)", fontSize: 12 }}>
                    {option.detail}
                  </span>
                </span>
              </label>
            ))}
          </fieldset>
        )}

        <fieldset style={{ border: 0, padding: 0, margin: "0 0 12px" }}>
          <legend className="card__title" style={{ padding: 0 }}>
            Options
          </legend>
          {isPhotos ? null : (
            <label style={{ display: "flex", gap: 8, alignItems: "center", padding: "3px 0" }}>
              <input
                type="checkbox"
                checked={includeAttachments}
                onChange={(e) => setIncludeAttachments(e.target.checked)}
                disabled={format === "files"}
              />
              Copy attachment files
            </label>
          )}
          {props.source === "whatsapp" && props.scope.kind === "section" ? (
            <label style={{ display: "flex", gap: 8, alignItems: "center", padding: "3px 0" }}>
              <input
                type="checkbox"
                checked={includeBroadcasts}
                onChange={(e) => setIncludeBroadcasts(e.target.checked)}
              />
              Include status and channel feeds
            </label>
          ) : null}
          <div style={{ display: "flex", gap: 8, alignItems: "center", paddingTop: 6 }}>
            <label style={{ fontSize: 12, color: "var(--text-muted)" }}>
              From{" "}
              <input
                className="input"
                type="date"
                value={from}
                onChange={(e) => setFrom(e.target.value)}
              />
            </label>
            <label style={{ fontSize: 12, color: "var(--text-muted)" }}>
              To{" "}
              <input
                className="input"
                type="date"
                value={to}
                onChange={(e) => setTo(e.target.value)}
              />
            </label>
          </div>
        </fieldset>

        <p style={{ color: "var(--text-muted)", fontSize: 12, margin: 0 }}>
          You will be asked where to save. Nothing is sent anywhere, and the backup
          itself is never modified. An <span className="mono">export-report.json</span>{" "}
          records exactly what was written and anything that could not be.
        </p>

        <div className="modal__actions">
          <button type="button" className="button" onClick={props.onClose}>
            Cancel
          </button>
          <button
            type="button"
            className="button button--primary"
            onClick={() => void submit()}
            disabled={startExport.isPending}
          >
            <FolderOpen size={14} aria-hidden="true" />
            {startExport.isPending ? "Starting…" : "Choose folder and export…"}
          </button>
        </div>
      </div>
    </div>
  );
}
