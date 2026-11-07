//! Byte-order-mark handling shared by every text-based importer.
//!
//! Windows tooling writes "UTF-8 with BOM" by default (Notepad, PowerShell 5.1
//! `Out-File`, a common VS Code setting), so a survey file's first line can
//! arrive with `EF BB BF` glued to it.  The Walls and Compass readers map each
//! byte to a Windows-1252 char and the `.dmp` reader parses its first token as
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

/// The importers' file-size ceilings, one per format family (#576 B27(g)), so
/// the policy lives in one table instead of in each importer. A ceiling
/// refuses a file outright, so each sits well above the largest real file of
/// its kind while still bounding what an untrusted file can make the app
/// allocate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FileFamily {
    /// Ariane `.tmlu` and GPX: XML, the largest real files on record (a 57 MB
    /// Ariane `Data.xml`; multi-day GPS logs reach hundreds of MB). The
    /// zipped `.tml` archive is capped at it too, and so is what its `Data.xml`
    /// decompresses to (`tml::decode`).
    Xml,
    /// Compass `.DAT` and `.MAK` (and every `.DAT` a `.MAK` links), Walls
    /// `.SRV` and `.WPJ`: survey text, typically a few MB, which decodes to up
    /// to 3x its size in memory.
    SurveyText,
    /// mNemo `.dmp`: semicolon-separated integers, never this large for a
    /// real dump.
    Dmp,
}

impl FileFamily {
    /// The most bytes a file of this family may hold.
    pub(crate) const fn byte_limit(self) -> u64 {
        const MB: u64 = 1024 * 1024;
        match self {
            Self::Xml => 512 * MB,
            Self::SurveyText => 128 * MB,
            Self::Dmp => 50 * MB,
        }
    }

    /// The family as the refusal names it.
    fn description(self) -> &'static str {
        match self {
            Self::Xml => "XML survey files (.tml, .tmlu, .gpx)",
            Self::SurveyText => "survey text files (.DAT, .MAK, .SRV, .WPJ)",
            Self::Dmp => "mNemo .dmp files",
        }
    }

    /// The family of an import file, by its extension, for a reader handed any
    /// of them (the duplicate-import hash, #576 RV3-R-11). A zipped `.tml` and
    /// an unknown extension take the XML ceiling, the largest.
    fn of_path(path: &Path) -> Self {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase);
        match ext.as_deref() {
            Some("dmp") => Self::Dmp,
            Some("dat" | "mak" | "srv" | "wpj") => Self::SurveyText,
            _ => Self::Xml,
        }
    }
}

/// The refusal for a file over `limit`, in the words every capped reader uses.
pub(crate) fn too_large(path: &Path, family: FileFamily, limit: u64) -> anyhow::Error {
    anyhow!(
        "'{}' is larger than the {} limit for {}; it was not read.",
        path.display(),
        describe_byte_limit(limit),
        family.description()
    )
}

/// Render a byte limit the way the person reading the error thinks about it:
/// whole megabytes from 1 MB up, exact bytes below (where test limits live).
pub(crate) fn describe_byte_limit(bytes: u64) -> String {
    const MB: u64 = 1024 * 1024;
    if bytes >= MB {
        format!("{} MB", bytes / MB)
    } else {
        format!("{bytes} bytes")
    }
}

/// Open `path` for reading, with its metadata, or refuse it when it is not a
/// regular file (#576 B27(g)): reading a FIFO blocks until somebody writes, and
/// a device may never end. The type is checked on the OPENED handle (`fstat`),
/// and every caller reads through that handle and never opens the path again
/// (#576 W30-7d): checking the path and then opening it left a window in which
/// a FIFO swapped in after the check blocked the open. On unix the open is
/// non-blocking (`O_NONBLOCK`), so opening a FIFO returns at once instead of
/// waiting for a writer; the flag has no effect on reads of a regular file,
/// the only kind handed back. A reader that streams instead of reading whole
/// (the zipped `.tml`) refuses the same paths [`read_capped`] does.
pub(crate) fn open_regular_file(path: &Path) -> Result<(std::fs::File, std::fs::Metadata)> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        // Windows will not open a folder at all, nor unix a socket: name either
        // the way the check below does rather than as an access error. Only the
        // wording of a refusal rests on this look at the path; nothing is read.
        Err(_) if std::fs::metadata(path).is_ok_and(|metadata| !metadata.is_file()) => {
            return Err(not_a_regular_file(path));
        }
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let metadata = file
        .metadata()
        .with_context(|| format!("reading {}", path.display()))?;
    if !metadata.is_file() {
        return Err(not_a_regular_file(path));
    }
    Ok((file, metadata))
}

