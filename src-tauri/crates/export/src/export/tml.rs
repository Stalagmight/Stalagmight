//! Ariane TML/TMLU export: the document writer and every pass after the rows
//! are built (the GPS stamp on the STARTs, the geometry repair, the SpeleoDB
//! hardening, the orphan guard). The rows themselves are read off the station
//! graph by [`survey`] (`tml_survey.rs`), whose `export_tml` and
//! `export_tml_for_speleodb` are the entry points (re-exported as
//! `export::survey::{export_tml, export_tml_for_speleodb}`).
//!
//! `.tml` files are ZIP archives containing `Data.xml`; `.tmlu` files are plain
//! XML.  TML uses long element names (`SurveyData`, `Azimut`, …) while TMLU
//! uses short abbreviations (`SRVD`, `AZ`, …).  This module faithfully
//! reproduces the naming convention used by Ariane.
//!
//! Key Ariane format rules learned from real files:
//! - One START shot per ENTRANCE, not per file. A line joined to the survey at a
//!   junction connects via `FromID` on its first REAL/VIRTUAL shot pointing
//!   directly at the junction's row, and gets no START; a line joined to nothing
//!   is a separate entrance and gets one of its own, as Ariane's own
//!   multi-entrance files do.
//!   A parentless shot that is NOT a START is an orphan root, and Ariane draws an
//!   empty map for any survey that has one alongside a closure - see
//!   `ensure_rooted_chain`.
//! - The ID element uses all-caps `<ID>` (not `<Id>`).
//! - Every shot carries `<Locked>`, `<Name>`, and `<Profiletype>` fields.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{Cursor, Write as _};
use std::path::Path;

use anyhow::{Context, Result};
use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event};
use quick_xml::Writer;

use crate::models::normalize_azimuth;

use super::atomic_write::write_atomic;
use super::tml_azimuth::AzimuthExport;

#[path = "tml_datums.rs"]
mod datums;
#[path = "tml_survey.rs"]
pub(crate) mod survey;

// ============================================================================
// Element-name map (long vs. short)
// ============================================================================

/// Provides XML element names matching the target format.
struct Names {
    cave_file: &'static str,
    cave_name: &'static str,
    unit: &'static str,
    use_magnetic_azimuth: &'static str,
    first_start_absolute_elevation: &'static str,
    data: &'static str,
    survey_data: &'static str,
    id: &'static str,
    from_id: &'static str,
    shot_type: &'static str,
    line: &'static str,
    azimuth: &'static str,
    length: &'static str,
    depth: &'static str,
    depth_in: &'static str,
    inclination: &'static str,
    left: &'static str,
    right: &'static str,
    up: &'static str,
    down: &'static str,
    comment: &'static str,
    explorer: &'static str,
    date: &'static str,
    color: &'static str,
    excluded: &'static str,
    latitude: &'static str,
    longitude: &'static str,
    closure_to_id: &'static str,
    locked: &'static str,
    name: &'static str,
    profiletype: &'static str,
    uuid: &'static str,
}

/// Long names (used by `.tml` ZIP files).
const LONG: Names = Names {
    cave_file: "CaveFile",
    cave_name: "caveName",
    unit: "unit",
    use_magnetic_azimuth: "useMagneticAzimuth",
    first_start_absolute_elevation: "firstStartAbsoluteElevation",
    data: "Data",
    survey_data: "SurveyData",
    id: "ID",
    from_id: "FromID",
    shot_type: "Type",
    line: "Section",
    azimuth: "Azimut",
    length: "Length",
    depth: "Depth",
    depth_in: "DepthIn",
    inclination: "Inclination",
    left: "Left",
    right: "Right",
    up: "Up",
    down: "Down",
    comment: "Comment",
    explorer: "Explorer",
    date: "Date",
    color: "Color",
    excluded: "Excluded",
    latitude: "Latitude",
    longitude: "Longitude",
    closure_to_id: "ClosureToID",
    locked: "Locked",
    name: "Name",
    profiletype: "Profiletype",
    uuid: "UUID",
};

/// Short names (used by `.tmlu` plain-XML files).
const SHORT: Names = Names {
    cave_file: "CaveFile",
    cave_name: "caveName",
    unit: "unit",
    use_magnetic_azimuth: "useMagneticAzimuth",
    first_start_absolute_elevation: "firstStartAbsoluteElevation",
    data: "Data",
    survey_data: "SRVD",
    id: "ID",
    from_id: "FRID",
    shot_type: "TY",
    line: "SC",
    azimuth: "AZ",
    length: "LG",
    depth: "DP",
    depth_in: "DPI",
    inclination: "INC",
    left: "L",
    right: "R",
    up: "U",
    down: "D",
    comment: "CM",
    explorer: "EX",
    date: "DT",
    color: "CL",
    excluded: "EXC",
    latitude: "LT",
    longitude: "LGT",
    closure_to_id: "CID",
    locked: "LCK",
    name: "NM",
    profiletype: "PT",
    uuid: "UU",
};

/// Convert a model pitch into Ariane's `<Inclination>`.
///
/// The model is +down (positive descends, matching `depth_in` / `depth_out`).
/// Ariane is taken to be +up, like every other survey format we read: Walls
/// and Compass both store +up in-file, and the MNemo device reports +up. That
/// could not be confirmed from data. Across every Ariane file available, all
/// 5,686 shots carry `Inclination` 0, because these surveys are depth-gauge
/// based and Ariane never populates the field. So this is an inference from the
/// surrounding ecosystem, not a measurement, and it is unobservable on real
/// files; it only changes what we emit. `None` and `0` both export as a plain
/// `0.0` rather than `-0.0`. Inverse of `parsers::tml::inclination_to_pitch`.
fn pitch_to_inclination(pitch: Option<f64>) -> f64 {
    match pitch {
        Some(p) if p != 0.0 => -p,
        _ => 0.0,
    }
}

/// Pitch for a synthesized reverse leg (the reversed and pre-zero-datum
/// duplicates Ariane needs, since it carries a single forward azimuth per shot).
///
/// Walking a shot backwards turns a descent into an ascent, so the pitch
/// negates. `pitch_out` is deliberately not used as the reversed reading:
/// despite the name it is a foresight in the same sense as `pitch_in`, not a
/// backsight. Checked across the DMP fixtures, counting only shots where both
/// legs exceed 3 degrees so gauge noise cannot vote: 49 shots agree in sign and
/// 6 disagree. Validation relies on the same property when it averages the two
/// legs. So the reverse leg is the negation of the forward pitch, whichever leg
/// recorded it. Takes the shot's two readings, `pitch_in` and `pitch_out`.
fn reversed_pitch(pitch_in: Option<f64>, pitch_out: Option<f64>) -> Option<f64> {
    pitch_in.or(pitch_out).map(|p| -p)
}

