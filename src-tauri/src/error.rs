//! The error shape that crosses IPC.
//!
//! Every command returns `Result<T, IpcError>`. The frontend shows `message`
//! and keeps `code` for deciding what to do — a wrong password or an
//! unencrypted-only build needs a specific UI state, not a generic toast.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// A failed command.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct IpcError {
    /// Stable machine-readable code, e.g. `not_a_backup`, `bad_password`.
    pub code: String,
    /// One line for the user. Carries no message content or contact details.
    pub message: String,
}

impl IpcError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
        }
    }

    /// No backup is open under that handle — usually a stale handle after the
    /// backup was closed.
    pub fn unknown_handle() -> Self {
        Self::new("unknown_handle", "That backup is no longer open")
    }

    /// The file ID does not appear in this backup's manifest.
    pub fn unknown_file() -> Self {
        Self::new("unknown_file", "No such file in this backup")
    }

    /// A blocking task panicked or was cancelled.
    pub fn task_failed() -> Self {
        Self::new("task_failed", "The operation did not finish")
    }
}

impl std::fmt::Display for IpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for IpcError {}

impl From<ibex_core::Error> for IpcError {
    fn from(error: ibex_core::Error) -> Self {
        // Core already provides stable codes and user-facing Display text.
        Self {
            code: error.code().to_string(),
            message: error.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_errors_keep_their_code() {
        let core = ibex_core::Error::Backup(ibex_core::BackupError::NotABackup {
            path: "/tmp/not-a-backup".into(),
        });
        let ipc = IpcError::from(core);
        assert_eq!(ipc.code, "not_a_backup");
        assert!(
            ipc.message.contains("Manifest.db"),
            "the message should say what to look for: {}",
            ipc.message
        );
    }

    #[test]
    fn the_password_states_are_distinguishable() {
        let required = IpcError::from(ibex_core::Error::Backup(
            ibex_core::BackupError::PasswordRequired,
        ));
        assert_eq!(required.code, "password_required");

        let unsupported = IpcError::from(ibex_core::Error::Backup(
            ibex_core::BackupError::EncryptionUnsupported,
        ));
        assert_eq!(unsupported.code, "encryption_unsupported");
    }

    #[test]
    fn it_serializes_as_camel_case_json() {
        let json = serde_json::to_string(&IpcError::new("x", "y")).unwrap();
        assert_eq!(json, r#"{"code":"x","message":"y"}"#);
    }
}
