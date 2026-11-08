//! Long-running work: exports now, the search index in M8.
//!
//! A job runs on a blocking worker and reports back by event, so the UI never
//! waits on it. Two details matter:
//!
//! - **Progress is throttled here, not in the UI.** Exporting a
//!   66,407-message conversation would otherwise emit tens of thousands of
//!   events and the IPC channel becomes the bottleneck. Roughly ten a second is
//!   enough for a progress bar to look alive.
//! - **Finished jobs stay in the registry** for the session, so the Exports
//!   view can show their reports after the toast has gone.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use ibex_core::export::{CancelToken, ExportReport};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Runtime};
use ts_rs::TS;

/// Smallest gap between progress events.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// Opaque job id.
pub type JobId = String;

/// What a job is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum JobState {
    Running,
    Finished,
    Failed,
    Cancelled,
}

/// A job as the UI sees it.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct JobInfo {
    pub id: JobId,
    /// What kind of work: `export` for now.
    pub kind: String,
    /// A short description with no message content in it.
    pub label: String,
    pub state: JobState,
    pub done: u64,
    pub total: Option<u64>,
    /// Present once an export has finished.
    pub report: Option<ExportReport>,
    pub error: Option<String>,
    pub started_at: String,
}

/// Progress payload for `job://progress`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct JobProgressEvent {
    pub job_id: JobId,
    pub done: u64,
    pub total: Option<u64>,
    pub message: String,
}

/// Payload for `job://finished`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct JobFinishedEvent {
    pub job_id: JobId,
    pub report: ExportReport,
}

/// Payload for `job://failed`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct JobFailedEvent {
    pub job_id: JobId,
    pub error: String,
}

/// One job's live state and its cancel handle.
struct Job {
    info: JobInfo,
    cancel: CancelToken,
}

/// Every job this session has started.
#[derive(Default)]
pub struct JobRegistry {
    jobs: Mutex<HashMap<JobId, Job>>,
    next: AtomicU64,
}

impl JobRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new job and hand back its id and cancel token.
    pub fn start(&self, kind: &str, label: &str) -> (JobId, CancelToken) {
        let id = format!("job-{}", self.next.fetch_add(1, Ordering::Relaxed) + 1);
        let cancel = CancelToken::new();
        let info = JobInfo {
            id: id.clone(),
            kind: kind.to_string(),
            label: label.to_string(),
            state: JobState::Running,
            done: 0,
            total: None,
            report: None,
            error: None,
            started_at: chrono::Utc::now().to_rfc3339(),
        };
        self.lock().insert(
            id.clone(),
            Job {
                info,
                cancel: cancel.clone(),
            },
        );
        (id, cancel)
    }

    /// Update a running job's counters.
    pub fn progress(&self, id: &str, done: u64, total: Option<u64>) {
        if let Some(job) = self.lock().get_mut(id) {
            job.info.done = done;
            job.info.total = total;
        }
    }

    /// Record a finished export.
    pub fn finished(&self, id: &str, report: ExportReport) {
        if let Some(job) = self.lock().get_mut(id) {
            job.info.state = if report.cancelled {
                JobState::Cancelled
            } else {
                JobState::Finished
            };
            job.info.done = report.totals.conversations;
            job.info.total = Some(report.totals.conversations);
            job.info.report = Some(report);
        }
    }

    /// Record a failure.
    pub fn failed(&self, id: &str, error: String) {
        if let Some(job) = self.lock().get_mut(id) {
            job.info.state = JobState::Failed;
            job.info.error = Some(error);
        }
    }

    /// Ask a job to stop. Returns whether it was running.
    pub fn cancel(&self, id: &str) -> bool {
        match self.lock().get(id) {
            Some(job) if job.info.state == JobState::Running => {
                job.cancel.cancel();
                true
            }
            _ => false,
        }
    }

    /// Every job, newest first.
    pub fn list(&self) -> Vec<JobInfo> {
        let mut jobs: Vec<JobInfo> = self.lock().values().map(|j| j.info.clone()).collect();
        jobs.sort_by(|a, b| b.started_at.cmp(&a.started_at).then(b.id.cmp(&a.id)));
        jobs
    }

    /// One job.
    pub fn get(&self, id: &str) -> Option<JobInfo> {
        self.lock().get(id).map(|j| j.info.clone())
    }

    /// A poisoned lock means a previous holder panicked. The registry is
    /// bookkeeping, so recovering beats failing every later job.
    fn lock(&self) -> MutexGuard<'_, HashMap<JobId, Job>> {
        self.jobs.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Emits `job://progress`, no more often than [`PROGRESS_INTERVAL`].
