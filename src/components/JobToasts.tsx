import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { CheckCircle2, CircleAlert, X } from "lucide-react";
import { useState } from "react";
import { useCancelJob, useJobEvents, useJobs } from "../api/jobs";
import type { JobInfo } from "../bindings/JobInfo";
import { formatBytes, formatCount } from "./format";
import { toastError } from "./Toasts";

/** A one-line summary of what an export did. */
function summarise(job: JobInfo): string {
  const report = job.report;
  if (!report) return job.label;
  // Photos has no "conversations" or "messages": the chat-shaped report
  // fields are reused as a count of folders and items, so the label matches.
  const isPhotos = report.source === "photos";
  const parts = isPhotos
    ? [
        `${formatCount(report.totals.conversations)} folder(s)`,
        `${formatCount(report.totals.messages)} item(s)`,
      ]
    : [
        `${formatCount(report.totals.conversations)} conversation(s)`,
        `${formatCount(report.totals.messages)} message(s)`,
      ];
  if (!isPhotos && report.totals.attachmentsCopied > 0) {
    parts.push(`${formatCount(report.totals.attachmentsCopied)} attachment(s)`);
  }
  if (report.totals.bytesWritten > 0) {
    parts.push(formatBytes(report.totals.bytesWritten));
  }
  return parts.join(" · ");
}

/**
 * Progress and completion toasts for running jobs.
 *
 * Only running jobs and recently finished ones appear; the Exports view is the
 * full history for the session.
 */
export function JobToasts() {
  useJobEvents();
  const jobs = useJobs();
  const cancelJob = useCancelJob();
  const [dismissed, setDismissed] = useState<string[]>([]);

  const visible = (jobs.data ?? []).filter(
    (job) => !dismissed.includes(job.id) && job.state !== "finished",
  );
  // Show the most recent finished export too, until it is dismissed.
  const lastFinished = (jobs.data ?? []).find(
    (job) => job.state === "finished" && !dismissed.includes(job.id),
  );
  const shown = lastFinished ? [...visible, lastFinished] : visible;
  if (shown.length === 0) return null;

  const reveal = async (path: string) => {
    try {
      await revealItemInDir(path);
    } catch (error) {
      toastError("Could not open the folder", error);
    }
  };

  return (
    <div className="toasts">
      {shown.map((job) => {
        const percent =
          job.total && job.total > 0
            ? Math.min(100, Math.round((job.done / job.total) * 100))
            : null;
        return (
          <div
            key={job.id}
            className={job.state === "failed" ? "toast toast--error" : "toast"}
            role="status"
          >
            <div className="toast__title">
              {job.state === "finished" ? (
                <>
                  <CheckCircle2 size={12} aria-hidden="true" /> Exported {job.label}
                </>
              ) : job.state === "failed" ? (
                <>
                  <CircleAlert size={12} aria-hidden="true" /> Export failed
                </>
              ) : job.state === "cancelled" ? (
                <>Export cancelled</>
              ) : (
                <>Exporting {job.label}…</>
              )}
            </div>

            {job.state === "running" ? (
              <>
                <p className="toast__detail">
                  {percent === null ? "Working…" : `${percent}%`}
                  {job.total ? ` (${job.done}/${job.total})` : ""}
                </p>
                <div
                  aria-hidden="true"
                  style={{
                    height: 3,
                    borderRadius: 2,
                    background: "var(--border)",
                    overflow: "hidden",
                  }}
                >
                  <div
                    style={{
                      width: `${percent ?? 10}%`,
                      height: "100%",
                      background: "var(--accent)",
                    }}
                  />
                </div>
              </>
            ) : (
              <p className="toast__detail">
                {job.state === "failed" ? job.error : summarise(job)}
                {job.report && job.report.totals.attachmentsMissing > 0 ? (
                  <>
                    <br />
                    {formatCount(job.report.totals.attachmentsMissing)}{" "}
                    {job.report.source === "photos" ? "item(s)" : "attachment(s)"}
                    were not in the backup.
                  </>
                ) : null}
              </p>
            )}

            <div className="modal__actions">
              {job.state === "running" ? (
                <button
                  type="button"
                  className="button button--quiet"
                  onClick={() => cancelJob.mutate(job.id)}
                >
                  <X size={12} aria-hidden="true" /> Cancel
                </button>
              ) : (
                <>
                  {job.report ? (
                    <button
                      type="button"
                      className="button button--quiet"
                      onClick={() => void reveal(job.report!.outputDir)}
                    >
                      Open folder
                    </button>
                  ) : null}
                  <button
                    type="button"
                    className="button button--quiet"
                    onClick={() => setDismissed((d) => [...d, job.id])}
                  >
                    Dismiss
                  </button>
                </>
              )}
            </div>
          </div>
        );
      })}
    </div>
  );
}
