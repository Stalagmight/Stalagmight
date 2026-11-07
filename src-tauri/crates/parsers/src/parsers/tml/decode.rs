//! XML loading and text decoding for TML/TMLU files.
//!
//! Extracting `Data.xml` from a ZIP (TML) or reading a plain file (TMLU), then
//! turning its bytes into XML text: the encoding fallback (valid UTF-8, then a
//! declared single-byte Latin code page, then a lossy last resort) and the
//! decompression cap that protects against an untrusted archive.

use std::fs;
use std::io::Read as _;
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};

use crate::models::zip_entry_bound::{entry_count_excess, EntryExcess, MAX_END_CANDIDATES};
use crate::parsers::encoding::{
    cp1252_char, describe_byte_limit, describe_replaced_count, open_regular_file, read_capped,
    strip_bom_owned, too_large, FileFamily,
};

// ============================================================================
// XML loading
// ============================================================================

/// XML text plus whatever had to be said about how it was decoded.
#[derive(Debug)]
pub(super) struct LoadedXml {
    pub(super) xml: String,
    pub(super) encoding_warning: Option<String>,
}

/// Load XML content from a `.tml` (ZIP) or `.tmlu` (plain XML) file.
pub(super) fn load_xml(path: &Path) -> Result<LoadedXml> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
        .unwrap_or_default();

    match ext.as_str() {
        "tml" => load_xml_from_zip(path),
        "tmlu" => {
            // The XML family's capped, regular-files-only reader (#576 B27(g)).
            let bytes = read_capped(path, FileFamily::Xml)?;
            decode_xml_bytes(bytes)
        }
        _ => bail!("Unsupported file extension '.{ext}'. Expected .tml or .tmlu"),
    }
}

/// Most a `Data.xml` entry may decompress to: the XML family's ceiling, the
/// same a `.tmlu` is read under (one table, #576 NF-W3P-3). The largest real
/// one seen is 57 MB, so this is a wide margin; it exists because the archive
/// is untrusted input and a deflate stream can otherwise expand without bound.
const MAX_DATA_XML_BYTES: u64 = FileFamily::Xml.byte_limit();

/// Most bytes the `.tml` archive itself may hold (#576 W30-7c): the XML
/// family's ceiling again, refused in the words every capped reader uses.
/// Checked before the ZIP reader sees the file, so a huge archive is not
/// parsed at all.
const MAX_TML_ARCHIVE_BYTES: u64 = FileFamily::Xml.byte_limit();

/// Most entries a `.tml` archive may hold (#576 W30-7c). A real one holds
/// `Data.xml` and nothing else (docs/ariane-import.md); 10 000 is the ceiling
/// the SpeleoDB dump reader puts on an archive of a handful of files
/// (`speleodb::dump::MAX_ENTRIES`), orders of magnitude of headroom while
/// still bounding the walk over the entry names.
///
/// `ZipArchive::new` builds a record for every entry the central directory
/// declares before it returns - 575 MB and 4 s for a million entries, and the
/// 512 MB archive cap leaves room for ten times that (#576 RV19a-4) - and the
/// `zip` crate exposes no hook to bound it. So the limit is enforced on the
/// archive's raw bytes BEFORE that parse, by [`refuse_too_many_entries`]; the
/// count after it stays as defence in depth.
const MAX_TML_ENTRIES: usize = 10_000;

/// Extract `Data.xml` from a ZIP archive.
fn load_xml_from_zip(path: &Path) -> Result<LoadedXml> {
    load_xml_from_zip_capped(path, MAX_TML_ARCHIVE_BYTES, MAX_DATA_XML_BYTES)
}

/// `load_xml_from_zip` with the archive-size cap and the decompression cap
/// spelled out, so a test can exercise them without building half a gigabyte.
fn load_xml_from_zip_capped(path: &Path, archive_limit: u64, cap: u64) -> Result<LoadedXml> {
    // Checked on the handle the archive is then read through, so a FIFO
    // swapped in after the check cannot block the read (#576 B27(g), W30-7d).
    let (mut file, metadata) = open_regular_file(path)?;
    if metadata.len() > archive_limit {
        return Err(too_large(path, FileFamily::Xml, archive_limit));
    }
    refuse_too_many_entries(path, &mut file)?;
    let mut archive = zip::ZipArchive::new(file).context("Failed to read TML as ZIP archive")?;
    // Defence in depth only: the checks above bounded the parse, and this
    // counts unique names (the `zip` crate keys its entries by name, zip
    // 2.4.2 read.rs:72-75), so duplicates would slip past it.
    let entries = archive.len();
    if entries > MAX_TML_ENTRIES {
        bail!(
            "'{}' is a ZIP archive of {entries} entries, over the {MAX_TML_ENTRIES}-entry limit \
             for a TML file; it was not read.",
            path.display()
        );
    }

    let entry_name = find_data_entry(&archive)
        .ok_or_else(|| anyhow!("TML archive does not contain 'Data.xml'"))?;

    let data_file = archive
        .by_name(&entry_name)
        .context("Failed to open Data.xml in archive")?;

    // Cap the bytes actually read, not the entry's declared size: a crafted
    // archive can declare anything, and only the read is bounded by reality.
    let mut bytes = Vec::new();
    data_file
        .take(cap.saturating_add(1))
        .read_to_end(&mut bytes)
        .context("Failed to read Data.xml from archive")?;
    if bytes.len() as u64 > cap {
        bail!(
            "Data.xml in this TML archive is larger than the {} decompression limit",
            describe_byte_limit(cap)
        );
    }

    decode_xml_bytes(bytes)
}

