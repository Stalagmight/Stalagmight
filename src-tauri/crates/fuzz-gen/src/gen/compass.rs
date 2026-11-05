//! Compass `.dat` / `.mak` generator (placeholder until the full generator lands).

use crate::fileset::FileSet;
use crate::source::ByteSource;

/// Render one Compass survey opened on its own.
pub fn generate_dat(src: &mut ByteSource<'_>) -> FileSet {
    let _ = src.u8();
    FileSet::single("survey.dat", b"CAVE\r\n".to_vec())
}

/// Render one Compass project with its linked surveys.
pub fn generate_mak(src: &mut ByteSource<'_>) -> FileSet {
    let _ = src.u8();
    let mut set = FileSet::single("project.mak", b"#a.dat;\r\n".to_vec());
    set.push("a.dat", b"CAVE\r\n".to_vec());
    set
}