// ============================================================================
// Internal shot representation for export
// ============================================================================

#[derive(Debug)]
struct ExportShot {
    id: i64,
    /// Persistent Stalagmight shot UUID, emitted as `<UUID>`. `None` for the
    /// synthesized shots (START, CLOSURE, reversed / pre-zero-datum legs) that
    /// have no 1:1 Stalagmight shot behind them. This cannot be derived from
    /// `shot_type`: a reversed leg re-exports a REAL shot in the opposite
    /// direction, so it carries a real type but no identity of its own.
    uuid: Option<String>,
    from_id: i64,
    shot_type: &'static str, // "START", "REAL", "VIRTUAL", "CLOSURE"
    line: String,
    azimuth: f64,
    length: f64,
    depth: f64, // absolute station depth (cumulative)
    /// The TRUE start depth of the shot, always a real value. The Ariane
    /// sentinel is carried separately so a repair pass can read the measured
    /// geometry back; conflating the two made -1.0 mean both "one metre above
    /// the datum" and "same as the previous shot".
    depth_in: f64,
    /// Emit the `-1.0` "same as the previous shot" sentinel for `DepthIn`.
    depth_in_same_as_prev: bool,
    inclination: f64,
    left: f64,
    right: f64,
    up: f64,
    down: f64,
    comment: Option<String>,
    explorer: Option<String>,
    date: Option<String>,
    color: String, // "0xRRGGBBff"
    latitude: f64,
    longitude: f64,
    closure_to_id: i64,
    locked: bool,
}

/// Which consumer the emitted document is written for.
///
/// Ariane reads the file as a survey; SpeleoDB feeds it to `openspeleo_lib`,
/// which is stricter in places Ariane never looks and crashes or silently drops
/// the whole map when a rule is broken. The sidecar profile bends the exported
/// REPRESENTATION into what that converter can read - never the project - and
/// exists so those concessions cannot leak into the file a user asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TmlProfile {
    /// The user-facing "Export to Ariane" output. Byte-for-byte what we have
    /// always written; nothing below may change it.
    User,
    /// The throwaway `ariane.tml` pushed alongside a SpeleoDB commit, which the
    /// server converts to GeoJSON for the map preview.
    SpeleoDbSidecar,
}

// ============================================================================
// Output
// ============================================================================

fn names_for_path(path: &Path) -> &'static Names {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
        .unwrap_or_default();
    if ext == "tml" {
        &LONG
    } else {
        &SHORT
    }
}

fn write_output(path: &Path, xml: &[u8]) -> Result<()> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
        .unwrap_or_default();
    match ext.as_str() {
        "tml" => write_zip(path, xml),
        "tmlu" => {
            write_atomic(path, |w| Ok(w.write_all(xml)?)).context("Failed to write TMLU file")
        }
        _ => {
            write_atomic(path, |w| Ok(w.write_all(xml)?)).context("Failed to write TML export file")
        }
    }
}

// ============================================================================
// XML generation
// ============================================================================

/// The document-level values the rows do not carry.
struct DocumentHead<'a> {
    cave_name: &'a str,
    surface_elevation: Option<f64>,
    azimuth_export: AzimuthExport,
}

/// The export notes, the orphan-root guard and the XML document for finished
/// rows: everything after the station-graph walk (`survey::build_rows`).
fn write_document(
    head: &DocumentHead<'_>,
    shots: &[ExportShot],
    left_out: survey::LeftOut,
    n: &Names,
    profile: TmlProfile,
) -> Result<(Vec<u8>, Vec<String>)> {
    let azimuth_export = head.azimuth_export;
    // The two azimuth compromises are decisions the FILE carries and the
    // project does not, so they are stated rather than done quietly (#438):
    // returned to the caller, which raises a toast, and logged for a run that
    // has no user.
    let mut notes: Vec<String> = Vec::new();
    if let Some(code) = azimuth_export.note_code() {
        tracing::warn!(
            decision = ?azimuth_export,
            note = code,
            purpose = "honest single-flag Ariane document",
            "{}",
            azimuth_export.log_message()
        );
        notes.push(code.to_string());
    }
    // Same contract as the azimuth notes above (#438): a fact the FILE carries
    // and the project does not must reach the user, not just the log. The
    // detailed warn lives in the GPS pass; this is its UI-facing code.
    if left_out.datums > 0 {
        notes.push("gps-anchors-not-written".to_string());
    }
    // A comment on an entrance's station has no row to ride (its row is a
    // START), in 0.6.0 too; the file is silent about it, so the user is told
    // (demo issue N-14). Each station is named in the debug log.
    if left_out.station_comments > 0 {
        tracing::info!(
            comments = left_out.station_comments,
            purpose = "tell the user what the Ariane file leaves out",
            "Station comments on entrance stations left out of the .tml export"
        );
        notes.push("station-comments-not-written".to_string());
    }
    ensure_rooted_chain(shots, profile)?;

    let mut buf = Vec::new();
    let mut writer = Writer::new_with_indent(Cursor::new(&mut buf), b' ', 4);

    // <?xml version="1.0" encoding="UTF-8" standalone="yes"?>
    writer.write_event(Event::Decl(BytesDecl::new(
        "1.0",
        Some("UTF-8"),
        Some("yes"),
    )))?;

    // <CaveFile>
    writer.write_event(Event::Start(BytesStart::new(n.cave_file)))?;

    // Metadata elements
    write_text_element(&mut writer, n.cave_name, head.cave_name)?;
    write_text_element(
        &mut writer,
        n.first_start_absolute_elevation,
        &format_f64(head.surface_elevation.unwrap_or(0.0)),
    )?;

    // <Data>
    writer.write_event(Event::Start(BytesStart::new(n.data)))?;

    for shot in shots {
        write_shot(&mut writer, n, shot)?;
    }

    // </Data>
    writer.write_event(Event::End(BytesEnd::new(n.data)))?;

    // Remaining metadata after Data (matching Ariane's ordering)
    write_text_element(&mut writer, n.unit, "m")?;
    write_text_element(
        &mut writer,
        n.use_magnetic_azimuth,
        if azimuth_export.is_magnetic() {
            "true"
        } else {
            "false"
        },
    )?;

    // </CaveFile>
    writer.write_event(Event::End(BytesEnd::new(n.cave_file)))?;

    drop(writer);
    Ok((buf, notes))
}

