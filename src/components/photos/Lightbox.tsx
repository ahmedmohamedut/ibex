import {
  ChevronLeft,
  ChevronRight,
  Clapperboard,
  Download,
  Heart,
  MapPin,
  Pencil,
  Sparkles,
  X,
} from "lucide-react";
import { useEffect } from "react";
import type { PhotoAsset } from "../../bindings/PhotoAsset";
import { fileUrl, thumbUrl } from "../../api/media";
import { formatBytes, formatDateTime } from "../format";

/** Full-screen view of one asset, with arrow-key navigation and metadata. */
export function Lightbox(props: {
  handle: string;
  asset: PhotoAsset;
  hasPrev: boolean;
  hasNext: boolean;
  onPrev: () => void;
  onNext: () => void;
  onClose: () => void;
  onExportOne: () => void;
}) {
  const { asset } = props;

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") props.onClose();
      else if (event.key === "ArrowLeft" && props.hasPrev) props.onPrev();
      else if (event.key === "ArrowRight" && props.hasNext) props.onNext();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [asset.id, props.hasPrev, props.hasNext]);

  const displayFileId = asset.editedFileId ?? asset.fileId;
  const liveFileId = asset.editedFileId ? asset.editedLiveFileId ?? asset.liveFileId : asset.liveFileId;
  const isMissing = !displayFileId;

  return (
    <div
      className="lightbox"
      role="dialog"
      aria-modal="true"
      aria-label={asset.fileName}
    >
      <div className="lightbox__toolbar">
        <button type="button" className="button button--quiet" onClick={props.onClose}>
          <X size={16} aria-hidden="true" /> Close
        </button>
        <span className="topbar__spacer" />
        <button type="button" className="button" onClick={props.onExportOne} disabled={isMissing}>
          <Download size={14} aria-hidden="true" /> Export
        </button>
      </div>

      <div className="lightbox__stage">
        {props.hasPrev ? (
          <button
            type="button"
            className="lightbox__nav lightbox__nav--prev"
            onClick={props.onPrev}
            aria-label="Previous"
          >
            <ChevronLeft size={28} aria-hidden="true" />
          </button>
        ) : null}

        {isMissing ? (
          <div className="lightbox__missing">This asset is not in the backup.</div>
        ) : asset.kind === "video" && asset.fileId ? (
          // eslint-disable-next-line jsx-a11y/media-has-caption
          <video
            key={asset.id}
            src={fileUrl(props.handle, asset.fileId)}
            controls
            autoPlay={false}
            className="lightbox__media"
          />
        ) : liveFileId ? (
          <LivePhoto
            key={asset.id}
            handle={props.handle}
            stillId={displayFileId!}
            liveId={liveFileId}
          />
        ) : (
          <img
            key={asset.id}
            src={thumbUrl(props.handle, displayFileId!)}
            alt=""
            className="lightbox__media"
          />
        )}

        {props.hasNext ? (
          <button
            type="button"
            className="lightbox__nav lightbox__nav--next"
            onClick={props.onNext}
            aria-label="Next"
          >
            <ChevronRight size={28} aria-hidden="true" />
          </button>
        ) : null}
      </div>

      <div className="lightbox__meta">
        <div className="lightbox__meta-title">
          {asset.fileName}
          {asset.favorite ? <Heart size={13} aria-hidden="true" /> : null}
          {asset.edited ? (
            <span title="Edited">
              <Pencil size={13} aria-hidden="true" />
            </span>
          ) : null}
          {asset.live ? (
            <span title="Live Photo">
              <Sparkles size={13} aria-hidden="true" />
            </span>
          ) : null}
          {asset.kind === "video" ? <Clapperboard size={13} aria-hidden="true" /> : null}
        </div>
        <dl className="lightbox__meta-grid">
          <dt>Taken</dt>
          <dd>{formatDateTime(asset.createdAt)}</dd>
          <dt>Dimensions</dt>
          <dd>
            {asset.width && asset.height ? `${asset.width} × ${asset.height}` : "—"}
          </dd>
          {asset.duration ? (
            <>
              <dt>Duration</dt>
              <dd>{Math.round(asset.duration)}s</dd>
            </>
          ) : null}
          <dt>File</dt>
          <dd>{formatBytes(asset.size)}</dd>
          {asset.originalFileName ? (
            <>
              <dt>Original name</dt>
              <dd>{asset.originalFileName}</dd>
            </>
          ) : null}
          {asset.location ? (
            <>
              <dt>
                <MapPin size={11} aria-hidden="true" /> Location
              </dt>
              <dd>
                {asset.location.latitude.toFixed(5)}, {asset.location.longitude.toFixed(5)}
              </dd>
            </>
          ) : null}
          {asset.availability === "cloudOnly" ? (
            <>
              <dt>Status</dt>
              <dd>Not downloaded to the device (iCloud "Optimize Storage")</dd>
            </>
          ) : null}
        </dl>
      </div>
    </div>
  );
}

/** A still that plays its companion video while the pointer is held down. */
function LivePhoto(props: { handle: string; stillId: string; liveId: string }) {
  return (
    <div
      className="lightbox__live"
      onPointerDown={(event) => {
        const video = event.currentTarget.querySelector("video");
        if (video) {
          video.currentTime = 0;
          void video.play();
        }
      }}
      onPointerUp={(event) => {
        const video = event.currentTarget.querySelector("video");
        video?.pause();
      }}
    >
      <img src={thumbUrl(props.handle, props.stillId)} alt="" className="lightbox__media" />
      <video
        src={fileUrl(props.handle, props.liveId)}
        muted
        loop
        playsInline
        className="lightbox__media lightbox__live-video"
      />
      <span className="lightbox__live-badge">
        <Sparkles size={11} aria-hidden="true" /> Live — press and hold
      </span>
    </div>
  );
}