/// The refusal for a path that is not a regular file.
fn not_a_regular_file(path: &Path) -> anyhow::Error {
    anyhow!(
        "'{}' is not a regular file (a folder, device or pipe), so it was not read.",
        path.display()
    )
}

/// Read a whole file for an importer (#576 B27(g)). Anything that is not a
/// regular file is refused before a byte is read - a FIFO or a device would
/// block the read, or never end it - and so is a file larger than its
/// family's [`FileFamily::byte_limit`], with a message naming the limit. The
/// read itself is capped too, so a file that grows after the size check
/// cannot take more.
pub(crate) fn read_capped(path: &Path, family: FileFamily) -> Result<Vec<u8>> {
    read_with_limit(path, family, family.byte_limit())
}

/// [`read_capped`] with the limit spelled out, so a test can exercise it
/// without writing the real ceiling's worth of bytes.
fn read_with_limit(path: &Path, family: FileFamily, limit: u64) -> Result<Vec<u8>> {
    let (file, metadata) = open_regular_file(path)?;
    if metadata.len() > limit {
        return Err(too_large(path, family, limit));
    }
    // Within the limit, so a safe size to reserve up front.
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .with_context(|| format!("reading {}", path.display()))?;
    if bytes.len() as u64 > limit {
        return Err(too_large(path, family, limit));
    }
    Ok(bytes)
}

/// Hand an import file to `consume` in chunks, for a caller that only needs
/// to stream it - the duplicate-import hash (#576 RV3-R-11), which used to read
/// the whole file with no cap and no file-type check, so a FIFO swapped in
/// after the parse blocked the command, and a file grown past its ceiling was
/// read whole. The same refusals as [`read_capped`] - not a regular file, or
/// over the ceiling of its family (picked from the extension) - without ever
/// holding the file whole. Returns how many bytes it streamed.
pub fn stream_import_file(path: &Path, consume: impl FnMut(&[u8])) -> Result<u64> {
    let family = FileFamily::of_path(path);
    stream_with_limit(path, family, family.byte_limit(), consume)
}

/// [`stream_import_file`] with the limit spelled out, for the tests.
fn stream_with_limit(
    path: &Path,
    family: FileFamily,
    limit: u64,
    mut consume: impl FnMut(&[u8]),
) -> Result<u64> {
    let (file, metadata) = open_regular_file(path)?;
    if metadata.len() > limit {
        return Err(too_large(path, family, limit));
    }
    let mut reader = file.take(limit.saturating_add(1));
    let mut buffer = vec![0u8; 64 * 1024];
    let mut streamed = 0u64;
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) => return Ok(streamed),
            Ok(read) => read,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        streamed += read as u64;
        // Grown past the ceiling since the size check.
        if streamed > limit {
            return Err(too_large(path, family, limit));
        }
        consume(buffer.get(..read).unwrap_or_default());
    }
}

/// The Windows-1252 code points for `0x80..=0x9F`, the only range where it
/// differs from ISO-8859-1 (which is byte == code point). Written as escapes so
/// the table stays ASCII in source. `U+FFFD` marks the five undefined slots.
const CP1252_HIGH: [char; 32] = [
    '\u{20ac}', '\u{fffd}', '\u{201a}', '\u{0192}', '\u{201e}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{02c6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{fffd}', '\u{017d}', '\u{fffd}',
    '\u{fffd}', '\u{2018}', '\u{2019}', '\u{201c}', '\u{201d}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{02dc}', '\u{2122}', '\u{0161}', '\u{203a}', '\u{0153}', '\u{fffd}', '\u{017e}', '\u{0178}',
];

