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

use crate::parsers::encoding::strip_bom_owned;

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
            let bytes = fs::read(path).context("Failed to read TMLU file")?;
            decode_xml_bytes(bytes)
        }
        _ => bail!("Unsupported file extension '.{ext}'. Expected .tml or .tmlu"),
    }
}

/// Most a `Data.xml` entry may decompress to. The largest real one seen is
/// 57 MB, so this is a wide margin; it exists because the archive is untrusted
/// input and a deflate stream can otherwise expand without bound.
const MAX_DATA_XML_BYTES: u64 = 512 * 1024 * 1024;

/// Extract `Data.xml` from a ZIP archive.
fn load_xml_from_zip(path: &Path) -> Result<LoadedXml> {
    load_xml_from_zip_capped(path, MAX_DATA_XML_BYTES)
}

/// `load_xml_from_zip` with the decompression cap spelled out, so a test can
/// exercise the limit without building half a gigabyte.
fn load_xml_from_zip_capped(path: &Path, cap: u64) -> Result<LoadedXml> {
    let file = fs::File::open(path).context("Failed to open TML file")?;
    let mut archive = zip::ZipArchive::new(file).context("Failed to read TML as ZIP archive")?;

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

/// Render a byte limit the way the person reading the error thinks about it.
/// Whole megabytes above 1 MB, exact bytes below (the test caps live there).
fn describe_byte_limit(bytes: u64) -> String {
    const MB: u64 = 1024 * 1024;
    if bytes >= MB {
        format!("{} MB", bytes / MB)
    } else {
        format!("{bytes} bytes")
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

/// The Windows-1252 code points for `0x80..=0x9F`, the only range where it
/// differs from ISO-8859-1 (which is byte == code point). Written as escapes so
/// the table stays ASCII in source. `U+FFFD` marks the five undefined slots.
const CP1252_HIGH: [char; 32] = [
    '\u{20ac}', '\u{fffd}', '\u{201a}', '\u{0192}', '\u{201e}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{02c6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{fffd}', '\u{017d}', '\u{fffd}',
    '\u{fffd}', '\u{2018}', '\u{2019}', '\u{201c}', '\u{201d}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{02dc}', '\u{2122}', '\u{0161}', '\u{203a}', '\u{0153}', '\u{fffd}', '\u{017e}', '\u{0178}',
];

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
/// The Latin decode is done here rather than through a decoder crate on
/// purpose: nothing in the dependency tree provides one (`quick-xml` is built
/// without its `encoding` feature), and two code pages do not justify adding
/// one.
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
    let replaced = xml
        .chars()
        .filter(|&c| c == char::REPLACEMENT_CHARACTER)
        .count();
    match replaced {
        0 => String::new(),
        1 => "; 1 character was replaced".to_string(),
        n => format!("; {n} characters were replaced"),
    }
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
            if windows && (0x80..=0x9f).contains(&b) {
                CP1252_HIGH[usize::from(b - 0x80)]
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
#[expect(clippy::unwrap_used)]
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
        assert_eq!(tml.lines.len(), 1);

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
        assert_eq!(tml.lines.len(), 1);
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
        assert_eq!(tml.lines.len(), 1);
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

        let err = load_xml_from_zip_capped(&path, 1024)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("1024 bytes decompression limit"),
            "the error must name the limit, got {err:?}"
        );

        let loaded = load_xml_from_zip_capped(&path, 512 * 1024).unwrap();
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
}