fn write_text_element<W: std::io::Write>(
    writer: &mut Writer<W>,
    tag: &str,
    text: &str,
) -> Result<()> {
    writer.write_event(Event::Start(BytesStart::new(tag)))?;
    writer.write_event(Event::Text(BytesText::new(text)))?;
    writer.write_event(Event::End(BytesEnd::new(tag)))?;
    Ok(())
}

fn write_shot<W: std::io::Write>(
    writer: &mut Writer<W>,
    n: &Names,
    shot: &ExportShot,
) -> Result<()> {
    writer.write_event(Event::Start(BytesStart::new(n.survey_data)))?;

    // Write fields in alphabetical order (matching Ariane's output)
    write_text_element(writer, n.azimuth, &format_f64(shot.azimuth))?;
    write_text_element(writer, n.closure_to_id, &shot.closure_to_id.to_string())?;
    write_text_element(writer, n.color, &shot.color)?;

    let comment_str = shot.comment.as_deref().unwrap_or("");
    write_text_element(writer, n.comment, comment_str)?;

    let date_str = shot.date.as_deref().unwrap_or(DEFAULT_EXPORT_DATE);
    write_text_element(writer, n.date, date_str)?;

    write_text_element(writer, n.depth, &format_f64(shot.depth))?;
    write_text_element(writer, n.depth_in, &format_f64(emitted_depth_in(shot)))?;
    write_text_element(writer, n.down, &format_f64(shot.down))?;
    write_text_element(writer, n.excluded, "false")?;

    let explorer_str = shot.explorer.as_deref().unwrap_or("unknown");
    write_text_element(writer, n.explorer, explorer_str)?;

    write_text_element(writer, n.from_id, &shot.from_id.to_string())?;
    write_text_element(writer, n.id, &shot.id.to_string())?;
    write_text_element(writer, n.inclination, &format_f64(shot.inclination))?;
    write_text_element(writer, n.latitude, &format_f64(shot.latitude))?;
    write_text_element(writer, n.left, &format_f64(shot.left))?;
    write_text_element(writer, n.length, &format_f64(shot.length))?;
    write_text_element(writer, n.locked, if shot.locked { "true" } else { "false" })?;
    write_text_element(writer, n.longitude, &format_f64(shot.longitude))?;

    // Ariane conventions: START has START name, CLOSURE uses CLOSURE:CLOSURE.
    let name_str = match shot.shot_type {
        "START" => "START",
        "CLOSURE" => "CLOSURE:CLOSURE",
        _ => "",
    };
    write_text_element(writer, n.name, name_str)?;

    write_text_element(writer, n.profiletype, "VERTICAL")?;
    write_text_element(writer, n.right, &format_f64(shot.right))?;
    write_text_element(writer, n.line, &shot.line)?;
    write_text_element(writer, n.shot_type, shot.shot_type)?;
    write_text_element(writer, n.up, &format_f64(shot.up))?;

    // Stable per-shot UUID. Emitted for every block that stands for a REAL
    // PROJECT SHOT - the forward blocks and the reversed "(reversed)" /
    // "(pre-zero-datum)" legs alike - and omitted only for the blocks this exporter
    // SYNTHESIZES, which are the START stations, the CLOSURE connectors and the
    // VIRTUAL padding shot. It is what makes a re-export of an unchanged project
    // byte-identical, which the SpeleoDB co-push render relies on. Ariane 26+
    // preserves it across edits, <=25 ignores it.
    //
    // The reversed legs used to be omitted too, and that cost their shots their
    // identity on the way back in: the reader has nothing to preserve, so
    // `Shot::with_uuid` mints a fresh one. On a real 532-shot survey that was 78
    // shots re-identified by a round trip through a file this app wrote (found
    // by `export-roundtrip-fuzz`'s X6). A reversed leg is not a synthetic shot -
    // it is a real shot emitted with its heading and LRUD mirrored, and it is
    // that shot's ONLY appearance in the document, because every constructor
    // pairs it with a forward block over a DISJOINT index range (`0..split` vs
    // `split..len`) or replaces the forward block entirely (the whole-line
    // reversal). So no shot can appear twice, and `remint_duplicate_uuids`
    // cannot see a forward/reverse pair as a duplicate - pinned by
    // `a_reversed_leg_is_a_shots_only_appearance_so_nothing_is_reminted`.
    if let Some(uuid) = &shot.uuid {
        write_text_element(writer, n.uuid, uuid)?;
    }

    writer.write_event(Event::End(BytesEnd::new(n.survey_data)))?;
    Ok(())
}

/// Ariane's `Explorer` field for one row (#329), in a form every reader
/// takes to the same people in the same roles.
///
/// * Anyone credited: the element form
///   `<Explorer>e1, e2</Explorer><Surveyor>s1, s2</Surveyor>` (entity-escaped
///   by the writer, exactly what the importer's element branch reads), a role
///   with nobody left empty. Never the plain form: this importer, like a
///   `.stalx` legacy column, reads a single plain value as surveyors, while
///   Ariane shows it in its Explorer field and openspeleo-lib reads it as
///   explorers - the element form names the role for all of them.
/// * `None` names nobody, written as Ariane's `unknown`.
///
/// Within a role, names are joined with `", "` ([`role_text`]). `<` and `>`
/// are dropped from every exported name: inside this pseudo-XML a name holding
/// `</Explorer><Surveyor>` would move people between roles on re-import. A `,`
/// or `;` inside ONE name cannot round-trip: Ariane itself separates the
/// people of a role with commas, so the importer splits there.
fn row_explorer(credit: &crate::models::SurveyCredit) -> Option<String> {
    let explorers = role_text(&credit.explorers);
    let surveyors = role_text(&credit.surveyors);
    if explorers.is_none() && surveyors.is_none() {
        return None;
    }
    Some(format!(
        "<Explorer>{}</Explorer><Surveyor>{}</Surveyor>",
        explorers.unwrap_or_default(),
        surveyors.unwrap_or_default()
    ))
}

/// One role's names for [`row_explorer`], joined with `", "`: the importer
/// reads every TML role as a name list ([`crate::models::NameSource`]), with
/// no `Last, First` rule, so two one-word names come back as two people.
fn role_text(people: &[crate::models::Person]) -> Option<String> {
    let names: Vec<String> = people
        .iter()
        .map(|p| p.name.replace(['<', '>'], "").trim().to_string())
        .filter(|n| !n.is_empty())
        .collect();
    (!names.is_empty()).then(|| names.join(", "))
}

