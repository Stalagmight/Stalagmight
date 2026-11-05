//! Walls `.srv` / `.wpj` generator (placeholder until the full generator lands).

use crate::fileset::FileSet;
use crate::source::ByteSource;

/// Render one Walls survey opened on its own.
pub fn generate_srv(src: &mut ByteSource<'_>) -> FileSet {
    let _ = src.u8();
    FileSet::single("survey.srv", b"A1 A2 10 0 0\n".to_vec())
}

/// Render one Walls project with its linked surveys.
pub fn generate_wpj(src: &mut ByteSource<'_>) -> FileSet {
    let _ = src.u8();
    let mut set = FileSet::single(
        "project.wpj",
        b".BOOK P\n.NAME P\n.SURVEY S\n.NAME a\n.ENDBOOK\n".to_vec(),
    );
    set.push("a.srv", b"A1 A2 10 0 0\n".to_vec());
    set
}
