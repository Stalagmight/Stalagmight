//! Why a legacy open failed.

/// Why [`crate::open_path`] / [`crate::open_bytes`] refused a file.
///
/// Every variant except [`LegacyError::Io`] is deterministic for a given
/// input: retrying cannot help.
#[derive(Debug, thiserror::Error)]
pub enum LegacyError {
    /// Not an SQLite database at all (wrong header, too short, or SQLite says
    /// so). A v2 JSON `.stalx` lands here too: sniff before calling.
    #[error("not an SQLite .stalx file")]
    NotSqlite,
    /// Written by a schema newer than the frozen 0.6.0 reader knows.
    #[error("file uses .stalx schema v{found}; the legacy reader supports up to v{supported}")]
    NewerSchema { found: i32, supported: i32 },
    /// Larger than [`crate::OpenOptions::max_file_bytes`] (the file, or the
    /// file plus its journal sidecar).
    #[error("input is over the {max}-byte limit")]
    TooLarge { max: u64 },
    /// SQLite, but not something the 0.6.0 app opened: a failed integrity or
    /// foreign-key check, a schema that would run its own SQL, no project
    /// data, or any other refusal of the old load path. The text can quote the
    /// file (SQLite error context): log it, do not show it to other users.
    #[error("not a readable legacy .stalx: {0}")]
    Corrupt(String),
    /// Reading the input or writing the private copy failed.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

impl LegacyError {
    /// Classify an error from the frozen load path: SQLite result codes from
    /// the error chain first, then the frozen persistence layer's typed
    /// refusals, then [`LegacyError::Corrupt`] with the whole chain as text.
    pub(crate) fn from_load(e: &anyhow::Error) -> Self {
        if let Some(newer) = e
            .chain()
            .find_map(|cause| cause.downcast_ref::<crate::models::NewerSchema>())
        {
            return Self::NewerSchema {
                found: newer.found,
                supported: newer.supported,
            };
        }
        let sqlite = e
            .chain()
            .find_map(|cause| cause.downcast_ref::<rusqlite::Error>())
            .and_then(rusqlite::Error::sqlite_error_code);
        if sqlite == Some(rusqlite::ErrorCode::NotADatabase) {
            return Self::NotSqlite;
        }
        Self::Corrupt(format!("{e:#}"))
    }

    pub(crate) fn from_sqlite(e: &rusqlite::Error) -> Self {
        if e.sqlite_error_code() == Some(rusqlite::ErrorCode::NotADatabase) {
            Self::NotSqlite
        } else {
            Self::Corrupt(e.to_string())
        }
    }
}
