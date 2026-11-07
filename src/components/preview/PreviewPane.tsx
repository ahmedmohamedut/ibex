import { useState } from "react";
import { useHexPreview, useTextPreview } from "../../api/raw";
import { fileUrl } from "../../api/media";
import type { FileInfo } from "../../bindings/FileInfo";
import { EmptyState, Spinner } from "../EmptyState";
import { formatBytes } from "../format";

/** Render the bytes as a hex dump with an ASCII gutter. */
function hexDump(hex: string, offset: number): string {
  const bytes: number[] = [];
  for (let i = 0; i + 1 < hex.length; i += 2) {
    bytes.push(Number.parseInt(hex.slice(i, i + 2), 16));
  }
  const lines: string[] = [];
  for (let row = 0; row < bytes.length; row += 16) {
    const chunk = bytes.slice(row, row + 16);
    const address = (offset + row).toString(16).padStart(8, "0");
    const columns = chunk.map((b) => b.toString(16).padStart(2, "0"));
    // A gap after 8 bytes, the conventional hex-dump shape.
    const left = columns.slice(0, 8).join(" ").padEnd(23, " ");
    const right = columns.slice(8).join(" ").padEnd(23, " ");
    const ascii = chunk
      .map((b) => (b >= 0x20 && b < 0x7f ? String.fromCharCode(b) : "."))
      .join("");
    lines.push(`${address}  ${left}  ${right}  |${ascii}|`);
  }
  return lines.join("\n");
}

function HexPreviewView(props: { handle: string; fileId: string }) {
  const [offset, setOffset] = useState(0);
  const query = useHexPreview(props.handle, props.fileId, offset, true);

  if (query.isPending) return <Spinner label="Reading…" />;
  if (query.isError || !query.data) {
    return <EmptyState title="Could not read this file" detail={String(query.error)} />;
  }

  const { hex, totalBytes } = query.data;
  const end = offset + query.data.len;

  return (
    <div className="preview">
      <div className="preview__note">
        Bytes {offset.toLocaleString()}–{end.toLocaleString()} of{" "}
        {totalBytes.toLocaleString()}
        <span style={{ marginLeft: 8, display: "inline-flex", gap: 6 }}>
          <button
            type="button"
            className="button button--quiet"
            disabled={offset === 0}
            onClick={() => setOffset(Math.max(0, offset - 2048))}
          >
            Previous
          </button>
          <button
            type="button"
            className="button button--quiet"
            disabled={end >= totalBytes}
            onClick={() => setOffset(end)}
          >
            Next
          </button>
        </span>
      </div>
      <div className="pane__body">
        <pre className="preview__hex">{hexDump(hex, offset)}</pre>
      </div>
    </div>
  );
}

function TextPreviewView(props: { handle: string; fileId: string; isPlist: boolean }) {
  const query = useTextPreview(props.handle, props.fileId, true);

  if (query.isPending) return <Spinner label="Reading…" />;
  if (query.isError || !query.data) {
    return <EmptyState title="Could not read this file" detail={String(query.error)} />;
  }
  const preview = query.data;

  return (
    <div className="preview">
      {preview.truncated || preview.lossy || (props.isPlist && !preview.plistRendered) ? (
        <div className="preview__note">
          {preview.truncated
            ? `Showing the first ${formatBytes(preview.bytesRead)} of ${formatBytes(preview.totalBytes)}. `
            : ""}
          {preview.lossy ? "Some bytes were not valid text and are shown as �. " : ""}
          {props.isPlist && !preview.plistRendered
            ? "This file did not parse as a property list, so it is shown as-is."
            : ""}
        </div>
      ) : null}
      {props.isPlist && preview.plistRendered ? (
        <div className="preview__note">Property list, shown as XML.</div>
      ) : null}
      <div className="pane__body">
        <pre className="preview__text">{preview.text}</pre>
      </div>
    </div>
  );
}

/**
 * The detail pane's preview, chosen by the type Rust sniffed.
 *
 * Media and images load over `ibex://` rather than through `invoke`, so the
 * webview can stream and seek them.
 */
export function PreviewPane(props: { handle: string; info: FileInfo }) {
  const { info, handle } = props;
  const [forceHex, setForceHex] = useState(false);

  if (info.kind === "directory") {
    return (
      <EmptyState
        title="Folder"
        detail="Double-click it in the list to open it."
      />
    );
  }
  if (!info.dataPresent) {
    return (
      <EmptyState
        title="Not in backup"
        detail="This file is listed in the manifest, but its contents were never captured. That is normal for iCloud-only data and for apps that opt out of backup."
      />
    );
  }

  const url = fileUrl(handle, info.fileId);
  const kind = forceHex ? "binary" : (info.sniffed?.preview ?? "binary");

  const hexToggle = (
    <div className="preview__note">
      <button
        type="button"
        className="button button--quiet"
        onClick={() => setForceHex((v) => !v)}
      >
        {forceHex ? "Show preview" : "Show hex"}
      </button>
    </div>
  );

  switch (kind) {
    case "image":
      return (
        <div className="preview">
          {hexToggle}
          <div className="preview__canvas">
            <img className="preview__image" src={url} alt={info.name} />
          </div>
        </div>
      );

    case "imageUnsupported":
      return (
        <div className="preview">
          {hexToggle}
          <EmptyState
            title="Preview needs HEIC support"
            detail={`This is ${info.sniffed?.mime ?? "an image"}, which this webview cannot decode. Milestone M5 adds conversion. You can still save the file out.`}
          />
        </div>
      );

    case "audio":
      return (
        <div className="preview">
          {hexToggle}
          <div className="preview__canvas">
            {/* eslint-disable-next-line jsx-a11y/media-has-caption */}
            <audio className="preview__media" controls preload="metadata" src={url} />
          </div>
        </div>
      );

    case "video":
      return (
        <div className="preview">
          {hexToggle}
          <div className="preview__canvas">
            {/* eslint-disable-next-line jsx-a11y/media-has-caption */}
            <video className="preview__media" controls preload="metadata" src={url} />
          </div>
        </div>
      );

    case "pdf":
      return (
        <div className="preview">
          {hexToggle}
          {/* WebKit's own PDF viewer, reached through ibex://. */}
          <iframe className="preview__pdf" src={url} title={info.name} />
        </div>
      );

    case "text":
      return (
        <div className="preview">
          {hexToggle}
          <TextPreviewView handle={handle} fileId={info.fileId} isPlist={false} />
        </div>
      );

    case "plist":
      return (
        <div className="preview">
          {hexToggle}
          <TextPreviewView handle={handle} fileId={info.fileId} isPlist />
        </div>
      );

    default:
      return (
        <div className="preview">
          {forceHex ? hexToggle : null}
          <HexPreviewView handle={handle} fileId={info.fileId} />
        </div>
      );
  }
}
