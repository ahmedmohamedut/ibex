//! Error types for `ibex-core`.
//!
//! Missing data is deliberately *not* an error: an attachment that is absent
//! from the backup or an iCloud-only photo is represented in the data instead.
//! See `docs/ARCHITECTURE.md`.

use std::path::PathBuf;

/// Convenience alias used throughout the crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Anything that can go wrong in `ibex-core`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Backup(#[from] BackupError),

    #[error(transparent)]
    Extract(#[from] ExtractError),

    #[error(transparent)]
    Crypto(#[from] CryptoError),

    #[error("i/o error at {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("sqlite error")]
    Sqlite(#[from] rusqlite::Error),
}

impl Error {
    /// A short, stable machine-readable code, surfaced to the UI as
    /// `IpcError.code`.
    pub fn code(&self) -> &'static str {
        match self {
            Error::Backup(e) => e.code(),
            Error::Extract(e) => e.code(),
            Error::Crypto(e) => e.code(),
            Error::Io { .. } => "io",
            Error::Sqlite(_) => "sqlite",
        }
    }
}

/// Opening or reading the structure of a backup.
#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    #[error("{path} does not look like an iPhone backup: no Manifest.db")]
    NotABackup { path: PathBuf },

    #[error("{path} is not a directory")]
    NotADirectory { path: PathBuf },

    #[error("this backup is encrypted; a password is required")]
    PasswordRequired,

    /// Removed in milestone M7, which implements the crypto.
    #[error("encrypted backups are not supported yet (milestone M7)")]
    EncryptionUnsupported,

    #[error("{file} is missing from the backup")]
    MissingMetadataFile { file: &'static str },

    #[error("could not parse {file}")]
    Plist {
        file: String,
        #[source]
        source: plist::Error,
    },

    #[error("unexpected structure in {file}: {detail}")]
    MalformedMetadata { file: String, detail: String },

    #[error("no file in domain {domain} at {path}")]
    NoSuchEntry { domain: String, path: String },

    #[error("{domain}:{path} is in the manifest but its data is not in the backup")]
    DataNotInBackup { domain: String, path: String },

    #[error("could not determine a cache directory: neither XDG_DATA_HOME nor HOME is set")]
    NoCacheDir,
}

impl BackupError {
    pub fn code(&self) -> &'static str {
        match self {
            BackupError::NotABackup { .. } => "not_a_backup",
            BackupError::NotADirectory { .. } => "not_a_directory",
            BackupError::PasswordRequired => "password_required",
            BackupError::EncryptionUnsupported => "encryption_unsupported",
            BackupError::MissingMetadataFile { .. } => "missing_metadata",
            BackupError::Plist { .. } => "bad_plist",
            BackupError::MalformedMetadata { .. } => "malformed_metadata",
            BackupError::NoSuchEntry { .. } => "no_such_entry",
            BackupError::DataNotInBackup { .. } => "data_not_in_backup",
            BackupError::NoCacheDir => "no_cache_dir",
        }
    }
}

/// Reading a data source (SMS, WhatsApp, Photos, ...) out of a backup.
#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    #[error("{source_id} is not present in this backup")]
    SourceAbsent { source_id: &'static str },

    #[error("{source_id}: {detail}")]
    Unsupported {
        source_id: &'static str,
        detail: String,
    },

    #[error("{source_id}: table {table} is missing")]
    MissingTable {
        source_id: &'static str,
        table: String,
    },

    #[error("{source_id}: column {column} is missing from {table}")]
    MissingColumn {
        source_id: &'static str,
        table: String,
        column: String,
    },
}

impl ExtractError {
    pub fn code(&self) -> &'static str {
        match self {
            ExtractError::SourceAbsent { .. } => "source_absent",
            ExtractError::Unsupported { .. } => "source_unsupported",
            ExtractError::MissingTable { .. } => "missing_table",
            ExtractError::MissingColumn { .. } => "missing_column",
        }
    }
}

/// Encrypted-backup handling. Implemented in milestone M7.
#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("wrong backup password")]
    BadPassword,

    #[error("unsupported keybag: {detail}")]
    UnsupportedKeybag { detail: String },

    #[error("no key available for protection class {class}")]
    NoClassKey { class: u32 },

    #[error("decryption failed: {detail}")]
    Decrypt { detail: String },
}

impl CryptoError {
    pub fn code(&self) -> &'static str {
        match self {
            CryptoError::BadPassword => "bad_password",
            CryptoError::UnsupportedKeybag { .. } => "unsupported_keybag",
            CryptoError::NoClassKey { .. } => "no_class_key",
            CryptoError::Decrypt { .. } => "decrypt_failed",
        }
    }
}

/// Attach a path to an [`std::io::Error`]. Keeps call sites short.
pub(crate) trait IoResultExt<T> {
    fn at(self, path: impl Into<PathBuf>) -> Result<T>;
}

impl<T> IoResultExt<T> for std::io::Result<T> {
    fn at(self, path: impl Into<PathBuf>) -> Result<T> {
        self.map_err(|source| Error::Io {
            path: path.into(),
            source,
        })
    }
}