/// Decode one byte as Windows-1252: `0x80..=0x9F` through [`CP1252_HIGH`],
/// every other byte as the code point of the same value.
pub(crate) fn cp1252_char(byte: u8) -> char {
    if (0x80..=0x9f).contains(&byte) {
        CP1252_HIGH[usize::from(byte - 0x80)]
    } else {
        char::from(byte)
    }
}

/// A file decoded by [`read_cp1252`].
pub(crate) struct Cp1252Text {
    pub(crate) text: String,
    /// How many of the file's bytes are among the five that Windows-1252
    /// leaves undefined, each of which became U+FFFD in `text`.
    pub(crate) replaced: usize,
}

/// Read a file written in Windows-1252, the ANSI code page that Compass and
/// Walls (both Windows programs) write their text in. Each byte becomes one
/// `char`, so accented surveyor names and comments never fail the read, and
/// the five bytes the code page leaves undefined become U+FFFD - counted, so
/// the caller can say so with [`cp1252_replacement_warning`]. Every such file
/// is survey text, read through [`read_capped`] with that family's ceiling.
pub(crate) fn read_cp1252(path: &Path) -> Result<Cp1252Text> {
    let bytes = read_capped(path, FileFamily::SurveyText)?;
    let bytes = strip_bom(&bytes)?;
    let replaced = bytes
        .iter()
        .filter(|&&b| cp1252_char(b) == char::REPLACEMENT_CHARACTER)
        .count();
    Ok(Cp1252Text {
        text: bytes.iter().map(|&b| cp1252_char(b)).collect(),
        replaced,
    })
}

/// The tail of an encoding warning that accounts for `replaced` undecodable
/// characters, or an empty string when nothing was lost. Shared by the TML
/// reader and [`cp1252_replacement_warning`], so every importer counts its
/// losses in the same words.
pub(crate) fn describe_replaced_count(replaced: usize) -> String {
    match replaced {
        0 => String::new(),
        1 => "; 1 character was replaced".to_string(),
        n => format!("; {n} characters were replaced"),
    }
}

/// The warning for a file [`read_cp1252`] decoded with `replaced` undefined
/// bytes, or `None` when it had none. `file` names the file when the import
/// reads several (a Walls project); a single-file import passes `None`, since
/// the file is the one the user just picked. `program` is the one that writes
/// the format.
pub(crate) fn cp1252_replacement_warning(
    file: Option<&Path>,
    program: &str,
    replaced: usize,
) -> Option<String> {
    if replaced == 0 {
        return None;
    }
    let subject = file.map_or_else(
        || "The file".to_string(),
        |path| format!("'{}'", path.display()),
    );
    Some(format!(
        "{subject} was read as windows-1252, the encoding {program} writes{}",
        describe_replaced_count(replaced)
    ))
}

/// Read a UTF-8 text file (GPX, mNemo `.dmp`) through [`read_capped`] with
/// its `family`'s ceiling.
pub(crate) fn read_utf8(path: &Path, family: FileFamily) -> Result<String> {
    let bytes = read_capped(path, family)?;
    String::from_utf8(strip_bom_owned(bytes)?)
        .map_err(|e| anyhow!("{} is not valid UTF-8 text: {e}", path.display()))
}