/// The row's `Date`: the shot's survey date, ISO.
fn row_date(credit: &crate::models::SurveyCredit) -> Option<String> {
    credit.date.map(|d| d.format("%Y-%m-%d").to_string())
}

/// Stand-in for a line with no survey date. `<Date>` is not optional to every
/// consumer - SpeleoDB rejects the whole file when it is missing or empty - so a
/// dateless line still gets a parseable one.
const DEFAULT_EXPORT_DATE: &str = "2000-01-01";

/// Ariane's `DepthIn` sentinel: "this shot starts where the previous one ended".
const DEPTH_IN_SENTINEL: f64 = -1.0;

/// The `DepthIn` value to write for `shot`.
///
/// A real depth of exactly -1.0 is representable (a station one metre ABOVE the
/// datum) and would be read back as the sentinel, silently moving the shot to
/// wherever the previous one ended. Nudging it below the sixth decimal keeps the
/// written value distinguishable while staying a micrometre from the truth - far
/// under any survey's resolution, and the only lossless alternative would be to
/// change the file format.
///
/// The collision has to be judged on the value AS WRITTEN, not on the f64 we hold:
/// `format_f64` emits six decimals, so -0.9999996 and -1.0000004 both land in the
/// file as "-1.0" and come back as the sentinel through our own parser. Comparing
/// the raw f64s let exactly those values through the guard that exists to stop
/// them.
fn emitted_depth_in(shot: &ExportShot) -> f64 {
    if shot.depth_in_same_as_prev {
        return DEPTH_IN_SENTINEL;
    }
    // `round_as_written` lands on a multiple of 1e-6, so the nearest value that is
    // NOT the sentinel is a full micrometre away - an epsilon compare here is an
    // exact-equality test on the written value, without tripping `float_cmp`.
    if (round_as_written(shot.depth_in) - DEPTH_IN_SENTINEL).abs() < f64::EPSILON {
        return DEPTH_IN_SENTINEL - 1e-6;
    }
    shot.depth_in
}

/// Format an f64 without unnecessary trailing zeros but preserving at least one
/// decimal place (Ariane convention).
fn format_f64(v: f64) -> String {
    if v == 0.0 {
        return "0.0".to_string();
    }
    // Use enough precision to round-trip, then strip trailing zeros
    let s = format!("{v:.6}");
    let s = s.trim_end_matches('0');
    // Ensure at least one digit after the decimal point
    if s.ends_with('.') {
        format!("{s}0")
    } else {
        s.to_string()
    }
}

// ============================================================================
// ZIP wrapper
// ============================================================================

fn write_zip(path: &Path, xml: &[u8]) -> Result<()> {
    write_atomic(path, |file| {
        let mut zip = zip::ZipWriter::new(file);

        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);

        zip.start_file("Data.xml", options)
            .context("Failed to start Data.xml in ZIP")?;
        zip.write_all(xml)
            .context("Failed to write Data.xml to ZIP")?;
        zip.finish().context("Failed to finalize TML ZIP")?;
        Ok(())
    })
}

/// A parentless row that is not a START: Ariane reads it as an entrance it
/// cannot place, and draws an empty map once any CLOSURE row exists.
fn is_orphan_root(shot: &ExportShot) -> bool {
    shot.from_id == -1 && shot.shot_type != "START"
}

/// Refuse to write a chain Ariane would draw as an empty map.
///
/// Ariane reads `FromID = -1` as an entrance and expects `Type = START` there; every
/// parentless shot in an Ariane-authored file is one. A shot left parentless for any
/// other reason is an orphan root, and although Ariane still lists it in the data
/// table, ONE such shot anywhere in a survey that ALSO has a `CLOSURE` row makes it
/// abandon the loop solve and draw a completely empty map. That combination shipped:
/// the export succeeded, warned into a log nobody reads, and produced a file that
/// opened blank in Ariane 26.3.
///
/// It takes BOTH, so this refuses only when both are present. An orphan on its own is
/// survivable - Ariane places it, and such files exported and drew correctly before -
/// so failing that case would turn a drawable file into no file at all, which is a
/// worse trade than the one this guard exists to make.
fn ensure_rooted_chain(shots: &[ExportShot], profile: TmlProfile) -> Result<()> {
    // The SpeleoDB sidecar pads a lone START with a parentless VIRTUAL shot on
    // purpose (see `the_sidecar_never_emits_exactly_one_survey_data`). That file is
    // read by `openspeleo_lib`, which drops VIRTUAL from the map and never runs a
    // loop solve, so the orphan is inert there. Only the Ariane-facing export cares.
    if !matches!(profile, TmlProfile::User) {
        return Ok(());
    }

    let orphans: Vec<&ExportShot> = shots.iter().filter(|s| is_orphan_root(s)).collect();

    let Some(first) = orphans.first() else {
        return Ok(());
    };

    let mut lines: Vec<&str> = orphans.iter().map(|s| s.line.as_str()).collect();
    lines.sort_unstable();
    lines.dedup();
    // Line names come from imported third-party files, so neither their count
    // nor their length is ours to trust - and this string reaches both the log
    // and the UI (#378 item g). Cap the list and truncate each name.
    let listed = list_capped(&lines);

    // Without a closure Ariane never runs the loop solve, so the orphan is inert and
    // the file draws. Say so once and let the export through.
    if !shots.iter().any(|s| s.shot_type == "CLOSURE") {
        tracing::warn!(
            orphan_count = orphans.len(),
            lines = listed,
            "Exported shots have no parent station; Ariane will draw them as separate \
             entrances. Adding a loop closure to this survey would blank its map."
        );
        return Ok(());
    }

    anyhow::bail!(
        "Cannot export {listed}: {} shot(s) there have no parent station, and this survey \
         also has loop closures - Ariane draws an empty map for that combination. Connect \
         the line to the rest of the survey, or remove the closures. \
         (First unrooted shot: id {}, type {}.)",
        orphans.len(),
        first.id,
        first.shot_type,
    )
}

/// How many line names an error/warning lists before summarising, and how
/// long each shown name may be. Imported files decide both, so the message must
/// not scale with them.
const LISTED_LINES_MAX: usize = 5;
const LISTED_LINE_NAME_MAX: usize = 60;

