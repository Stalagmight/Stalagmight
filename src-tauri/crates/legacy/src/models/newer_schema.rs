use serde::Serialize;

/// A file a newer Stalagmight saved than this one can read (#649): a `.stalx`
/// whose schema version, or a `.plnx` whose dive-plan format, is above what this
/// build knows.
///
/// One type for every surface that reports the refusal, so they all say the
/// same thing: the persistence error inside an open, the load error's payload,
/// and the SpeleoDB error's payload. Its `Display` is the one English wording;
/// the frontend renders the same sentence from `newerSchema.message` in
/// `en.json` with these fields as parameters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NewerSchema {
    /// The refused file's name (no directory), as the user knows it.
    pub file: String,
    /// The file's format version.
    pub found: i32,
    /// The newest format version this build reads.
    pub supported: i32,
}

impl std::fmt::Display for NewerSchema {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "\"{}\" was saved by a newer version of Stalagmight (file format {}; this \
             version reads up to {}). Update Stalagmight to open it.",
            self.file, self.found, self.supported
        )
    }
}

impl std::error::Error for NewerSchema {}