/// Refuse an archive of more than [`MAX_TML_ENTRIES`] entries from its raw
/// bytes, before the ZIP reader parses its directory (#576 RV19a-4, RV19a-4b),
/// and leave `file` at its start for that reader. The checks, and why they
/// bound the parse, are [`entry_count_excess`]'s (shared with the SpeleoDB
/// dump reader, #576 W52-4); this words the refusal for a TML file.
fn refuse_too_many_entries(path: &Path, file: &mut fs::File) -> Result<()> {
    let excess = entry_count_excess(file, MAX_TML_ENTRIES as u64)
        .with_context(|| format!("reading {}", path.display()))?;
    match excess {
        None => Ok(()),
        Some(EntryExcess::Declared(declared)) => bail!(
            "'{}' is a ZIP archive that declares {declared} entries, over the \
             {MAX_TML_ENTRIES}-entry limit for a TML file; it was not read.",
            path.display()
        ),
        Some(EntryExcess::DirectoryRecords(records)) => bail!(
            "'{}' is a ZIP archive with {records} central-directory records, over the \
             {MAX_TML_ENTRIES}-entry limit for a TML file; it was not read.",
            path.display()
        ),
        Some(EntryExcess::EndRecords) => bail!(
            "'{}' is a ZIP archive with more than {MAX_END_CANDIDATES} ZIP64 end records \
             or locators; it was not read.",
            path.display()
        ),
    }
}

/// Find the `Data.xml` entry, preferring the exact spelling and falling back on
/// any case variant: archives written on a case-insensitive filesystem carry
/// `data.xml` or `DATA.XML` just as happily.
fn find_data_entry(archive: &zip::ZipArchive<fs::File>) -> Option<String> {
    let mut variant: Option<&str> = None;
    for name in archive.file_names() {
        if name == "Data.xml" {
            return Some(name.to_string());
        }
        if variant.is_none() && name.eq_ignore_ascii_case("Data.xml") {
            variant = Some(name);
        }
    }
    variant.map(str::to_string)
}

// ============================================================================
// Text decoding
// ============================================================================

/// How far into the file we look for the XML declaration's `encoding=`.
const DECLARATION_SCAN_BYTES: usize = 256;

/// Turn file bytes into XML text.
///
/// The contract, in order:
///
/// 1. Valid UTF-8 wins unconditionally, declaration or no declaration. A file
///    that declares ISO-8859-1 but whose bytes happen to be valid UTF-8 is read
///    as UTF-8, with no warning - the two agree on all-ASCII content, which is
///    almost every such file, and second-guessing valid UTF-8 would corrupt the
///    ones where they do not.
/// 2. Only once UTF-8 has failed is the declaration consulted, and only to pick
///    between the single-byte Latin code pages (a hand-edited file from a French
///    or Spanish Windows install is the realistic source).
/// 3. Anything else decodes lossily.
///
/// Cases 2 and 3 both used to fail the whole import with a raw Rust decode
/// error. Either way the receipt now says what happened, including how many
/// characters were lost - a wrong accent beats a refused file, but not silently.
///
/// The Latin decode is done in this crate (the Windows-1252 table is shared
/// with the Compass and Walls readers in [`crate::parsers::encoding`]) rather
/// than through a decoder crate on purpose: nothing in the dependency tree
/// provides one (`quick-xml` is built without its `encoding` feature), and two
/// code pages do not justify adding one.
///
/// A leading UTF-8 byte-order mark is dropped before any of that, and a UTF-16
/// one is refused outright - see [`crate::parsers::encoding`].
fn decode_xml_bytes(bytes: Vec<u8>) -> Result<LoadedXml> {
    let bytes = match String::from_utf8(strip_bom_owned(bytes)?) {
        Ok(xml) => {
            return Ok(LoadedXml {
                xml,
                encoding_warning: None,
            })
        }
        Err(e) => e.into_bytes(),
    };

    if let Some(label) = declared_encoding(&bytes) {
        if let Some((xml, name)) = decode_single_byte_latin(&bytes, &label) {
            // Even the declared code page has holes: five bytes are undefined in
            // Windows-1252 and decode to U+FFFD here, which the warning must own
            // rather than read as "handled fine".
            let warning = format!(
                "The file is not valid UTF-8; it was read as {name}, the encoding its XML header \
                 declares{}",
                describe_replacements(&xml)
            );
            return Ok(LoadedXml {
                xml,
                encoding_warning: Some(warning),
            });
        }
    }

    let xml = String::from_utf8_lossy(&bytes).into_owned();
    Ok(LoadedXml {
        encoding_warning: Some(format!(
            "The file was not valid UTF-8 and its declared encoding (if any) could not be used{}",
            {
                let lost = describe_replacements(&xml);
                if lost.is_empty() {
                    "; it was decoded lossily".to_string()
                } else {
                    lost
                }
            }
        )),
        xml,
    })
}

/// The tail of an encoding warning that accounts for undecodable input, or an
/// empty string when nothing was lost.
fn describe_replacements(xml: &str) -> String {
    describe_replaced_count(
        xml.chars()
            .filter(|&c| c == char::REPLACEMENT_CHARACTER)
            .count(),
    )
}

/// Read the `encoding=` value out of the XML declaration, lowercased.
fn declared_encoding(bytes: &[u8]) -> Option<String> {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(DECLARATION_SCAN_BYTES)]);
    let declaration = &head[..head.find("?>")?];
    let after_key = declaration[declaration.find("encoding")? + "encoding".len()..].trim_start();
    let quoted = after_key.strip_prefix('=')?.trim_start();
    let quote = quoted.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    Some(quoted[1..].split(quote).next()?.trim().to_lowercase())
}

/// Decode `bytes` as a single-byte Latin code page, returning the text and the
/// canonical name of what was used. `None` for any other label.
fn decode_single_byte_latin(bytes: &[u8], label: &str) -> Option<(String, &'static str)> {
    let (name, windows) = match label {
        "iso-8859-1" | "iso8859-1" | "iso_8859-1" | "latin1" | "latin-1" => ("ISO-8859-1", false),
        "windows-1252" | "cp1252" => ("windows-1252", true),
        _ => return None,
    };

    let text = bytes
        .iter()
        .map(|&b| {
            if windows {
                cp1252_char(b)
            } else {
                char::from(b)
            }
        })
        .collect();
    Some((text, name))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::print_stdout)]