fn list_capped(names: &[&str]) -> String {
    let mut shown: Vec<String> = names
        .iter()
        .take(LISTED_LINES_MAX)
        .map(|name| {
            // Control characters would let an imported name forge log lines or
            // steer the terminal; the length caps alone don't stop that.
            let clean: String = name
                .chars()
                .filter(|c| !c.is_control())
                .take(LISTED_LINE_NAME_MAX + 1)
                .collect();
            if clean.chars().count() > LISTED_LINE_NAME_MAX {
                let cut: String = clean.chars().take(LISTED_LINE_NAME_MAX).collect();
                format!("{cut}...")
            } else {
                clean
            }
        })
        .collect();
    if names.len() > LISTED_LINES_MAX {
        shown.push(format!("and {} more", names.len() - LISTED_LINES_MAX));
    }
    shown.join(", ")
}

// ============================================================================
// Build flat shot list from project model
// ============================================================================

/// The UUID emitted as `<UUID>` for a real forward shot, from its stored uuid.
/// Every shot of the station graph carries one (the model mints it), so the mint
/// here is a fallback that keeps the writer total - without it a shot holding no
/// identity would render to different bytes on each export.
///
/// What this value guarantees is STABILITY across re-exports. Uniqueness is the
/// model's: a shot UUID is held by one shot in the whole project, an invariant
/// `Project::validate` checks (every door that brings ids in re-mints repeats,
/// the frozen 0.6.0 reader heals an old file that repeats one, and a version 2
/// file that repeats one is refused at load). Every edit and history step is
/// validated, so a project the app holds has no repeat. The export still
/// settles repeats after the full shot list is assembled, with
/// `remint_duplicate_uuids` (user profile) and `harden_for_speleodb`
/// (sidecar), as a safety net only: neither can fire on a valid project.
///
/// Never returns an empty string: an empty stored UUID is treated as "no identity
/// yet" and gets the same fresh mint as `None`, because an empty `<UUID>` element
/// is worse than an ephemeral one (it reads as a real, shared key).
fn export_uuid(stored: Option<&str>) -> String {
    match stored {
        Some(u) if !u.is_empty() => u.to_string(),
        _ => uuid::Uuid::new_v4().to_string(),
    }
}

/// Document-level `<UUID>` uniqueness for the user-facing export.
///
/// The model keeps shot UUIDs unique across the project (see `export_uuid`),
/// so this is a safety net: a project that held a repeat anyway (an invalid
/// one, which no edit, history step or load leaves) would otherwise carry the
/// same `<UUID>` twice in one document. How Ariane treats a repeated
/// id is untestable from here, and a consumer keying features on `<UUID>` would
/// alias the copies - so the export fails safe the same way the sidecar does: the
/// FIRST occurrence keeps its identity (round-trip fidelity for the copy that
/// legitimately owns it), every later repeat gets a fresh mint. Unlike
/// `harden_for_speleodb` this never drops the element - Ariane's own files carry
/// a `<UUID>` per row, so an omitted one is the shape least like what it writes.
///
/// Verbatim string comparison, deliberately: the duplicate class this exists for
/// is byte-identical copies of one imported id, and anchors resolve on the stored
/// string too - two spellings of one v4 are two identities everywhere else in the
/// model, so treating them as one here would be the inconsistency.
///
/// The fidelity guarantee is scoped: it holds for the FIRST occurrence only,
/// and only that copy is stable across exports - a later repeat draws a fresh
/// v4 per export (only the `ExportShot` is touched, never the project), so a
/// project holding a cross-line duplicate does not re-export byte-identical.
/// Accepted: the sidecar profile has no such repeats (it drops them), and the
/// user-facing byte-stability claim (`export_uuid`) was always conditional
/// on the project having been saved, i.e. on identities being settled.
fn remint_duplicate_uuids(shots: &mut [ExportShot]) {
    let mut seen: HashSet<String> = HashSet::new();
    for shot in shots.iter_mut() {
        if let Some(uuid) = shot.uuid.as_mut() {
            if !seen.insert(uuid.clone()) {
                *uuid = uuid::Uuid::new_v4().to_string();
                seen.insert(uuid.clone());
            }
        }
    }
}

/// Decimal places `format_f64` writes. Every check that has to agree with what a
/// reader will see - the final geometry sweep, and the `DepthIn` sentinel guard -
/// has to hold on the values as they land in the FILE, not on the f64s we happen
/// to hold, so it compares numbers rounded exactly the way they will be written.
const ARIANE_WRITTEN_DECIMALS: f64 = 1e6;

fn round_as_written(v: f64) -> f64 {
    (v * ARIANE_WRITTEN_DECIMALS).round() / ARIANE_WRITTEN_DECIMALS
}

/// The value a consumer parsing the file will actually hold: what `format_f64`
/// puts in the XML, read back.
///
/// `round_as_written` names the same number for every value we have ever
/// exported, but not by construction: `{:.6}` rounds half to even while
/// `f64::round` rounds half away from zero, so on a tie the two disagree by a
/// micrometre. A check that has to survive contact with a server re-deriving the
/// bound from the decimal strings has to be made on the strings.
fn as_written(v: f64) -> f64 {
    format_f64(v).parse::<f64>().unwrap_or(v)
}

/// The shortest tape whose WRITTEN form still covers `delta`.
///
/// Assigning the raw `delta` is not enough for a consumer that re-parses the
/// file: a 1.3000000000000007 m drop written to six decimals comes back as
/// "1.3", which is BELOW the drop it was minted to cover, and SpeleoDB rejects
/// the whole survey for it ("Shot is shorter than the vertical variation").
/// Rounding up to the next micrometre covers every case; the loop is the check
/// against the rounding tie, and takes at most an iteration or two.
fn length_covering_written_delta(delta: f64) -> f64 {
    let mut fixed = (delta * ARIANE_WRITTEN_DECIMALS).ceil() / ARIANE_WRITTEN_DECIMALS;
    for _ in 0..4 {
        if as_written(fixed) >= delta {
            break;
        }
        fixed += 1.0 / ARIANE_WRITTEN_DECIMALS;
    }
    fixed
}

