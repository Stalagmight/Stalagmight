//! Byte-order-mark handling shared by every text-based importer.
//!
//! Windows tooling writes "UTF-8 with BOM" by default (Notepad, PowerShell 5.1
//! `Out-File`, a common VS Code setting), so a survey file's first line can
//! arrive with `EF BB BF` glued to it.  The Walls and Compass readers map each
//! byte to a Latin-1 code point and the `.dmp` reader parses its first token as
//! an integer, so those three bytes void the file's first `#UNITS` / `.BOOK` /
//! cave name with no warning at all, or fail the import outright (#429). The
//! XML readers (GPX, TML/TMLU) survive a mark today, but only because
//! `quick-xml` skips text ahead of the root element; they read through here too
//! so that stays a property of one place rather than of the XML crate.
//!
//! A UTF-16 mark is refused rather than transcoded.  None of these formats are
//! written as UTF-16 in practice, and reading one anyway yields NUL-separated
//! bytes that match no directive at all - an import that comes back empty with
//! nothing to explain it.

use std::io::Read;
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};

/// `EF BB BF`, the UTF-8 encoding of U+FEFF.
const UTF8_BOM: [u8; 3] = [0xef, 0xbb, 0xbf];

/// The endianness a leading UTF-16 byte-order mark declares, if any.
fn utf16_endianness(bytes: &[u8]) -> Option<&'static str> {
    match bytes {
        [0xff, 0xfe, ..] => Some("little-endian"),
        [0xfe, 0xff, ..] => Some("big-endian"),
        _ => None,
    }
}

/// Drop a leading UTF-8 byte-order mark, and refuse a UTF-16 one.
///
/// The error is self-contained: callers must not wrap it in a path context,
/// which would hide it behind the context line when the message reaches the
/// import dialog.
pub(crate) fn strip_bom(bytes: &[u8]) -> Result<&[u8]> {
    if let Some(endianness) = utf16_endianness(bytes) {
        bail!(
            "This file is UTF-16 ({endianness}) text; the importer reads UTF-8 or Latin-1. \
             Re-save it as UTF-8 and import it again."
        );
    }
    Ok(bytes.strip_prefix(&UTF8_BOM).unwrap_or(bytes))
}

/// [`strip_bom`] for bytes the caller already owns - a `Data.xml` unpacked
/// from a TML archive is up to half a gigabyte, so this shifts in place rather
/// than copying the remainder.
pub(crate) fn strip_bom_owned(mut bytes: Vec<u8>) -> Result<Vec<u8>> {
    let mark_len = bytes.len() - strip_bom(&bytes)?.len();
    bytes.drain(..mark_len);
    Ok(bytes)
}

/// Refuse a file that carries any byte-order mark at all.
///
/// For the one reader that takes a path and cannot be handed stripped bytes:
/// the vendored `compass_data` crate opens a `.MAK` itself, so there is no
/// boundary of ours to strip at.  Its lexer rejects the mark as an unexpected
/// character and names a glyph that renders as nothing, which tells the user
/// less than nothing; this says what is actually wrong first.
pub(crate) fn reject_bom(path: &Path) -> Result<()> {
    let bytes = read_mark(path).with_context(|| format!("reading {}", path.display()))?;
    strip_bom(&bytes)?;
    if bytes.starts_with(&UTF8_BOM) {
        bail!(
            "This file starts with a UTF-8 byte-order mark (what a Windows editor calls \
             \"UTF-8 with BOM\"), which this format cannot carry. Re-save it as UTF-8 without \
             the mark and import it again."
        );
    }
    Ok(())
}

/// The leading bytes a byte-order mark could occupy - never more, because the
/// caller only asks whether the file STARTS with one and a `.MAK`'s project
/// tree can run to gigabytes.  A shorter file yields a shorter slice, which no
/// mark can match, rather than an error.
fn read_mark(path: &Path) -> std::io::Result<Vec<u8>> {
    let mut mark = Vec::with_capacity(UTF8_BOM.len());
    std::fs::File::open(path)?
        .take(UTF8_BOM.len() as u64)
        .read_to_end(&mut mark)?;
    Ok(mark)
}

/// Read a file written in a single-byte Latin code page - Walls is ISO-8859-1,
/// Compass is Windows-1252 - by decoding each byte as a code point, which
/// avoids failing on accented surveyor names and comments.
pub(crate) fn read_latin1(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(strip_bom(&bytes)?.iter().map(|&b| char::from(b)).collect())
}

/// Read a UTF-8 text file (GPX, MNemo `.dmp`).
pub(crate) fn read_utf8(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    String::from_utf8(strip_bom_owned(bytes)?)
        .map_err(|e| anyhow!("{} is not valid UTF-8 text: {e}", path.display()))
}

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_utf8_bom_is_dropped_and_nothing_else_is() {
        assert_eq!(strip_bom(b"\xef\xbb\xbf#UNITS").unwrap(), b"#UNITS");
        assert_eq!(strip_bom(b"#UNITS").unwrap(), b"#UNITS");
        // Only a LEADING mark: the same bytes mid-file are content.
        assert_eq!(
            strip_bom(b"#U\xef\xbb\xbfNITS").unwrap(),
            b"#U\xef\xbb\xbfNITS"
        );
        // A short file cannot be mistaken for a marked one.
        assert_eq!(strip_bom(b"\xef\xbb").unwrap(), b"\xef\xbb");
        assert_eq!(strip_bom(b"").unwrap(), b"");
    }

    #[test]
    fn reject_bom_reads_only_the_mark_and_judges_short_files() {
        let dir = tempfile::tempdir().unwrap();
        // A file too short to hold a mark is not marked, and reading a
        // fraction of one must not be an error in itself.
        for (name, contents) in [
            ("empty.mak", b"".as_slice()),
            ("one.mak", b"#".as_slice()),
            ("two.mak", b"\xef\xbb".as_slice()),
            ("plain.mak", b"#cave.dat;".as_slice()),
        ] {
            let path = dir.path().join(name);
            std::fs::write(&path, contents).unwrap();
            assert!(reject_bom(&path).is_ok(), "{name} carries no mark");
        }

        // Only the first three bytes are consulted, so a mark's worth of bytes
        // further in is content.
        let midfile = dir.path().join("midfile.mak");
        std::fs::write(&midfile, b"#c\xef\xbb\xbfave.dat;").unwrap();
        assert!(reject_bom(&midfile).is_ok());

        let marked = dir.path().join("marked.mak");
        std::fs::write(&marked, b"\xef\xbb\xbf#cave.dat;").unwrap();
        let err = reject_bom(&marked)
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(
            err.contains("byte-order mark"),
            "a marked file must be named as such, got {err:?}"
        );

        let utf16 = dir.path().join("utf16.mak");
        std::fs::write(&utf16, b"\xff\xfe#\0c\0").unwrap();
        let err = reject_bom(&utf16)
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(
            err.contains("UTF-16") && err.contains("little-endian"),
            "got {err:?}"
        );

        assert!(
            reject_bom(&dir.path().join("absent.mak")).is_err(),
            "a missing file is still a read failure"
        );
    }

    #[test]
    fn either_utf16_mark_is_refused_by_name() {
        for (bytes, endianness) in [
            (b"\xff\xfe#\0".as_slice(), "little-endian"),
            (b"\xfe\xff\0#".as_slice(), "big-endian"),
        ] {
            let err = strip_bom(bytes)
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default();
            assert!(
                err.contains("UTF-16") && err.contains(endianness),
                "the error must name the encoding, got {err:?}"
            );
        }
    }
}
