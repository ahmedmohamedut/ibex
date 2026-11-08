import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { useCancelJob, useJobEvents, useJobs } from "../api/jobs";
import { EmptyState, Spinner } from "../components/EmptyState";
import { formatBytes, formatCount, formatDateTime } from "../components/format";
import { toastError } from "../components/Toasts";

/** This session's exports. */
export function Exports() {
  useJobEvents();
  const jobs = useJobs();
  const cancelJob = useCancelJob();

  const reveal = async (path: string) => {
    try {
      await revealItemInDir(path);
    } catch (error) {
      toastError("Could not open the folder", error);
    }
  };

  if (jobs.isPending) return <Spinner label="Reading jobs…" />;
  if (jobs.isError) {
    return <EmptyState title="Could not list exports" detail={String(jobs.error)} />;
  }
  const items = jobs.data ?? [];
  if (items.length === 0) {
    return (
      <EmptyState
        title="No exports yet"
        detail="Use the Export button in Messages or WhatsApp. This list covers the current session; the export folders themselves stay where you put them."
      />
    );
  }

  return (
    <div className="pane__body">
      <div className="cards">
        {items.map((job) => {
          const report = job.report;
          return (
            <section className="card" key={job.id}>
              <h2 className="card__title">
                {job.label} · {job.state}
              </h2>
              <dl className="kv">
                <dt>Started</dt>
                <dd>{formatDateTime(job.startedAt)}</dd>
                {report ? (
                  <>
                    <dt>Format</dt>
                    <dd>{report.format}</dd>
                    <dt>Conversations</dt>
                    <dd>{formatCount(report.totals.conversations)}</dd>
                    <dt>Messages</dt>
                    <dd>{formatCount(report.totals.messages)}</dd>
                    <dt>Attachments</dt>
                    <dd>
                      {formatCount(report.totals.attachmentsCopied)} copied
                      {report.totals.attachmentsMissing > 0
                        ? `, ${formatCount(report.totals.attachmentsMissing)} not in backup`
                        : ""}
                    </dd>
                    <dt>Written</dt>
                    <dd>{formatBytes(report.totals.bytesWritten)}</dd>
                    <dt>Folder</dt>
                    <dd className="mono">{report.outputDir}</dd>
                  </>
                ) : (
                  <>
                    <dt>Progress</dt>
                    <dd>
                      {job.done}
                      {job.total ? ` / ${job.total}` : ""}
                    </dd>
                  </>
                )}
                {job.error ? (
                  <>
                    <dt>Error</dt>
                    <dd style={{ color: "var(--danger)" }}>{job.error}</dd>
                  </>
                ) : null}
              </dl>

              {report && report.warnings.length > 0 ? (
                <ul style={{ margin: "8px 0 0", paddingLeft: 18, fontSize: 12, color: "var(--warning)" }}>
                  {report.warnings.map((warning, index) => (
                    <li key={index}>{warning}</li>
                  ))}
                </ul>
              ) : null}

              <div style={{ display: "flex", gap: 6, marginTop: 10 }}>
                {job.state === "running" ? (
                  <button
                    type="button"
                    className="button"
                    onClick={() => cancelJob.mutate(job.id)}
                  >
                    Cancel
                  </button>
                ) : null}
                {report ? (
                  <button
                    type="button"
                    className="button"
                    onClick={() => void reveal(report.outputDir)}
                  >
                    Open folder
                  </button>
                ) : null}
              </div>
            </section>
          );
        })}
      </div>
    </div>
  );
}