/// Make the exported shot chain satisfy the geometry invariants an Ariane consumer
/// enforces, so a file we produce can always be read back as geometry.
///
/// The one that bites is that a consumer derives each shot's vertical change from
/// its PARENT's `Depth`, not from the shot's own `DepthIn` - so the chain must
/// satisfy `|Depth - parent.Depth| <= Length` for every shot. Our own model is
/// richer: a shot carries `depth_in` and `depth_out` independently, so a branch
/// that starts at a different depth than the station it hangs off is
/// representable here and simply is not representable in Ariane's chain. Exported
/// verbatim, such a junction reads as a shot with more vertical drop than length -
/// geometrically impossible - and SpeleoDB's `openspeleo_lib` raises on it, which
/// aborts GeoJSON generation for the ENTIRE project (the upload still returns 200,
/// so nothing surfaces to the user).
///
/// The repair re-hangs the offending shot from its parent's depth while KEEPING
/// its own measured vertical change, and says so in the file by marking `DepthIn`
/// as "same as the previous shot" - so `DepthIn`, `Depth` and `Length` agree with
/// each other afterwards. Reading the measured change from `depth_in` is why that
/// field holds the true depth rather than the sentinel: a branch's first shot is
/// always flagged same-as-previous, so a pass that inferred the change from the
/// flag would discard the measurement and re-derive it from the very gap that
/// violated the bound - exporting the shot at the wrong depth, and in the wrong
/// vertical direction.
///
/// Moving one shot moves the rest of its branch with it. Every shot after the
/// re-hung one is contiguous in the model and therefore flagged
/// same-as-previous, which puts the `-1.0` sentinel in the file - "I start where
/// my parent ended" - so a consumer reads its origin off the parent's MOVED
/// depth. Leaving such a shot at the depth the model gave it makes the file
/// claim a vertical change that was never surveyed, in whichever direction the
/// arithmetic happens to land: a 4.79 m re-hang turned a measured 0.5 m descent
/// into a 4.29 m ascent. So wherever the sentinel is written the exported depth
/// is re-derived as `parent.Depth + own measured change` - a no-op for the
/// ordinary contiguous shot, whose parent ends exactly where it starts, and a
/// rigid carry of the offset down the rest of the branch. A shot carrying an
/// explicit `DepthIn` states its own origin, so it is only repaired if it breaks
/// the bound.
///
/// The branch's FIRST shot is the same story with no cascade behind it: it is
/// flagged against its own line, so the file hangs it off a station it was
/// never measured from. Where the tape is long enough for the bound to hold, that
/// used to pass every check while stating a vertical change nobody surveyed.
///
/// Only a shot whose own drop already exceeds its own length is clamped further,
/// and that is broken data either way. Shots that already satisfy the bound and
/// hang off a parent that did not move - every continuous chain, which is nearly
/// all of them - are left untouched.
///
/// Also sweeps non-finite values, which reach the same comparison and fail it (any
/// comparison against NaN is false), and would otherwise be written into the file.
fn enforce_ariane_geometry_invariants(shots: &mut [ExportShot], profile: TmlProfile) {
    let mut sanitized = 0usize;
    for shot in shots.iter_mut() {
        // Every numeric field that reaches the file, not just the ones the depth
        // bound reads: `NaN` written into the XML fails a strict consumer at parse
        // time, which loses the whole file rather than the one shot.
        for value in [
            &mut shot.azimuth,
            &mut shot.length,
            &mut shot.depth,
            &mut shot.depth_in,
            &mut shot.inclination,
            &mut shot.left,
            &mut shot.right,
            &mut shot.up,
            &mut shot.down,
            &mut shot.latitude,
            &mut shot.longitude,
        ] {
            if !value.is_finite() {
                *value = 0.0;
                sanitized += 1;
            }
        }
        // A negative tape reading is not a direction, and it makes the bound below
        // unsatisfiable for every child.
        if shot.length < 0.0 {
            shot.length = 0.0;
            sanitized += 1;
        }
    }
    if sanitized > 0 {
        tracing::warn!(
            fields = sanitized,
            "Ariane export: replaced non-finite or negative geometry values with 0"
        );
    }

    let index_by_id: HashMap<i64, usize> =
        shots.iter().enumerate().map(|(i, s)| (s.id, i)).collect();
    // Adjacency by INDEX, so the walk can borrow it while mutating `shots` without
    // cloning a child list per node.
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); shots.len()];
    let mut queue: VecDeque<usize> = VecDeque::new();
    for (i, shot) in shots.iter().enumerate() {
        match index_by_id.get(&shot.from_id) {
            Some(&parent) if shot.from_id != -1 => children[parent].push(i),
            // A root, or a dangling parent id: nothing to measure against.
            _ => queue.push_back(i),
        }
    }

    // Parent-before-child, so a child is measured against its parent's ALREADY
    // repaired depth and one junction cannot cascade into its whole branch.
    let mut visited = vec![false; shots.len()];
    // Split so the log distinguishes the shots Ariane's chain could not have held
    // at all (their exported drop was longer than their tape) from the ones merely
    // put where the file already says they are. The second is not a consequence of
    // the first: a branch's first shot is flagged same-as-previous against its OWN
    // line, so it writes the sentinel and needs moving onto its parent even when
    // nothing upstream was repaired.
    let mut repaired = 0usize;
    let mut carried = 0usize;
    while let Some(i) = queue.pop_front() {
        if visited[i] {
            continue;
        }
        visited[i] = true;
        let parent_depth = shots[i].depth;
        for k_pos in 0..children[i].len() {
            let k = children[i][k_pos];
            if visited[k] {
                continue; // a cycle; leave it alone rather than rewriting twice
            }
            queue.push_back(k);
            let shot = &mut shots[k];
            let breaks_the_bound = (shot.depth - parent_depth).abs() > shot.length;
            // How far the parent's exported depth sits from where this shot was
            // measured to start, counted only when the file says the two meet -
            // which is precisely when a consumer takes this shot's origin from the
            // parent instead of from `DepthIn`. Non-zero for a branch's first shot
            // (flagged against its own line, not against the station it hangs
            // off) and for everything below a shot the repair has moved. The
            // tolerance matches the one the same-as-previous flag was computed
            // with, so a contiguous shot whose depths agree to within it is left
            // exactly as it is.
            let inherited_offset = if shot.depth_in_same_as_prev {
                parent_depth - shot.depth_in
            } else {
                0.0
            };
            if !breaks_the_bound && inherited_offset.abs() < 1e-9 {
                continue;
            }
            // The measured vertical change, taken from the true `depth_in` - the
            // part of the survey we can be sure of. What we cannot honour is the
            // absolute depth, because the junction it is measured from does not
            // exist in Ariane's chain.
            let own_delta = shot.depth - shot.depth_in;
            let clamped = own_delta.clamp(-shot.length, shot.length);
            shot.depth = parent_depth + clamped;
            // Say in the file what we just did: the shot now starts where its
            // parent ended, so DepthIn/Depth/Length agree with each other and a
            // consumer honouring DepthIn reads the same geometry as one walking
            // the chain.
            shot.depth_in = parent_depth;
            shot.depth_in_same_as_prev = true;
            if breaks_the_bound {
                repaired += 1;
            } else {
                carried += 1;
            }
        }
    }
    if repaired > 0 {
        tracing::warn!(
            shots = repaired,
            "Ariane export: re-hung shots whose depth change exceeded their length \
             (a junction depth discontinuity Ariane's chain cannot express)"
        );
    }
    if carried > 0 {
        tracing::warn!(
            shots = carried,
            "Ariane export: re-hung shots onto the parent depth their DepthIn \
             sentinel already claims for them; each keeps its measured vertical \
             change, so only absolute depths move"
        );
    }

    // Final sweep on the values AS WRITTEN. Everything above works in f64 while the
    // file carries six decimals, and the two rounding errors (Depth, Length) are
    // independent - so a shot sitting exactly on the bound, which every CLOSURE
    // does by construction, can still cross it once written. Lengthening the tape
    // by under a micrometre is the smallest possible lie and keeps the file
    // readable; shortening the drop instead would move a station.
    //
    // Only for shots the walk above actually reached, because only there is the
    // correction bounded by rounding. An unvisited shot hangs off a `from_id`
    // cycle (tolerated above rather than rewritten twice) and its gap to that
    // parent is the full, unrepaired one - overwriting `Length` with it would
    // replace a measured tape reading with a fabricated one, silently, and buy
    // nothing: a consumer reaches shots by walking down from an anchor, exactly as
    // this pass does, so a shot we could not reach is one it will drop as an
    // orphan or a cycle member long before it checks the shot's geometry
    // (`openspeleo_lib` 0.0.15 `find_valid_shot_ids`). Warn instead - the survey
    // stays intact and the pathological topology is on the record.
    let mut unreachable = 0usize;
    for i in 0..shots.len() {
        let Some(&parent) = index_by_id.get(&shots[i].from_id) else {
            continue;
        };
        if shots[i].from_id == -1 {
            continue;
        }
        if !visited[i] {
            unreachable += 1;
            continue;
        }
        // The sidecar is measured on the values as the SERVER will parse them,
        // and repaired with a length that survives being written back out. The
        // user profile keeps the arithmetic it has always used: the two agree on
        // every value we have seen, and the difference is only ever a micrometre
        // of tape, but a byte-identical Ariane export is worth more than that
        // here - Ariane never applies this check.
        let (parent_depth, depth, length) = match profile {
            TmlProfile::User => (
                round_as_written(shots[parent].depth),
                round_as_written(shots[i].depth),
                round_as_written(shots[i].length),
            ),
            TmlProfile::SpeleoDbSidecar => (
                as_written(shots[parent].depth),
                as_written(shots[i].depth),
                as_written(shots[i].length),
            ),
        };
        let delta = (depth - parent_depth).abs();
        if delta > length {
            shots[i].length = match profile {
                TmlProfile::User => delta,
                TmlProfile::SpeleoDbSidecar => length_covering_written_delta(delta),
            };
        }
    }
    if unreachable > 0 {
        tracing::warn!(
            shots = unreachable,
            "Ariane export: shots unreachable from any root (a `from_id` cycle) were \
             left with their measured geometry; a consumer walking down from the \
             anchor will drop them"
        );
    }
}

