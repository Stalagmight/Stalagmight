//! A downloaded native SQLite project, kept private until validated and installed.
use super::SpeleoDbError;
use std::path::Path;

#[derive(Debug)]
pub struct NativeFile(tempfile::NamedTempFile);
impl NativeFile {
    pub(crate) fn new() -> Result<Self, SpeleoDbError> {
        tempfile::Builder::new()
            .prefix("stalagmight-download-")
            .suffix(".stalx")
            .tempfile()
            .map(Self)
            .map_err(|e| SpeleoDbError::Io(e.to_string()))
    }
    pub fn path(&self) -> &Path {
        self.0.path()
    }
}
