//! Ariane `.tml` / `.tmlu` generator (placeholder until the full generator lands).

use crate::fileset::FileSet;
use crate::source::ByteSource;

/// Render one Ariane import: a zipped `.tml` when `zipped`, else a `.tmlu`.
pub fn generate(src: &mut ByteSource<'_>, zipped: bool) -> FileSet {
    let _ = src.u8();
    let name = if zipped { "survey.tml" } else { "survey.tmlu" };
    FileSet::single(name, b"<CaveFile><Data></Data></CaveFile>".to_vec())
}
