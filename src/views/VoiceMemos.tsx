import { Mic, Pause, Play, Share } from "lucide-react";
import { useMemo, useRef, useState } from "react";
import { useVoiceRecordings } from "../api/voice";
import { fileUrl } from "../api/media";
import type { VoiceRecording } from "../bindings/VoiceRecording";
import { EmptyState, Spinner } from "../components/EmptyState";
import { ExportDialog, type ExportScope } from "../components/ExportDialog";
import { formatBytes, formatDateTime } from "../components/format";
import { useUi } from "../store/ui";

function label(recording: VoiceRecording): string {
  return recording.title?.trim() || "Untitled recording";
}

function formatDuration(seconds: number | null): string {
  if (seconds === null) return "—";
  const m = Math.floor(seconds / 60);
  const s = Math.round(seconds % 60);
  return `${m}:${s.toString().padStart(2, "0")}`;
}

export function VoiceMemos() {
  const backup = useUi((s) => s.backup);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [exporting, setExporting] = useState<ExportScope | null>(null);
  const [playing, setPlaying] = useState(false);
  const audioRef = useRef<HTMLAudioElement>(null);

  const recordings = useVoiceRecordings(backup?.handle);
  const items = useMemo(
    () => recordings.data?.pages.flatMap((p) => p.items) ?? [],
    [recordings.data],
  );
  const total = recordings.data?.pages[0]?.total ?? null;
  const current = items.find((r) => r.id === selectedId) ?? null;

  if (!backup) return null;

  return (
    <div className="panes">
      {exporting ? (
        <ExportDialog
          handle={backup.handle}
          source="voice"
          scope={exporting}
          messageCount={
            exporting.kind === "voice" ? exporting.recordingIds?.length : undefined
          }
          onClose={() => setExporting(null)}
        />
      ) : null}

      <div className="pane pane--list">
        <div className="pane__header">
          <span className="pane__title">Voice Memos</span>
          <span className="topbar__spacer" />
          <button
            type="button"
            className="button"
            onClick={() => setExporting({ kind: "voice", label: "every recording" })}
          >
            <Share size={14} aria-hidden="true" /> Export all
          </button>
        </div>

        {recordings.isPending ? (
          <Spinner label="Reading recordings…" />
        ) : recordings.isError ? (
          <EmptyState title="Could not read Voice Memos" detail={String(recordings.error)} />
        ) : items.length === 0 ? (
          <EmptyState title="No recordings" detail="This backup has no Voice Memos in it." />
        ) : (
          <div className="pane__body">
            <div
              className="row"
              style={{
                gridTemplateColumns: "1fr 90px 90px",
                height: 26,
                color: "var(--text-muted)",
                fontSize: 11,
              }}
            >
              <span>Title</span>
              <span>Date</span>
              <span>Duration</span>
            </div>
            {items.map((recording) => (
              <button
                key={recording.id}
                type="button"
                className="row"
                style={{ gridTemplateColumns: "1fr 90px 90px", height: 30 }}
                aria-selected={recording.id === selectedId}
                onClick={() => {
                  setSelectedId(recording.id);
                  setPlaying(false);
                }}
              >
                <span className="row__name">
                  <Mic size={12} aria-hidden="true" style={{ marginRight: 6 }} />
                  {label(recording)}
                </span>
                <span className="row__size">{formatDateTime(recording.createdAt)}</span>
                <span className="row__size">{formatDuration(recording.duration)}</span>
              </button>
            ))}
            <div style={{ padding: "8px 10px", color: "var(--text-muted)", fontSize: 11 }}>
              {items.length} of {total ?? items.length} recordings
            </div>
          </div>
        )}
      </div>

      <div className="pane">
        {!current ? (
          <EmptyState title="Pick a recording" detail="Select one to play it." />
        ) : (
          <div className="pane__body" style={{ padding: 16 }}>
            <h2 className="card__title">{label(current)}</h2>
            <p style={{ color: "var(--text-muted)", fontSize: 12 }}>
              {formatDateTime(current.createdAt)} · {formatDuration(current.duration)} ·{" "}
              {formatBytes(current.size)}
            </p>

            {current.fileId ? (
              <div style={{ display: "flex", flexDirection: "column", gap: 10, marginTop: 12 }}>
                {/* A plain <audio> element, not a custom player: every
                    recording — .m4a or the newer .qta QuickTime container —
                    plays through it, the same as .mov already does for
                    WhatsApp voice notes. */}
                {/* eslint-disable-next-line jsx-a11y/media-has-caption */}
                <audio
                  ref={audioRef}
                  key={current.id}
                  src={fileUrl(backup.handle, current.fileId)}
                  controls
                  onPlay={() => setPlaying(true)}
                  onPause={() => setPlaying(false)}
                  onEnded={() => setPlaying(false)}
                />
                <div style={{ display: "flex", gap: 8 }}>
                  <button
                    type="button"
                    className="button"
                    onClick={() => {
                      const audio = audioRef.current;
                      if (!audio) return;
                      if (playing) audio.pause();
                      else void audio.play();
                    }}
                  >
                    {playing ? (
                      <>
                        <Pause size={14} aria-hidden="true" /> Pause
                      </>
                    ) : (
                      <>
                        <Play size={14} aria-hidden="true" /> Play
                      </>
                    )}
                  </button>
                  <button
                    type="button"
                    className="button"
                    onClick={() =>
                      setExporting({
                        kind: "voice",
                        label: label(current),
                        recordingIds: [current.id],
                      })
                    }
                  >
                    <Share size={14} aria-hidden="true" /> Export
                  </button>
                </div>
              </div>
            ) : (
              <EmptyState
                title="Not in backup"
                detail="This recording is listed but its file was not included in the backup."
              />
            )}
          </div>
        )}
      </div>
    </div>
  );
}
