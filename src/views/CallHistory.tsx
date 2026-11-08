import { PhoneIncoming, PhoneMissed, PhoneOutgoing, Video } from "lucide-react";
import { useMemo } from "react";
import { useCallHistory } from "../api/callHistory";
import type { CallRecord } from "../bindings/CallRecord";
import { EmptyState, Spinner } from "../components/EmptyState";
import { formatDateTime } from "../components/format";
import { useUi } from "../store/ui";

function formatDuration(seconds: number): string {
  if (seconds <= 0) return "—";
  const m = Math.floor(seconds / 60);
  const s = Math.round(seconds % 60);
  return `${m}:${s.toString().padStart(2, "0")}`;
}

function serviceLabel(service: CallRecord["service"]): string {
  switch (service.kind) {
    case "telephony":
      return "Cellular";
    case "faceTime":
      return "FaceTime";
    case "other":
      return service.value || "Unknown";
  }
}

const COLUMNS = "1fr 110px 170px 90px 90px";

export function CallHistory() {
  const backup = useUi((s) => s.backup);
  const calls = useCallHistory(backup?.handle);
  const items = useMemo(() => calls.data?.items ?? [], [calls.data]);

  if (!backup) return null;

  return (
    <div className="panes panes--single">
      <div className="pane">
        <div className="pane__header">
          <span className="pane__title">Call History</span>
        </div>

        {calls.isPending ? (
          <Spinner label="Reading call history…" />
        ) : calls.isError ? (
          <EmptyState title="Could not read Call History" detail={String(calls.error)} />
        ) : items.length === 0 ? (
          <EmptyState title="No calls" detail="This backup has no call history in it." />
        ) : (
          <div className="pane__body">
            <div
              className="row"
              style={{
                gridTemplateColumns: COLUMNS,
                height: 26,
                color: "var(--text-muted)",
                fontSize: 11,
              }}
            >
              <span>From / To</span>
              <span>Direction</span>
              <span>Date</span>
              <span>Duration</span>
              <span>Service</span>
            </div>
            {items.map((call) => {
              const missed = call.answered === false;
              return (
                <div
                  key={call.id}
                  className="row"
                  style={{ gridTemplateColumns: COLUMNS, height: 30 }}
                >
                  <span
                    className="row__name"
                    dir="auto"
                    style={missed ? { color: "var(--danger)" } : undefined}
                  >
                    {call.contact.displayName ?? call.address ?? "Unknown"}
                  </span>
                  <span className="row__size" style={{ display: "flex", alignItems: "center" }}>
                    {missed ? (
                      <PhoneMissed size={13} aria-hidden="true" style={{ color: "var(--danger)" }} />
                    ) : call.outgoing ? (
                      <PhoneOutgoing size={13} aria-hidden="true" />
                    ) : (
                      <PhoneIncoming size={13} aria-hidden="true" />
                    )}
                  </span>
                  <span className="row__size">{formatDateTime(call.date)}</span>
                  <span className="row__size">{formatDuration(call.durationSeconds)}</span>
                  <span
                    className="row__size"
                    style={{ display: "flex", alignItems: "center", gap: 4 }}
                  >
                    {call.service.kind === "faceTime" ? (
                      <Video size={12} aria-hidden="true" />
                    ) : null}
                    {serviceLabel(call.service)}
                  </span>
                </div>
              );
            })}
            <div style={{ padding: "8px 10px", color: "var(--text-muted)", fontSize: 11 }}>
              {items.length} of {calls.data?.total ?? items.length} calls
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