// ============================================================================
// SpeleoDB sidecar hardening
// ============================================================================

/// The character SpeleoDB joins `name/description/date/explorers/surveyors` with
/// before hashing a shot into a line (U+220E). Left inside one of those
/// fields it re-cuts the survey into lines nobody drew.
const SPELEODB_LINE_FIELD_SEPARATOR: char = '\u{220e}';

/// Year range of the IGRF model SpeleoDB derives magnetic declination from.
/// Outside it the declination silently becomes 0.0, rotating the whole map by
/// however much the survey's declination was.
const SPELEODB_MIN_YEAR: i32 = 1900;
const SPELEODB_MAX_YEAR: i32 = 2035;

/// An anchor component this close to zero is read as "no anchor at all", and
/// SpeleoDB skips the map rather than placing it - so a survey on the equator or
/// the prime meridian silently gets no preview.
const SPELEODB_ANCHOR_EPSILON: f64 = 1e-7;

/// Shot count past which the server's conversion worker is at risk of running
/// out of memory (roughly 7 KB per shot against a 1.25 GiB cap).
const SPELEODB_SHOT_COUNT_WARN: usize = 25_000;

/// Move a GPS anchor to somewhere SpeleoDB will actually use.
///
/// A component whose written value is zero reads as "unset" there, so a survey
/// sitting on the equator or the prime meridian gets no map at all. The nudge is
/// one micro-degree - about 11 cm, under a GPS fix's own error, and the smallest
/// move that survives `format_f64`'s six decimals at all (a tenth of it writes
/// as "0.0" and changes nothing). Out-of-range values pass SpeleoDB's parse and
/// blow up later inside its projection, so they are clamped rather than left.
/// A survey with no datum keeps its 0/0 and gets no map, which is correct.
fn speleodb_anchor(latitude: f64, longitude: f64) -> (f64, f64) {
    (
        speleodb_anchor_component(latitude, 90.0),
        speleodb_anchor_component(longitude, 180.0),
    )
}

fn speleodb_anchor_component(value: f64, limit: f64) -> f64 {
    // NaN survives both the clamp and the compare, and would then be written as an
    // anchor of exactly 0.0 - the no-map case this nudge exists to prevent.
    //
    // It cannot arrive: `GpsCoordinates::new` rejects a non-finite component at
    // construction, so no datum in a loaded project carries one. NOT the geometry
    // sweep, which an earlier version of this comment credited - that sweep runs in
    // `enforce_ariane_geometry_invariants`, AFTER this function, so it could not have
    // cleaned up behind us even if it were the only guard.
    let clamped = value.clamp(-limit, limit);
    if as_written(clamped).abs() <= SPELEODB_ANCHOR_EPSILON {
        let nudge = 1.0 / ARIANE_WRITTEN_DECIMALS;
        if clamped < 0.0 {
            -nudge
        } else {
            nudge
        }
    } else {
        clamped
    }
}

/// A `<Date>` SpeleoDB will accept, and whose year its declination model covers.
///
/// A missing, empty or non-ISO date fails validation for the ENTIRE file, not
/// for the one line, so an unparseable date falls back rather than being
/// passed through. The year clamp trades a wrong date for a right declination:
/// out of range the model returns 0.0 without saying so, which rotates the map.
fn speleodb_date(raw: Option<&str>) -> String {
    use chrono::Datelike as _;

    let Some(parsed) = raw
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
    else {
        return DEFAULT_EXPORT_DATE.to_owned();
    };
    if parsed.year() < SPELEODB_MIN_YEAR {
        return format!("{SPELEODB_MIN_YEAR}-01-01");
    }
    if parsed.year() > SPELEODB_MAX_YEAR {
        return format!("{SPELEODB_MAX_YEAR}-12-31");
    }
    parsed.format("%Y-%m-%d").to_string()
}