mod tests {
    use super::super::test_helpers::{
        parse_synthetic_tmlu, tml_shot_xml, wrap_xml, write_tml_archive, write_tml_archive_entries,
    };
    use super::super::{convert_tml_to_import_configs, parse_tml, TmlFile};
    use super::*;

    /// The whole point of the error: it has to reach the user on the same path
    /// the truncated-ZIP error already takes - `parsers::tml::parse_tml`, which
    /// the `parse_tml` command returns as `Err(String)` to `store.parseTml`,
    /// before `openOrImport` opens the new-project dialog. A truncated
    /// `Data.xml` inside a perfectly valid ZIP is the shape a killed save
    /// leaves behind.
    ///
    /// Without the fix this returns `Ok`, and the caller carries on into
    /// project creation with an empty survey.
    #[test]
    fn a_truncated_data_xml_in_a_valid_zip_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("synthetic.tml");
        let full = wrap_xml(&tml_shot_xml(1, -1, "REAL", "A"));
        let cut = full.find("<Azimut>").unwrap();
        write_tml_archive(&path, "Data.xml", &full.as_bytes()[..cut]);

        // The archive itself is intact - only its payload is short.
        assert!(load_xml_from_zip(&path).is_ok());

        let err = parse_tml(&path).unwrap_err().to_string();
        assert!(
            err.contains("truncated or damaged"),
            "the error must say the file is damaged, got {err:?}"
        );
    }

    #[test]
    fn test_wrong_extension() {
        let result = load_xml(Path::new("/tmp/test.xyz"));
        assert!(result.is_err());
    }

    // -------------------------------------------------------------------
    // Degrading gracefully on files the old reader refused or misread
    // -------------------------------------------------------------------

    /// Encode ASCII/Latin-1 text one byte per char (byte == code point).
    fn latin1_bytes(text: &str) -> Vec<u8> {
        text.chars()
            .map(|c| u8::try_from(u32::from(c)).unwrap())
            .collect()
    }

    /// Run the real `parse_tml` over synthetic bytes written to a throwaway
    /// `.tmlu`, so the byte-level decode path is the one under test.
    fn parse_synthetic_tmlu_bytes(bytes: &[u8]) -> Result<TmlFile> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("synthetic.tmlu");
        fs::write(&path, bytes)?;
        parse_tml(&path)
    }

    /// A one-shot document with the declaration spelled out, so a test can pick
    /// the encoding it claims.
    fn document_with_declaration(declaration: &str, cave_name: &str) -> String {
        format!(
            "{declaration}\n<CaveFile>\n<caveName>{cave_name}</caveName>\n<unit>m</unit>\n\
             <Data>\n{}\n{}\n</Data>\n</CaveFile>",
            tml_shot_xml(0, -1, "START", "Start"),
            tml_shot_xml(1, 0, "REAL", "A"),
        )
    }

    /// A Latin-1 file (a French or Spanish install writes accents as single
    /// bytes) used to fail the whole import on `read_to_string`. It must import,
    /// with the accent intact and the substitution on the receipt.
    ///
    /// Without the fix `parse_tml` returns Err("stream did not contain valid
    /// UTF-8") and nothing is imported at all.
    #[test]
    fn a_latin1_file_is_decoded_from_its_declared_encoding() {
        let bytes = latin1_bytes(&document_with_declaration(
            r#"<?xml version="1.0" encoding="ISO-8859-1"?>"#,
            "Grotte de la Rivi\u{e8}re",
        ));
        assert!(
            std::str::from_utf8(&bytes).is_err(),
            "premise: these bytes really are not valid UTF-8"
        );

        let tml = parse_synthetic_tmlu_bytes(&bytes).unwrap();
        assert_eq!(tml.cave_name, "Grotte de la Rivi\u{e8}re");
        assert_eq!(tml.sections.len(), 1);

        let warning = tml.encoding_warning.clone().unwrap();
        assert!(
            warning.contains("ISO-8859-1"),
            "the warning must name the encoding used, got {warning:?}"
        );

        let (_, _, warnings) = convert_tml_to_import_configs(&tml, false, false).unwrap();
        assert!(
            warnings.contains(&warning),
            "the encoding warning must reach the import receipt, got {warnings:?}"
        );
    }

    /// Windows-1252 differs from Latin-1 only in `0x80..=0x9F`; the byte for a
    /// typographic apostrophe must not come back as a C1 control.
    #[test]
    fn a_windows_1252_file_decodes_the_high_range_from_its_own_table() {
        let mut bytes = latin1_bytes(&document_with_declaration(
            r#"<?xml version="1.0" encoding="windows-1252"?>"#,
            "PLACEHOLDER",
        ));
        let at = bytes.windows(11).position(|w| w == b"PLACEHOLDER").unwrap();
        bytes.splice(at..at + 11, [b'L', 0x92, b'A', 0xe9, b'v', b'e', b'n']);

        let tml = parse_synthetic_tmlu_bytes(&bytes).unwrap();
        assert_eq!(tml.cave_name, "L\u{2019}A\u{e9}ven");
        let warning = tml.encoding_warning.unwrap();
        assert!(warning.contains("windows-1252"));
        assert!(
            !warning.contains("replaced"),
            "nothing was lost here, so the warning must not claim otherwise: {warning:?}"
        );
    }

    /// Five bytes are undefined in Windows-1252 and decode to U+FFFD. The
    /// declared-encoding branch must own that loss, not report a clean decode.
    ///
    /// Without the fix the count is only taken on the lossy branch, so this
    /// warning ends at "declares" and never mentions the replaced character.
    #[test]
    fn an_undefined_windows_1252_byte_is_counted_in_the_warning() {
        let mut bytes = latin1_bytes(&document_with_declaration(
            r#"<?xml version="1.0" encoding="windows-1252"?>"#,
            "PLACEHOLDER",
        ));
        let at = bytes.windows(11).position(|w| w == b"PLACEHOLDER").unwrap();
        // 0x81 is one of the five undefined slots; 0xe9 is a plain accent.
        bytes.splice(at..at + 11, [b'C', 0x81, b'v', 0xe9]);

        let tml = parse_synthetic_tmlu_bytes(&bytes).unwrap();
        assert_eq!(tml.cave_name, "C\u{fffd}v\u{e9}");
        let warning = tml.encoding_warning.unwrap();
        assert!(
            warning.contains("windows-1252") && warning.contains("1 character was replaced"),
            "the declared-encoding warning must count its own losses, got {warning:?}"
        );
    }

    /// The whole encoding warning, byte for byte, on both decode branches and
    /// for no, one and several replaced characters. The count's wording is
    /// shared with the Compass and Walls readers, so this pins that sharing it
    /// left the TML receipt exactly as it was.
    #[test]
    fn the_encoding_warning_wording_is_pinned() {
        let declared = |encoding: &str, name: &[u8]| -> String {
            let mut bytes = latin1_bytes(&document_with_declaration(
                &format!(r#"<?xml version="1.0" encoding="{encoding}"?>"#),
                "PLACEHOLDER",
            ));
            let at = bytes.windows(11).position(|w| w == b"PLACEHOLDER").unwrap();
            bytes.splice(at..at + 11, name.iter().copied());
            decode_xml_bytes(bytes).unwrap().encoding_warning.unwrap()
        };
        let cases = [
            (
                declared("windows-1252", b"L\x92A\xe9ven"),
                "The file is not valid UTF-8; it was read as windows-1252, the encoding its XML \
                 header declares",
            ),
            (
                declared("windows-1252", b"C\x81v\xe9"),
                "The file is not valid UTF-8; it was read as windows-1252, the encoding its XML \
                 header declares; 1 character was replaced",
            ),
            (
                declared("windows-1252", b"C\x81v\x8d\x9d"),
                "The file is not valid UTF-8; it was read as windows-1252, the encoding its XML \
                 header declares; 3 characters were replaced",
            ),
            (
                declared("ISO-8859-1", b"Rivi\xe8re"),
                "The file is not valid UTF-8; it was read as ISO-8859-1, the encoding its XML \
                 header declares",
            ),
            (
                declared("UTF-8", b"C\xffv\xfe"),
                "The file was not valid UTF-8 and its declared encoding (if any) could not be \
                 used; 2 characters were replaced",
            ),
        ];
        for (warning, expected) in &cases {
            println!("{warning}");
            assert_eq!(warning, expected);
        }
    }

    /// A single-quoted declaration is as legal as a double-quoted one.
    ///
    /// Without the fix (accepting only `"`) the declaration is not read, the
    /// file falls to the lossy branch and the accent is lost.
    #[test]
    fn a_single_quoted_encoding_declaration_is_honoured() {
        let bytes = latin1_bytes(&document_with_declaration(
            "<?xml version='1.0' encoding='ISO-8859-1'?>",
            "Rivi\u{e8}re",
        ));

        let tml = parse_synthetic_tmlu_bytes(&bytes).unwrap();
        assert_eq!(tml.cave_name, "Rivi\u{e8}re");
        assert!(tml.encoding_warning.unwrap().contains("ISO-8859-1"));
    }

    /// A file that declares UTF-8 and then is not UTF-8 gets the lossy path, not
    /// a guess at what it meant. This pins the label set: widening the decoder to
    /// "assume Latin-1 when the declaration lies" would silently change what
    /// every such file imports as, and must be a deliberate decision.
    #[test]
    fn a_file_that_declares_utf8_and_lies_is_read_lossily_not_guessed() {
        let mut bytes = latin1_bytes(&document_with_declaration(
            r#"<?xml version="1.0" encoding="UTF-8"?>"#,
            "PLACEHOLDER",
        ));
        let at = bytes.windows(11).position(|w| w == b"PLACEHOLDER").unwrap();
        bytes.splice(at..at + 11, [b'R', b'i', b'v', 0xe8, b'r', b'e']);

        let tml = parse_synthetic_tmlu_bytes(&bytes).unwrap();
        assert_eq!(tml.cave_name, "Riv\u{fffd}re");
        let warning = tml.encoding_warning.unwrap();
        assert!(
            warning.contains("declared encoding (if any) could not be used")
                && warning.contains("1 character was replaced"),
            "a lying declaration must read as unusable, not as honoured: {warning:?}"
        );
    }

    /// Valid UTF-8 wins over the declaration, so a file that declares Latin-1
    /// but is really UTF-8 keeps its characters and raises nothing.
    #[test]
    fn valid_utf8_beats_a_declaration_that_says_otherwise() {
        let xml = document_with_declaration(
            r#"<?xml version="1.0" encoding="ISO-8859-1"?>"#,
            "Rivi\u{e8}re",
        );
        assert!(
            !xml.is_ascii(),
            "premise: the two encodings would disagree about this file"
        );

        let tml = parse_synthetic_tmlu_bytes(xml.as_bytes()).unwrap();
        assert_eq!(tml.cave_name, "Rivi\u{e8}re");
        assert!(tml.encoding_warning.is_none());
    }

    /// No usable declaration and undecodable bytes: import anyway, lossily, and
    /// say how much was lost. Without the fix this is a hard failure.
    #[test]
    fn an_undeclared_non_utf8_file_is_read_lossily_with_a_counted_warning() {
        let mut bytes = latin1_bytes(&document_with_declaration(
            r#"<?xml version="1.0"?>"#,
            "PLACEHOLDER",
        ));
        let at = bytes.windows(11).position(|w| w == b"PLACEHOLDER").unwrap();
        bytes.splice(at..at + 11, [b'C', 0xff, b'v', b'e']);

        let tml = parse_synthetic_tmlu_bytes(&bytes).unwrap();
        assert_eq!(tml.cave_name, "C\u{fffd}ve");
        let warning = tml.encoding_warning.unwrap();
        assert!(
            warning.contains("1 character was replaced"),
            "the warning must count what was lost, got {warning:?}"
        );
    }

    /// A well-formed UTF-8 file gets no encoding warning - the warning has to
    /// stay rare enough to mean something.
    #[test]
    fn a_utf8_file_carries_no_encoding_warning() {
        let tml = parse_synthetic_tmlu(&wrap_xml(&tml_shot_xml(1, -1, "REAL", "A")));
        assert!(tml.encoding_warning.is_none());
    }

    /// Ariane writes UTF-8, but a `.tmlu` that has been through a Windows
    /// editor comes back with a BOM ahead of the XML declaration. `quick-xml`
    /// tolerates one, so this pins that a marked file still imports whole - and
    /// that dropping the mark is not itself worth an encoding warning.
    #[test]
    fn a_utf8_bom_does_not_break_a_tmlu_file() {
        let xml = wrap_xml(&tml_shot_xml(1, -1, "REAL", "A"));
        let mut bytes = vec![0xef, 0xbb, 0xbf];
        bytes.extend_from_slice(xml.as_bytes());

        let tml = parse_synthetic_tmlu_bytes(&bytes).unwrap();
        assert_eq!(tml.cave_name, "Test Cave");
        assert_eq!(tml.sections.len(), 1);
        assert!(tml.encoding_warning.is_none());
    }

    /// UTF-16 bytes are neither valid UTF-8 nor a Latin code page, so they
    /// would fall to the lossy branch and import as nothing with only a
    /// generic warning. Refuse them by name instead.
    #[test]
    fn a_utf16_tmlu_is_refused_by_name() {
        let xml = wrap_xml(&tml_shot_xml(1, -1, "REAL", "A"));
        let mut bytes = vec![0xff, 0xfe];
        for unit in xml.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }

        let err = parse_synthetic_tmlu_bytes(&bytes)
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(
            err.contains("UTF-16"),
            "the error must name the encoding, got {err:?}"
        );
    }

    /// An archive written on a case-insensitive filesystem can hold `data.XML`.
    /// Without the fix the exact-name lookup fails and the file is rejected as
    /// not containing `Data.xml`.
    #[test]
    fn the_zip_entry_is_found_whatever_its_case() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("synthetic.tml");
        let xml = wrap_xml(&tml_shot_xml(1, -1, "REAL", "A"));
        write_tml_archive(&path, "data.XML", xml.as_bytes());

        let tml = parse_tml(&path).unwrap();
        assert_eq!(tml.sections.len(), 1);
    }

    /// The decompressed entry is capped, so a crafted archive cannot make the
    /// reader allocate without bound. Without the fix there is no limit to hit -
    /// `read_to_string` runs to whatever the deflate stream expands to.
    #[test]
    fn an_oversized_zip_entry_is_refused_by_the_cap() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("synthetic.tml");
        // Highly compressible padding: on disk this archive is a few hundred
        // bytes, decompressed it is over 8 KB.
        let xml = format!(
            "{}\n<!--{}-->",
            wrap_xml(&tml_shot_xml(1, -1, "REAL", "A")),
            "x".repeat(8192)
        );
        write_tml_archive(&path, "Data.xml", xml.as_bytes());
        assert!(
            fs::metadata(&path).unwrap().len() < 1024,
            "premise: the archive itself is smaller than the cap it blows past"
        );

        let err = load_xml_from_zip_capped(&path, MAX_TML_ARCHIVE_BYTES, 1024)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("1024 bytes decompression limit"),
            "the error must name the limit, got {err:?}"
        );

        let loaded = load_xml_from_zip_capped(&path, MAX_TML_ARCHIVE_BYTES, 512 * 1024).unwrap();
        assert_eq!(loaded.xml.len(), xml.len());
    }

    /// The production reader must hand the capped loader the real limit, and the
    /// limit must read as a size a person recognises.
    ///
    /// A genuine 512 MB round trip is too heavy to run on every `cargo test`, so
    /// this pins the constant and its rendering instead. `MAX_DATA_XML_BYTES` has
    /// exactly one use - `load_xml_from_zip` - so the only mutation this leaves
    /// uncovered is rewriting that one call site to an inline literal.
    ///
    /// Without the fix there is no constant and no rendering to assert.
    #[test]
    fn the_production_reader_caps_at_512_mb() {
        assert_eq!(MAX_DATA_XML_BYTES, 512 * 1024 * 1024);
        assert_eq!(describe_byte_limit(MAX_DATA_XML_BYTES), "512 MB");
        assert_eq!(describe_byte_limit(1024), "1024 bytes");

        // And the wrapper really does read a file, so the cap it passes is not
        // so small that ordinary archives trip it.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("synthetic.tml");
        let xml = wrap_xml(&tml_shot_xml(1, -1, "REAL", "A"));
        write_tml_archive(&path, "Data.xml", xml.as_bytes());
        assert_eq!(load_xml_from_zip(&path).unwrap().xml.len(), xml.len());
    }

    /// An archive holding both spellings must use the exact one. The
    /// case-insensitive search is a fallback, not a preference - picking
    /// whichever entry the directory happens to list first would make the import
    /// depend on the writer's ordering.
    ///
    /// Without the fix (taking the first case-insensitive hit) this reads the
    /// decoy, because the decoy is written first.
    #[test]
    fn the_exact_zip_entry_wins_over_a_case_variant() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("synthetic.tml");
        let decoy = wrap_xml(&tml_shot_xml(1, -1, "REAL", "A")).replace("Test Cave", "Decoy Cave");
        let real = wrap_xml(&tml_shot_xml(1, -1, "REAL", "A"));
        write_tml_archive_entries(
            &path,
            &[
                ("data.XML", decoy.as_bytes()),
                ("Data.xml", real.as_bytes()),
            ],
        );

        let tml = parse_tml(&path).unwrap();
        assert_eq!(tml.cave_name, "Test Cave");
    }

    /// #576 B27(g): a path that is not a regular file - a FIFO named like a
    /// `.tmlu` or a `.tml` - blocked the import forever, and imports run on
    /// the app's main thread. Both are refused at once.
    #[cfg(unix)]
    #[test]
    fn a_fifo_named_like_a_tml_or_tmlu_is_refused_at_once() {
        use crate::parsers::encoding::fifo_test_support::{make_fifo, within_five_seconds};
        for name in ["pipe.tmlu", "pipe.tml"] {
            let dir = tempfile::tempdir().unwrap();
            let fifo = dir.path().join(name);
            make_fifo(&fifo);
            let path = fifo.clone();
            let outcome = within_five_seconds(&fifo, move || {
                parse_tml(&path)
                    .map(|t| t.sections.len())
                    .map_err(|e| format!("{e:#}"))
            });
            println!("parse_tml({name}): {outcome:?}");
            assert!(
                outcome.is_some(),
                "the read of {name} must not block on a FIFO"
            );
            let err = outcome.unwrap().unwrap_err();
            assert!(err.contains("not a regular file"), "{name}: {err}");
        }
    }

    /// #576 RV19a-6: the zipped reader reads the archive through the handle
    /// `open_regular_file` checked. A FIFO is swapped in at the path the
    /// moment the check passes, so a reader that opened the path again would
    /// block on it, and one that stopped calling `open_regular_file` would not
    /// swap at all: the archive that was checked must be the one read.
    #[cfg(unix)]
    #[test]
    fn the_tml_reader_reads_the_checked_handle_not_a_fifo_swapped_in_after() {
        use crate::parsers::encoding::fifo_test_support::{
            swap_in_a_fifo_after_the_check, within_five_seconds,
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("synthetic.tml");
        let xml = wrap_xml(&tml_shot_xml(1, -1, "REAL", "A"));
        write_tml_archive(&path, "Data.xml", xml.as_bytes());

        let checked = path.clone();
        let outcome = within_five_seconds(&path, move || {
            let swapped = swap_in_a_fifo_after_the_check();
            let read = load_xml(&checked)
                .map(|loaded| loaded.xml)
                .map_err(|e| format!("{e:#}"));
            (swapped.get(), read)
        });
        println!(
            "load_xml(.tml): (FIFO swapped in, XML bytes read) = {:?}",
            outcome
                .as_ref()
                .map(|(swapped, read)| (swapped, read.as_ref().map(String::len)))
        );
        assert_eq!(outcome, Some((true, Ok(xml))));
    }

    /// What a zipped load produced, in words for the test log.
    fn describe(outcome: Result<LoadedXml>) -> String {
        match outcome {
            Ok(loaded) => format!("Ok: read {} bytes of XML", loaded.xml.len()),
            Err(e) => format!("Err: {e}"),
        }
    }

    /// #576 W30-7c: the archive itself is capped before the ZIP reader parses
    /// it - at the XML family's ceiling, refused in the words every capped
    /// reader uses. One byte over a (test-sized) cap is refused; at the cap
    /// the archive is read.
    #[test]
    fn a_tml_archive_over_the_size_cap_is_refused_before_it_is_parsed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("synthetic.tml");
        let xml = wrap_xml(&tml_shot_xml(1, -1, "REAL", "A"));
        write_tml_archive(&path, "Data.xml", xml.as_bytes());
        let size = fs::metadata(&path).unwrap().len();

        let over = describe(load_xml_from_zip_capped(
            &path,
            size - 1,
            MAX_DATA_XML_BYTES,
        ));
        println!("{size}-byte archive, {}-byte cap: {over}", size - 1);
        let limit = format!(
            "larger than the {} bytes limit for XML survey files (.tml",
            size - 1
        );
        assert!(
            over.contains(&limit),
            "the refusal must name the limit and the family, got {over:?}"
        );

        let at = load_xml_from_zip_capped(&path, size, MAX_DATA_XML_BYTES).unwrap();
        assert_eq!(at.xml.len(), xml.len());
        assert_eq!(MAX_TML_ARCHIVE_BYTES, FileFamily::Xml.byte_limit());
    }

    /// #576 W30-7c: an archive holding more entries than a `.tml` ever does
    /// is refused with the limit named - through the production reader and
    /// its real ceiling, although it holds a perfectly good `Data.xml`.
    #[test]
    fn a_tml_archive_with_more_entries_than_the_cap_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("synthetic.tml");
        let xml = wrap_xml(&tml_shot_xml(1, -1, "REAL", "A"));
        let names: Vec<String> = (0..MAX_TML_ENTRIES).map(|i| format!("pad/{i}")).collect();
        let mut entries: Vec<(&str, &[u8])> = vec![("Data.xml", xml.as_bytes())];
        entries.extend(names.iter().map(|name| (name.as_str(), &b""[..])));
        write_tml_archive_entries(&path, &entries);

        let outcome = describe(load_xml_from_zip(&path));
        println!("{} entries: {outcome}", entries.len());
        assert!(
            outcome.contains("10001 entries") && outcome.contains("10000-entry limit"),
            "the refusal must name the count and the limit, got {outcome:?}"
        );
    }

    // -------------------------------------------------------------------
    // The entry limit holds before the ZIP reader parses (#576 RV19a-4)
    // -------------------------------------------------------------------

    /// A one-entry `Data.xml` archive as the `zip` crate writes it, cut into
    /// what precedes its central directory (the entry's local header and
    /// data) and the directory's one record, so a test can repeat the record
    /// and end the archive with records of its own.
    fn one_entry_archive_parts() -> (Vec<u8>, Vec<u8>) {
        let xml = wrap_xml(&tml_shot_xml(1, -1, "REAL", "A"));
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        writer
            .start_file("Data.xml", zip::write::SimpleFileOptions::default())
            .unwrap();
        std::io::Write::write_all(&mut writer, xml.as_bytes()).unwrap();
        let bytes = writer.finish().unwrap().into_inner();
        let end = bytes.len() - 22;
        assert!(
            bytes[end..].starts_with(b"PK\x05\x06"),
            "premise: a plain 22-byte end record closes the archive"
        );
        let directory = u32::from_le_bytes(bytes[end + 16..end + 20].try_into().unwrap()) as usize;
        (bytes[..directory].to_vec(), bytes[directory..end].to_vec())
    }

    /// A 22-byte end-of-central-directory record without a comment.
    fn end_record(entries: u16, directory_len: u32, directory_at: u32) -> Vec<u8> {
        let mut record = b"PK\x05\x06".to_vec();
        record.extend_from_slice(&[0; 4]); // this disk, the directory's disk
        record.extend_from_slice(&entries.to_le_bytes()); // on this disk
        record.extend_from_slice(&entries.to_le_bytes()); // in all
        record.extend_from_slice(&directory_len.to_le_bytes());
        record.extend_from_slice(&directory_at.to_le_bytes());
        record.extend_from_slice(&[0; 2]);
        record
    }

    /// The ZIP64 end of an archive whose directory ends at `at`: the ZIP64
    /// end record, the locator pointing at it, and a 32-bit end record whose
    /// every field defers to it.
    fn zip64_end(entries: u64, directory_len: u64, directory_at: u64, at: u64) -> Vec<u8> {
        let mut end = b"PK\x06\x06".to_vec();
        end.extend_from_slice(&44u64.to_le_bytes()); // the record's size after this field
        end.extend_from_slice(&45u16.to_le_bytes()); // made by version 4.5
        end.extend_from_slice(&45u16.to_le_bytes()); // needs version 4.5
        end.extend_from_slice(&[0; 8]); // this disk, the directory's disk
        end.extend_from_slice(&entries.to_le_bytes()); // on this disk
        end.extend_from_slice(&entries.to_le_bytes()); // in all
        end.extend_from_slice(&directory_len.to_le_bytes());
        end.extend_from_slice(&directory_at.to_le_bytes());
        end.extend_from_slice(b"PK\x06\x07");
        end.extend_from_slice(&[0; 4]); // the ZIP64 record's disk
        end.extend_from_slice(&at.to_le_bytes());
        end.extend_from_slice(&1u32.to_le_bytes()); // disks in all
        end.extend(end_record(u16::MAX, u32::MAX, u32::MAX));
        end
    }

    /// A ZIP64 archive of `records` copies of the one `Data.xml` record whose
    /// end record declares `entries`.
    fn zip64_archive(entries: u64, records: usize) -> Vec<u8> {
        let (mut archive, record) = one_entry_archive_parts();
        let directory_at = archive.len() as u64;
        archive.extend(record.repeat(records));
        let at = archive.len() as u64;
        archive.extend(zip64_end(entries, at - directory_at, directory_at, at));
        archive
    }

    /// Write `bytes` as `synthetic.tml` in `dir`.
    fn write_synthetic_tml(dir: &tempfile::TempDir, bytes: &[u8]) -> std::path::PathBuf {
        let path = dir.path().join("synthetic.tml");
        fs::write(&path, bytes).unwrap();
        path
    }

    /// An end record declaring 10 001 entries is refused on what it declares,
    /// before `ZipArchive::new` builds a record per declared entry. This one
    /// holds a single real entry, so the ZIP reader fails on its own; without
    /// the fix that parse failure, not the limit, is what the user is told.
    #[test]
    fn an_end_record_declaring_more_entries_than_the_cap_is_refused_before_the_parse() {
        let dir = tempfile::tempdir().unwrap();
        let (mut archive, record) = one_entry_archive_parts();
        let directory_at = archive.len() as u32;
        archive.extend_from_slice(&record);
        archive.extend(end_record(10_001, record.len() as u32, directory_at));
        let path = write_synthetic_tml(&dir, &archive);

        let outcome = describe(load_xml_from_zip(&path));
        println!("end record declaring 10001 entries, 1 real: {outcome}");
        assert!(
            outcome.contains("declares 10001 entries") && outcome.contains("10000-entry limit"),
            "the refusal must name the declared count and the limit, got {outcome:?}"
        );
    }

    /// The same through ZIP64, where the count is 64 bits wide: the 5.5
    /// million entries a 493 MB archive held when it cost 3.2 GB and 25 s
    /// before the old check refused it. Here the archive is a few hundred
    /// bytes, so the ZIP reader rejects it as inconsistent; without the fix
    /// the user is told that, not the limit.
    #[test]
    fn a_zip64_end_record_declaring_millions_of_entries_is_refused_before_the_parse() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_synthetic_tml(&dir, &zip64_archive(5_500_001, 1));

        let outcome = describe(load_xml_from_zip(&path));
        println!("ZIP64 end record declaring 5500001 entries, 1 real: {outcome}");
        assert!(
            outcome.contains("declares 5500001 entries") && outcome.contains("10000-entry limit"),
            "the refusal must name the declared count and the limit, got {outcome:?}"
        );
    }

    /// The ZIP reader searches the whole file backwards for end records and
    /// falls back on an earlier one whenever the last fails, so the last
    /// record's count alone bounds nothing: here it declares one entry and
    /// points its directory at itself, the reader rejects it and parses the
    /// earlier record's 10 001 entries instead - all named `Data.xml`, so the
    /// count of names it keeps is 1 and the old check let the archive
    /// through. Counting the directory records in the file refuses it.
    #[test]
    fn entries_behind_an_earlier_end_record_are_counted_before_the_parse() {
        let dir = tempfile::tempdir().unwrap();
        let (mut archive, record) = one_entry_archive_parts();
        let records = MAX_TML_ENTRIES + 1;
        let directory_at = archive.len() as u32;
        archive.extend(record.repeat(records));
        let directory_len = archive.len() as u32 - directory_at;
        archive.extend(end_record(records as u16, directory_len, directory_at));
        let last = archive.len() as u32;
        archive.extend(end_record(1, record.len() as u32, last));
        let path = write_synthetic_tml(&dir, &archive);

        let parsed = zip::ZipArchive::new(fs::File::open(&path).unwrap()).unwrap();
        println!(
            "premise: the ZIP reader falls back on the earlier end record and keeps {} name(s)",
            parsed.len()
        );
        assert_eq!(parsed.len(), 1);

        let outcome = describe(load_xml_from_zip(&path));
        println!("{records} records behind a last end record declaring 1: {outcome}");
        assert!(
            outcome.contains("10001 central-directory records")
                && outcome.contains("10000-entry limit"),
            "the refusal must name the record count and the limit, got {outcome:?}"
        );
    }

    /// The checks refuse nothing a real archive holds: a plain one-entry
    /// archive, the same entry under a ZIP64 end, and an archive at exactly
    /// the limit all load.
    #[test]
    fn archives_within_the_entry_limit_still_load() {
        let dir = tempfile::tempdir().unwrap();
        let xml = wrap_xml(&tml_shot_xml(1, -1, "REAL", "A"));
        let load = |label: &str, path: &Path| {
            let loaded = load_xml_from_zip(path)
                .map(|loaded| loaded.xml)
                .map_err(|e| format!("{e:#}"));
            println!("{label}: {:?}", loaded.as_ref().map(String::len));
            assert_eq!(loaded.as_ref(), Ok(&xml), "{label}");
        };

        let path = dir.path().join("synthetic.tml");
        write_tml_archive(&path, "Data.xml", xml.as_bytes());
        load("one entry", &path);

        load(
            "one entry under a ZIP64 end",
            &write_synthetic_tml(&dir, &zip64_archive(1, 1)),
        );

        let names: Vec<String> = (1..MAX_TML_ENTRIES).map(|i| format!("pad/{i}")).collect();
        let mut entries: Vec<(&str, &[u8])> = vec![("Data.xml", xml.as_bytes())];
        entries.extend(names.iter().map(|name| (name.as_str(), &b""[..])));
        write_tml_archive_entries(&path, &entries);
        load(&format!("{} entries", entries.len()), &path);
    }

    /// [#576 RV19a-4b] The ZIP reader does not take the ZIP64 end record the
    /// locator points at: it searches forward from there for one that ends
    /// where the locator starts, and reserves room for every entry that one
    /// declares before it reads a directory record (11 million entries: 2 GB).
    /// Here the locator points at the archive's first byte, the record sits
    /// 460 100 bytes in - past what 10 001 directory records would fill, the
    /// reader's one check of the count against the file - and the file holds
    /// no directory record. The end-record check reads only where the locator
    /// points and the record count is 0, so without the fix the ZIP reader
    /// parsed it and the user was told that parse failed.
    #[test]
    fn a_zip64_end_record_away_from_where_the_locator_points_is_refused_before_the_parse() {
        let dir = tempfile::tempdir().unwrap();
        let mut archive = vec![0u8; 460_100];
        archive.extend(zip64_end(10_001, 0, 0, 0));
        let path = write_synthetic_tml(&dir, &archive);

        let outcome = describe(load_xml_from_zip(&path));
        println!(
            "ZIP64 end record declaring 10001 entries 460100 bytes in, locator pointing at 0: \
             {outcome}"
        );
        assert!(
            outcome.contains("declares 10001 entries") && outcome.contains("10000-entry limit"),
            "the refusal must name the declared count and the limit, got {outcome:?}"
        );
    }

    /// The file is read in chunks; a ZIP64 end record or its locator that
    /// straddles the seam between two reads is seen all the same. The record
    /// is moved across the first seam (65 537 bytes in) a byte at a time.
    #[test]
    fn a_zip64_end_record_across_a_read_seam_is_seen() {
        let dir = tempfile::tempdir().unwrap();
        let seam = 64 * 1024 + 1;
        for pad in seam - 80..=seam + 2 {
            let mut archive = vec![0u8; pad];
            archive.extend(zip64_end(10_001, 0, 0, 0));
            let path = write_synthetic_tml(&dir, &archive);
            let outcome = describe(load_xml_from_zip(&path));
            assert!(
                outcome.contains("declares 10001 entries"),
                "record {pad} bytes in: got {outcome:?}"
            );
        }
    }

    /// A ZIP64 end signature can turn up in an entry's data, with counts read
    /// past it far over the limit. Its size field does not end it at a
    /// locator, so the ZIP reader would never take it, and the archive loads.
    #[test]
    fn a_zip64_signature_inside_an_entrys_data_refuses_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let xml = wrap_xml(&tml_shot_xml(1, -1, "REAL", "A"));
        let mut lookalike = b"PK\x06\x06".to_vec();
        lookalike.extend_from_slice(&[0xFF; 60]);
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        writer.start_file("Data.xml", stored).unwrap();
        std::io::Write::write_all(&mut writer, xml.as_bytes()).unwrap();
        writer.start_file("pad/blob", stored).unwrap();
        std::io::Write::write_all(&mut writer, &lookalike).unwrap();
        let archive = writer.finish().unwrap().into_inner();
        assert!(
            archive.windows(4).any(|w| w == b"PK\x06\x06"),
            "premise: the signature is in the archive's bytes"
        );
        let path = write_synthetic_tml(&dir, &archive);

        let loaded = load_xml_from_zip(&path)
            .map(|loaded| loaded.xml)
            .map_err(|e| format!("{e:#}"));
        println!("ZIP64 end signature with saturated counts in a stored entry: {loaded:?}");
        assert_eq!(loaded, Ok(xml));
    }
}