///
/// The first and last updates always go out, so a job that finishes quickly
/// still shows something and a finished bar always reaches its end.
pub struct ProgressThrottle<R: Runtime> {
    app: AppHandle<R>,
    job_id: JobId,
    last: Mutex<Option<Instant>>,
}

impl<R: Runtime> ProgressThrottle<R> {
    pub fn new(app: AppHandle<R>, job_id: JobId) -> Self {
        Self {
            app,
            job_id,
            last: Mutex::new(None),
        }
    }

    /// Forward one progress update, dropping it if it is too soon after the
    /// previous one.
    pub fn emit(&self, progress: &ibex_core::export::Progress, registry: &JobRegistry) {
        registry.progress(&self.job_id, progress.done, progress.total);

        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let due = match *last {
            None => true,
            Some(previous) => now.duration_since(previous) >= PROGRESS_INTERVAL,
        };
        // Always let the final update through, so the bar completes.
        let finished = progress.total.is_some_and(|t| progress.done >= t);
        if !due && !finished {
            return;
        }
        *last = Some(now);
        drop(last);

        let _ = self.app.emit(
            "job://progress",
            JobProgressEvent {
                job_id: self.job_id.clone(),
                done: progress.done,
                total: progress.total,
                message: progress.message.clone(),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_jobs_start_running() {
        let registry = JobRegistry::new();
        let (first, _) = registry.start("export", "SMS to HTML");
        let (second, _) = registry.start("export", "WhatsApp to CSV");
        assert_ne!(first, second);

        let info = registry.get(&first).unwrap();
        assert_eq!(info.state, JobState::Running);
        assert_eq!(info.kind, "export");
        assert_eq!(info.done, 0);
        assert_eq!(registry.list().len(), 2);
    }

    #[test]
    fn cancelling_trips_the_token_the_exporter_holds() {
        let registry = JobRegistry::new();
        let (id, token) = registry.start("export", "x");
        assert!(!token.is_cancelled());
        assert!(registry.cancel(&id));
        assert!(token.is_cancelled());

        // Cancelling something that is not running is a no-op, not an error.
        assert!(!registry.cancel("job-does-not-exist"));
    }

    #[test]
    fn a_finished_job_keeps_its_report_for_the_session() {
        let registry = JobRegistry::new();
        let (id, _) = registry.start("export", "x");

        let report = ExportReport::empty(
            ibex_core::ExportFormat::Html,
            "sms",
            std::path::Path::new("/o"),
            false,
        );
        registry.finished(&id, report);

        let info = registry.get(&id).unwrap();
        assert_eq!(info.state, JobState::Finished);
        assert!(info.report.is_some(), "the Exports view reads this later");
        // And cancelling a finished job does nothing.
        assert!(!registry.cancel(&id));
    }

    #[test]
    fn a_cancelled_export_is_recorded_as_cancelled_not_finished() {
        let registry = JobRegistry::new();
        let (id, _) = registry.start("export", "x");

        let report = ExportReport::empty(
            ibex_core::ExportFormat::Csv,
            "sms",
            std::path::Path::new("/o"),
            true,
        );
        registry.finished(&id, report);

        assert_eq!(registry.get(&id).unwrap().state, JobState::Cancelled);
    }

    #[test]
    fn a_failure_is_recorded_with_its_message() {
        let registry = JobRegistry::new();
        let (id, _) = registry.start("export", "x");
        registry.failed(&id, "disk full".to_string());

        let info = registry.get(&id).unwrap();
        assert_eq!(info.state, JobState::Failed);
        assert_eq!(info.error.as_deref(), Some("disk full"));
    }

    #[test]
    fn progress_updates_the_registry_even_between_emitted_events() {
        // Throttling controls how often the *UI* hears, not what the registry
        // knows, so the Exports view is never stale.
        let registry = JobRegistry::new();
        let (id, _) = registry.start("export", "x");
        registry.progress(&id, 5, Some(10));
        let info = registry.get(&id).unwrap();
        assert_eq!(info.done, 5);
        assert_eq!(info.total, Some(10));
    }

    #[test]
    fn jobs_are_listed_newest_first() {
        let registry = JobRegistry::new();
        let (first, _) = registry.start("export", "first");
        let (second, _) = registry.start("export", "second");
        let listed = registry.list();
        assert_eq!(listed.len(), 2);
        // Ids break ties when two jobs start in the same instant.
        assert_eq!(listed[0].id, second);
        assert_eq!(listed[1].id, first);
    }
}