#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::print_stdout)]
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

    /// Every byte decodes to exactly one char: `0x80..=0x9F` through the
    /// Windows-1252 table (the five slots it leaves undefined as U+FFFD, as the
    /// TML reader decodes them), every other byte as the same code point. A
    /// leading UTF-8 mark is still dropped first.
    #[test]
    fn read_cp1252_decodes_the_c1_range_as_windows_1252() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("all-bytes.srv");
        let mut bytes = UTF8_BOM.to_vec();
        bytes.extend(0..=u8::MAX);
        std::fs::write(&path, &bytes).unwrap();

        let decoded = read_cp1252(&path).unwrap();
        assert_eq!(decoded.replaced, 5, "the five undefined bytes are counted");
        let text: Vec<char> = decoded.text.chars().collect();
        assert_eq!(text.len(), 256, "one char per byte, mark dropped");
        for (byte, &c) in (0..=u8::MAX).zip(&text) {
            if (0x80..=0x9f).contains(&byte) {
                assert!(
                    !('\u{80}'..='\u{9f}').contains(&c),
                    "byte {byte:#04x} decoded to the C1 control U+{:04X}",
                    u32::from(c)
                );
            } else {
                assert_eq!(c, char::from(byte), "byte {byte:#04x}");
            }
        }
        for (byte, expected) in [
            (0x80_u8, '\u{20ac}'),
            (0x85, '\u{2026}'),
            (0x92, '\u{2019}'),
            (0x96, '\u{2013}'),
            (0x9f, '\u{0178}'),
            (0x81, '\u{fffd}'),
            (0x8d, '\u{fffd}'),
            (0x8f, '\u{fffd}'),
            (0x90, '\u{fffd}'),
            (0x9d, '\u{fffd}'),
        ] {
            assert_eq!(text[usize::from(byte)], expected, "byte {byte:#04x}");
        }
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

    /// #576 B27(g): one table, pinned so a change to a ceiling is deliberate.
    #[test]
    fn the_size_ceilings_per_format_family() {
        assert_eq!(describe_byte_limit(FileFamily::Xml.byte_limit()), "512 MB");
        assert_eq!(
            describe_byte_limit(FileFamily::SurveyText.byte_limit()),
            "128 MB"
        );
        assert_eq!(describe_byte_limit(FileFamily::Dmp.byte_limit()), "50 MB");
        assert_eq!(describe_byte_limit(1024), "1024 bytes");
    }

    /// A file at the limit is read whole; one byte more is refused with the
    /// limit and the family named.
    #[test]
    fn a_capped_read_refuses_one_byte_over_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cave.srv");
        std::fs::write(&path, [b'x'; 16]).unwrap();
        assert_eq!(
            read_with_limit(&path, FileFamily::SurveyText, 16)
                .unwrap()
                .len(),
            16
        );
        let err = read_with_limit(&path, FileFamily::SurveyText, 15)
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(
            err.contains("larger than the 15 bytes limit for survey text files"),
            "{err}"
        );
    }

    /// A folder is not a file to read.
    #[test]
    fn a_capped_read_refuses_a_folder() {
        let dir = tempfile::tempdir().unwrap();
        let err = read_capped(dir.path(), FileFamily::SurveyText)
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(err.contains("not a regular file"), "{err}");
    }

    /// A FIFO is refused before it is read - reading one blocks until
    /// somebody writes, which nobody will.
    #[cfg(unix)]
    #[test]
    fn a_capped_read_refuses_a_fifo_without_reading_it() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("pipe.dat");
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap();
        assert!(status.success(), "premise: mkfifo");
        let err = read_capped(&fifo, FileFamily::SurveyText)
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(err.contains("not a regular file"), "{err}");
    }

    /// #576 RV3-R-11: the duplicate-import hash streams the file - every byte,
    /// in order, across many chunks - and refuses what the capped readers
    /// refuse: one byte over the limit, a folder, a FIFO (without reading it).
    #[test]
    fn a_streamed_import_file_is_whole_and_capped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cave.dat");
        let bytes: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&path, &bytes).unwrap();
        let mut streamed = Vec::new();
        let mut chunks = 0;
        let count = stream_import_file(&path, |chunk| {
            chunks += 1;
            streamed.extend_from_slice(chunk);
        })
        .unwrap();
        assert!(count == 200_000 && streamed == bytes && chunks > 1);

        let over = stream_with_limit(&path, FileFamily::SurveyText, 199_999, |_| {})
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(
            over.contains("larger than the 199999 bytes limit for survey text files"),
            "{over}"
        );
        let folder = stream_import_file(dir.path(), |_| {})
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(folder.contains("not a regular file"), "{folder}");
    }

    #[cfg(unix)]
    #[test]
    fn a_streamed_import_file_refuses_a_fifo_without_reading_it() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("pipe.tml");
        fifo_test_support::make_fifo(&fifo);
        let release = fifo.clone();
        let outcome = fifo_test_support::within_five_seconds(&release, move || {
            stream_import_file(&fifo, |_| {})
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default()
        });
        assert!(
            outcome
                .as_deref()
                .is_some_and(|err| err.contains("not a regular file")),
            "{outcome:?}"
        );
    }

    /// #576 W30-7d: the check and the read share one handle. The base checked
    /// the PATH and then opened it again, so a FIFO swapped in between blocked
    /// that second open. Now the read goes through the handle the check was
    /// made on - the file that was there - and opening the swapped-in FIFO is
    /// refused at once rather than waiting for a writer.
    #[cfg(unix)]
    #[test]
    fn a_fifo_swapped_in_after_the_check_cannot_block_the_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cave.dat");
        std::fs::write(&path, b"#UNITS").unwrap();

        let (file, metadata) = open_regular_file(&path).unwrap();
        println!(
            "check on the regular file: is_file = {}",
            metadata.is_file()
        );
        std::fs::remove_file(&path).unwrap();
        fifo_test_support::make_fifo(&path);
        println!("swapped a FIFO in at the checked path");

        let read = fifo_test_support::within_five_seconds(&path, move || {
            let mut text = String::new();
            file.take(1024)
                .read_to_string(&mut text)
                .map(|_| text)
                .map_err(|e| e.to_string())
        });
        println!("read through the checked handle: {read:?}");
        assert_eq!(read, Some(Ok("#UNITS".to_string())));

        let swapped = path.clone();
        let reopened = fifo_test_support::within_five_seconds(&path, move || {
            open_regular_file(&swapped)
                .map(|_| ())
                .map_err(|e| e.to_string())
        });
        println!("open_regular_file on the swapped-in FIFO: {reopened:?}");
        assert!(
            reopened
                .as_ref()
                .is_some_and(|r| r.as_ref().is_err_and(|e| e.contains("not a regular file"))),
            "opening a FIFO must return at once with the refusal, got {reopened:?}"
        );
    }

    /// Each import extension streams under its own family's ceiling.
    #[test]
    fn an_import_file_takes_its_familys_ceiling_by_extension() {
        for (name, family) in [
            ("a.DMP", FileFamily::Dmp),
            ("a.dat", FileFamily::SurveyText),
            ("a.MAK", FileFamily::SurveyText),
            ("a.srv", FileFamily::SurveyText),
            ("a.wpj", FileFamily::SurveyText),
            ("a.tml", FileFamily::Xml),
            ("a.tmlu", FileFamily::Xml),
            ("a.gpx", FileFamily::Xml),
            ("a", FileFamily::Xml),
        ] {
            assert_eq!(FileFamily::of_path(Path::new(name)), family, "{name}");
        }
    }
}