/// The UUID as SpeleoDB will read it, or `None` if it must not be emitted.
///
/// The field is typed `UUID4` there, so anything else - a v1, a nil, a string
/// that is not a UUID at all - fails validation for the whole file. Variant is
/// part of it: a version nibble of 4 outside the RFC 4122 variant leaves
/// Python's `uuid.UUID.version` as `None`, which is not 4 either. We pass
/// `Shot::uuid` through verbatim and it can hold anything an imported file did,
/// so it is checked rather than trusted.
fn speleodb_uuid(raw: &str) -> Option<uuid::Uuid> {
    let parsed = uuid::Uuid::parse_str(raw).ok()?;
    (parsed.get_variant() == uuid::Variant::RFC4122 && parsed.get_version_num() == 4)
        .then_some(parsed)
}

fn strip_line_separator(field: &str) -> String {
    field.replace(SPELEODB_LINE_FIELD_SEPARATOR, " ")
}

/// Bend the exported document into the shape SpeleoDB's converter can read.
///
/// Every rule here is one that costs the map (or the whole file) when broken,
/// and none of them is Ariane's: the user-facing export keeps the values as
/// measured, and this runs on the throwaway sidecar only. What it does NOT do is
/// invent survey data - no anchor where there is no datum, no shot dropped, no
/// tape shortened.
fn harden_for_speleodb(shots: &mut Vec<ExportShot>) {
    let mut seen_uuids: HashSet<uuid::Uuid> = HashSet::new();
    let mut ids: HashSet<i64> = HashSet::new();
    let mut duplicate_ids = 0usize;

    for shot in shots.iter_mut() {
        if !ids.insert(shot.id) {
            duplicate_ids += 1;
        }
        // Only REAL shots get normalised server-side; everything else is used
        // raw, and an azimuth outside [0, 360) then walks the shot somewhere the
        // survey never went.
        shot.azimuth = normalize_azimuth(shot.azimuth);
        shot.line = strip_line_separator(&shot.line);
        shot.explorer = shot.explorer.as_deref().map(strip_line_separator);
        shot.date = Some(speleodb_date(shot.date.as_deref()));
        // A repeat aliases the two shots onto one GeoJSON feature id. The model
        // keeps shot UUIDs unique across the project (see `export_uuid`), so
        // this is a safety net that a valid project never trips; omitting the
        // optional element costs nothing downstream.
        //
        // The CANONICAL spelling is written back, not the one that came in.
        // `Uuid::parse_str` accepts `{braced}`, `urn:uuid:…` and hyphenless forms, so
        // the gate below passes strings that are perfectly good v4s and nothing like
        // the `8-4-4-4-12` the server's field expects. Emitting the input verbatim
        // would fail the whole file on a spelling the check had just approved, which
        // is the exact class of failure this function exists to prevent.
        let canonical = shot
            .uuid
            .as_deref()
            .and_then(speleodb_uuid)
            .filter(|parsed| seen_uuids.insert(*parsed))
            .map(|parsed| parsed.hyphenated().to_string());
        shot.uuid = canonical;
    }

    // A row's id is the Station ID of the station it ends at (a START's, the
    // station it stands on), and CLOSURE rows and GPS-fix STARTs are numbered
    // above every station. In a valid project each line is walked once, from
    // the one station it shares with the lines walked before it (the forest
    // invariant), so there is one row per station and this cannot fire. It is
    // here because a duplicate `<ID>` fails the whole file rather than one shot,
    // and the failure would surface as a missing map days later.
    debug_assert_eq!(duplicate_ids, 0, "exported Ariane ids must be unique");
    if duplicate_ids > 0 {
        tracing::error!(
            shots = duplicate_ids,
            "SpeleoDB sidecar: duplicate Ariane ids; the server will reject the whole file"
        );
    }

    // Exactly one `<SurveyData>` element crashes SpeleoDB's XML decoder: a lone
    // element parses as a dict rather than a list, and iterating it yields the
    // field NAMES. Zero elements is handled cleanly there, two or more is the
    // normal path - so the only fix needed is to never emit one. The padding
    // shot is a VIRTUAL root, which the converter drops from its output and
    // ignores as an orphan, so it changes nothing but the element count.
    if shots.len() == 1 {
        let pad_id = shots.iter().map(|s| s.id).max().unwrap_or(-1) + 1;
        let anchor = &shots[0];
        let pad = ExportShot {
            id: pad_id,
            uuid: None,
            from_id: -1,
            shot_type: "VIRTUAL",
            line: anchor.line.clone(),
            azimuth: 0.0,
            length: 0.0,
            depth: anchor.depth,
            depth_in: anchor.depth,
            depth_in_same_as_prev: false,
            inclination: 0.0,
            left: 0.0,
            right: 0.0,
            up: 0.0,
            down: 0.0,
            comment: None,
            explorer: None,
            date: Some(DEFAULT_EXPORT_DATE.to_owned()),
            color: anchor.color.clone(),
            latitude: 0.0,
            longitude: 0.0,
            closure_to_id: -1,
            locked: false,
        };
        shots.push(pad);
    }

    if shots.len() > SPELEODB_SHOT_COUNT_WARN {
        tracing::warn!(
            shots = shots.len(),
            limit = SPELEODB_SHOT_COUNT_WARN,
            "SpeleoDB may fail to generate the map preview for very large surveys; \
             the survey itself is pushed in full"
        );
    }
}

/// Compute azimuth for the generated pre-zero-datum reverse segment.
///
/// Ariane does not provide a backsight field, so `azimuth_out` from Stalagmight
/// must not be interpreted as an Ariane backsight during export.
fn reverse_pre_zero_azimuth(azimuth_in: f64) -> f64 {
    normalize_azimuth(azimuth_in + 180.0)
}

/// Convert `#RRGGBB` to Ariane's `0xRRGGBBff` format.
fn color_to_ariane(hex: &str) -> String {
    let hex = hex.trim_start_matches('#');
    if hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        format!("0x{hex}ff")
    } else {
        "0x4ecdc4ff".to_string()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::float_cmp)]
#[path = "tml_entrance_tests.rs"]
mod entrance_tests;

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::panic,
    clippy::approx_constant,
    reason = "test module: panics on a failed assumption are the point; 3.14159 is a format sample"
)]
#[path = "tml_tests.rs"]
mod tests;