/// FIFO helpers for the importers' "not a regular file" tests (#576 B27(g)):
/// opening a FIFO for reading blocks until somebody writes, so a reader that
/// does not refuse one hangs the test instead of failing it.
#[cfg(all(test, unix))]
#[expect(clippy::unwrap_used, clippy::print_stdout)]
pub(crate) mod fifo_test_support {
    use std::path::Path;

    /// A named pipe at `path`, made with the system's `mkfifo`.
    pub(crate) fn make_fifo(path: &Path) {
        let status = std::process::Command::new("mkfifo")
            .arg(path)
            .status()
            .unwrap();
        assert!(status.success(), "premise: mkfifo made {}", path.display());
    }

    /// Run `f` on its own thread and give it five seconds. On a timeout the
    /// FIFO at `release` is opened for writing and closed again - the blocked
    /// reader then sees an empty file and the thread can finish - and the
    /// result is `None`.
    pub(crate) fn within_five_seconds<T: Send + 'static>(
        release: &Path,
        f: impl FnOnce() -> T + Send + 'static,
    ) -> Option<T> {
        let (sender, receiver) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _ = sender.send(f());
        });
        let started = std::time::Instant::now();
        let outcome = receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .ok();
        println!(
            "  returned after {:.1} s{}",
            started.elapsed().as_secs_f64(),
            if outcome.is_some() {
                ""
            } else {
                " (still blocked; released by the test)"
            }
        );
        if outcome.is_none() {
            drop(std::fs::OpenOptions::new().write(true).open(release));
        }
        let _ = worker.join();
        outcome
    }
}
