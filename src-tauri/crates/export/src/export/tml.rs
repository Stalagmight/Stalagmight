//! Ariane TML/TMLU export.
//!
//! `.tml` files are ZIP archives containing `Data.xml`; `.tmlu` files are plain
//! XML.  TML uses long element names (`SurveyData`, `Azimut`, …) while TMLU
//! uses short abbreviations (`SRVD`, `AZ`, …).  This module faithfully
//! reproduces the naming convention used by Ariane.
//!
//! Key Ariane format rules learned from real files:
//! - One START shot per ENTRANCE, not per file. A section merged into the survey
//!   connects via `FromID` on its first REAL/VIRTUAL shot pointing directly at the
//!   parent station, and gets no START; a section merged into nothing is a separate
//!   entrance and gets one of its own, as Ariane's own multi-entrance files do.
//!   A parentless shot that is NOT a START is an orphan root, and Ariane draws an
//!   empty map for any survey that has one alongside a closure - see
//!   `ensure_rooted_chain`.
//! - The ID element uses all-caps `<ID>` (not `<Id>`).
//! - Every shot carries `<Locked>`, `<Name>`, and `<Profiletype>` fields.

use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Write as _};
use std::path::Path;

use anyhow::{Context, Result};
use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, BytesText, Event};
use quick_xml::Writer;

use crate::models::comment_split;
use crate::models::{normalize_azimuth, AzimuthReference, Project, SectionId, Shot, StationRef};
use crate::parsers::tml::schema::{
    self, color_to_ariane, element as el, pitch_to_inclination, ElementName, Spelling,
};

use super::atomic::{write_atomic, write_atomic_bytes};
use super::tml_azimuth::AzimuthExport;
use super::tml_topology::{
    build_parent_map, choose_default_root_section_id, section_datum, station_depth,
};

mod junction_depths;
#[cfg(test)]
mod junction_depths_property_tests;
#[cfg(test)]
mod junction_depths_tests;
use junction_depths::JunctionDepths;

// ============================================================================
// Schema
// ============================================================================
//
// Element names, shot-type strings, the `DepthIn` sentinel and the inclination
// and colour conversions all come from `parsers::tml::schema`, the one table
// the importer reads with too (#576 B4). Nothing here spells an element.

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
/// recorded it.
fn reversed_leg_pitch(shot: &Shot) -> Option<f64> {
    shot.pitch_in.or(shot.pitch_out).map(|p| -p)
}

// ============================================================================
// Internal shot representation for export
// ============================================================================

#[derive(Debug)]
struct ExportShot {
    id: i64,
    /// Persistent Stalagmight shot UUID, emitted as `<UUID>`. Carried by every
    /// row that stands for a real project shot - forward legs AND the reversed /
    /// pre-zero-datum legs, each a shot's only appearance in the document (see
    /// `write_shot`). `None` only for the rows this exporter synthesizes: START,
    /// CLOSURE and the sidecar's VIRTUAL padding shot.
    uuid: Option<String>,
    from_id: i64,
    shot_type: &'static str, // one of the `schema::TYPE_*` values
    section: String,
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
    /// On the first row of a block hung off ANOTHER section's station (its
    /// `FromID` resolved from a station merge): the section the block belongs
    /// to, whose reading of the junction is its `depth_in`. A section merged
    /// part-way along starts two blocks there and is still one reading. See
    /// `junction_depths`.
    branch_of: Option<SectionId>,
    /// The shot walked against the way it was surveyed (`ExportShot::reversed`),
    /// so a block it starts leaves its station with the depth the section
    /// ARRIVED there with.
    reversed: bool,
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

/// What every row one section emits shares: its trip (surveyor and date), its
/// colour, and the north reference its bearings are written in (#438).
struct RowContext {
    explorer: Option<String>,
    date: Option<String>,
    color: String,
    emit_reference: AzimuthReference,
}

/// The Ariane `<Type>` of a project shot: one virtual category (#27 q9).
fn ariane_shot_type(shot: &Shot) -> &'static str {
    if shot.kind.is_virtual() {
        schema::TYPE_VIRTUAL
    } else {
        schema::TYPE_REAL
    }
}

/// The one place each kind of row is built (#576 B27(a)). The shot loops used
/// to spell all 23 fields out ten times over, restating the reversed-leg rules
/// in each of the three reversing blocks; one missed copy of exactly that shape
/// once cost 78 shots their identity.
impl ExportShot {
    /// A row with no identity and every measurement at Ariane's zero - the base
    /// of the rows this exporter synthesizes.
    fn synthesized(
        id: i64,
        from_id: i64,
        shot_type: &'static str,
        section: String,
        color: String,
        (depth_in, depth): (f64, f64),
    ) -> Self {
        Self {
            id,
            uuid: None,
            from_id,
            shot_type,
            section,
            azimuth: 0.0,
            length: 0.0,
            depth,
            depth_in,
            depth_in_same_as_prev: false,
            branch_of: None,
            reversed: false,
            inclination: 0.0,
            left: 0.0,
            right: 0.0,
            up: 0.0,
            down: 0.0,
            comment: None,
            explorer: None,
            date: None,
            color,
            latitude: 0.0,
            longitude: 0.0,
            closure_to_id: -1,
            locked: false,
        }
    }

    /// The START row of an entrance whose station stands at `depth`. Its GPS
    /// is stamped later, by the pass that resolves each entrance's datum.
    fn start(id: i64, section: &str, depth: f64, rows: &RowContext) -> Self {
        Self {
            comment: Some("START".to_string()),
            explorer: rows.explorer.clone(),
            date: rows.date.clone(),
            locked: true,
            ..Self::synthesized(
                id,
                -1,
                schema::TYPE_START,
                section.to_string(),
                rows.color.clone(),
                (depth, depth),
            )
        }
    }

    /// `shot` walked the way it was surveyed. `prev_depth` is where the chain
    /// it hangs off ends, which decides whether `DepthIn` can be the
    /// same-as-previous sentinel.
    fn forward(
        id: i64,
        from_id: i64,
        section: &str,
        shot: &Shot,
        prev_depth: f64,
        rows: &RowContext,
    ) -> Self {
        Self {
            id,
            uuid: Some(shot_export_uuid(shot)),
            from_id,
            shot_type: ariane_shot_type(shot),
            section: section.to_string(),
            azimuth: rows.emit_reference.to_true(shot.average_azimuth_measured()),
            length: shot.length,
            depth: shot.depth_out,
            depth_in: shot.depth_in,
            depth_in_same_as_prev: (shot.depth_in - prev_depth).abs() < 1e-9,
            branch_of: None,
            reversed: false,
            inclination: pitch_to_inclination(shot.pitch_in),
            left: shot.left.unwrap_or(0.0),
            right: shot.right.unwrap_or(0.0),
            up: shot.up.unwrap_or(0.0),
            down: shot.down.unwrap_or(0.0),
            comment: shot.comment.clone(),
            explorer: rows.explorer.clone(),
            date: rows.date.clone(),
            color: rows.color.clone(),
            latitude: 0.0,
            longitude: 0.0,
            closure_to_id: -1,
            locked: false,
        }
    }

    /// `shot` walked backwards - the reversed and pre-zero-datum legs Ariane
    /// needs because it carries one forward azimuth per shot. The rules of
    /// turning round, stated once: the bearing turns through 180 degrees, the
    /// pitch negates, the two depths swap and so do left and right (up and down
    /// do not - turning round does not turn you over). It keeps the shot's
    /// identity: a reversed leg is that shot's only appearance in the document.
    fn reversed(
        id: i64,
        from_id: i64,
        section: &str,
        shot: &Shot,
        prev_depth: f64,
        rows: &RowContext,
    ) -> Self {
        let forward = Self::forward(id, from_id, section, shot, prev_depth, rows);
        Self {
            azimuth: reverse_pre_zero_azimuth(forward.azimuth),
            depth: shot.depth_in,
            depth_in: shot.depth_out,
            depth_in_same_as_prev: (shot.depth_out - prev_depth).abs() < 1e-9,
            reversed: true,
            inclination: pitch_to_inclination(reversed_leg_pitch(shot)),
            left: forward.right,
            right: forward.left,
            ..forward
        }
    }

    /// A CLOSURE row tying Ariane node `from_id` to `to_id`, whose emitted
    /// depths are `depth_a` and `depth_b`. The two ends are the same physical
    /// point surveyed twice, so the connector's length IS the vertical part of
    /// the closure error: exactly that keeps `|depth - origin_depth| <= length`
    /// true by construction, and stays honest - a perfectly closed loop still
    /// gets 0.
    fn closure(
        id: i64,
        (from_id, to_id): (i64, i64),
        section: String,
        color: String,
        (depth_a, depth_b): (f64, f64),
    ) -> Self {
        Self {
            length: (depth_b - depth_a).abs(),
            closure_to_id: to_id,
            ..Self::synthesized(
                id,
                from_id,
                schema::TYPE_CLOSURE,
                section,
                color,
                (depth_a, depth_b),
            )
        }
    }

    /// The sidecar's padding row: a parentless VIRTUAL standing on `anchor`,
    /// so it can displace nothing (see `harden_for_speleodb`).
    fn padding(id: i64, anchor: &ExportShot) -> Self {
        Self {
            date: Some(schema::UNDATED_PLACEHOLDER.to_owned()),
            ..Self::synthesized(
                id,
                -1,
                schema::TYPE_VIRTUAL,
                anchor.section.clone(),
                anchor.color.clone(),
                (anchor.depth, anchor.depth),
            )
        }
    }
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
    /// The user-facing "Export to Ariane" output. Its bytes change only
    /// deliberately: `tml_golden_tests` pins the whole document, so a change
    /// lands together with its golden.
    User,
    /// The throwaway `ariane.tml` pushed alongside a SpeleoDB commit, which the
    /// server converts to GeoJSON for the map preview.
    SpeleoDbSidecar,
}

// ============================================================================
// Public API
// ============================================================================

/// Export a project to Ariane TML (`.tml` = ZIP) or TMLU (`.tmlu` = plain XML).
/// Returns the notes the user has to be told about THIS document - the azimuth
/// compromises (#438), GPS anchors no START could carry, and text characters
/// XML could not carry (#576 B11) - all empty for an ordinary project.
/// Notes, not warnings: nothing failed, but something about the file is not
/// what the reader would assume, and the export command is the only moment the
/// user is looking.
pub fn export_tml(project: &Project, path: &Path) -> Result<Vec<String>> {
    export_with_profile(project, path, TmlProfile::User)
}

/// Render the SpeleoDB co-push sidecar (#196) - the same document, hardened for
/// `openspeleo_lib`'s GeoJSON conversion. See `TmlProfile` and
/// `harden_for_speleodb` for what differs and why; the survey itself does not.
pub fn export_tml_for_speleodb(project: &Project, path: &Path) -> Result<()> {
    // The sidecar has no user watching it; its notes go to the log with the
    // rest of the push.
    export_with_profile(project, path, TmlProfile::SpeleoDbSidecar).map(|_| ())
}

fn export_with_profile(project: &Project, path: &Path, profile: TmlProfile) -> Result<Vec<String>> {
    let (xml, notes) = build_xml(project, spelling_for_path(path), profile)?;
    write_output(path, &xml)?;
    Ok(notes)
}

fn spelling_for_path(path: &Path) -> Spelling {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
        .unwrap_or_default();
    if ext == "tml" {
        Spelling::Long
    } else {
        Spelling::Short
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
        "tmlu" => write_atomic_bytes(path, xml).context("Failed to write TMLU file"),
        _ => write_atomic_bytes(path, xml).context("Failed to write TML export file"),
    }
}

// ============================================================================
// XML generation
// ============================================================================

fn build_xml(
    project: &Project,
    sp: Spelling,
    profile: TmlProfile,
) -> Result<(Vec<u8>, Vec<String>)> {
    let azimuth_export = AzimuthExport::decide(project);
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
    let (shots, unwritten_datums, junctions) =
        build_export_shots_with_report(project, profile, azimuth_export);
    // Same contract as the azimuth notes above (#438): a fact the FILE carries
    // and the project does not must reach the user, not just the log. The
    // detailed warn lives in the GPS pass; this is its UI-facing code.
    if unwritten_datums > 0 {
        notes.push("gps-anchors-not-written".to_string());
    }
    // [#576 TML-DEPTH-D2] Junctions exported at the mean of their readings.
    notes.extend(junctions.note());
    ensure_rooted_chain(&shots, profile)?;

    let mut buf = Vec::new();
    let mut out = TmlWriter {
        xml: Writer::new_with_indent(Cursor::new(&mut buf), b' ', 4),
        sp,
        replaced_chars: 0,
    };

    // <?xml version="1.0" encoding="UTF-8" standalone="yes"?>
    out.xml.write_event(Event::Decl(BytesDecl::new(
        "1.0",
        Some("UTF-8"),
        Some("yes"),
    )))?;

    out.start(el::cave_file::NAME)?;

    // Metadata elements
    out.field(el::cave_name::NAME, &project.name)?;
    out.field(
        el::first_start_absolute_elevation::NAME,
        &format_f64(project.surface_elevation.unwrap_or(0.0)),
    )?;

    out.start(el::data::NAME)?;
    for shot in &shots {
        write_shot(&mut out, shot)?;
    }
    out.end(el::data::NAME)?;

    // Remaining metadata after Data (matching Ariane's ordering)
    out.field(el::unit::NAME, "m")?;
    out.field(
        el::use_magnetic_azimuth::NAME,
        if azimuth_export.is_magnetic() {
            "true"
        } else {
            "false"
        },
    )?;

    out.end(el::cave_file::NAME)?;

    // A character XML cannot carry was replaced rather than written (#576
    // B11): the file opens, but its text is not exactly the project's, so the
    // user hears about it like any other fact the file carries and the project
    // does not.
    if out.replaced_chars > 0 {
        tracing::warn!(
            replaced = out.replaced_chars,
            note = XML_CHARS_REPLACED_NOTE,
            "Ariane export: replaced characters XML 1.0 cannot carry with U+FFFD"
        );
        notes.push(XML_CHARS_REPLACED_NOTE.to_string());
    }

    drop(out);
    Ok((buf, notes))
}

/// The export note for text that lost characters XML cannot carry (#576 B11).
const XML_CHARS_REPLACED_NOTE: &str = "xml-chars-replaced";

/// The document being written: the XML writer, the spelling its element names
/// take, and how many characters had to be replaced on the way in.
///
/// Every element's text goes through [`TmlWriter::text_element`], so there is
/// exactly one place where project text meets the file - which is where the
/// XML 1.0 gate lives (#576 B11).
struct TmlWriter<W: std::io::Write> {
    xml: Writer<W>,
    sp: Spelling,
    replaced_chars: usize,
}

impl<W: std::io::Write> TmlWriter<W> {
    fn start(&mut self, element: ElementName) -> Result<()> {
        let tag = element.spelled(self.sp);
        self.xml.write_event(Event::Start(BytesStart::new(tag)))?;
        Ok(())
    }

    fn end(&mut self, element: ElementName) -> Result<()> {
        let tag = element.spelled(self.sp);
        self.xml.write_event(Event::End(BytesEnd::new(tag)))?;
        Ok(())
    }

    /// `<tag>text</tag>`, with every character XML 1.0 cannot carry replaced
    /// by U+FFFD (`BytesText::new` escapes only `<>&"'`; anything else would be
    /// written raw and make the document unreadable) and counted.
    fn text_element(&mut self, tag: &str, text: &str) -> Result<()> {
        let (safe, replaced) = schema::xml_text(text);
        self.replaced_chars += replaced;
        self.xml.write_event(Event::Start(BytesStart::new(tag)))?;
        self.xml.write_event(Event::Text(BytesText::new(&safe)))?;
        self.xml.write_event(Event::End(BytesEnd::new(tag)))?;
        Ok(())
    }

    /// `<!--text-->`. The text is ours and fixed, never project data.
    fn comment(&mut self, text: &'static str) -> Result<()> {
        self.xml
            .write_event(Event::Comment(BytesText::from_escaped(text)))?;
        Ok(())
    }

    /// [`Self::text_element`] for a schema element, in the document's spelling.
    fn field(&mut self, element: ElementName, text: &str) -> Result<()> {
        self.text_element(element.spelled(self.sp), text)
    }
}

fn write_shot<W: std::io::Write>(out: &mut TmlWriter<W>, shot: &ExportShot) -> Result<()> {
    out.start(el::survey_data::NAME)?;

    // Write fields in alphabetical order (matching Ariane's output)
    out.field(el::azimuth::NAME, &format_f64(shot.azimuth))?;
    out.field(el::closure_to_id::NAME, &shot.closure_to_id.to_string())?;
    out.field(el::color::NAME, &shot.color)?;
    out.field(el::comment::NAME, shot.comment.as_deref().unwrap_or(""))?;
    if let Some(date) = shot.date.as_deref() {
        out.field(el::date::NAME, date)?;
    } else {
        // No survey date. The placeholder keeps the file readable everywhere;
        // our reader takes it as "no date" (#576 B2), so a re-import neither
        // shows 1 January 2000 nor derives a declination for it, and the
        // marker says the same to a person reading the file. Only ever reached
        // in the user profile: the sidecar has already replaced every date by
        // `speleodb_date`.
        out.field(el::date::NAME, schema::UNDATED_PLACEHOLDER)?;
        out.comment(schema::UNDATED_MARKER)?;
    }
    out.field(el::depth::NAME, &format_f64(shot.depth))?;
    out.field(el::depth_in::NAME, &format_f64(emitted_depth_in(shot)))?;
    out.field(el::down::NAME, &format_f64(shot.down))?;
    out.field(el::excluded::NAME, "false")?;
    let explorer = shot.explorer.as_deref();
    out.field(
        el::explorer::NAME,
        explorer.unwrap_or(schema::EXPLORER_PLACEHOLDER),
    )?;
    out.field(el::from_id::NAME, &shot.from_id.to_string())?;
    out.field(el::id::NAME, &shot.id.to_string())?;
    out.field(el::inclination::NAME, &format_f64(shot.inclination))?;
    out.field(el::latitude::NAME, &format_f64(shot.latitude))?;
    out.field(el::left::NAME, &format_f64(shot.left))?;
    out.field(el::length::NAME, &format_f64(shot.length))?;
    out.field(el::locked::NAME, if shot.locked { "true" } else { "false" })?;
    out.field(el::longitude::NAME, &format_f64(shot.longitude))?;

    // Ariane conventions: START has START name, CLOSURE uses CLOSURE:CLOSURE.
    let name_str = match shot.shot_type {
        schema::TYPE_START => schema::NAME_START,
        schema::TYPE_CLOSURE => schema::NAME_CLOSURE,
        _ => "",
    };
    out.field(el::name::NAME, name_str)?;

    out.field(el::profiletype::NAME, schema::PROFILE_TYPE_VERTICAL)?;
    out.field(el::right::NAME, &format_f64(shot.right))?;
    out.field(el::section::NAME, &shot.section)?;
    out.field(el::shot_type::NAME, shot.shot_type)?;
    out.field(el::up::NAME, &format_f64(shot.up))?;

    // Stable per-shot UUID. Emitted for every block that stands for a REAL
    // PROJECT SHOT - the forward blocks and the reversed "(pre-zero-datum)" /
    // merge legs alike - and omitted only for the blocks this exporter
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
    // `split..len`) or replaces the forward block entirely (the whole-section
    // reversal). So no shot can appear twice, and `remint_duplicate_uuids`
    // cannot see a forward/reverse pair as a duplicate - pinned by
    // `a_reversed_leg_is_a_shots_only_appearance_so_nothing_is_reminted`.
    if let Some(uuid) = &shot.uuid {
        out.field(el::uuid::NAME, uuid)?;
    }

    out.end(el::survey_data::NAME)
}

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
        return schema::DEPTH_IN_SAME_AS_PREVIOUS;
    }
    // `round_as_written` lands on a multiple of 1e-6, so the nearest value that is
    // NOT the sentinel is a full micrometre away - an epsilon compare here is an
    // exact-equality test on the written value, without tripping `float_cmp`.
    if (round_as_written(shot.depth_in) - schema::DEPTH_IN_SAME_AS_PREVIOUS).abs() < f64::EPSILON {
        return schema::DEPTH_IN_SAME_AS_PREVIOUS - 1e-6;
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

    let orphans: Vec<&ExportShot> = shots
        .iter()
        .filter(|s| s.from_id == -1 && s.shot_type != schema::TYPE_START)
        .collect();

    let Some(first) = orphans.first() else {
        return Ok(());
    };

    let mut sections: Vec<&str> = orphans.iter().map(|s| s.section.as_str()).collect();
    sections.sort_unstable();
    sections.dedup();
    // Section names come from imported third-party files, so neither their count
    // nor their length is ours to trust - and this string reaches both the log
    // and the UI (#378 item g). Cap the list and truncate each name.
    let listed = list_capped(&sections);

    // Without a closure Ariane never runs the loop solve, so the orphan is inert and
    // the file draws. Say so once and let the export through.
    if !shots.iter().any(|s| s.shot_type == schema::TYPE_CLOSURE) {
        tracing::warn!(
            orphan_count = orphans.len(),
            sections = listed,
            "Exported shots have no parent station; Ariane will draw them as separate \
             entrances. Adding a loop closure to this survey would blank its map."
        );
        return Ok(());
    }

    anyhow::bail!(
        "Cannot export {listed}: {} shot(s) there have no parent station, and this survey \
         also has loop closures - Ariane draws an empty map for that combination. Connect \
         the section to the rest of the survey, or remove the closures. \
         (First unrooted shot: id {}, type {}.)",
        orphans.len(),
        first.id,
        first.shot_type,
    )
}

/// How many section names an error/warning lists before summarising, and how
/// long each shown name may be. Imported files decide both, so the message must
/// not scale with them.
const LISTED_SECTIONS_MAX: usize = 5;
const LISTED_SECTION_NAME_MAX: usize = 60;

fn list_capped(names: &[&str]) -> String {
    let mut shown: Vec<String> = names
        .iter()
        .take(LISTED_SECTIONS_MAX)
        .map(|name| {
            // Control characters would let an imported name forge log lines or
            // steer the terminal; the length caps alone don't stop that.
            let clean: String = name
                .chars()
                .filter(|c| !c.is_control())
                .take(LISTED_SECTION_NAME_MAX + 1)
                .collect();
            if clean.chars().count() > LISTED_SECTION_NAME_MAX {
                let cut: String = clean.chars().take(LISTED_SECTION_NAME_MAX).collect();
                format!("{cut}...")
            } else {
                clean
            }
        })
        .collect();
    if names.len() > LISTED_SECTIONS_MAX {
        shown.push(format!("and {} more", names.len() - LISTED_SECTIONS_MAX));
    }
    shown.join(", ")
}

// ============================================================================
// Build flat shot list from project model
// ============================================================================

/// The UUID emitted as `<UUID>` for a real forward shot. `Project::ensure_shot_uuids`
/// settles these before every save and before the SpeleoDB render, so the mint is a
/// fallback for an export of a project that was never saved - without it the same
/// project would render to different bytes on each export.
///
/// What this value guarantees is STABILITY across re-exports, not uniqueness within
/// the file. Shot-UUID uniqueness in this model is PER-SECTION only, deliberately:
/// `Project::ensure_unique_shot_uuids` resets its dedup set inside the section loop,
/// and the companion `duplicate_shot_uuid` integrity check is section-scoped for the
/// same reason - `resolve_key` is section-local, so a cross-section repeat cannot
/// mis-resolve an anchor and is not a violation. Document-level uniqueness is an
/// EXPORT-boundary guarantee instead: `remint_duplicate_uuids` (user profile) and
/// `harden_for_speleodb` (sidecar) each settle repeats their own way after the
/// full shot list is assembled.
/// Never returns an empty string: an empty stored UUID is treated as "no identity
/// yet" and gets the same fresh mint as `None`, because an empty `<UUID>` element
/// is worse than an ephemeral one (it reads as a real, shared key).
fn shot_export_uuid(shot: &Shot) -> String {
    match shot.uuid.as_deref() {
        Some(u) if !u.is_empty() => u.to_string(),
        _ => uuid::Uuid::new_v4().to_string(),
    }
}

/// Document-level `<UUID>` uniqueness for the user-facing export.
///
/// Per-section uniqueness (all the model enforces - see `shot_export_uuid`) still
/// lets one document carry the same `<UUID>` twice: import the same `.tml` into a
/// project twice, or merge two projects that came from one file, and every shot's
/// foreign identity repeats across the two copies. How Ariane treats a repeated
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
/// project holding a cross-section duplicate does not re-export byte-identical.
/// Accepted: the sidecar profile has no such repeats (it drops them), and the
/// user-facing byte-stability claim (`shot_export_uuid`) was always conditional
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

/// Returns the shots plus how many of the project's GPS anchors ended up on no
/// START (the count behind the `gps-anchors-not-written` note).
fn build_export_shots_with_report(
    project: &Project,
    profile: TmlProfile,
    azimuth_export: AzimuthExport,
) -> (Vec<ExportShot>, usize, JunctionDepths) {
    if project.sections.is_empty() {
        return (Vec::new(), 0, JunctionDepths::default());
    }

    // Pick the section the topology hangs off. A survey can emit more than one START
    // (one per entrance - see the module docs), but only this one is the graph root:
    // when a primary zero datum exists, rooting here lets its GPS land on a START
    // without remapping.
    //
    // The FRAME datum, not `zero_datums[0]`: local (0, 0) means the first
    // RESOLVABLE datum's GPS (`Project::frame_datum`), so stamping the positional
    // first one onto every START would georeference an export whose geometry is
    // laid out against a different anchor - every station displaced by the whole
    // inter-datum vector, silently, in the file the user hands to someone else.
    let frame_datum = project.frame_datum();
    let root_section_id = frame_datum
        .map(|d| d.station_ref.section_id)
        .filter(|sid| project.sections.iter().any(|s| s.id == *sid))
        .unwrap_or_else(|| choose_default_root_section_id(project));

    // Build rooted parent links from station merges.
    let parent_map = build_parent_map(project, root_section_id);
    let primary_datum = frame_datum;

    let mut next_id: i64 = 0;

    struct SectionBlock {
        section_id: SectionId,
        apply_parent_link: bool,
        merge_station_index_override: Option<u32>,
        shots: Vec<ExportShot>,
    }

    let mut blocks: Vec<SectionBlock> = Vec::new();
    // Map: (section_id, station_index) → ariane_id
    let mut station_to_ariane: HashMap<(SectionId, u32), i64> = HashMap::new();
    // Every START and the station it stands on: (ariane id, section, station index).
    // The GPS pass below resolves each entrance's own datum through this, instead
    // of stamping one datum onto every START (#378 item e).
    let mut start_anchors: Vec<(i64, SectionId, u32)> = Vec::new();

    for section in &project.sections {
        let is_root = section.id == root_section_id;
        // #438: identity unless the project's sections disagree about north, in
        // which case each section's own correction is applied here so the whole
        // document is true north - the only thing one file-level flag can say.
        let emit_reference = azimuth_export.emit_reference(project, section);

        // A section that never merges into the tree is a separate entrance, not a
        // child of anything - two unconnected galleries in one project is an ordinary
        // state. Ariane represents those as additional START shots, and files Ariane
        // writes for a survey with two entrances carry two. Leaving the first shot
        // parentless instead produces an orphan root, which makes Ariane abandon the
        // loop solve and draw an empty map for the WHOLE survey once any closure
        // exists. Zero-datum splitting below applies to every entrance (#378 item
        // e), so a START coincides with its own datum station wherever one exists.
        //
        // Deliberately NOT profile-gated (#378 item c): the SpeleoDB sidecar carries
        // the same extra STARTs, and the server side supports that. Verified in
        // OpenSpeleo/pytool_openspeleo_lib @ 4d5e9e106365 (2026-07-17),
        // openspeleo_lib/geojson.py: `find_valid_shot_ids` and
        // `propagate_coordinates` BFS from ALL anchor shots ("Found %d anchor
        // shots") - with zero anchors the former warns and drops every shot while
        // the latter raises - and the Ariane decoder never counts STARTs. One trap encoded there: `is_geolocation_known()` treats
        // |lat| or |lon| within ~1e-6 of zero as UNKNOWN, so a START left at 0,0
        // is not an anchor and its whole component is dropped as orphans - which
        // is why every START must be GPS-stamped below whenever a datum exists.
        // Guarded by `the_sidecar_accepts_multiple_starts`.
        let is_entrance = is_root || !parent_map.contains_key(&section.id);

        let section_name = section
            .name
            .as_deref()
            .unwrap_or(&format!("Section {}", section.id))
            .to_string();
        let rows = RowContext {
            explorer: section.surveyor.clone(),
            date: section.date.map(|d| d.format("%Y-%m-%d").to_string()),
            color: color_to_ariane(&section.color),
            emit_reference,
        };

        // Which datum this entrance's START sits on. The root uses the FRAME datum
        // (see `root_section_id` above); any other entrance uses its own section's
        // first resolvable datum (#378 item e). GPS lands only on START shots, so
        // an entrance whose datum is mid-section must SPLIT there - stamping that
        // datum's GPS onto a station-0 START would displace the whole component by
        // the station-to-datum vector, silently.
        let entrance_datum = if is_root {
            primary_datum.filter(|datum| datum.station_ref.section_id == section.id)
        } else if is_entrance {
            section_datum(project, section.id)
        } else {
            None
        };

        let split_entrance_at = if is_entrance {
            entrance_datum
                .and_then(|datum| project.resolve_key(&datum.station_ref))
                .and_then(|sr| {
                    let idx = sr.station_index as usize;
                    if idx > section.shots.len() {
                        tracing::warn!(
                            section_id = section.id,
                            station_index = sr.station_index,
                            shot_count = section.shots.len(),
                            "Zero datum station is out of bounds; exporting section without split"
                        );
                        None
                    } else if idx > 0 {
                        Some(idx)
                    } else {
                        None
                    }
                })
        } else {
            None
        };

        if let Some(split_idx) = split_entrance_at {
            let start_id = next_id;
            next_id += 1;
            let start_depth = station_depth(section, split_idx);
            start_anchors.push((start_id, section.id, split_idx as u32));

            let mut root_shots = vec![ExportShot::start(
                start_id,
                &section_name,
                start_depth,
                &rows,
            )];
            station_to_ariane.insert((section.id, split_idx as u32), start_id);

            // Forward segment: datum station -> original section end.
            let mut cumulative_depth = start_depth;
            for shot_idx in split_idx..section.shots.len() {
                let shot = &section.shots[shot_idx];
                let shot_ariane_id = next_id;
                next_id += 1;

                let prev_id = if shot_idx == split_idx {
                    start_id
                } else {
                    shot_ariane_id - 1
                };
                let leg = ExportShot::forward(
                    shot_ariane_id,
                    prev_id,
                    &section_name,
                    shot,
                    cumulative_depth,
                    &rows,
                );
                cumulative_depth = leg.depth;
                root_shots.push(leg);

                station_to_ariane.insert((section.id, (shot_idx + 1) as u32), shot_ariane_id);
            }

            blocks.push(SectionBlock {
                section_id: section.id,
                apply_parent_link: false,
                merge_station_index_override: None,
                shots: root_shots,
            });

            // Reverse segment: datum station -> original section start.
            if split_idx > 0 {
                let split_name = format!("{section_name} (pre-zero-datum)");
                let mut reverse_shots = Vec::new();
                let mut cumulative_depth = start_depth;

                for shot_idx in (0..split_idx).rev() {
                    let shot = &section.shots[shot_idx];
                    let shot_ariane_id = next_id;
                    next_id += 1;

                    let prev_id = if shot_idx == split_idx - 1 {
                        start_id
                    } else {
                        shot_ariane_id - 1
                    };
                    let leg = ExportShot::reversed(
                        shot_ariane_id,
                        prev_id,
                        &split_name,
                        shot,
                        cumulative_depth,
                        &rows,
                    );
                    cumulative_depth = leg.depth;
                    reverse_shots.push(leg);

                    // Reversed shot from station (shot_idx + 1) to shot_idx:
                    // it ends at the original station shot_idx.
                    station_to_ariane.insert((section.id, shot_idx as u32), shot_ariane_id);
                }

                blocks.push(SectionBlock {
                    section_id: section.id,
                    apply_parent_link: false,
                    merge_station_index_override: None,
                    shots: reverse_shots,
                });
            }

            continue;
        }

        // Non-root child sections merged at a non-zero station need splitting
        // or reversal so the merge point becomes the first shot in each block.
        if !is_root {
            if let Some(parent_link) = parent_map.get(&section.id) {
                let child_merge_idx = parent_link.child_station_index as usize;

                if child_merge_idx > 0 && child_merge_idx < section.shots.len() {
                    // Mid-section merge: split into forward + reverse blocks.

                    // Forward block: shots from child_merge_idx to end.
                    let mut fwd_shots = Vec::new();
                    let mut cumulative_depth = station_depth(section, child_merge_idx);
                    for shot_idx in child_merge_idx..section.shots.len() {
                        let shot = &section.shots[shot_idx];
                        let shot_ariane_id = next_id;
                        next_id += 1;

                        let prev_id = if shot_idx == child_merge_idx {
                            -1 // placeholder - resolved in second pass
                        } else {
                            shot_ariane_id - 1
                        };
                        let leg = ExportShot::forward(
                            shot_ariane_id,
                            prev_id,
                            &section_name,
                            shot,
                            cumulative_depth,
                            &rows,
                        );
                        cumulative_depth = leg.depth;
                        fwd_shots.push(leg);

                        station_to_ariane
                            .insert((section.id, (shot_idx + 1) as u32), shot_ariane_id);
                    }

                    blocks.push(SectionBlock {
                        section_id: section.id,
                        apply_parent_link: true,
                        merge_station_index_override: Some(0),
                        shots: fwd_shots,
                    });

                    // Reverse block: shots from child_merge_idx-1 down to 0, reversed.
                    let rev_name = format!("{section_name} (reversed)");
                    let mut rev_shots = Vec::new();
                    let mut cumulative_depth = station_depth(section, child_merge_idx);

                    for shot_idx in (0..child_merge_idx).rev() {
                        let shot = &section.shots[shot_idx];
                        let shot_ariane_id = next_id;
                        next_id += 1;

                        let prev_id = if shot_idx == child_merge_idx - 1 {
                            -1 // placeholder - resolved in second pass
                        } else {
                            shot_ariane_id - 1
                        };
                        let leg = ExportShot::reversed(
                            shot_ariane_id,
                            prev_id,
                            &rev_name,
                            shot,
                            cumulative_depth,
                            &rows,
                        );
                        cumulative_depth = leg.depth;
                        rev_shots.push(leg);

                        // Reversed shot ends at original station shot_idx.
                        station_to_ariane.insert((section.id, shot_idx as u32), shot_ariane_id);
                    }

                    blocks.push(SectionBlock {
                        section_id: section.id,
                        apply_parent_link: true,
                        merge_station_index_override: Some(0),
                        shots: rev_shots,
                    });

                    continue;
                } else if child_merge_idx > 0 && child_merge_idx >= section.shots.len() {
                    // End-of-section merge: reverse the entire section.
                    let rev_name = format!("{section_name} (reversed)");
                    let mut rev_shots = Vec::new();
                    let mut cumulative_depth = station_depth(section, section.shots.len());

                    for shot_idx in (0..section.shots.len()).rev() {
                        let shot = &section.shots[shot_idx];
                        let shot_ariane_id = next_id;
                        next_id += 1;

                        let prev_id = if shot_idx == section.shots.len() - 1 {
                            -1 // placeholder - resolved in second pass
                        } else {
                            shot_ariane_id - 1
                        };
                        let leg = ExportShot::reversed(
                            shot_ariane_id,
                            prev_id,
                            &rev_name,
                            shot,
                            cumulative_depth,
                            &rows,
                        );
                        cumulative_depth = leg.depth;
                        rev_shots.push(leg);

                        // Reversed shot ends at original station shot_idx.
                        station_to_ariane.insert((section.id, shot_idx as u32), shot_ariane_id);
                    }

                    blocks.push(SectionBlock {
                        section_id: section.id,
                        apply_parent_link: true,
                        merge_station_index_override: Some(0),
                        shots: rev_shots,
                    });

                    continue;
                }
            }
        }

        let mut shots = Vec::new();

        if is_entrance {
            // The root, and any section not merged into it, each get a START shot
            let start_id = next_id;
            next_id += 1;
            start_anchors.push((start_id, section.id, 0));

            let start_depth = section.shots.first().map_or(0.0, |first| first.depth_in);
            shots.push(ExportShot::start(
                start_id,
                &section_name,
                start_depth,
                &rows,
            ));

            station_to_ariane.insert((section.id, 0), start_id);
        }

        let mut cumulative_depth = section.shots.first().map_or(0.0, |first| first.depth_in);

        for (i, shot) in section.shots.iter().enumerate() {
            let shot_ariane_id = next_id;
            next_id += 1;

            // Chain from_id: first shot chains from START (root) or placeholder (child)
            let prev_id = if i == 0 {
                if is_entrance {
                    // Chain from the START shot
                    shots[0].id
                } else {
                    // Placeholder - will be resolved in second pass
                    -1
                }
            } else {
                shot_ariane_id - 1
            };
            let leg = ExportShot::forward(
                shot_ariane_id,
                prev_id,
                &section_name,
                shot,
                cumulative_depth,
                &rows,
            );
            cumulative_depth = leg.depth;
            shots.push(leg);

            // Station (i+1) is after this shot
            station_to_ariane.insert((section.id, (i + 1) as u32), shot_ariane_id);
        }

        blocks.push(SectionBlock {
            section_id: section.id,
            apply_parent_link: !is_entrance && !shots.is_empty(),
            merge_station_index_override: None,
            shots,
        });
    }

    // Second pass: resolve parent links from station merges.
    //
    // A section's parent station is often another section's MERGE station, and that
    // station only enters `station_to_ariane` through the alias at the bottom of this
    // loop - that is, when the other section resolves. Walking `blocks` once therefore
    // makes resolution order-dependent even though the tree `build_parent_map` builds
    // is not: a child stored before its parent missed the lookup and kept the `-1`
    // placeholder. Ariane reads a parentless non-START shot as a second entrance, gives
    // up on the whole loop solve, and draws an empty map while the data table still
    // fills - silently, because this only warned. Iterate to a fixpoint instead, since
    // every pass that resolves something can unblock the next.
    let mut pending: Vec<usize> = (0..blocks.len())
        .filter(|&i| blocks[i].apply_parent_link && !blocks[i].shots.is_empty())
        .collect();

    while !pending.is_empty() {
        let mut unresolved = Vec::new();
        let mut progressed = false;

        for i in pending {
            let section_id = blocks[i].section_id;
            let override_idx = blocks[i].merge_station_index_override;

            // No parent link at all means a genuinely detached section, which is a
            // different shape from an unresolved one; `ensure_rooted_chain` decides.
            let Some(parent_link) = parent_map.get(&section_id) else {
                continue;
            };

            let Some(&parent_ariane_id) = station_to_ariane.get(&(
                parent_link.parent_station.section_id,
                parent_link.parent_station.station_index,
            )) else {
                unresolved.push(i);
                continue;
            };

            let child_station_idx =
                override_idx.unwrap_or(parent_link.child_station_index) as usize;
            if child_station_idx < blocks[i].shots.len() {
                blocks[i].shots[child_station_idx].from_id = parent_ariane_id;
                blocks[i].shots[child_station_idx].branch_of = Some(section_id);
            } else {
                tracing::warn!(
                    child_section_id = section_id,
                    child_station_index = parent_link.child_station_index,
                    shot_count = blocks[i].shots.len(),
                    "Skipping parent from_id rewrite: child merge station has no outgoing shot"
                );
            }

            // Treat merged stations as the same Ariane node ID so closures and GPS
            // references resolve to the parent-attached node.
            station_to_ariane.insert(
                (section_id, parent_link.child_station_index),
                parent_ariane_id,
            );
            progressed = true;
        }

        pending = unresolved;
        if !progressed {
            // Nothing moved, so nothing will: the rest are unreachable parents.
            for &i in &pending {
                let parent_link = &parent_map[&blocks[i].section_id];
                tracing::warn!(
                    child_section_id = blocks[i].section_id,
                    parent_section_id = parent_link.parent_station.section_id,
                    parent_station_index = parent_link.parent_station.station_index,
                    "Unresolved parent link: parent station is not representable in Ariane export"
                );
            }
            break;
        }
    }

    // Fold comments into the TML <Comment> field. The shot side is ZWSP-marked
    // on EVERY REAL/VIRTUAL shot, not only when a station comment rides along:
    // the importer routes unmarked lines to the station (the default), so an
    // unmarked shot comment this exporter wrote would be indistinguishable from
    // text an Ariane user typed and would come back on the wrong side (#564).
    // Station comments attach to the shot whose `to` station matches, written
    // verbatim after the marked lines. Multiple StationRefs that alias the same
    // Ariane shot (parent <-> merged child) have their comments joined by '\n'.
    // See `models::comment_split` for the marker convention.
    //
    // ariane_id -> list of (StationRef, comment), in a deterministic order
    // (sorted by section_id then station_index) so the joined output is
    // reproducible across exports.
    let mut comments_by_ariane: HashMap<i64, Vec<(StationRef, &str)>> = HashMap::new();
    let mut entries: Vec<(StationRef, &str)> = project
        .station_comments
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .filter_map(|(k, v)| match project.resolve_key(k) {
            Some(sr) => Some((sr, v.as_str())),
            None => {
                tracing::debug!(
                    key = ?k,
                    "Station comment dropped from .tml export: its station key no longer resolves"
                );
                None
            }
        })
        .collect();
    entries.sort_by_key(|(sr, _)| (sr.section_id, sr.station_index));
    for (sr, text) in entries {
        let Some(&ariane_id) = station_to_ariane.get(&(sr.section_id, sr.station_index)) else {
            tracing::debug!(
                section_id = sr.section_id,
                station_index = sr.station_index,
                "Station comment dropped from .tml export: no Ariane shot represents this station"
            );
            continue;
        };
        comments_by_ariane
            .entry(ariane_id)
            .or_default()
            .push((sr, text));
    }

    // Apply to each block's REAL/VIRTUAL shots (skip START so its
    // hardcoded "START" content stays untouched and unmarked - the importer
    // drops START shots entirely; skip CLOSURE as it has no `to` station in
    // the create-station sense).
    //
    // `remove`, not `get`: an Ariane id belongs to exactly one emitted shot, so
    // taking the entry can never steal it from another shot - and what is LEFT
    // in the map once every block has been walked is precisely the set of
    // station comments this export dropped, which the loop below reports. That
    // is the only way the START-aliased drops become visible at all: the START
    // shot's id IS the id `station_to_ariane` holds for root station 0, and for
    // the primary zero datum's station when the root is split there, so the
    // lookup that reports "no Ariane shot represents this station" above
    // succeeds and the comment is then discarded HERE, by a shot-type skip that
    // used to run before the lookup and say nothing.
    for block in &mut blocks {
        for shot in &mut block.shots {
            if shot.shot_type != schema::TYPE_REAL && shot.shot_type != schema::TYPE_VIRTUAL {
                continue;
            }
            let joined_station: Option<String> =
                comments_by_ariane.remove(&shot.id).map(|attachments| {
                    attachments
                        .iter()
                        .map(|(_, t)| *t)
                        .collect::<Vec<_>>()
                        .join("\n")
                });
            shot.comment = comment_split::merge(shot.comment.as_deref(), joined_station.as_deref());
        }
    }

    // Anything still in the map named an Ariane node that no fold-eligible shot
    // carries - a START, or (defensively) an id `station_to_ariane` holds with
    // no shot of its own. Those comments are simply absent from the file, and
    // WHICH ones is not derivable from the output, so the drop is named here -
    // deliberately at debug level only, not as an export note; see the
    // co-encoding caveats in docs/ariane-export.md. Sorted so the log reads
    // the same on every export, like the fold itself.
    let mut dropped: Vec<(StationRef, i64)> = comments_by_ariane
        .into_iter()
        .flat_map(|(ariane_id, attachments)| {
            attachments.into_iter().map(move |(sr, _)| (sr, ariane_id))
        })
        .collect();
    dropped.sort_by_key(|(sr, _)| (sr.section_id, sr.station_index));
    for (sr, ariane_id) in dropped {
        tracing::debug!(
            section_id = sr.section_id,
            station_index = sr.station_index,
            ariane_id,
            "Station comment dropped from .tml export: the Ariane shot representing this station takes no station comment (START, or no shot of its own)"
        );
    }

    // Third pass: generate CLOSURE shots from loop_closures
    // Depth of every Ariane node emitted so far, so a closure can be hung off
    // the node it actually references rather than off its endpoint's section.
    let node_depth: HashMap<i64, f64> = blocks
        .iter()
        .flat_map(|b| b.shots.iter())
        .map(|s| (s.id, s.depth))
        .collect();
    let mut closure_shots = Vec::new();
    for closure in &project.loop_closures {
        let (Some(a_ref), Some(b_ref)) = (
            project.resolve_key(&closure.station_a),
            project.resolve_key(&closure.station_b),
        ) else {
            tracing::warn!(
                closure_id = closure.id,
                "Skipping closure export: an endpoint anchor no longer resolves"
            );
            continue;
        };
        let Some(&from_id) = station_to_ariane.get(&(a_ref.section_id, a_ref.station_index)) else {
            tracing::warn!(
                closure_id = closure.id,
                from_section_id = a_ref.section_id,
                from_station_index = a_ref.station_index,
                to_section_id = b_ref.section_id,
                to_station_index = b_ref.station_index,
                "Skipping closure export: from station is not representable in Ariane topology"
            );
            continue;
        };

        let Some(&to_id) = station_to_ariane.get(&(b_ref.section_id, b_ref.station_index)) else {
            tracing::warn!(
                closure_id = closure.id,
                from_section_id = a_ref.section_id,
                from_station_index = a_ref.station_index,
                to_section_id = b_ref.section_id,
                to_station_index = b_ref.station_index,
                "Skipping closure export: target station is not representable in Ariane topology"
            );
            continue;
        };

        if from_id == to_id {
            tracing::warn!(
                closure_id = closure.id,
                section_id = a_ref.section_id,
                station_index = a_ref.station_index,
                "Skipping closure export: closure endpoints collapse to the same Ariane node"
            );
            continue;
        }

        let closure_id = next_id;
        next_id += 1;

        let section_name = project
            .sections
            .iter()
            .find(|s| s.id == closure.station_a.section_id)
            .and_then(|s| s.name.clone())
            .unwrap_or_else(|| format!("Section {}", closure.station_a.section_id));

        let color = project
            .sections
            .iter()
            .find(|s| s.id == closure.station_a.section_id)
            .map(|s| color_to_ariane(&s.color))
            .unwrap_or_else(|| schema::FALLBACK_ARIANE_COLOR.to_string());

        // The depths of the two Ariane NODES the closure ties together. A closure
        // carrying `depth: 0.0` is not merely cosmetic: a reader takes the chain as
        // geometry, so a connector sitting at depth 0 hanging off a station 13 m
        // down is a shot with a 13 m vertical drop and no length - impossible.
        // SpeleoDB's `openspeleo_lib` is not the reader that would trip over it
        // (0.0.15 drops CLOSURE shots from both `build_shot_graph` and
        // `build_shots_map`, so they never reach the `length_2d` bound check); this
        // is for Ariane itself and for anything that does walk them.
        //
        // Read from the emitted nodes, NOT from the endpoint's own section table:
        // `station_to_ariane` maps a merged child station to the PARENT's node, so
        // at a junction the child section's station depth is not the depth the
        // consumer will use as this connector's origin. Taking it from the node is
        // what makes `length` cover the gap the consumer actually measures.
        //
        // Two consequences of that are deliberately left alone:
        //
        // - The connector goes out with `Azimut` 0 and `Inclination` 0 while
        //   carrying a real length, which reads as a horizontal run due north. It
        //   still cannot displace anything. A reader that projects a shot into plan
        //   view uses the depths when it has them - horizontal =
        //   sqrt(length^2 - drop^2), which is zero here because the length IS the
        //   drop (to within the micrometre the as-written sweep may add to the
        //   tape) - and only falls back to `Inclination` when `Depth` is missing,
        //   which we never emit. The closure's own node is a leaf besides: its id
        //   is minted after every `from_id` in the file has been assigned, so
        //   nothing hangs off it and no position propagates through it.
        //
        // - `node_depth` is snapshotted BEFORE the geometry repair, which can move
        //   a node: a junction exported at the mean of its readings, the only
        //   node it moves. `junction_depths::settle` re-derives a connector
        //   touching a moved node - both depths and its length - so the file
        //   states the depths the nodes are written at.
        let depth_a = node_depth.get(&from_id).copied().unwrap_or(0.0);
        let depth_b = node_depth.get(&to_id).copied().unwrap_or(0.0);
        closure_shots.push(ExportShot::closure(
            closure_id,
            (from_id, to_id),
            section_name,
            color,
            (depth_a, depth_b),
        ));
    }

    // Flatten all blocks + closure shots
    let mut all_shots: Vec<ExportShot> = blocks.into_iter().flat_map(|b| b.shots).collect();
    all_shots.extend(closure_shots);

    // Final pass: Ariane stores GPS on the START shots.
    //
    // There can be more than one - the root, plus any section that never merges into
    // it. Each entrance carries its OWN datum's GPS when it has one (#378 item e;
    // the entrance split above put its START on that datum's station), and the
    // frame datum's otherwise - a START left at 0,0 drops its component on null
    // island thousands of kilometres from the survey, and a zoom-to-fit then shows
    // half a hemisphere.
    let mut unwritten_datums = 0usize;
    if let Some(primary_datum) = primary_datum {
        // First-that-resolves per station: a second datum on the SAME station is
        // a conflict this file shape cannot carry, so it counts as unwritten
        // below rather than silently aliasing the one that won.
        let mut datum_by_station: HashMap<(SectionId, u32), usize> = HashMap::new();
        let mut resolved = 0usize;
        for (index, datum) in project.zero_datums.iter().enumerate() {
            if let Some(sr) = project.resolve_key(&datum.station_ref) {
                resolved += 1;
                datum_by_station
                    .entry((sr.section_id, sr.station_index))
                    .or_insert(index);
            }
        }
        let start_index_by_id: HashMap<i64, usize> = all_shots
            .iter()
            .enumerate()
            .filter(|(_, s)| s.shot_type == schema::TYPE_START)
            .map(|(i, s)| (s.id, i))
            .collect();

        let mut written_datums: HashSet<usize> = HashSet::new();
        // The frame datum anchors every entrance that has no datum of its own, so
        // one fallback stamp writes its coordinates even when its own station
        // carries no START (e.g. it stopped resolving onto one).
        let frame_index = project
            .zero_datums
            .iter()
            .position(|d| std::ptr::eq(d, primary_datum));
        for (start_id, section_id, station_index) in &start_anchors {
            let own = datum_by_station
                .get(&(*section_id, *station_index))
                .copied();
            let gps = match own {
                Some(index) => {
                    written_datums.insert(index);
                    &project.zero_datums[index].gps
                }
                None => {
                    written_datums.extend(frame_index);
                    &primary_datum.gps
                }
            };
            let (latitude, longitude) = match profile {
                TmlProfile::User => (gps.latitude, gps.longitude),
                TmlProfile::SpeleoDbSidecar => speleodb_anchor(gps.latitude, gps.longitude),
            };
            if let Some(&shot_index) = start_index_by_id.get(start_id) {
                all_shots[shot_index].latitude = latitude;
                all_shots[shot_index].longitude = longitude;
            }
        }

        // More than one datum is representable now, so account per DATUM: one is
        // unwritten when no START carries its coordinates - it sits mid-way
        // through a child section, lost its station, or collided with another
        // datum on the same station.
        unwritten_datums = project.zero_datums.len() - written_datums.len();
        if unwritten_datums > 0 {
            tracing::warn!(
                datum_count = project.zero_datums.len(),
                resolved,
                unwritten = unwritten_datums,
                "Ariane stores GPS on entrance START stations only; {unwritten_datums} GPS \
                 anchor(s) were not written. Entrances without their own datum are \
                 anchored at the frame datum's coordinates."
            );
        }
    }

    let junctions = enforce_ariane_geometry_invariants(&mut all_shots, profile);

    match profile {
        TmlProfile::SpeleoDbSidecar => harden_for_speleodb(&mut all_shots),
        TmlProfile::User => remint_duplicate_uuids(&mut all_shots),
    }

    (all_shots, unwritten_datums, junctions)
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
/// enforces, so a file we produce can always be read back as geometry, and say
/// what the junction pass changed (the export receipt's line, #576 TML-DEPTH-D2).
///
/// The one that bites is that a consumer derives each shot's vertical change from
/// its PARENT's `Depth`, not from the shot's own `DepthIn` - so the chain must
/// satisfy `|Depth - parent.Depth| <= Length` for every shot. Our own model is
/// richer: a shot carries `depth_in` and `depth_out` independently, so a branch
/// that starts at a different depth than the station it hangs off is
/// representable here and not in Ariane's chain, where a station has ONE depth.
/// Exported verbatim, such a junction reads as a shot with more vertical drop
/// than length - geometrically impossible - and SpeleoDB's `openspeleo_lib`
/// raises on it, which aborts GeoJSON generation for the ENTIRE project (the
/// upload still returns 200, so nothing surfaces to the user).
///
/// [#576 TML-DEPTH-D1] Depth readings are true and a junction disagreement does
/// not offset them: such a junction is exported at the mean of its readings,
/// every other station keeps its true depth, and only the shots touching the
/// junction change - their slope, never their plan. `junction_depths` states the
/// rule. It replaces the re-hang, which started the branch at the parent's depth
/// and carried the mismatch down every later shot of the branch.
///
/// Also sweeps non-finite values, which reach the same comparison and fail it (any
/// comparison against NaN is false), and would otherwise be written into the file.
fn enforce_ariane_geometry_invariants(
    shots: &mut [ExportShot],
    profile: TmlProfile,
) -> JunctionDepths {
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
    let (junctions, visited) = junction_depths::settle(shots, &index_by_id);

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
    junctions
}

// ============================================================================
// SpeleoDB sidecar hardening
// ============================================================================

/// The character SpeleoDB joins `name/description/date/explorers/surveyors` with
/// before hashing a shot into a section (U+220E). Left inside one of those
/// fields it re-cuts the survey into sections nobody drew.
const SPELEODB_SECTION_FIELD_SEPARATOR: char = '\u{220e}';

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
/// for the one section, so an unparseable date falls back rather than being
/// passed through. The year clamp trades a wrong date for a right declination:
/// out of range the model returns 0.0 without saying so, which rotates the map.
fn speleodb_date(raw: Option<&str>) -> String {
    use chrono::Datelike as _;

    let Some(parsed) = raw
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
    else {
        return schema::UNDATED_PLACEHOLDER.to_owned();
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

fn strip_section_separator(field: &str) -> String {
    field.replace(SPELEODB_SECTION_FIELD_SEPARATOR, " ")
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
        shot.section = strip_section_separator(&shot.section);
        shot.explorer = shot.explorer.as_deref().map(strip_section_separator);
        shot.date = Some(speleodb_date(shot.date.as_deref()));
        // A repeat aliases the two shots onto one GeoJSON feature id. Shot UUIDs
        // are unique per SECTION by design (see `shot_export_uuid`), so a repeat
        // across sections is legal in the model and has to be dropped here;
        // omitting the optional element costs nothing downstream.
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

    // Ids come off one counter that is never reset, so this cannot fire; it is
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
        let pad = ExportShot::padding(pad_id, &shots[0]);
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

/// The bearing of a leg walked backwards: the reciprocal of the forward one.
///
/// `forward` is the bearing the app draws - the circular mean of the shot's
/// two forward readings, `azimuth_in` and `azimuth_out` (SPEC: they are two
/// readings of the SAME forward line, not a foresight/backsight pair). Ariane
/// carries a single forward azimuth per shot, so the reversed and
/// pre-zero-datum legs are synthesized as its reciprocal; `azimuth_out` is
/// never a backsight to read a reverse bearing from.
fn reverse_pre_zero_azimuth(forward: f64) -> f64 {
    normalize_azimuth(forward + 180.0)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::approx_constant)]
mod tests {
    use super::*;
    use crate::models::test_support::require_env_file;
    use crate::models::{
        ClosureStrategy, GpsCoordinates, LoopClosure, SectionData, ShotData, ShotKind,
        StationAnchor, StationKey, StationMerge, ZeroDatum,
    };
    use tempfile::NamedTempFile;

    /// The shot list alone, for the tests that inspect it; production goes
    /// through [`build_export_shots_with_report`] (via `build_xml`), which
    /// also counts the GPS anchors no START could carry. Test-only, so it
    /// lives here rather than as dead code in every non-test build.
    fn build_export_shots(
        project: &Project,
        profile: TmlProfile,
        azimuth_export: AzimuthExport,
    ) -> Vec<ExportShot> {
        build_export_shots_with_report(project, profile, azimuth_export).0
    }

    /// The model is +down and Ariane is treated as +up, so the two must be
    /// mirror images and a round-trip must land back on the original value
    /// (issue #296).  Zero stays a plain `0.0` in both directions: `-0.0` would
    /// survive into the XML, where it renders as a different string for a value
    /// that did not change.
    #[test]
    fn tml_inclination_round_trips_through_the_sign_flip() {
        use crate::parsers::tml::inclination_to_pitch;

        for pitch in [30.0_f64, -30.0, 90.0, -90.0, 0.5, -0.5] {
            let inclination = pitch_to_inclination(Some(pitch));
            assert!(
                (inclination + pitch).abs() < f64::EPSILON,
                "pitch {pitch} should export as {}, got {inclination}",
                -pitch
            );
            assert!(
                (inclination_to_pitch(inclination) - pitch).abs() < f64::EPSILON,
                "round-trip of pitch {pitch} did not return it"
            );
        }

        for zero in [pitch_to_inclination(None), pitch_to_inclination(Some(0.0))] {
            assert!(
                zero == 0.0 && zero.is_sign_positive(),
                "zero pitch must export as plain 0.0, not -0.0"
            );
        }
        let back = inclination_to_pitch(0.0);
        assert!(
            back == 0.0 && back.is_sign_positive(),
            "zero inclination must import as plain 0.0, not -0.0"
        );
    }

    /// A reverse leg walks the shot backwards, so a descent becomes an ascent
    /// and the pitch negates. `pitch_out` must not be used as the reversed
    /// reading: it is a foresight in the same sense as `pitch_in`.
    #[test]
    fn reversed_leg_pitch_negates_the_forward_reading() {
        let project = simple_project();
        let mut shot = project.sections[0].shots[0].clone();
        shot.pitch_in = Some(20.0);
        shot.pitch_out = Some(22.0);
        assert_eq!(
            reversed_leg_pitch(&shot),
            Some(-20.0),
            "a 20 degree descent reversed is a 20 degree ascent, not +22"
        );

        // Falls back to the other leg only when the forward one is absent.
        shot.pitch_in = None;
        assert_eq!(reversed_leg_pitch(&shot), Some(-22.0));

        shot.pitch_out = None;
        assert_eq!(reversed_leg_pitch(&shot), None);

        // A flat shot must still export as a plain 0.0, never -0.0.
        shot.pitch_in = Some(0.0);
        let inclination = pitch_to_inclination(reversed_leg_pitch(&shot));
        assert!(
            inclination == 0.0 && inclination.is_sign_positive(),
            "reversing a flat shot must not produce -0.0"
        );
    }

    /// The helper-level test above pins `pitch_to_inclination` against
    /// `inclination_to_pitch`, but both could be reverted together, or one of
    /// the export call sites could stop calling the helper, without it noticing.
    /// This drives a real project through the file: export to `.tml`, parse it
    /// back, and require the pitch to return unchanged.
    ///
    /// Every `.tml` fixture in the tree carries `Inclination` 0 (Ariane does not
    /// populate the field), so non-zero inclination is only reachable through a
    /// file Stalagmight wrote itself - an export the user re-imports, or the
    /// sidecar the SpeleoDB push renders.
    #[test]
    fn tml_inclination_survives_a_file_level_round_trip() {
        let project = simple_project();
        // Ariane has no way to say "no inclination recorded", so an absent pitch
        // necessarily comes back as 0.0. That is format lossiness, not a sign
        // error, so compare against the value the format can actually carry.
        let exported: Vec<f64> = project.sections[0]
            .shots
            .iter()
            .map(|s| s.pitch_in.unwrap_or(0.0))
            .collect();
        assert!(
            exported.iter().any(|p| p.abs() > 1.0),
            "fixture has no recorded inclination, so this test would prove nothing"
        );

        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("round_trip.tmlu");
        export_tml(&project, &path).unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        // `.tmlu` uses the short element names, so inclination is `<INC>`.
        assert!(
            raw.contains("<INC>-5"),
            "a +5 (down) pitch must leave as -5 in Ariane's +up frame: {raw}"
        );

        let parsed = crate::parsers::tml::parse_tml(&path).unwrap();
        let reimported: Vec<f64> = parsed.sections[0]
            .shots
            .iter()
            .map(|s| {
                crate::parsers::tml::normalize_tml_shot(s, s.depth_in)
                    .pitch_in
                    .unwrap_or(0.0)
            })
            .collect();

        assert_eq!(
            reimported, exported,
            "pitch must survive export and re-import unchanged; a flip applied \
             zero or two times here silently inverts every shot"
        );
    }

    fn simple_project() -> Project {
        let mut project = Project::new("Test Cave".to_string(), None);
        project.add_section(SectionData {
            name: Some("Main".to_string()),
            surveyor: Some("Alice".to_string()),
            date: chrono::NaiveDate::from_ymd_opt(2025, 6, 15),
            color: "#ff0000".to_string(),
            // `azimuth_in`/`azimuth_out` are two forward readings of one line
            // (SPEC), so a plain fixture repeats the bearing; a backsight-shaped
            // 45/225 pair is a gross blunder whose mean the app draws at 315.
            shots: vec![
                ShotData {
                    azimuth_in: 45.0,
                    azimuth_out: 45.0,
                    length: 10.0,
                    depth_in: 0.0,
                    depth_out: 3.0,
                    pitch_in: Some(5.0),
                    left: Some(1.0),
                    right: Some(2.0),
                    up: Some(3.0),
                    down: Some(0.5),
                    comment: Some("first shot".to_string()),
                    ..Default::default()
                },
                ShotData {
                    azimuth_in: 90.0,
                    azimuth_out: 90.0,
                    length: 8.0,
                    depth_in: 3.0,
                    depth_out: 5.0,
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
        project
    }

    #[test]
    fn test_export_emits_a_uuid_per_real_shot() {
        // The `<UUID>`/`<UU>` element is what makes a re-export of an unchanged
        // project byte-identical, which the SpeleoDB co-push render depends on
        // (#196). Synthesized shots (START here) carry none.
        let project = simple_project();
        let total_shots: usize = project.sections.iter().map(|s| s.shots.len()).sum();
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("out.tmlu");

        export_tml(&project, &path).unwrap();

        // The .tmlu uses short tag names, so each real shot emits a <UU> element.
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content.matches("<UU>").count(), total_shots);
        // START shot carries no UUID.
        assert!(content.contains("START"));

        // Each emitted UUID is the shot's own persistent identity, not a
        // per-export mint.
        for section in &project.sections {
            for shot in &section.shots {
                let uuid = shot.uuid.as_deref().unwrap_or_default();
                assert!(
                    !uuid.is_empty(),
                    "shot must carry a uuid after construction"
                );
                assert!(
                    content.contains(&format!("<UU>{uuid}</UU>")),
                    "shot uuid {uuid} missing from the export"
                );
            }
        }
    }

    /// The fold now TAKES each entry out of `comments_by_ariane` as it applies it
    /// and reports whatever is left as dropped, so the two halves have to stay in
    /// step. Both directions matter: a comment on a station some REAL shot creates
    /// must still be folded onto that shot, and a comment on a station whose Ariane
    /// id is the START shot's - root station 0 here - must still be dropped rather
    /// than overwriting START's hardcoded content. If the take ever moved above the
    /// shot-type skip, the START-aliased comment would leave the map without being
    /// applied OR reported, which is exactly the silent loss the report exists to
    /// end.
    #[test]
    fn station_comments_fold_onto_their_shot_and_a_start_aliased_one_is_still_dropped() {
        let mut project = simple_project();
        let section_id = project.sections[0].id;
        let station_0 = project.key_for_ref(StationRef::new(section_id, 0)).unwrap();
        let station_1 = project.key_for_ref(StationRef::new(section_id, 1)).unwrap();
        project
            .station_comments
            .insert(station_0, "on the START station".to_string());
        project
            .station_comments
            .insert(station_1, "at the bend".to_string());

        let shots = build_export_shots(&project, TmlProfile::User, AzimuthExport::decide(&project));

        // Station 1 is created by the section's first shot, so its comment rides
        // along there - beside that shot's own comment, split by the ZWSP marker.
        let first_real = shots.iter().find(|s| s.shot_type == "REAL").unwrap();
        let folded = first_real.comment.as_deref().unwrap_or_default();
        assert!(
            folded.contains("at the bend") && folded.contains("first shot"),
            "the station comment must be folded beside the shot comment: {folded:?}"
        );

        // Station 0 IS the START shot, which the fold skips, so its comment is
        // absent from the file and START keeps what it was given.
        let start = shots.iter().find(|s| s.shot_type == "START").unwrap();
        assert_eq!(start.comment.as_deref(), Some("START"));
        assert!(
            !shots.iter().any(|s| s
                .comment
                .as_deref()
                .is_some_and(|c| c.contains("on the START station"))),
            "a START-aliased station comment must not reach the file"
        );
    }

    /// #564: the shape that used to export unmarked - a shot comment with no
    /// station comment anywhere. An unmarked line is indistinguishable from
    /// Ariane-typed text, which the default import routes to the station.
    #[test]
    fn a_lone_shot_comment_exports_marked() {
        let project = simple_project();
        assert!(
            project.station_comments.is_empty(),
            "the point of this shape is NO station comment riding along"
        );

        let shots = build_export_shots(&project, TmlProfile::User, AzimuthExport::decide(&project));
        let first_real = shots.iter().find(|s| s.shot_type == "REAL").unwrap();
        assert_eq!(
            first_real.comment.as_deref(),
            Some(format!("{}first shot", comment_split::SHOT_MARKER).as_str()),
            "a lone shot comment must carry the ZWSP marker, or a default \
             re-import moves it to the to-station"
        );
        // START stays exactly as minted - unmarked, hardcoded.
        let start = shots.iter().find(|s| s.shot_type == "START").unwrap();
        assert_eq!(start.comment.as_deref(), Some("START"));
    }

    #[test]
    fn uuid_survives_insert_between_exports() {
        // Regression for the silent-UUID-rebinding bug: identity is carried on
        // `Shot::uuid`, not derived from the shot's position, so inserting a
        // shot must not re-assign any existing shot's UUID.
        let mut project = simple_project();
        let dir = tempfile::TempDir::new().unwrap();
        let v1 = dir.path().join("v1.tmlu");
        let v2 = dir.path().join("v2.tmlu");

        export_tml(&project, &v1).unwrap();
        let uuids_v1: Vec<String> = project.sections[0]
            .shots
            .iter()
            .map(|s| s.uuid.clone().unwrap_or_default())
            .collect();
        assert!(
            uuids_v1.iter().all(|u| !u.is_empty()),
            "every shot must carry a uuid after construction"
        );

        // Insert a brand-new shot at the FRONT of section 0. All pre-existing
        // shots' indices shift by +1.
        use crate::models::ShotData;
        project.sections[0].shots.insert(
            0,
            Shot::new(
                42,
                ShotData {
                    azimuth_in: 270.0,
                    length: 1.5,
                    ..Default::default()
                },
            ),
        );

        export_tml(&project, &v2).unwrap();
        let uuids_v2: Vec<String> = project.sections[0]
            .shots
            .iter()
            .map(|s| s.uuid.clone().unwrap_or_default())
            .collect();
        assert_eq!(uuids_v2.len(), uuids_v1.len() + 1);
        for (i, prev_uuid) in uuids_v1.iter().enumerate() {
            assert_eq!(
                &uuids_v2[i + 1],
                prev_uuid,
                "pre-existing shot at new index {} lost its uuid",
                i + 1
            );
        }
        assert!(!uuids_v2[0].is_empty(), "new shot must have a uuid");
        assert!(
            !uuids_v1.contains(&uuids_v2[0]),
            "new shot's uuid must not collide with any pre-existing one"
        );

        // Every pre-existing UUID is still in the emitted XML, at its new slot.
        let content = std::fs::read_to_string(&v2).unwrap();
        for uuid in &uuids_v1 {
            assert!(
                content.contains(&format!("<UU>{uuid}</UU>")),
                "uuid {uuid} lost from the export after an insert"
            );
        }
    }

    /// #576 B11: a character XML 1.0 cannot carry - it arrives in names and
    /// comments from imported files and pasted text - used to be written as a
    /// raw byte, and one such byte makes Ariane and SpeleoDB reject the WHOLE
    /// document. It is replaced with U+FFFD and the export says so; line
    /// breaks and the U+200B shot-comment marker are legal and survive.
    #[test]
    fn text_xml_cannot_carry_is_replaced_and_noted() {
        let mut project = Project::new("Cave\u{1}A".to_string(), None);
        project.add_section(SectionData {
            name: Some("Main\u{B}".to_string()),
            surveyor: Some("Diver\u{FFFE}A".to_string()),
            color: "#ff0000".to_string(),
            shots: vec![ShotData {
                length: 3.0,
                comment: Some("line one\nline two\u{0}".to_string()),
                ..Default::default()
            }],
            ..Default::default()
        });
        let key = project.key_for_ref(StationRef::new(0, 1)).unwrap();
        project
            .station_comments
            .insert(key, "station\u{1F}note".to_string());

        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("illegal.tmlu");
        let notes = export_tml(&project, &path).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();

        let illegal: Vec<char> = content
            .chars()
            .filter(|&c| !schema::is_xml_char(c))
            .collect();
        assert!(
            illegal.is_empty(),
            "the document must be well-formed XML; still carries {illegal:?}"
        );
        assert_eq!(
            content.matches('\u{FFFD}').count(),
            // name, section (on the START and the shot), surveyor (both rows),
            // the shot comment and the station comment
            1 + 2 + 2 + 1 + 1,
            "{content}"
        );
        assert!(
            notes.contains(&"xml-chars-replaced".to_string()),
            "the user must be told the text changed: {notes:?}"
        );
        assert!(content.contains("<caveName>Cave\u{FFFD}A</caveName>"));
        assert!(
            content.contains("\u{200B}line one\n\u{200B}line two\u{FFFD}\nstation\u{FFFD}note"),
            "line breaks and the shot-comment markers survive: {content}"
        );

        // Our own reader takes the file back with the replacement in place.
        let reparsed = crate::parsers::tml::parse_tml(&path).unwrap();
        assert_eq!(reparsed.cave_name, "Cave\u{FFFD}A");
        assert_eq!(reparsed.sections[0].name, "Main\u{FFFD}");

        // The sidecar gets the same treatment (its notes go to the log).
        let sidecar = dir.path().join("sidecar.tmlu");
        export_tml_for_speleodb(&project, &sidecar).unwrap();
        let sidecar_content = std::fs::read_to_string(&sidecar).unwrap();
        assert!(sidecar_content.chars().all(schema::is_xml_char));

        // And an ordinary project says nothing.
        let ordinary = dir.path().join("ordinary.tmlu");
        assert!(export_tml(&simple_project(), &ordinary).unwrap().is_empty());
    }

    /// A georeferenced, magnetic project whose sections carry the given dates.
    fn dated_project(dates: &[Option<chrono::NaiveDate>]) -> Project {
        let mut project = Project::new("CaveA".to_string(), None);
        for (i, date) in dates.iter().enumerate() {
            project.add_section(SectionData {
                name: Some(format!("Section{i}")),
                date: *date,
                color: "#ff0000".to_string(),
                shots: vec![ShotData {
                    azimuth_in: 90.0,
                    azimuth_out: 90.0,
                    length: 4.0,
                    ..Default::default()
                }],
                ..Default::default()
            });
        }
        project.zero_datums.push(ZeroDatum {
            station_ref: StationRef::new(0, 0).into(),
            gps: GpsCoordinates::new(20.5, -87.3, 0.0).unwrap(),
        });
        project
    }

    /// Export to `.tmlu`, re-import, and return what the import records.
    fn reimport(project: &Project) -> Vec<crate::models::ImportSectionConfig> {
        let file = NamedTempFile::with_suffix(".tmlu").unwrap();
        export_tml(project, file.path()).unwrap();
        let tml = crate::parsers::tml::parse_tml(file.path()).unwrap();
        crate::parsers::tml::convert_tml_to_import_configs(&tml, false, true)
            .unwrap()
            .0
    }

    /// #576 B2: an UNDATED section went out with the 2000-01-01 placeholder
    /// and came back dated 1 January 2000, so a magnetic, georeferenced file
    /// derived an IGRF declination for the year 2000 and rotated the survey.
    /// The placeholder stays (SpeleoDB rejects a missing date, and Ariane's
    /// handling of an empty one is unverified) and reads back as "no date".
    #[test]
    fn an_undated_section_round_trips_undated_and_unrotated() {
        let configs = reimport(&dated_project(&[None]));
        assert_eq!(configs.len(), 1);
        assert_eq!(
            configs[0].date, None,
            "the placeholder is not a survey date"
        );
        let reference = configs[0].azimuth_reference.unwrap();
        assert_eq!(
            reference,
            AzimuthReference::magnetic(0.0, crate::models::CorrectionSource::Unknown),
            "no date means no epoch to evaluate a declination at"
        );
    }

    /// The placeholder no longer displaces a REAL earliest date when the file
    /// mixes dated and undated sections: the epoch is the real date's year.
    #[test]
    fn an_undated_section_does_not_move_the_declination_epoch() {
        let configs = reimport(&dated_project(&[
            chrono::NaiveDate::from_ymd_opt(2024, 5, 1),
            None,
        ]));
        let reference = configs[0].azimuth_reference.unwrap();
        assert_eq!(
            reference.source,
            crate::models::CorrectionSource::IgrfAuto { epoch_year: 2024 },
            "{reference:?}"
        );
        assert_eq!(configs[1].date, None);
    }

    /// #576 B2-MARKER fallback: ANY 2000-01-01 reads back as undated, marker
    /// or not, so a section genuinely surveyed that day imports undated. That
    /// is the accepted cost of reading every unmarked placeholder (a file
    /// Ariane re-saved, an export from before the marker) as what it is.
    #[test]
    fn a_first_of_january_2000_reads_back_undated() {
        let new_year = chrono::NaiveDate::from_ymd_opt(2000, 1, 1);
        let configs = reimport(&dated_project(&[new_year]));
        assert_eq!(configs[0].date, None);
        assert_eq!(
            configs[0].azimuth_reference.unwrap(),
            AzimuthReference::magnetic(0.0, crate::models::CorrectionSource::Unknown),
            "no date means no epoch to evaluate a declination at"
        );
    }

    /// The SpeleoDB sidecar never carries the marker (`speleodb_date` writes
    /// the bare placeholder) and SpeleoDB adoption re-imports it through the
    /// same reader, so an undated section must come back undated from it too.
    #[test]
    fn an_undated_section_comes_back_undated_from_the_speleodb_sidecar() {
        let dir = tempfile::TempDir::new().unwrap();
        let sidecar = dir.path().join("sidecar.tmlu");
        export_tml_for_speleodb(&dated_project(&[None]), &sidecar).unwrap();
        let content = std::fs::read_to_string(&sidecar).unwrap();
        assert!(
            !content.contains(schema::UNDATED_MARKER),
            "the sidecar is the unmarked shape this test is about"
        );

        let tml = crate::parsers::tml::parse_tml(&sidecar).unwrap();
        let configs = crate::parsers::tml::convert_tml_to_import_configs(&tml, false, true)
            .unwrap()
            .0;
        let section = configs
            .iter()
            .find(|c| c.name.as_deref() == Some("Section0"))
            .unwrap();
        assert_eq!(section.date, None);
        assert_eq!(
            section.azimuth_reference.unwrap(),
            AzimuthReference::magnetic(0.0, crate::models::CorrectionSource::Unknown)
        );
    }

    #[test]
    fn export_tml_is_byte_stable_across_calls() {
        // #196 render-on-push: two renders of the same project must be
        // byte-identical so re-pushing unchanged content is a server no-op (304).
        // Modern shots (via add_section → Shot::new) already carry stable UUIDs.
        let project = simple_project();
        let dir = tempfile::TempDir::new().unwrap();
        let a = dir.path().join("a.tml");
        let b = dir.path().join("b.tml");
        export_tml(&project, &a).unwrap();
        export_tml(&project, &b).unwrap();
        assert_eq!(
            std::fs::read(&a).unwrap(),
            std::fs::read(&b).unwrap(),
            "two renders of the same project must be byte-identical"
        );
    }

    #[test]
    fn ensure_shot_uuids_makes_legacy_render_byte_stable() {
        // #196 guard: a legacy project whose shots have NO UUID would otherwise
        // get a fresh ephemeral UUID minted per export (shot_export_uuid's last resort),
        // making renders differ. ensure_shot_uuids() mints stable UUIDs up front.
        let mut project = simple_project();
        for section in &mut project.sections {
            for shot in &mut section.shots {
                shot.uuid = None;
            }
        }
        let dir = tempfile::TempDir::new().unwrap();

        // Without the guard: ephemeral per-call UUIDs → renders differ.
        let n1 = dir.path().join("n1.tml");
        let n2 = dir.path().join("n2.tml");
        export_tml(&project, &n1).unwrap();
        export_tml(&project, &n2).unwrap();
        assert_ne!(
            std::fs::read(&n1).unwrap(),
            std::fs::read(&n2).unwrap(),
            "legacy None-uuid shots mint ephemeral UUIDs → renders differ without the guard"
        );

        // With the guard: stable UUIDs → identical renders.
        project.ensure_shot_uuids();
        let g1 = dir.path().join("g1.tml");
        let g2 = dir.path().join("g2.tml");
        export_tml(&project, &g1).unwrap();
        export_tml(&project, &g2).unwrap();
        assert_eq!(
            std::fs::read(&g1).unwrap(),
            std::fs::read(&g2).unwrap(),
            "ensure_shot_uuids() restores byte-stability for legacy projects"
        );
    }

    #[test]
    fn test_export_tmlu_creates_valid_xml() {
        let project = simple_project();
        let file = NamedTempFile::with_suffix(".tmlu").unwrap();
        let result = export_tml(&project, file.path());
        assert!(result.is_ok(), "export_tml failed: {result:?}");

        let content = std::fs::read_to_string(file.path()).unwrap();
        assert!(content.contains("<?xml"));
        assert!(content.contains("<CaveFile>"));
        assert!(content.contains("Test Cave"));
        // TMLU uses short names
        assert!(content.contains("<SRVD>"));
        assert!(content.contains("<AZ>"));
        assert!(content.contains("<LG>"));
    }

    #[test]
    fn test_export_tml_creates_zip() {
        let project = simple_project();
        let file = NamedTempFile::with_suffix(".tml").unwrap();
        let result = export_tml(&project, file.path());
        assert!(result.is_ok(), "export_tml failed: {result:?}");

        // Verify it's a valid ZIP containing Data.xml
        let zip_file = std::fs::File::open(file.path()).unwrap();
        let mut archive = zip::ZipArchive::new(zip_file).unwrap();
        assert!(archive.by_name("Data.xml").is_ok());

        let mut data_xml = String::new();
        std::io::Read::read_to_string(&mut archive.by_name("Data.xml").unwrap(), &mut data_xml)
            .unwrap();
        // TML uses long names - verify ID is all-caps
        assert!(data_xml.contains("<SurveyData>"));
        assert!(data_xml.contains("<ID>"));
        assert!(data_xml.contains("<Azimut>"));
        assert!(data_xml.contains("<Length>"));
        assert!(data_xml.contains("<Locked>"));
        assert!(data_xml.contains("<Name>"));
        assert!(data_xml.contains("<Profiletype>"));
    }

    #[test]
    fn test_export_empty_project() {
        let project = Project::new("Empty".to_string(), None);
        let file = NamedTempFile::with_suffix(".tmlu").unwrap();
        let result = export_tml(&project, file.path());
        assert!(result.is_ok());

        let content = std::fs::read_to_string(file.path()).unwrap();
        assert!(content.contains("<CaveFile>"));
        assert!(content.contains("<Data>"));
        // No survey data elements
        assert!(!content.contains("<SRVD>"));
    }

    #[test]
    fn test_color_to_ariane() {
        assert_eq!(color_to_ariane("#ff0000"), "0xff0000ff");
        assert_eq!(color_to_ariane("#4ecdc4"), "0x4ecdc4ff");
        assert_eq!(color_to_ariane("invalid"), "0x4ecdc4ff");
        assert_eq!(color_to_ariane("#abc"), "0x4ecdc4ff"); // too short
    }

    #[test]
    fn test_format_f64() {
        assert_eq!(format_f64(0.0), "0.0");
        assert_eq!(format_f64(10.0), "10.0");
        assert_eq!(format_f64(3.14159), "3.14159");
        assert_eq!(format_f64(1.5), "1.5");
        assert_eq!(format_f64(-2.0), "-2.0");
    }

    #[test]
    fn test_reverse_pre_zero_azimuth_is_reciprocal() {
        assert!((reverse_pre_zero_azimuth(42.0) - 222.0).abs() < 1e-9);
        assert!((reverse_pre_zero_azimuth(350.0) - 170.0).abs() < 1e-9);
    }

    /// #576 NF-AN5-1: `azimuth_in` and `azimuth_out` are two readings of the
    /// same forward line and the app draws their circular mean, but the export
    /// wrote `azimuth_in` alone - so a shot drawn at 15 degrees went out at 10
    /// and a re-import moved the cave. Every leg, forward and reversed, now
    /// carries the bearing the app draws, wraparound included.
    #[test]
    fn every_leg_exports_the_bearing_the_app_draws() {
        let mut project = Project::new("CaveA".to_string(), None);
        let readings = [(10.0, 20.0), (350.0, 10.0), (100.0, 100.0)];
        project.add_section(SectionData {
            name: Some("Main".to_string()),
            color: "#ff0000".to_string(),
            shots: readings
                .iter()
                .map(|&(azimuth_in, azimuth_out)| ShotData {
                    azimuth_in,
                    azimuth_out,
                    length: 5.0,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        });
        // Datum on station 2: shots 0-1 go out reversed, shot 2 forward.
        project.zero_datums.push(ZeroDatum {
            station_ref: StationRef::new(0, 2).into(),
            gps: GpsCoordinates::new(20.5, -87.3, 0.0).unwrap(),
        });

        let shots = build_export_shots(&project, TmlProfile::User, AzimuthExport::decide(&project));
        let azimuth_of = |uuid: &str| {
            shots
                .iter()
                .find(|s| s.uuid.as_deref() == Some(uuid))
                .unwrap()
                .azimuth
        };
        let drawn: Vec<f64> = project.sections[0]
            .shots
            .iter()
            .map(Shot::average_azimuth_measured)
            .collect();
        let uuids: Vec<String> = project.sections[0]
            .shots
            .iter()
            .map(|s| s.uuid.clone().unwrap())
            .collect();
        // Reversed legs: the reciprocal of the drawn bearing.
        assert!((azimuth_of(&uuids[0]) - 195.0).abs() < 1e-9, "{shots:?}");
        assert!((azimuth_of(&uuids[1]) - 180.0).abs() < 1e-9, "{shots:?}");
        // Forward leg.
        assert!((azimuth_of(&uuids[2]) - 100.0).abs() < 1e-9);
        assert!((drawn[0] - 15.0).abs() < 1e-9 && drawn[1].abs() < 1e-9);

        // And the round trip keeps the geometry: what comes back is the mean.
        let file = NamedTempFile::with_suffix(".tmlu").unwrap();
        export_tml(&project, file.path()).unwrap();
        let tml = crate::parsers::tml::parse_tml(file.path()).unwrap();
        let reimported: Vec<f64> = tml
            .sections
            .iter()
            .flat_map(|s| &s.shots)
            .map(|s| (s.azimuth + 180.0).rem_euclid(360.0))
            .collect();
        assert!(
            reimported.iter().any(|a| (a - 15.0).abs() < 1e-6),
            "the 10/20 shot comes back at 15: {reimported:?}"
        );
    }

    #[test]
    fn test_single_start_shot_at_root() {
        let project = simple_project();
        let shots = build_export_shots(&project, TmlProfile::User, AzimuthExport::decide(&project));

        // One entrance in this fixture, so one START. A survey with a section merged
        // into nothing emits one per entrance - see `a_detached_section_exports_as_its_own_entrance`.
        let start_count = shots.iter().filter(|s| s.shot_type == "START").count();
        assert_eq!(start_count, 1, "expected exactly 1 START shot");

        // First shot should be START with from_id = -1
        assert_eq!(shots[0].shot_type, "START");
        assert_eq!(shots[0].from_id, -1);
        assert!(shots[0].locked, "START shot should be locked");

        // Next shots should be REAL
        assert_eq!(shots[1].shot_type, "REAL");
        assert_eq!(shots[2].shot_type, "REAL");
    }

    #[test]
    fn a_twice_imported_identity_exports_once_and_reminted_after() {
        // Import the same .tml twice (or merge two projects from one file) and
        // both copies of every shot carry the same foreign identity. The
        // user-facing document must not repeat a <UUID>: the first copy keeps
        // the Ariane id, later copies are re-minted (never dropped).
        const FOREIGN: &str = "beefbeef-0000-4000-8000-0000000000aa";
        let shot = |uuid: Option<&str>| ShotData {
            azimuth_in: 45.0,
            azimuth_out: 225.0,
            length: 10.0,
            depth_in: 0.0,
            depth_out: 3.0,
            uuid: uuid.map(str::to_string),
            ..Default::default()
        };
        let mut project = Project::new("Twice".to_string(), None);
        for name in ["Copy A", "Copy B"] {
            project.add_section(SectionData {
                name: Some(name.to_string()),
                color: "#ff0000".to_string(),
                shots: vec![shot(Some(FOREIGN)), shot(None)],
                ..Default::default()
            });
        }

        let shots = build_export_shots(&project, TmlProfile::User, AzimuthExport::decide(&project));

        let uuids: Vec<&str> = shots.iter().filter_map(|s| s.uuid.as_deref()).collect();
        assert_eq!(uuids.len(), 4, "every REAL shot still emits a <UUID>");
        assert_eq!(
            uuids.iter().filter(|u| **u == FOREIGN).count(),
            1,
            "exactly one copy keeps the foreign identity"
        );
        let distinct: HashSet<&str> = uuids.iter().copied().collect();
        assert_eq!(
            distinct.len(),
            uuids.len(),
            "no <UUID> repeats in the document"
        );
        // Document order puts Copy A's shots first, so the surviving foreign id
        // is the FIRST occurrence.
        let first_foreign = shots
            .iter()
            .find(|s| s.uuid.as_deref() == Some(FOREIGN))
            .unwrap();
        assert_eq!(first_foreign.section, "Copy A");
    }

    #[test]
    fn test_sequential_ids() {
        let project = simple_project();
        let shots = build_export_shots(&project, TmlProfile::User, AzimuthExport::decide(&project));

        for (i, shot) in shots.iter().enumerate() {
            assert_eq!(shot.id, i as i64, "shot {i} should have id {i}");
        }
    }

    #[test]
    fn test_from_id_chain() {
        let project = simple_project();
        let shots = build_export_shots(&project, TmlProfile::User, AzimuthExport::decide(&project));

        // START has from_id = -1
        assert_eq!(shots[0].from_id, -1);
        // First real shot chains from START
        assert_eq!(shots[1].from_id, shots[0].id);
        // Second real shot chains from first
        assert_eq!(shots[2].from_id, shots[1].id);
    }

    #[test]
    fn test_virtual_shot_type() {
        let mut project = Project::new("Virtual Test".to_string(), None);
        project.add_section(SectionData {
            name: Some("Sec".to_string()),
            color: "#000000".to_string(),
            shots: vec![
                ShotData {
                    kind: ShotKind::Jump,
                    length: 5.0,
                    ..Default::default()
                },
                ShotData {
                    kind: ShotKind::Normal,
                    length: 3.0,
                    ..Default::default()
                },
            ],
            ..Default::default()
        });

        let shots = build_export_shots(&project, TmlProfile::User, AzimuthExport::decide(&project));
        // shots[0] = START, shots[1] = VIRTUAL, shots[2] = REAL
        assert_eq!(shots[1].shot_type, "VIRTUAL");
        assert_eq!(shots[2].shot_type, "REAL");
    }

    #[test]
    fn test_multi_section_no_extra_starts() {
        let mut project = Project::new("Multi".to_string(), None);

        // Section 0: 2 shots (root)
        project.add_section(SectionData {
            name: Some("Sec A".to_string()),
            color: "#ff0000".to_string(),
            shots: vec![
                ShotData {
                    length: 10.0,
                    depth_out: 5.0,
                    ..Default::default()
                },
                ShotData {
                    length: 8.0,
                    depth_in: 5.0,
                    depth_out: 7.0,
                    ..Default::default()
                },
            ],
            ..Default::default()
        });

        // Section 1: 1 shot (child, connected to station 1 of section 0)
        project.add_section(SectionData {
            name: Some("Sec B".to_string()),
            color: "#00ff00".to_string(),
            shots: vec![ShotData {
                length: 6.0,
                depth_out: 2.0,
                ..Default::default()
            }],
            ..Default::default()
        });

        // Merge: section 1, station 0 → section 0, station 1
        project.station_merges.push(StationMerge {
            station_a: StationRef {
                section_id: 0,
                station_index: 1,
            }
            .into(),
            station_b: StationRef {
                section_id: 1,
                station_index: 0,
            }
            .into(),
            merged_at: chrono::Utc::now(),
        });

        let shots = build_export_shots(&project, TmlProfile::User, AzimuthExport::decide(&project));

        // Only ONE start shot should exist (for root section A)
        let start_count = shots.iter().filter(|s| s.shot_type == "START").count();
        assert_eq!(start_count, 1, "expected exactly 1 START shot");

        // Section A: START(0), REAL(1), REAL(2)
        // Section B: REAL(3) - no START, first shot links to parent
        assert_eq!(shots.len(), 4);

        assert_eq!(shots[0].shot_type, "START");
        assert_eq!(shots[1].shot_type, "REAL");
        assert_eq!(shots[2].shot_type, "REAL");
        assert_eq!(shots[3].shot_type, "REAL"); // Sec B's shot, NOT a START

        // Section B's first shot should reference ariane ID of section A station 1
        // Station 1 in section A = ariane ID 1 (first real shot)
        assert_eq!(shots[3].from_id, 1);
    }

    #[test]
    fn test_child_merge_station_index_rewrites_matching_shot() {
        let mut project = Project::new("Child Merge Index".to_string(), None);

        // Root section with 3 shots: station IDs become START(0), 1, 2, 3.
        project.add_section(SectionData {
            name: Some("Root".to_string()),
            color: "#ff0000".to_string(),
            shots: vec![
                ShotData {
                    length: 5.0,
                    ..Default::default()
                },
                ShotData {
                    length: 5.0,
                    ..Default::default()
                },
                ShotData {
                    length: 5.0,
                    ..Default::default()
                },
            ],
            ..Default::default()
        });

        // Child section with 3 shots.
        project.add_section(SectionData {
            name: Some("Child".to_string()),
            color: "#00ff00".to_string(),
            shots: vec![
                ShotData {
                    length: 2.0,
                    ..Default::default()
                },
                ShotData {
                    length: 2.0,
                    ..Default::default()
                },
                ShotData {
                    length: 2.0,
                    ..Default::default()
                },
            ],
            ..Default::default()
        });

        // Merge child station 1 to root station 2.
        project.station_merges.push(StationMerge {
            station_a: StationRef {
                section_id: 0,
                station_index: 2,
            }
            .into(),
            station_b: StationRef {
                section_id: 1,
                station_index: 1,
            }
            .into(),
            merged_at: chrono::Utc::now(),
        });

        let shots = build_export_shots(&project, TmlProfile::User, AzimuthExport::decide(&project));

        // With non-zero merge at station 1, child is split:
        // Forward block (shots 1,2): id=4 from_id=2 (parent), id=5 from_id=4
        // Reverse block (shot 0 reversed): id=6 from_id=2 (parent)
        let fwd_shot0 = shots.iter().find(|s| s.id == 4).unwrap();
        let fwd_shot1 = shots.iter().find(|s| s.id == 5).unwrap();
        let rev_shot0 = shots.iter().find(|s| s.id == 6).unwrap();
        assert_eq!(
            fwd_shot0.from_id, 2,
            "forward block first shot should link to parent (root station 2)"
        );
        assert_eq!(
            fwd_shot1.from_id, 4,
            "forward block second shot should chain from previous"
        );
        assert_eq!(
            rev_shot0.from_id, 2,
            "reverse block first shot should link to parent (root station 2)"
        );
    }

    #[test]
    fn test_child_station_zero_closure_resolves_to_parent_node() {
        let mut project = Project::new("Child Station Zero".to_string(), None);

        project.add_section(SectionData {
            name: Some("A".to_string()),
            color: "#ff0000".to_string(),
            shots: vec![
                ShotData {
                    length: 4.0,
                    ..Default::default()
                },
                ShotData {
                    length: 4.0,
                    ..Default::default()
                },
            ],
            ..Default::default()
        });

        project.add_section(SectionData {
            name: Some("B".to_string()),
            color: "#00ff00".to_string(),
            shots: vec![ShotData {
                length: 3.0,
                ..Default::default()
            }],
            ..Default::default()
        });

        // B:0 merges to A:1.
        project.station_merges.push(StationMerge {
            station_a: StationRef {
                section_id: 0,
                station_index: 1,
            }
            .into(),
            station_b: StationRef {
                section_id: 1,
                station_index: 0,
            }
            .into(),
            merged_at: chrono::Utc::now(),
        });

        // Closure from B:0 (merged) to A:2 should resolve to IDs 1 -> 2.
        project.loop_closures.push(LoopClosure {
            id: 42,
            station_a: StationRef {
                section_id: 1,
                station_index: 0,
            }
            .into(),
            station_b: StationRef {
                section_id: 0,
                station_index: 2,
            }
            .into(),
            path_length: 10.0,
            misclosure_distance: 0.4,
            error_percentage: 4.0,
            horizontal_misclosure: 0.4,
            vertical_misclosure: 0.0,
            strategy: ClosureStrategy::Batch,
            is_aligned: false,
        });

        let shots = build_export_shots(&project, TmlProfile::User, AzimuthExport::decide(&project));
        let closure = shots.iter().find(|s| s.shot_type == "CLOSURE").unwrap();
        assert_eq!(
            closure.from_id, 1,
            "B:0 should map to merged parent Ariane ID"
        );
        assert_eq!(closure.closure_to_id, 2, "A:2 should map to Ariane ID 2");
    }

    #[test]
    fn test_closure_export_uses_ariane_name_and_depth_in() {
        let mut project = Project::new("Closure Name".to_string(), None);
        project.add_section(SectionData {
            name: Some("Main".to_string()),
            color: "#ff0000".to_string(),
            shots: vec![
                ShotData {
                    length: 4.0,
                    ..Default::default()
                },
                ShotData {
                    length: 4.0,
                    ..Default::default()
                },
            ],
            ..Default::default()
        });

        project.loop_closures.push(LoopClosure {
            id: 7,
            station_a: StationRef {
                section_id: 0,
                station_index: 1,
            }
            .into(),
            station_b: StationRef {
                section_id: 0,
                station_index: 2,
            }
            .into(),
            path_length: 8.0,
            misclosure_distance: 0.2,
            error_percentage: 2.5,
            horizontal_misclosure: 0.2,
            vertical_misclosure: 0.0,
            strategy: ClosureStrategy::Batch,
            is_aligned: false,
        });

        let shots = build_export_shots(&project, TmlProfile::User, AzimuthExport::decide(&project));
        let closure = shots.iter().find(|s| s.shot_type == "CLOSURE").unwrap();
        assert!(
            closure.depth_in.abs() < f64::EPSILON,
            "closure DepthIn should be exported as 0.0"
        );

        let file = NamedTempFile::with_suffix(".tmlu").unwrap();
        export_tml(&project, file.path()).unwrap();
        let content = std::fs::read_to_string(file.path()).unwrap();
        assert!(content.contains("<TY>CLOSURE</TY>"));
        assert!(content.contains("<DPI>0.0</DPI>"));
        assert!(content.contains("<NM>CLOSURE:CLOSURE</NM>"));
    }

    #[test]
    fn test_zero_datum_coordinates_export_to_start_shot() {
        let mut project = Project::new("Datum Node".to_string(), None);
        project.add_section(SectionData {
            name: Some("Main".to_string()),
            color: "#ff0000".to_string(),
            shots: vec![
                ShotData {
                    length: 3.0,
                    ..Default::default()
                },
                ShotData {
                    length: 3.0,
                    ..Default::default()
                },
                ShotData {
                    length: 3.0,
                    ..Default::default()
                },
            ],
            ..Default::default()
        });

        project.zero_datums.push(ZeroDatum {
            station_ref: StationRef {
                section_id: 0,
                station_index: 2,
            }
            .into(),
            gps: GpsCoordinates::new(20.25, -87.5, 12.0).unwrap(),
        });

        let shots = build_export_shots(&project, TmlProfile::User, AzimuthExport::decide(&project));

        let start = shots.iter().find(|s| s.id == 0).unwrap();
        assert!((start.latitude - 20.25).abs() < f64::EPSILON);
        assert!((start.longitude - (-87.5)).abs() < f64::EPSILON);

        let shot_for_station_2 = shots.iter().find(|s| s.id == 2).unwrap();
        assert!(shot_for_station_2.latitude.abs() < f64::EPSILON);
        assert!(shot_for_station_2.longitude.abs() < f64::EPSILON);
    }

    /// The `START` lat/long has to come from the datum the survey is actually
    /// laid out against, not from `zero_datums[0]` by position.
    ///
    /// When the first datum DANGLES - its anchor key no longer resolves, which
    /// the app reaches by deleting the shot or section a datum was anchored to -
    /// every other reader of local metres falls through to the first datum that
    /// still resolves: that is `Project::frame_datum`, it is what
    /// `frame_datum_key` gates the renormalisation on, and a dangling datum is
    /// dropped from the wire entirely so the map and the data table never see it.
    /// The export used to stamp `zero_datums[0]`'s GPS onto every `START`
    /// regardless, which georeferences a file against an origin its own geometry
    /// does not use: every station displaced by the whole inter-datum vector, in
    /// the artifact the user hands to someone else, with nothing on screen
    /// saying so.
    ///
    /// Non-vacuous three ways: the two datums are about 11 km apart, so a wrong
    /// pick cannot hide inside a floating-point tolerance; the setup asserts that
    /// `zero_datums[0]` really does fail to resolve, so the fixture cannot rot
    /// into a test of nothing; and the galleries are unmerged, so BOTH entrances
    /// carry the anchor and a wrong pick shows up twice.
    #[test]
    fn the_start_gps_comes_from_the_frame_datum_not_a_dangling_first_one() {
        // Latitude/longitude of the datum that anchors nothing, and of the one
        // the survey is really framed on. Roughly 11 km apart.
        const DANGLING: (f64, f64) = (20.25, -87.5);
        const FRAME: (f64, f64) = (20.35, -87.4);

        let mut project = Project::new("Dangling Frame".to_string(), None);
        let gallery_a = project
            .add_section(SectionData {
                name: Some("Gallery A".to_string()),
                color: "#ff0000".to_string(),
                shots: vec![ShotData {
                    length: 5.0,
                    azimuth_in: 90.0,
                    ..Default::default()
                }],
                ..Default::default()
            })
            .id;
        let gallery_b = project
            .add_section(SectionData {
                name: Some("Gallery B".to_string()),
                color: "#00ff00".to_string(),
                shots: vec![ShotData {
                    length: 5.0,
                    azimuth_in: 90.0,
                    ..Default::default()
                }],
                ..Default::default()
            })
            .id;
        // No station_merges: each gallery is its own entrance and gets a START.

        // `zero_datums[0]`, anchored on a shot uuid that is not in this project.
        // Pushed straight onto the vector because no mutator will build one - and
        // that is the point: this shape is reached by a delete under a datum, not
        // by any command that asks for it.
        project.zero_datums.push(ZeroDatum {
            station_ref: StationKey::new(
                gallery_a,
                StationAnchor::AfterShot("no-such-uuid".to_string()),
            ),
            gps: GpsCoordinates::new(DANGLING.0, DANGLING.1, 0.0).unwrap(),
        });
        assert!(
            project
                .resolve_key(&project.zero_datums[0].station_ref)
                .is_none(),
            "fixture rot: zero_datums[0] must NOT resolve, or this test proves nothing"
        );

        // `zero_datums[1]`: the real datum, set through the mutator so the survey
        // is genuinely laid out against it.
        project
            .set_zero_datum(
                StationRef::new(gallery_b, 0),
                GpsCoordinates::new(FRAME.0, FRAME.1, 0.0).unwrap(),
            )
            .unwrap();
        assert_eq!(
            project.zero_datums.len(),
            2,
            "the real datum must be stored AFTER the dangling one"
        );

        let shots = build_export_shots(&project, TmlProfile::User, AzimuthExport::decide(&project));
        let starts: Vec<&ExportShot> = shots.iter().filter(|s| s.shot_type == "START").collect();
        assert_eq!(starts.len(), 2, "each detached gallery is its own entrance");
        for start in &starts {
            assert!(
                (start.latitude - FRAME.0).abs() < f64::EPSILON
                    && (start.longitude - FRAME.1).abs() < f64::EPSILON,
                "START in section {:?} exported at {},{} - the frame datum is at \
                 {},{} and the dangling zero_datums[0] at {},{}",
                start.section,
                start.latitude,
                start.longitude,
                FRAME.0,
                FRAME.1,
                DANGLING.0,
                DANGLING.1
            );
        }
    }

    /// #431 bug pin: a datum station 35 m deep must NOT export as 35 m of
    /// entrance elevation. `depth_reference` is "meters below surface"
    /// (+down); `firstStartAbsoluteElevation` is "meters above sea level"
    /// (+up) - two different fields with no relationship, and a project with
    /// no known elevation exports the documented no-data value, 0.0. Red on
    /// the pre-#431 `export/tml.rs`, which read the datum's `depth_reference`
    /// straight into this element.
    #[test]
    fn depth_reference_never_leaks_into_entrance_elevation() {
        let mut project = Project::new("Datum Depth".to_string(), None);
        project.add_section(SectionData {
            name: Some("Main".to_string()),
            color: "#ff0000".to_string(),
            shots: vec![ShotData {
                length: 3.0,
                ..Default::default()
            }],
            ..Default::default()
        });
        project.zero_datums.push(ZeroDatum {
            station_ref: StationRef {
                section_id: 0,
                station_index: 1,
            }
            .into(),
            gps: GpsCoordinates::new(20.25, -87.5, 35.0).unwrap(),
        });
        assert_eq!(project.surface_elevation, None);

        let file = NamedTempFile::with_suffix(".tmlu").unwrap();
        export_tml(&project, file.path()).unwrap();
        let content = std::fs::read_to_string(file.path()).unwrap();
        assert!(
            content.contains("<firstStartAbsoluteElevation>0.0</firstStartAbsoluteElevation>"),
            "no known surface_elevation must export as the documented no-data value 0.0: {content}"
        );
        assert!(
            !content.contains("<firstStartAbsoluteElevation>35.0</firstStartAbsoluteElevation>"),
            "the datum's depth_reference must never leak into entrance elevation: {content}"
        );
    }

    /// A surface below sea level (a below-sea-level cave entrance) survives its
    /// sign through export - not flipped positive.
    #[test]
    fn negative_surface_elevation_survives_its_sign() {
        let mut project = Project::new("Below Sea Level".to_string(), None);
        project.add_section(SectionData {
            name: Some("Main".to_string()),
            color: "#ff0000".to_string(),
            shots: vec![ShotData {
                length: 3.0,
                ..Default::default()
            }],
            ..Default::default()
        });
        project.surface_elevation = Some(-5.0);

        let file = NamedTempFile::with_suffix(".tmlu").unwrap();
        export_tml(&project, file.path()).unwrap();
        let content = std::fs::read_to_string(file.path()).unwrap();
        assert!(
            content.contains("<firstStartAbsoluteElevation>-5.0</firstStartAbsoluteElevation>"),
            "a negative surface_elevation must not be flipped positive: {content}"
        );
    }

    /// The `.tml` zip profile is a different code path from `.tmlu` (its own
    /// element-name table, `LONG` vs `SHORT`), so the elevation element must
    /// be checked there too, not just on the `.tmlu` plain-XML path above.
    #[test]
    fn surface_elevation_is_emitted_in_the_tml_zip_profile_too() {
        let mut project = Project::new("Zip Elevation".to_string(), None);
        project.add_section(SectionData {
            name: Some("Main".to_string()),
            color: "#ff0000".to_string(),
            shots: vec![ShotData {
                length: 3.0,
                ..Default::default()
            }],
            ..Default::default()
        });
        project.surface_elevation = Some(42.5);

        let file = NamedTempFile::with_suffix(".tml").unwrap();
        export_tml(&project, file.path()).unwrap();

        let zip_file = std::fs::File::open(file.path()).unwrap();
        let mut archive = zip::ZipArchive::new(zip_file).unwrap();
        let mut data_xml = String::new();
        std::io::Read::read_to_string(&mut archive.by_name("Data.xml").unwrap(), &mut data_xml)
            .unwrap();
        assert!(
            data_xml.contains("<firstStartAbsoluteElevation>42.5</firstStartAbsoluteElevation>"),
            "the .tml zip profile must emit the elevation same as .tmlu: {data_xml}"
        );
    }

    /// Every Ariane consumer derives a shot's vertical change from its PARENT's
    /// `Depth`, so the whole emitted chain has to satisfy
    /// `|Depth - parent.Depth| <= Length`. A file that breaks it is not merely
    /// odd - SpeleoDB's `openspeleo_lib` raises on the first offender and drops
    /// GeoJSON for the ENTIRE project, while the upload still returns 200, so the
    /// user is never told their survey has no map.
    fn assert_ariane_chain_is_representable(shots: &[ExportShot]) {
        let depth_by_id: HashMap<i64, f64> = shots.iter().map(|s| (s.id, s.depth)).collect();
        for shot in shots {
            if shot.from_id == -1 {
                continue;
            }
            let Some(&parent_depth) = depth_by_id.get(&shot.from_id) else {
                continue;
            };
            let delta = (shot.depth - parent_depth).abs();
            assert!(
                delta <= shot.length,
                "shot id={} (from {}) drops {delta} over a length of {} - impossible geometry",
                shot.id,
                shot.from_id,
                shot.length,
            );
            // The bound above is exactly what the repair enforces, so on its own it
            // certifies whatever the repair produced. Check the file's OTHER depth
            // axis too: a consumer honouring `DepthIn` (ours does) must read the
            // same shot as one walking the parent chain.
            let emitted_in = if shot.depth_in_same_as_prev {
                parent_depth
            } else {
                shot.depth_in
            };
            assert!(
                (shot.depth - emitted_in).abs() <= shot.length,
                "shot id={} drops {} from its own DepthIn over a length of {}",
                shot.id,
                (shot.depth - emitted_in).abs(),
                shot.length,
            );
            // And the change a consumer READS has to be the change the shot
            // RECORDS - same size, same direction. The two bounds above are both
            // satisfied by a shot that has been moved without its file-visible
            // origin moving with it, which is how a re-hung branch once exported a
            // measured descent as a several-metre ascent and still passed here.
            // `depth_in` is the truth throughout: the repair keeps it in step with
            // whatever it writes as the origin.
            let read_change = shot.depth - emitted_in;
            let recorded_change = shot.depth - shot.depth_in;
            assert!(
                (read_change - recorded_change).abs() < 1e-6,
                "shot id={} is read as a {read_change} m vertical change but records \
                 {recorded_change} m",
                shot.id,
            );
            assert!(
                shot.depth.is_finite() && shot.length.is_finite() && shot.azimuth.is_finite(),
                "shot id={} carries a non-finite value",
                shot.id,
            );
        }
    }

    /// Round-trip a REAL `.tml` through the importer and back out, then assert the
    /// emitted chain is representable. Env-gated in the same spirit as the SpeleoDB
    /// live tests: it needs a file we cannot commit (someone's survey), so a run
    /// without `TML_ROUNDTRIP_IN` fails naming it (#576 E13).
    ///
    /// ```text
    /// TML_ROUNDTRIP_IN=/path/in.tml TML_ROUNDTRIP_OUT=/path/out.tml \
    ///   cargo test --features telemetry reexport -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "needs a real .tml via TML_ROUNDTRIP_IN"]
    #[expect(
        clippy::print_stdout,
        reason = "a hand-run diagnostic tool; the shot count and output path are its result"
    )]
    fn reexport_of_a_real_tml_keeps_the_chain_representable() {
        let input = require_env_file("TML_ROUNDTRIP_IN");
        let tml = crate::parsers::tml::parse_tml(&input).unwrap();
        let (sections, _closures, _warnings) =
            crate::parsers::tml::convert_tml_to_import_configs(&tml, false, true).unwrap();

        // Straight into `SectionData` rather than through the import command: the
        // shot chain is what this asserts on, and the import epilogue (placement,
        // closures, merges) needs an `AppState` that adds nothing here.
        let mut project = Project::new("Roundtrip".to_string(), None);
        for section in sections {
            project.add_section(SectionData {
                name: section.name,
                color: section.color,
                shots: section.shots,
                surveyor: section.surveyor,
                date: section.date,
                direction: section.direction,
                quality: section.quality,
            });
        }

        // Optional anchor, so the re-exported file can be taken all the way to
        // GeoJSON by a consumer (which needs one) rather than stopping at the
        // by-design no-anchor skip.
        if std::env::var("TML_ROUNDTRIP_ANCHOR").is_ok() {
            project.zero_datums.push(ZeroDatum {
                station_ref: StationRef {
                    section_id: 0,
                    station_index: 0,
                }
                .into(),
                gps: GpsCoordinates::new(20.25, -87.5, 0.0).unwrap(),
            });
        }

        let shots = build_export_shots(&project, TmlProfile::User, AzimuthExport::decide(&project));
        println!("re-exported {} shots from {}", shots.len(), input.display());
        assert_ariane_chain_is_representable(&shots);

        if let Ok(out) = std::env::var("TML_ROUNDTRIP_OUT") {
            export_tml(&project, Path::new(&out)).unwrap();
            println!("wrote {out}");
        }
    }

    /// Companion to `reexport_of_a_real_tml_keeps_the_chain_representable`, from the
    /// project side: exports a real `.stalx` and checks Ariane can root every shot.
    /// A parentless non-START shot plus any closure blanks Ariane's map entirely, and
    /// the shape that produces one needs a real merge topology (a station shared by
    /// three sections) that is tedious to hand-build. Fails, naming the var, when it
    /// is unset (#576 E13).
    ///
    /// ```text
    /// STALX_EXPORT_IN=/path/project.stalx \
    ///   cargo test --features telemetry export_of_a_real_stalx -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "needs a real .stalx via STALX_EXPORT_IN"]
    #[expect(
        clippy::print_stdout,
        reason = "a hand-run diagnostic tool; the orphan list is its result"
    )]
    fn export_of_a_real_stalx_roots_every_shot() {
        let input = require_env_file("STALX_EXPORT_IN");
        let db =
            stalagmight_persistence::persistence::sqlite::Database::open_readonly(&input).unwrap();
        let project = db.load_project().unwrap();

        let shots = build_export_shots(&project, TmlProfile::User, AzimuthExport::decide(&project));
        let orphans: Vec<&ExportShot> = shots
            .iter()
            .filter(|s| s.from_id == -1 && s.shot_type != "START")
            .collect();
        for o in &orphans {
            println!(
                "  ORPHAN id={} type={} section={:?}",
                o.id, o.shot_type, o.section
            );
        }
        println!(
            "{} shots, {} orphan(s) from {}",
            shots.len(),
            orphans.len(),
            input.display()
        );

        assert!(
            orphans.is_empty(),
            "{} parentless non-START shot(s) would blank Ariane's map",
            orphans.len()
        );

        if let Ok(out) = std::env::var("STALX_EXPORT_OUT") {
            export_tml(&project, Path::new(&out)).unwrap();
            println!("wrote {out}");
        }
    }

    #[test]
    fn closure_shots_carry_the_depths_of_the_stations_they_tie() {
        // They used to be written with `depth: 0.0` regardless of where the
        // stations actually are, which reads as a connector with a full-depth
        // drop and no length.
        let mut project = Project::new("Closure Depths".to_string(), None);
        project.add_section(SectionData {
            name: Some("Main".to_string()),
            color: "#ff0000".to_string(),
            shots: vec![
                ShotData {
                    azimuth_in: 0.0,
                    azimuth_out: 0.0,
                    length: 10.0,
                    depth_in: 0.0,
                    depth_out: 5.0,
                    ..Default::default()
                },
                ShotData {
                    azimuth_in: 90.0,
                    azimuth_out: 90.0,
                    length: 10.0,
                    depth_in: 5.0,
                    depth_out: 9.0,
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
        project.loop_closures.push(LoopClosure {
            id: 1,
            station_a: StationRef {
                section_id: 0,
                station_index: 1,
            }
            .into(),
            station_b: StationRef {
                section_id: 0,
                station_index: 2,
            }
            .into(),
            path_length: 20.0,
            misclosure_distance: 0.0,
            error_percentage: 0.0,
            horizontal_misclosure: 0.0,
            vertical_misclosure: 4.0,
            strategy: ClosureStrategy::Batch,
            is_aligned: false,
        });

        let shots = build_export_shots(&project, TmlProfile::User, AzimuthExport::decide(&project));
        let closure = shots.iter().find(|s| s.shot_type == "CLOSURE").unwrap();
        assert!(
            (closure.depth - 9.0).abs() < 1e-9,
            "the connector sits at the depth of the station it closes onto, got {}",
            closure.depth
        );
        assert!(
            (closure.depth_in - 5.0).abs() < 1e-9,
            "and starts at the depth of the station it leaves, got {}",
            closure.depth_in
        );
        assert!(
            closure.length >= (closure.depth - closure.depth_in).abs(),
            "its length must cover its own vertical change"
        );
        assert_ariane_chain_is_representable(&shots);
    }

    #[test]
    fn a_junction_depth_discontinuity_is_never_exported_impossible() {
        // A branch that starts at a different depth than the station it hangs off
        // is representable in our model and is NOT representable in Ariane's
        // chain. Exported verbatim it reads as a shot with more drop than length.
        let mut project = Project::new("Junction".to_string(), None);
        project.add_section(SectionData {
            name: Some("Parent".to_string()),
            color: "#ff0000".to_string(),
            shots: vec![ShotData {
                azimuth_in: 0.0,
                azimuth_out: 0.0,
                length: 20.0,
                depth_in: 0.0,
                depth_out: 17.0,
                ..Default::default()
            }],
            ..Default::default()
        });
        // The child starts 12.2 m down while the station it merges into is 17 m
        // down: a 4.8 m step across a 1.07 m shot.
        project.add_section(SectionData {
            name: Some("Branch".to_string()),
            color: "#00ff00".to_string(),
            shots: vec![ShotData {
                azimuth_in: 45.0,
                azimuth_out: 45.0,
                length: 1.07,
                depth_in: 12.21,
                depth_out: 12.5,
                ..Default::default()
            }],
            ..Default::default()
        });
        project.station_merges.push(StationMerge {
            station_a: StationRef {
                section_id: 0,
                station_index: 1,
            }
            .into(),
            station_b: StationRef {
                section_id: 1,
                station_index: 0,
            }
            .into(),
            merged_at: chrono::Utc::now(),
        });

        let shots = build_export_shots(&project, TmlProfile::User, AzimuthExport::decide(&project));
        assert_ariane_chain_is_representable(&shots);
    }

    #[test]
    fn a_shot_hung_off_a_station_it_was_not_measured_from_keeps_its_true_depth() {
        // [#576 TML-DEPTH-D1] The shot starts 4.79 m off the station it hangs
        // off. It used to be re-hung there with its measured 0.29 m descent,
        // moving its own station by the whole mismatch. Its depth is a true
        // reading, so it stays; the shot takes the length that keeps its
        // measured horizontal run over the change the file now states.
        let mut shots = vec![
            geometry_shot(1, -1, 17.0, 17.0, 0.0, false),
            geometry_shot(2, 1, 12.21, 12.5, 1.07, true),
        ];

        enforce_ariane_geometry_invariants(&mut shots, TmlProfile::User);

        assert!(
            (shots[1].depth - 12.5).abs() < 1e-12,
            "got {}",
            shots[1].depth
        );
        assert!((shots[1].depth_in - 17.0).abs() < 1e-12);
        let run = (1.07_f64 * 1.07 - 0.29 * 0.29).sqrt();
        let want = run.hypot(12.5 - 17.0);
        assert!(
            shots[1].length >= want && shots[1].length <= want + 1e-6,
            "the length keeps the {run} m run: expected {want}, got {}",
            shots[1].length
        );
        assert_ariane_chain_is_representable(&shots);
    }

    /// A shot carrying only the fields the geometry pass reads, so a chain of
    /// them stays legible. Everything else is filler.
    fn geometry_shot(
        id: i64,
        from_id: i64,
        depth_in: f64,
        depth: f64,
        length: f64,
        depth_in_same_as_prev: bool,
    ) -> ExportShot {
        ExportShot {
            uuid: None,
            id,
            from_id,
            shot_type: if from_id == -1 { "START" } else { "REAL" },
            section: "S".to_string(),
            azimuth: 0.0,
            length,
            depth,
            depth_in,
            depth_in_same_as_prev,
            branch_of: None,
            reversed: false,
            inclination: 0.0,
            left: 0.0,
            right: 0.0,
            up: 0.0,
            down: 0.0,
            comment: None,
            explorer: None,
            date: None,
            color: "0xff0000ff".to_string(),
            latitude: 0.0,
            longitude: 0.0,
            closure_to_id: -1,
            locked: false,
        }
    }

    #[test]
    fn the_shot_after_a_moved_start_keeps_its_true_depth_and_tape() {
        // The re-hang carried its offset down the rest of the branch: the shot
        // after it was moved by the same 4.79 m. Under D1 nothing below the
        // junction moves - the shot after it states its own absolute depth.
        let mut shots = vec![
            geometry_shot(1, -1, 17.0, 17.0, 0.0, false),
            geometry_shot(2, 1, 12.21, 12.5, 1.07, true),
            geometry_shot(3, 2, 12.5, 13.0, 5.0, true),
        ];

        enforce_ariane_geometry_invariants(&mut shots, TmlProfile::User);

        assert!(
            (shots[1].depth - 12.5).abs() < 1e-12,
            "got {}",
            shots[1].depth
        );
        assert!(
            (shots[2].depth - 13.0).abs() < 1e-12,
            "got {}",
            shots[2].depth
        );
        assert!((shots[2].depth_in - 12.5).abs() < 1e-12);
        assert!(
            (shots[2].length - 5.0).abs() < 1e-12,
            "got {}",
            shots[2].length
        );
        assert_ariane_chain_is_representable(&shots);
    }

    #[test]
    fn a_branch_start_averages_its_junction_even_within_the_bound() {
        // A branch whose tape covers the gap used to pass every check while
        // stating a vertical change nobody surveyed. Now its start is one of
        // the junction's readings: the junction is exported at their mean.
        let mut shots = vec![
            geometry_shot(1, -1, 17.0, 17.0, 0.0, false),
            geometry_shot(2, 1, 12.21, 12.5, 10.0, true),
        ];
        shots[1].branch_of = Some(1);

        let junctions = enforce_ariane_geometry_invariants(&mut shots, TmlProfile::User);

        let mean = f64::midpoint(17.0, 12.21);
        assert!(
            (shots[0].depth - mean).abs() < 1e-12,
            "got {}",
            shots[0].depth
        );
        assert!(
            (shots[0].depth_in - mean).abs() < 1e-12,
            "a START stands on its station"
        );
        assert!(
            (shots[1].depth - 12.5).abs() < 1e-12,
            "got {}",
            shots[1].depth
        );
        assert_eq!(junctions.averaged, 1);
        assert!((junctions.largest_spread - 4.79).abs() < 1e-9);
        assert_ariane_chain_is_representable(&shots);
    }

    #[test]
    fn a_shot_exactly_on_the_bound_is_left_alone_and_one_past_it_goes_out_vertical() {
        // `|Depth - parent.Depth| == Length` is a legal, perfectly vertical shot,
        // not a violation - moving it would invent a horizontal component. One
        // ten-thousandth over is broken data whichever way it is read
        // (TML-DEPTH-Q2-D1): both its depths stay true and it goes out as the
        // vertical shot it can at most be, its drop as its length. No station
        // moves, so the shot after it keeps its start and its tape.
        let mut shots = vec![
            geometry_shot(1, -1, 0.0, 10.0, 0.0, false),
            geometry_shot(2, 1, 10.0, 13.0, 3.0, true),
            geometry_shot(3, 2, 13.0, 16.0001, 3.0, true),
            geometry_shot(4, 3, 16.0001, 17.0001, 2.0, true),
        ];

        enforce_ariane_geometry_invariants(&mut shots, TmlProfile::User);

        assert!(
            (shots[1].depth - 13.0).abs() < 1e-9,
            "the exact-bound shot is untouched, got {}",
            shots[1].depth
        );
        assert!(
            (shots[2].depth - 16.0001).abs() < 1e-9,
            "the over-bound shot keeps its true end depth, got {}",
            shots[2].depth
        );
        assert!(
            shots[2].length >= 3.0001 && shots[2].length <= 3.0001 + 2e-6,
            "the over-bound shot's length is its drop, 3.0001 m, got {}",
            shots[2].length
        );
        assert!(shots[2].depth_in_same_as_prev, "it starts at its parent");
        assert!(
            (shots[3].depth - 17.0001).abs() < 1e-9 && (shots[3].length - 2.0).abs() < 1e-12,
            "the shot after it is untouched, got depth {} length {}",
            shots[3].depth,
            shots[3].length
        );
        assert_ariane_chain_is_representable(&shots);
    }

    #[test]
    fn a_from_id_cycle_keeps_its_measured_lengths() {
        // A cycle is unreachable from the root, so the walk never repairs it - and
        // the as-written sweep must not "fix" it either: the gap to its parent is
        // the full unrepaired one, and writing that into Length would swap a
        // measured tape reading for a fabricated one. A consumer reaches shots the
        // same way we do, from the anchor down, so it drops these before it ever
        // looks at their geometry.
        let mut shots = vec![
            geometry_shot(1, -1, 0.0, 0.0, 0.0, false),
            geometry_shot(2, 3, 0.0, 40.0, 1.0, false),
            geometry_shot(3, 2, 40.0, 0.0, 1.0, false),
        ];

        enforce_ariane_geometry_invariants(&mut shots, TmlProfile::User);

        assert!(
            (shots[1].length - 1.0).abs() < f64::EPSILON
                && (shots[2].length - 1.0).abs() < f64::EPSILON,
            "the cycle's tape readings survive, got {} and {}",
            shots[1].length,
            shots[2].length
        );
    }

    #[test]
    fn a_depth_in_that_would_be_written_as_the_sentinel_is_nudged_off_it() {
        // The guard has to judge the value AS WRITTEN. `format_f64` rounds to six
        // decimals, so a station a hair under a metre above the datum lands in the
        // file as "-1.0" and comes back through our own parser as "starts where
        // the previous shot ended" - relocating it to wherever that was.
        for depth_in in [-1.0, -0.999_999_6, -1.000_000_4] {
            let shot = geometry_shot(2, 1, depth_in, depth_in, 1.0, false);
            let written = format_f64(emitted_depth_in(&shot));
            assert_ne!(
                written, "-1.0",
                "depth_in {depth_in} must not be written as the sentinel"
            );
            assert!(
                (emitted_depth_in(&shot) - depth_in).abs() < 1e-5,
                "and the nudge stays inside a rounding error of the truth, got {written}"
            );
        }
    }

    #[test]
    fn non_finite_geometry_never_reaches_the_file() {
        // NaN fails every comparison, including the one a consumer uses to decide
        // whether a shot is possible, so it aborts the same conversion.
        let mut shots = vec![
            ExportShot {
                uuid: None,
                id: 1,
                from_id: -1,
                shot_type: "START",
                section: "S".to_string(),
                azimuth: 0.0,
                length: 0.0,
                depth: 0.0,
                depth_in: 0.0,
                depth_in_same_as_prev: false,
                branch_of: None,
                reversed: false,
                inclination: 0.0,
                left: 0.0,
                right: 0.0,
                up: 0.0,
                down: 0.0,
                comment: None,
                explorer: None,
                date: None,
                color: "0xff0000ff".to_string(),
                latitude: 0.0,
                longitude: 0.0,
                closure_to_id: -1,
                locked: false,
            },
            ExportShot {
                uuid: None,
                id: 2,
                from_id: 1,
                shot_type: "REAL",
                section: "S".to_string(),
                azimuth: f64::NAN,
                length: f64::INFINITY,
                depth: f64::NAN,
                depth_in: -1.0,
                depth_in_same_as_prev: true,
                branch_of: None,
                reversed: false,
                inclination: f64::NEG_INFINITY,
                left: 0.0,
                right: 0.0,
                up: 0.0,
                down: 0.0,
                comment: None,
                explorer: None,
                date: None,
                color: "0xff0000ff".to_string(),
                latitude: 0.0,
                longitude: 0.0,
                closure_to_id: -1,
                locked: false,
            },
        ];

        enforce_ariane_geometry_invariants(&mut shots, TmlProfile::User);

        assert_ariane_chain_is_representable(&shots);
    }

    #[test]
    fn test_nonzero_root_datum_splits_section_into_two_branches() {
        let mut project = Project::new("Split Root".to_string(), None);
        project.add_section(SectionData {
            name: Some("Main".to_string()),
            color: "#ff0000".to_string(),
            shots: vec![
                ShotData {
                    azimuth_in: 10.0,
                    azimuth_out: 210.0,
                    length: 1.0,
                    depth_in: 0.0,
                    depth_out: 1.0,
                    ..Default::default()
                },
                ShotData {
                    azimuth_in: 20.0,
                    azimuth_out: 15.0,
                    length: 1.0,
                    depth_in: 1.0,
                    depth_out: 2.0,
                    ..Default::default()
                },
                ShotData {
                    azimuth_in: 30.0,
                    azimuth_out: 230.0,
                    length: 1.0,
                    depth_in: 2.0,
                    depth_out: 3.0,
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
        project.zero_datums.push(ZeroDatum {
            station_ref: StationRef {
                section_id: 0,
                station_index: 2,
            }
            .into(),
            gps: GpsCoordinates::new(20.0, -87.0, 0.0).unwrap(),
        });

        let shots = build_export_shots(&project, TmlProfile::User, AzimuthExport::decide(&project));
        let start = shots.iter().find(|s| s.shot_type == "START").unwrap();

        // Split produces two first shots from START: forward segment and reversed prefix.
        let from_start: Vec<&ExportShot> = shots
            .iter()
            .filter(|s| s.shot_type != "START" && s.from_id == start.id)
            .collect();
        assert_eq!(from_start.len(), 2);
        // Both legs carry the bearing the app DRAWS, the circular mean of the
        // two forward readings (#576 NF-AN5-1): 30/230 is drawn at 310, and
        // `azimuth_out` is never read as a backsight.
        assert!(
            from_start.iter().any(|s| (s.azimuth - 310.0).abs() < 1e-9),
            "forward branch should carry the drawn bearing at the datum: {from_start:?}"
        );
        assert!(
            from_start.iter().any(|s| (s.azimuth - 197.5).abs() < 1e-9),
            "reverse branch should invert the drawn bearing (mean of 20/15) of the prefix shot"
        );
    }

    #[test]
    fn test_parent_links_reoriented_when_datum_forces_root() {
        let mut project = Project::new("Datum Root Orientation".to_string(), None);
        project.add_section(SectionData {
            name: Some("A".to_string()),
            color: "#ff0000".to_string(),
            shots: vec![
                ShotData {
                    length: 2.0,
                    ..Default::default()
                },
                ShotData {
                    length: 2.0,
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
        project.add_section(SectionData {
            name: Some("B".to_string()),
            color: "#00ff00".to_string(),
            shots: vec![
                ShotData {
                    length: 2.0,
                    ..Default::default()
                },
                ShotData {
                    length: 2.0,
                    ..Default::default()
                },
                ShotData {
                    length: 2.0,
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
        project.station_merges.push(StationMerge {
            station_a: StationRef {
                section_id: 0,
                station_index: 1,
            }
            .into(),
            station_b: StationRef {
                section_id: 1,
                station_index: 2,
            }
            .into(),
            merged_at: chrono::Utc::now(),
        });
        project.zero_datums.push(ZeroDatum {
            station_ref: StationRef {
                section_id: 1,
                station_index: 2,
            }
            .into(),
            gps: GpsCoordinates::new(20.0, -87.0, 0.0).unwrap(),
        });

        let shots = build_export_shots(&project, TmlProfile::User, AzimuthExport::decide(&project));
        let start = shots.iter().find(|s| s.shot_type == "START").unwrap();
        assert_eq!(start.section, "B");

        // Section A should now link into the forced-root section B.
        let link_into_root = shots
            .iter()
            .find(|s| s.section == "A" && s.from_id == start.id);
        assert!(
            link_into_root.is_some(),
            "expected a section A shot to link to B START after rooting at datum"
        );
    }

    #[test]
    fn test_round_trip_tmlu() {
        let project = simple_project();
        let file = NamedTempFile::with_suffix(".tmlu").unwrap();
        export_tml(&project, file.path()).unwrap();

        // Re-import
        let parsed = crate::parsers::tml::parse_tml(file.path()).unwrap();

        assert_eq!(parsed.cave_name, "Test Cave");
        assert_eq!(parsed.unit, crate::parsers::tml::TmlUnit::Metric);
        assert!(parsed.use_magnetic_azimuth);

        // Should have one section with 2 shots
        assert_eq!(
            parsed.sections.len(),
            1,
            "expected 1 section, got {}",
            parsed.sections.len()
        );
        let sec = &parsed.sections[0];
        assert_eq!(
            sec.shots.len(),
            2,
            "expected 2 shots, got {}",
            sec.shots.len()
        );

        // Verify shot data round-trips
        let s0 = &sec.shots[0];
        assert!((s0.azimuth - 45.0).abs() < 0.01);
        assert!((s0.length - 10.0).abs() < 0.01);

        let s1 = &sec.shots[1];
        assert!((s1.azimuth - 90.0).abs() < 0.01);
        assert!((s1.length - 8.0).abs() < 0.01);
    }

    #[test]
    fn test_round_trip_tml_zip() {
        let project = simple_project();
        let file = NamedTempFile::with_suffix(".tml").unwrap();
        export_tml(&project, file.path()).unwrap();

        // Re-import
        let parsed = crate::parsers::tml::parse_tml(file.path()).unwrap();

        assert_eq!(parsed.cave_name, "Test Cave");
        assert_eq!(parsed.sections.len(), 1);
        assert_eq!(parsed.sections[0].shots.len(), 2);
    }

    #[test]
    fn test_unicode_section_name() {
        let mut project = Project::new("Grotte de l'Ours".to_string(), None);
        project.add_section(SectionData {
            name: Some("Galería Principal".to_string()),
            surveyor: Some("José García".to_string()),
            color: "#ff0000".to_string(),
            shots: vec![ShotData {
                length: 5.0,
                ..Default::default()
            }],
            ..Default::default()
        });

        let file = NamedTempFile::with_suffix(".tmlu").unwrap();
        export_tml(&project, file.path()).unwrap();

        let content = std::fs::read_to_string(file.path()).unwrap();
        // quick_xml escapes apostrophes to &apos;
        assert!(content.contains("Grotte de l") && content.contains("Ours"));
        assert!(content.contains("Galería Principal"));
        assert!(content.contains("José García"));
    }

    #[test]
    fn test_depth_in_sentinel() {
        // When depth_in equals the previous depth_out, export should use -1.0
        let mut project = Project::new("Depth".to_string(), None);
        project.add_section(SectionData {
            name: Some("Sec".to_string()),
            color: "#ff0000".to_string(),
            shots: vec![
                ShotData {
                    depth_in: 0.0,
                    depth_out: 5.0,
                    length: 10.0,
                    ..Default::default()
                },
                ShotData {
                    depth_in: 5.0, // equals previous depth_out → should be -1.0
                    depth_out: 8.0,
                    length: 7.0,
                    ..Default::default()
                },
                ShotData {
                    depth_in: 6.0, // different from previous depth_out (8.0) → should be 6.0
                    depth_out: 10.0,
                    length: 5.0,
                    ..Default::default()
                },
            ],
            ..Default::default()
        });

        let shots = build_export_shots(&project, TmlProfile::User, AzimuthExport::decide(&project));
        // shots[0] = START, shots[1..3] = REAL
        // `depth_in` now always carries the TRUE start depth and the sentinel rides
        // in its own flag, so assert on what actually reaches the file.
        assert!(
            (emitted_depth_in(&shots[1]) - (-1.0)).abs() < 1e-9,
            "first shot writes the sentinel (same as start)"
        );
        assert!(
            (emitted_depth_in(&shots[2]) - (-1.0)).abs() < 1e-9,
            "second shot writes the sentinel (same as prev depth_out)"
        );
        assert!(
            (emitted_depth_in(&shots[3]) - 6.0).abs() < 1e-9,
            "third shot writes 6.0 (differs from prev depth_out)"
        );
        assert!(
            (shots[2].depth_in - 5.0).abs() < 1e-9,
            "and the true start depth is kept alongside it, so a repair can read \
             the measured vertical change back"
        );
    }

    #[test]
    fn test_id_element_is_all_caps() {
        let project = simple_project();
        let file = NamedTempFile::with_suffix(".tml").unwrap();
        export_tml(&project, file.path()).unwrap();

        let zip_file = std::fs::File::open(file.path()).unwrap();
        let mut archive = zip::ZipArchive::new(zip_file).unwrap();
        let mut data_xml = String::new();
        std::io::Read::read_to_string(&mut archive.by_name("Data.xml").unwrap(), &mut data_xml)
            .unwrap();

        // Must be <ID>, never <Id>
        assert!(data_xml.contains("<ID>"), "ID element must be all-caps");
        assert!(!data_xml.contains("<Id>"), "must not use <Id> casing");
    }

    #[test]
    fn test_required_fields_present() {
        let project = simple_project();
        let file = NamedTempFile::with_suffix(".tmlu").unwrap();
        export_tml(&project, file.path()).unwrap();

        let content = std::fs::read_to_string(file.path()).unwrap();
        // Every shot must have Locked, Name, and Profiletype (short names: LCK, NM, PT)
        assert!(content.contains("<LCK>"));
        assert!(content.contains("<NM>"));
        assert!(content.contains("<PT>VERTICAL</PT>"));
    }

    /// A child whose parent station is ANOTHER section's merge station must still be
    /// rooted, whatever order the sections happen to be stored in.
    ///
    /// Station 0 of a non-root section only enters `station_to_ariane` when that
    /// section itself resolves, so resolving blocks in storage order left a child
    /// stored earlier pointing at nothing and it shipped with `from_id = -1`. Ariane
    /// reads that as a second entrance, abandons the loop solve, and draws an empty
    /// map for any survey that also has closures - which is exactly what happened to
    /// a real 538-shot Naharon export (section `Trifurcate  North`, whose parent was
    /// station 1 of `Trifurcate South`, a station shared by three sections).
    #[test]
    fn a_child_of_a_merge_station_is_rooted_regardless_of_section_order() {
        let mut project = Project::new("Out Of Order".to_string(), None);

        let shot = || ShotData {
            length: 5.0,
            azimuth_in: 90.0,
            ..Default::default()
        };

        // Deliberately added BEFORE the section it hangs off: Root(0), Branch(1), Middle(2).
        for name in ["Root", "Branch", "Middle"] {
            project.add_section(SectionData {
                name: Some(name.to_string()),
                color: "#ff0000".to_string(),
                shots: vec![shot(), shot()],
                ..Default::default()
            });
        }

        let at = |section_id, station_index| {
            StationRef {
                section_id,
                station_index,
            }
            .into()
        };

        // Middle hangs off Root, and Branch hangs off Middle's station 0 - the same
        // station, so three sections meet there just as they do in the real survey.
        project.station_merges.push(StationMerge {
            station_a: at(0, 1),
            station_b: at(2, 0),
            merged_at: chrono::Utc::now(),
        });
        project.station_merges.push(StationMerge {
            station_a: at(2, 0),
            station_b: at(1, 0),
            merged_at: chrono::Utc::now(),
        });

        let shots = build_export_shots(&project, TmlProfile::User, AzimuthExport::decide(&project));
        let orphans: Vec<&ExportShot> = shots
            .iter()
            .filter(|s| s.from_id == -1 && s.shot_type != "START")
            .collect();

        assert!(
            orphans.is_empty(),
            "parentless non-START shot(s) {:?} - Ariane would draw an empty map",
            orphans
                .iter()
                .map(|s| (s.id, s.shot_type, s.section.as_str()))
                .collect::<Vec<_>>()
        );

        // And the guard agrees, so a regression cannot slip out as a written file.
        assert!(ensure_rooted_chain(&shots, TmlProfile::User).is_ok());
    }

    /// Two galleries with no merge between them is an ordinary project, not an error.
    /// Each is an entrance and gets its own START, which is how Ariane stores them
    /// (a file Ariane writes for two entrances carries two). Before this, the second
    /// gallery's first shot was left parentless - harmless until the survey gained a
    /// closure, at which point Ariane drew nothing at all.
    #[test]
    fn a_detached_section_exports_as_its_own_entrance() {
        let mut project = Project::new("Detached".to_string(), None);
        for name in ["Gallery A", "Gallery B"] {
            project.add_section(SectionData {
                name: Some(name.to_string()),
                color: "#ff0000".to_string(),
                shots: vec![ShotData {
                    length: 5.0,
                    azimuth_in: 90.0,
                    ..Default::default()
                }],
                ..Default::default()
            });
        }
        // No station_merges at all.

        project.zero_datums.push(ZeroDatum {
            station_ref: StationRef {
                section_id: 0,
                station_index: 0,
            }
            .into(),
            gps: GpsCoordinates::new(20.25, -87.5, 0.0).unwrap(),
        });

        let shots = build_export_shots(&project, TmlProfile::User, AzimuthExport::decide(&project));
        let starts: Vec<&ExportShot> = shots.iter().filter(|s| s.shot_type == "START").collect();
        assert_eq!(starts.len(), 2, "each detached gallery is its own entrance");

        // Every entrance carries the anchor. Leaving the second at 0,0 puts that
        // gallery on null island, ~9000 km from the rest of the survey.
        for start in &starts {
            assert!(
                (start.latitude - 20.25).abs() < f64::EPSILON
                    && (start.longitude - -87.5).abs() < f64::EPSILON,
                "entrance in section {:?} was left unanchored at {},{}",
                start.section,
                start.latitude,
                start.longitude
            );
        }
        assert!(
            shots
                .iter()
                .all(|s| s.from_id != -1 || s.shot_type == "START"),
            "no shot may be left parentless without being an entrance"
        );
        assert!(ensure_rooted_chain(&shots, TmlProfile::User).is_ok());
    }

    /// A gallery merged to itself but not to the rest of the survey is ONE entrance, not
    /// one per section. `build_parent_map` used to traverse only from the export root, so
    /// every section in a second component was absent from the map and read as its own
    /// entrance: the gallery exported as N disconnected STARTs stacked on one anchor,
    /// with the merges between its sections silently dropped.
    #[test]
    fn a_disconnected_gallery_is_rooted_once_not_per_section() {
        let mut project = Project::new("Two Galleries".to_string(), None);

        let shot = || ShotData {
            length: 5.0,
            azimuth_in: 90.0,
            ..Default::default()
        };

        // Root component: A. Second component: B -> C -> D, chained to each other only.
        for name in ["A", "B", "C", "D"] {
            project.add_section(SectionData {
                name: Some(name.to_string()),
                color: "#ff0000".to_string(),
                shots: vec![shot(), shot()],
                ..Default::default()
            });
        }

        let at = |section_id, station_index| {
            StationRef {
                section_id,
                station_index,
            }
            .into()
        };
        // B(1)-C(0) and C(1)-D(0): a three-section gallery touching nothing in A.
        project.station_merges.push(StationMerge {
            station_a: at(1, 1),
            station_b: at(2, 0),
            merged_at: chrono::Utc::now(),
        });
        project.station_merges.push(StationMerge {
            station_a: at(2, 1),
            station_b: at(3, 0),
            merged_at: chrono::Utc::now(),
        });

        let shots = build_export_shots(&project, TmlProfile::User, AzimuthExport::decide(&project));
        let starts: Vec<&ExportShot> = shots.iter().filter(|s| s.shot_type == "START").collect();

        assert_eq!(
            starts.len(),
            2,
            "one entrance per component, got {:?}",
            starts
                .iter()
                .map(|s| s.section.as_str())
                .collect::<Vec<_>>()
        );
        assert!(
            ensure_rooted_chain(&shots, TmlProfile::User).is_ok(),
            "the second component must be rooted, not orphaned"
        );
    }

    /// The guard is the backstop for a chain the builder should never produce. It must
    /// fire on the orphan+closure COMBINATION only - an orphan alone still draws, so
    /// refusing it would cost a user an export for nothing - and it must stay
    /// Ariane-only: the SpeleoDB sidecar pads a lone START with a parentless VIRTUAL
    /// shot on purpose.
    #[test]
    fn the_guard_rejects_an_orphan_only_when_a_closure_is_present() {
        let mut project = Project::new("Guard".to_string(), None);
        project.add_section(SectionData {
            name: Some("Only".to_string()),
            color: "#ff0000".to_string(),
            shots: vec![
                ShotData {
                    length: 5.0,
                    azimuth_in: 90.0,
                    ..Default::default()
                },
                ShotData {
                    length: 5.0,
                    azimuth_in: 90.0,
                    ..Default::default()
                },
            ],
            ..Default::default()
        });

        let mut shots =
            build_export_shots(&project, TmlProfile::User, AzimuthExport::decide(&project));
        assert!(ensure_rooted_chain(&shots, TmlProfile::User).is_ok());

        // Force the exact shape the fix prevents.
        let last = shots.last_mut().unwrap();
        last.from_id = -1;
        assert_ne!(last.shot_type, "START");

        // An orphan ALONE is survivable, and such files exported and drew correctly
        // before. Refusing here would turn a drawable file into no file at all.
        assert!(
            ensure_rooted_chain(&shots, TmlProfile::User).is_ok(),
            "an orphan with no closure must still export - Ariane draws it"
        );

        // Turn a DIFFERENT shot into a closure - the same chain is now the blank-map
        // combination, and only the pairing changed.
        let orphan_idx = shots.len() - 1;
        shots[orphan_idx - 1].shot_type = "CLOSURE";
        assert_eq!(
            shots[orphan_idx].from_id, -1,
            "the orphan is still the orphan"
        );

        assert!(
            ensure_rooted_chain(&shots, TmlProfile::User).is_err(),
            "an orphan alongside a closure must not reach a written Ariane file"
        );
        assert!(
            ensure_rooted_chain(&shots, TmlProfile::SpeleoDbSidecar).is_ok(),
            "the sidecar's deliberate VIRTUAL pad must still be allowed"
        );
    }

    #[test]
    fn test_child_nonzero_merge_splits_section() {
        let mut project = Project::new("Split Child".to_string(), None);

        // Root: 2 shots → stations 0,1,2
        project.add_section(SectionData {
            name: Some("Root".to_string()),
            color: "#ff0000".to_string(),
            shots: vec![
                ShotData {
                    length: 5.0,
                    azimuth_in: 90.0,
                    azimuth_out: 90.0,
                    ..Default::default()
                },
                ShotData {
                    length: 5.0,
                    azimuth_in: 90.0,
                    azimuth_out: 90.0,
                    ..Default::default()
                },
            ],
            ..Default::default()
        });

        // Child: 4 shots → stations 0,1,2,3,4. Merge at station 2.
        project.add_section(SectionData {
            name: Some("Branch".to_string()),
            color: "#00ff00".to_string(),
            shots: vec![
                ShotData {
                    length: 3.0,
                    azimuth_in: 10.0,
                    azimuth_out: 10.0,
                    ..Default::default()
                },
                ShotData {
                    length: 4.0,
                    azimuth_in: 20.0,
                    azimuth_out: 20.0,
                    ..Default::default()
                },
                ShotData {
                    length: 5.0,
                    azimuth_in: 30.0,
                    azimuth_out: 30.0,
                    ..Default::default()
                },
                ShotData {
                    length: 6.0,
                    azimuth_in: 40.0,
                    azimuth_out: 40.0,
                    ..Default::default()
                },
            ],
            ..Default::default()
        });

        // Merge child station 2 → root station 1.
        project.station_merges.push(StationMerge {
            station_a: StationRef {
                section_id: 0,
                station_index: 1,
            }
            .into(),
            station_b: StationRef {
                section_id: 1,
                station_index: 2,
            }
            .into(),
            merged_at: chrono::Utc::now(),
        });

        let shots = build_export_shots(&project, TmlProfile::User, AzimuthExport::decide(&project));

        // Root: START(0), shot0(1), shot1(2). parent station 1 → ariane id 1.
        // Forward block (child shots 2,3): id=3 from_id=1, id=4 from_id=3
        // Reverse block (child shots 1,0 reversed): id=5 from_id=1, id=6 from_id=5

        // Forward block shots chain from parent
        let fwd0 = shots.iter().find(|s| s.id == 3).unwrap();
        let fwd1 = shots.iter().find(|s| s.id == 4).unwrap();
        assert_eq!(fwd0.from_id, 1, "forward first shot links to parent");
        assert_eq!(fwd0.section, "Branch");
        assert_eq!(fwd1.from_id, 3, "forward second shot chains from previous");

        // Reverse block shots chain from parent
        let rev0 = shots.iter().find(|s| s.id == 5).unwrap();
        let rev1 = shots.iter().find(|s| s.id == 6).unwrap();
        assert_eq!(rev0.from_id, 1, "reverse first shot links to parent");
        assert_eq!(rev0.section, "Branch (reversed)");
        assert_eq!(rev1.from_id, 5, "reverse second shot chains from previous");

        // Verify reversal: reverse shots should have reciprocal azimuths
        // Shot 1 (azimuth_in=20) reversed → 200
        assert!(
            (rev0.azimuth - 200.0).abs() < 1e-9,
            "first reversed shot should have reciprocal azimuth of shot 1"
        );
        // Shot 0 (azimuth_in=10) reversed → 190
        assert!(
            (rev1.azimuth - 190.0).abs() < 1e-9,
            "second reversed shot should have reciprocal azimuth of shot 0"
        );

        // No orphaned from_id = -1 in any non-START shot
        for s in &shots {
            if s.shot_type != "START" {
                assert_ne!(s.from_id, -1, "shot id={} has orphaned from_id", s.id);
            }
        }
    }

    #[test]
    fn test_child_end_merge_reverses_section() {
        let mut project = Project::new("End Merge".to_string(), None);

        // Root: 2 shots
        project.add_section(SectionData {
            name: Some("Root".to_string()),
            color: "#ff0000".to_string(),
            shots: vec![
                ShotData {
                    length: 5.0,
                    azimuth_in: 90.0,
                    azimuth_out: 90.0,
                    ..Default::default()
                },
                ShotData {
                    length: 5.0,
                    azimuth_in: 90.0,
                    azimuth_out: 90.0,
                    ..Default::default()
                },
            ],
            ..Default::default()
        });

        // Child: 3 shots → stations 0,1,2,3. Merge at station 3 (= shots.len()).
        project.add_section(SectionData {
            name: Some("Tail".to_string()),
            color: "#00ff00".to_string(),
            shots: vec![
                ShotData {
                    length: 3.0,
                    azimuth_in: 10.0,
                    azimuth_out: 10.0,
                    ..Default::default()
                },
                ShotData {
                    length: 4.0,
                    azimuth_in: 20.0,
                    azimuth_out: 20.0,
                    ..Default::default()
                },
                ShotData {
                    length: 5.0,
                    azimuth_in: 30.0,
                    azimuth_out: 30.0,
                    ..Default::default()
                },
            ],
            ..Default::default()
        });

        // Merge child station 3 (end) → root station 2.
        project.station_merges.push(StationMerge {
            station_a: StationRef {
                section_id: 0,
                station_index: 2,
            }
            .into(),
            station_b: StationRef {
                section_id: 1,
                station_index: 3,
            }
            .into(),
            merged_at: chrono::Utc::now(),
        });

        let shots = build_export_shots(&project, TmlProfile::User, AzimuthExport::decide(&project));

        // Root: START(0), shot0(1), shot1(2).
        // Reversed child: shots 2,1,0 reversed → id=3 from_id=2, id=4 from_id=3, id=5 from_id=4
        let rev0 = shots.iter().find(|s| s.id == 3).unwrap();
        let rev1 = shots.iter().find(|s| s.id == 4).unwrap();
        let rev2 = shots.iter().find(|s| s.id == 5).unwrap();
        assert_eq!(rev0.from_id, 2, "first reversed shot links to parent");
        assert_eq!(rev0.section, "Tail (reversed)");
        assert_eq!(rev1.from_id, 3, "second reversed shot chains");
        assert_eq!(rev2.from_id, 4, "third reversed shot chains");

        // Verify reciprocal azimuths
        assert!(
            (rev0.azimuth - 210.0).abs() < 1e-9,
            "shot 2 (az=30) reversed → 210"
        );
        assert!(
            (rev1.azimuth - 200.0).abs() < 1e-9,
            "shot 1 (az=20) reversed → 200"
        );
        assert!(
            (rev2.azimuth - 190.0).abs() < 1e-9,
            "shot 0 (az=10) reversed → 190"
        );

        // No orphaned from_id = -1 in non-START shots
        for s in &shots {
            if s.shot_type != "START" {
                assert_ne!(s.from_id, -1, "shot id={} has orphaned from_id", s.id);
            }
        }
    }

    #[test]
    fn test_downstream_of_split_child_resolves() {
        let mut project = Project::new("Downstream".to_string(), None);

        // Section A (root): 2 shots → stations 0,1,2
        project.add_section(SectionData {
            name: Some("A".to_string()),
            color: "#ff0000".to_string(),
            shots: vec![
                ShotData {
                    length: 5.0,
                    ..Default::default()
                },
                ShotData {
                    length: 5.0,
                    ..Default::default()
                },
            ],
            ..Default::default()
        });

        // Section B: 3 shots → stations 0,1,2,3. Merge at station 1 (non-zero).
        project.add_section(SectionData {
            name: Some("B".to_string()),
            color: "#00ff00".to_string(),
            shots: vec![
                ShotData {
                    length: 3.0,
                    ..Default::default()
                },
                ShotData {
                    length: 4.0,
                    ..Default::default()
                },
                ShotData {
                    length: 5.0,
                    ..Default::default()
                },
            ],
            ..Default::default()
        });

        // Section C: 1 shot → stations 0,1. Merge at station 0 (standard child).
        project.add_section(SectionData {
            name: Some("C".to_string()),
            color: "#0000ff".to_string(),
            shots: vec![ShotData {
                length: 2.0,
                ..Default::default()
            }],
            ..Default::default()
        });

        // B:1 merges to A:1
        project.station_merges.push(StationMerge {
            station_a: StationRef {
                section_id: 0,
                station_index: 1,
            }
            .into(),
            station_b: StationRef {
                section_id: 1,
                station_index: 1,
            }
            .into(),
            merged_at: chrono::Utc::now(),
        });

        // C:0 merges to B:3 (end of B's forward block)
        project.station_merges.push(StationMerge {
            station_a: StationRef {
                section_id: 1,
                station_index: 3,
            }
            .into(),
            station_b: StationRef {
                section_id: 2,
                station_index: 0,
            }
            .into(),
            merged_at: chrono::Utc::now(),
        });

        let shots = build_export_shots(&project, TmlProfile::User, AzimuthExport::decide(&project));

        // A: START(0), shot0(1), shot1(2)
        // B forward (shots 1,2): id=3 from_id→1, id=4 from_id=3
        //   station_to_ariane: (1,2)→3, (1,3)→4
        // B reverse (shot 0 reversed): id=5 from_id→1
        //   station_to_ariane: (1,0)→5
        // C: shot0(id=6). C merges at station 0 → normal child block.
        //   parent is B:3 → station_to_ariane[(1,3)] = 4
        //   second pass: child_station_idx=0, shots[0].from_id = 4

        let c_shot = shots.iter().find(|s| s.id == 6).unwrap();
        assert_eq!(
            c_shot.from_id, 4,
            "C's first shot should chain from B's forward block station 3"
        );
        assert_eq!(c_shot.section, "C");

        // No orphaned from_id = -1 in non-START shots
        for s in &shots {
            if s.shot_type != "START" {
                assert_ne!(s.from_id, -1, "shot id={} has orphaned from_id", s.id);
            }
        }
    }

    // ========================================================================
    // SpeleoDB sidecar profile
    // ========================================================================

    /// The check SpeleoDB's converter runs before it will draw anything: it
    /// parses `Depth` and `Length` back out of the XML and rejects the WHOLE
    /// survey when a shot's drop from its parent's depth exceeds its tape. There
    /// is no epsilon, so the six-decimal write is part of the check - which is
    /// why this measures the formatted values rather than the f64s we hold.
    fn assert_written_chain_satisfies_speleodb(shots: &[ExportShot]) {
        let depth_by_id: HashMap<i64, f64> =
            shots.iter().map(|s| (s.id, as_written(s.depth))).collect();
        for shot in shots {
            let Some(&parent_depth) = depth_by_id.get(&shot.from_id) else {
                continue;
            };
            let delta = (as_written(shot.depth) - parent_depth).abs();
            let length = as_written(shot.length);
            assert!(
                delta <= length,
                "shot {} writes Depth {} under a parent at {} with Length {}: the server \
                 reads a {delta} m drop over a {length} m tape and fails the file",
                shot.id,
                format_f64(shot.depth),
                format_f64(parent_depth),
                format_f64(shot.length),
            );
        }
    }

    /// A project whose only section descends 0 -> 4.1 -> 5.4 m, with a closure
    /// tying the two stations that end at those depths. The closure's length is
    /// their difference by construction, so it sits exactly on the bound in f64
    /// and crosses it once written: 5.4 - 4.1 is 1.3000000000000007, and the
    /// tape that covers it goes into the file as "1.3".
    fn closure_on_the_bound_project() -> Project {
        let mut project = Project::new("On The Bound".to_string(), None);
        project.add_section(SectionData {
            name: Some("Main".to_string()),
            color: "#ff0000".to_string(),
            shots: vec![
                ShotData {
                    length: 5.0,
                    depth_in: 0.0,
                    depth_out: 4.1,
                    ..Default::default()
                },
                ShotData {
                    length: 5.0,
                    depth_in: 4.1,
                    depth_out: 5.4,
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
        project.loop_closures.push(LoopClosure {
            id: 1,
            station_a: StationRef {
                section_id: 0,
                station_index: 1,
            }
            .into(),
            station_b: StationRef {
                section_id: 0,
                station_index: 2,
            }
            .into(),
            path_length: 10.0,
            misclosure_distance: 1.3,
            error_percentage: 13.0,
            horizontal_misclosure: 0.0,
            vertical_misclosure: 1.3,
            strategy: ClosureStrategy::Batch,
            is_aligned: false,
        });
        project
    }

    #[test]
    fn a_tape_that_would_write_short_is_rounded_up_for_the_sidecar() {
        // The production failure. A 4.1 m -> 5.4 m drop is 1.3000000000000007 m
        // in f64, and the tape that exactly covers it writes as "1.3". The
        // server re-parses both, computes the same 1.3000000000000007, and
        // fails the whole file with "Shot is shorter than the vertical
        // variation" - so repairing with the raw delta, which is what the user
        // profile does and must keep doing, changes nothing.
        let on_the_bound = 5.4_f64 - 4.1;
        let chain = || {
            vec![
                geometry_shot(1, -1, 4.1, 4.1, 0.0, false),
                geometry_shot(2, 1, 4.1, 5.4, on_the_bound, false),
            ]
        };

        let mut user = chain();
        enforce_ariane_geometry_invariants(&mut user, TmlProfile::User);
        assert_eq!(
            format_f64(user[1].length),
            "1.3",
            "the user export stays byte-for-byte what it always was"
        );

        let mut sidecar = chain();
        enforce_ariane_geometry_invariants(&mut sidecar, TmlProfile::SpeleoDbSidecar);
        assert_eq!(
            format_f64(sidecar[1].length),
            "1.300001",
            "the sidecar tape has to still cover the drop after being written"
        );
        assert_written_chain_satisfies_speleodb(&sidecar);
    }

    #[test]
    fn the_repaired_tape_never_writes_short() {
        // The ceil is on the f64 delta while the file carries six decimals, and
        // `{:.6}` rounds half to even where `round()` rounds half away from
        // zero. Whatever the pair does at a tie, the written tape must still
        // cover the written drop.
        for (parent, depth) in [
            (4.1_f64, 5.4_f64),
            (0.1, 0.3),
            (12.345_678, 19.87),
            (-3.3, 2.2),
            (0.0, 0.000_001),
            (99.999_999_5, 100.0),
        ] {
            let delta = (as_written(depth) - as_written(parent)).abs();
            let fixed = length_covering_written_delta(delta);
            assert!(
                as_written(fixed) >= delta,
                "a {delta} m drop repaired to {fixed} writes as {} and no longer covers it",
                format_f64(fixed)
            );
            // TIGHTNESS, which is the half the covering check above cannot see. That
            // one restates the postcondition the repair's own verify loop enforces, so
            // it holds for any implementation the loop can walk up to - including ones
            // that overshoot. This is the bound that says the repair absorbs the
            // rounding and does not invent survey data: a tape may grow into the last
            // written decimal and no further.
            //
            // Loop exhaustion needs no separate assertion - it returns a value that
            // still writes short, which is exactly what the covering check catches.
            let one_ulp = 1.0 / ARIANE_WRITTEN_DECIMALS;
            assert!(
                fixed - delta < 2.0 * one_ulp,
                "repairing a {delta} m drop to {fixed} lengthens the tape by more than \
                 the rounding it exists to absorb"
            );
        }
    }

    #[test]
    fn a_closure_sitting_on_the_bound_still_reads_back_for_the_sidecar() {
        // A closure's length IS the vertical part of its own error, so every one
        // of them sits exactly on the bound and only the rounding can push it
        // over. This is the whole-project path to the same defect as the unit
        // test above, through `build_export_shots`.
        let project = closure_on_the_bound_project();

        let user = build_export_shots(&project, TmlProfile::User, AzimuthExport::decide(&project));
        let user_closure = user.iter().find(|s| s.shot_type == "CLOSURE").unwrap();
        assert_eq!(
            format_f64(user_closure.length),
            "1.3",
            "the user export is unchanged"
        );

        let sidecar = build_export_shots(
            &project,
            TmlProfile::SpeleoDbSidecar,
            AzimuthExport::decide(&project),
        );
        let sidecar_closure = sidecar.iter().find(|s| s.shot_type == "CLOSURE").unwrap();
        assert_eq!(format_f64(sidecar_closure.length), "1.300001");
        assert_written_chain_satisfies_speleodb(&sidecar);
    }

    #[test]
    fn each_entrance_carries_its_own_datum() {
        // #378 item (e): two detached galleries with distinct GPS anchors used to
        // both get the first datum's coordinates - plausibly wrong, so likelier
        // shipped unnoticed than the old 0,0 null-island form.
        let mut project = Project::new("Two Anchors".to_string(), None);
        for name in ["North", "South"] {
            project.add_section(SectionData {
                name: Some(name.to_string()),
                color: "#ff0000".to_string(),
                shots: vec![ShotData {
                    length: 5.0,
                    azimuth_in: 90.0,
                    ..Default::default()
                }],
                ..Default::default()
            });
        }
        for (section_id, latitude, longitude) in [(0, 20.25, -87.5), (1, 20.31, -87.42)] {
            project.zero_datums.push(ZeroDatum {
                station_ref: StationRef {
                    section_id,
                    station_index: 0,
                }
                .into(),
                gps: GpsCoordinates::new(latitude, longitude, 0.0).unwrap(),
            });
        }

        let shots = build_export_shots(&project, TmlProfile::User, AzimuthExport::decide(&project));
        let start_of = |name: &str| {
            shots
                .iter()
                .find(|s| s.shot_type == "START" && s.section == name)
                .unwrap()
        };
        let north = start_of("North");
        assert!(
            (north.latitude - 20.25).abs() < f64::EPSILON
                && (north.longitude - -87.5).abs() < f64::EPSILON
        );
        let south = start_of("South");
        assert!(
            (south.latitude - 20.31).abs() < f64::EPSILON
                && (south.longitude - -87.42).abs() < f64::EPSILON,
            "the second entrance must carry its own datum, got {},{}",
            south.latitude,
            south.longitude
        );

        // Same resolution, sidecar profile: applied per entrance BEFORE the
        // speleodb_anchor transform, so both entrances stay distinct there too.
        let sidecar = build_export_shots(
            &project,
            TmlProfile::SpeleoDbSidecar,
            AzimuthExport::decide(&project),
        );
        let starts: Vec<&ExportShot> = sidecar.iter().filter(|s| s.shot_type == "START").collect();
        assert_eq!(starts.len(), 2);
        assert!(
            (starts[0].latitude - starts[1].latitude).abs() > 1e-6
                || (starts[0].longitude - starts[1].longitude).abs() > 1e-6,
            "sidecar entrances collapsed onto one anchor"
        );
    }

    #[test]
    fn a_detached_entrance_with_a_mid_section_datum_splits_there() {
        // #378 item (e), the geometric half: GPS lands only on a START, so an
        // entrance whose datum is mid-section must split at the datum station -
        // stamping that datum's GPS onto a station-0 START would displace the
        // whole component by the station-to-datum vector. The root already did
        // this; detached entrances now share the path.
        let mut project = Project::new("Mid Datum".to_string(), None);
        let shot = || ShotData {
            length: 5.0,
            azimuth_in: 90.0,
            depth_in: 0.0,
            depth_out: 0.0,
            ..Default::default()
        };
        project.add_section(SectionData {
            name: Some("Main".to_string()),
            color: "#ff0000".to_string(),
            shots: vec![shot()],
            ..Default::default()
        });
        project.add_section(SectionData {
            name: Some("Gallery".to_string()),
            color: "#00ff00".to_string(),
            shots: vec![shot(), shot()],
            ..Default::default()
        });
        project.zero_datums.push(ZeroDatum {
            station_ref: StationRef {
                section_id: 0,
                station_index: 0,
            }
            .into(),
            gps: GpsCoordinates::new(20.25, -87.5, 0.0).unwrap(),
        });
        // The detached gallery's datum sits on its MIDDLE station (index 1).
        project.zero_datums.push(ZeroDatum {
            station_ref: StationRef {
                section_id: 1,
                station_index: 1,
            }
            .into(),
            gps: GpsCoordinates::new(20.31, -87.42, 0.0).unwrap(),
        });

        let shots = build_export_shots(&project, TmlProfile::User, AzimuthExport::decide(&project));
        let gallery_start = shots
            .iter()
            .find(|s| s.shot_type == "START" && s.section == "Gallery")
            .unwrap();
        assert!(
            (gallery_start.latitude - 20.31).abs() < f64::EPSILON
                && (gallery_start.longitude - -87.42).abs() < f64::EPSILON,
            "the gallery START carries the gallery's own datum"
        );
        // The split exists: a reversed pre-datum leg means the section name shows
        // up in both its forms, and every shot still chains (no orphan roots).
        assert!(
            shots
                .iter()
                .any(|s| s.section == "Gallery (pre-zero-datum)"),
            "mid-section datum must split the entrance so the START sits on it"
        );
        assert!(
            shots
                .iter()
                .all(|s| s.from_id != -1 || s.shot_type == "START"),
            "no shot may be left parentless by the entrance split"
        );
        assert!(ensure_rooted_chain(&shots, TmlProfile::User).is_ok());
    }

    #[test]
    fn a_disconnected_component_roots_at_its_datum_section() {
        // #378 review follow-up: a detached gallery of three merged sections
        // with the datum on the MIDDLE one must make that section the
        // component's entrance - rooting at the first section in project order
        // would anchor the whole gallery at the frame datum's coordinates
        // while its own datum went unwritten.
        let mut project = Project::new("Datum Root".to_string(), None);
        let shot = || ShotData {
            length: 5.0,
            azimuth_in: 90.0,
            ..Default::default()
        };
        for name in ["Main", "Gallery A", "Gallery B", "Gallery C"] {
            project.add_section(SectionData {
                name: Some(name.to_string()),
                color: "#ff0000".to_string(),
                shots: vec![shot()],
                ..Default::default()
            });
        }
        // Chain the galleries (1-2-3) to each other, none to Main (0).
        for (a, b) in [(1, 2), (2, 3)] {
            project.station_merges.push(StationMerge {
                station_a: StationRef {
                    section_id: a,
                    station_index: 1,
                }
                .into(),
                station_b: StationRef {
                    section_id: b,
                    station_index: 0,
                }
                .into(),
                merged_at: chrono::Utc::now(),
            });
        }
        project.zero_datums.push(ZeroDatum {
            station_ref: StationRef {
                section_id: 0,
                station_index: 0,
            }
            .into(),
            gps: GpsCoordinates::new(20.25, -87.5, 0.0).unwrap(),
        });
        // The gallery's own anchor sits on its MIDDLE section's first station.
        project.zero_datums.push(ZeroDatum {
            station_ref: StationRef {
                section_id: 2,
                station_index: 0,
            }
            .into(),
            gps: GpsCoordinates::new(20.31, -87.42, 0.0).unwrap(),
        });

        let shots = build_export_shots(&project, TmlProfile::User, AzimuthExport::decide(&project));
        let starts: Vec<&ExportShot> = shots.iter().filter(|s| s.shot_type == "START").collect();
        assert_eq!(starts.len(), 2, "one entrance per component");
        let gallery_start = starts.iter().find(|s| s.section == "Gallery B");
        assert!(
            gallery_start.is_some(),
            "the datum-carrying section must be the component's entrance; STARTs: {:?}",
            starts
                .iter()
                .map(|s| s.section.as_str())
                .collect::<Vec<_>>()
        );
        let gallery_start = gallery_start.unwrap();
        assert!(
            (gallery_start.latitude - 20.31).abs() < f64::EPSILON
                && (gallery_start.longitude - -87.42).abs() < f64::EPSILON,
            "the component's START carries its own datum, got {},{}",
            gallery_start.latitude,
            gallery_start.longitude
        );
        assert!(
            shots
                .iter()
                .all(|s| s.from_id != -1 || s.shot_type == "START"),
            "re-rooting must leave no orphan: every non-START chains"
        );
        assert!(ensure_rooted_chain(&shots, TmlProfile::User).is_ok());
    }

    #[test]
    fn the_rooted_chain_error_caps_its_section_list() {
        // #378 item (g): section names come from imported files, so the refusal
        // message must not scale with their count or length, and must not carry
        // control characters a crafted name could forge log lines with.
        let long = "L".repeat(500);
        let names: Vec<&str> = vec!["A", "B", "C", "D", "E", "F", "G", &long];
        let listed = list_capped(&names);
        assert!(
            listed.ends_with("and 3 more"),
            "8 sections must summarise the tail: {listed}"
        );
        assert!(
            listed.len() < 120,
            "capped list stays bounded, got {} chars",
            listed.len()
        );

        let shown = list_capped(&[&long]);
        assert!(
            shown.len() <= LISTED_SECTION_NAME_MAX + "...".len(),
            "a single long name is truncated: {} chars",
            shown.len()
        );
        assert!(shown.ends_with("..."));

        // Multibyte: the cap is in CHARS, so the byte budget is chars x 4 at
        // worst - bounded either way, and no char-boundary panic.
        let wide = "é".repeat(500);
        let shown = list_capped(&[&wide]);
        assert_eq!(shown.chars().count(), LISTED_SECTION_NAME_MAX + "...".len());
        assert!(shown.len() <= LISTED_SECTION_NAME_MAX * 4 + "...".len());

        // Control characters are stripped, not merely truncated around: the
        // newline and the ESC go; the (printable) SGR remnant stays inert.
        assert_eq!(
            list_capped(&["A\nERROR forged line\x1b[31m"]),
            "AERROR forged line[31m"
        );

        // The common case stays verbatim - no cap artifacts in a normal message.
        assert_eq!(list_capped(&["Main", "Side"]), "Main, Side");

        // And the cap is actually wired into the refusal the user sees: an
        // orphan REAL alongside a CLOSURE takes the bail path, whose message
        // must carry the summarised list, not every name.
        let mut shots: Vec<ExportShot> = Vec::new();
        shots.push(geometry_shot(0, -1, 0.0, 0.0, 0.0, false));
        for i in 0..8_i64 {
            let mut orphan = geometry_shot(100 + i, -1, 0.0, 0.0, 5.0, false);
            orphan.shot_type = "REAL";
            orphan.section = format!("Orphan {i}");
            shots.push(orphan);
        }
        let mut closure = geometry_shot(200, 0, 0.0, 0.0, 0.0, false);
        closure.shot_type = "CLOSURE";
        shots.push(closure);
        let refusal = ensure_rooted_chain(&shots, TmlProfile::User);
        assert!(refusal.is_err(), "orphans + closure must refuse the export");
        let error = refusal.unwrap_err().to_string();
        assert!(
            error.contains("and 3 more"),
            "the refusal must use the capped list: {error}"
        );
        assert!(
            !error.contains("Orphan 7"),
            "the tail must be summarised: {error}"
        );
    }

    #[test]
    fn the_sidecar_accepts_multiple_starts() {
        // #378 item (c): `is_entrance` is not profile-gated, so a project with a
        // detached section ships a multi-START sidecar to SpeleoDB. The server's
        // converter supports that (see the comment at `is_entrance`), PROVIDED
        // each START is a usable anchor: `is_geolocation_known()` there reads a
        // coordinate within ~1e-6 of zero as unknown, and a component with no
        // anchor is dropped from the GeoJSON wholesale.
        let mut project = Project::new("Two Entrances".to_string(), None);
        for name in ["Main", "Detached"] {
            project.add_section(SectionData {
                name: Some(name.to_string()),
                color: "#ff0000".to_string(),
                shots: vec![ShotData {
                    length: 5.0,
                    azimuth_in: 90.0,
                    depth_in: 0.0,
                    depth_out: 2.0,
                    ..Default::default()
                }],
                ..Default::default()
            });
        }
        // No station_merges: two components, two entrances.
        project.zero_datums.push(ZeroDatum {
            station_ref: StationRef {
                section_id: 0,
                station_index: 0,
            }
            .into(),
            gps: GpsCoordinates::new(20.25, -87.5, 0.0).unwrap(),
        });

        let sidecar = build_export_shots(
            &project,
            TmlProfile::SpeleoDbSidecar,
            AzimuthExport::decide(&project),
        );
        let starts: Vec<&ExportShot> = sidecar.iter().filter(|s| s.shot_type == "START").collect();
        assert_eq!(starts.len(), 2, "the sidecar keeps one START per entrance");
        for start in &starts {
            assert!(
                start.latitude.abs() > 1e-6 && start.longitude.abs() > 1e-6,
                "START in {:?} is at {},{} - below openspeleo_lib's anchor \
                 threshold, so its whole component would be dropped as orphans",
                start.section,
                start.latitude,
                start.longitude,
            );
        }
        assert!(
            sidecar
                .iter()
                .all(|s| s.from_id != -1 || s.shot_type == "START"),
            "a parentless non-START shot would read as an orphan root"
        );
        assert_written_chain_satisfies_speleodb(&sidecar);
    }

    #[test]
    fn the_sidecar_never_emits_exactly_one_survey_data() {
        // A lone `<SurveyData>` parses as a dict in SpeleoDB's XML decoder, so
        // iterating it yields the field NAMES and the conversion dies with an
        // AttributeError. Zero elements it handles cleanly (EmptySurveyError),
        // and two or more is the normal path - so the fix is to never emit one.
        let mut project = Project::new("Lone Start".to_string(), None);
        project.add_section(SectionData {
            name: Some("Empty".to_string()),
            color: "#ff0000".to_string(),
            shots: vec![],
            ..Default::default()
        });

        let user = build_export_shots(&project, TmlProfile::User, AzimuthExport::decide(&project));
        assert_eq!(user.len(), 1, "a shotless section exports its START alone");

        let sidecar = build_export_shots(
            &project,
            TmlProfile::SpeleoDbSidecar,
            AzimuthExport::decide(&project),
        );
        assert_eq!(sidecar.len(), 2, "the sidecar pads the lone START");
        let pad = &sidecar[1];
        assert_eq!(pad.shot_type, "VIRTUAL", "VIRTUAL is dropped from the map");
        assert_eq!(pad.from_id, -1, "a root orphan the converter ignores");
        assert!(pad.uuid.is_none());
        assert!(pad.length.abs() < f64::EPSILON);
        assert!(
            (pad.depth - sidecar[0].depth).abs() < f64::EPSILON,
            "the pad sits on the START, so it cannot displace anything"
        );
        assert_ne!(pad.id, sidecar[0].id, "a duplicate ID fails the whole file");

        let file = NamedTempFile::with_suffix(".tmlu").unwrap();
        export_tml_for_speleodb(&project, file.path()).unwrap();
        let content = std::fs::read_to_string(file.path()).unwrap();
        assert_eq!(content.matches("<SRVD>").count(), 2);

        // Nothing else is padded: no section means no shots at all, which is
        // safe, and a project already emitting two or more is left alone.
        let empty = Project::new("Nothing".to_string(), None);
        assert!(build_export_shots(
            &empty,
            TmlProfile::SpeleoDbSidecar,
            AzimuthExport::decide(&empty)
        )
        .is_empty());
        let simple = simple_project();
        assert_eq!(
            build_export_shots(
                &simple,
                TmlProfile::SpeleoDbSidecar,
                AzimuthExport::decide(&simple)
            )
            .len(),
            build_export_shots(&simple, TmlProfile::User, AzimuthExport::decide(&simple)).len()
        );
    }

    #[test]
    fn the_sidecar_emits_a_date_the_server_can_parse_and_model() {
        // A missing, empty or non-ISO `<Date>` fails validation for the ENTIRE
        // file, and a year outside IGRF's [1900, 2035] silently drops the
        // magnetic declination to 0.0, rotating the map.
        assert_eq!(speleodb_date(None), schema::UNDATED_PLACEHOLDER);
        assert_eq!(speleodb_date(Some("")), schema::UNDATED_PLACEHOLDER);
        assert_eq!(speleodb_date(Some("   ")), schema::UNDATED_PLACEHOLDER);
        assert_eq!(
            speleodb_date(Some("15/06/2025")),
            schema::UNDATED_PLACEHOLDER
        );
        assert_eq!(speleodb_date(Some("2025-06-15")), "2025-06-15");
        assert_eq!(speleodb_date(Some("1850-06-01")), "1900-01-01");
        assert_eq!(speleodb_date(Some("2999-01-01")), "2035-12-31");

        // Every emitted shot carries one, including the closure and the START,
        // which have no section date of their own.
        let sidecar_project = closure_on_the_bound_project();
        let sidecar = build_export_shots(
            &sidecar_project,
            TmlProfile::SpeleoDbSidecar,
            AzimuthExport::decide(&sidecar_project),
        );
        for shot in &sidecar {
            let date = shot.date.as_deref().unwrap_or_default();
            assert!(
                chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").is_ok(),
                "shot {} carries {date:?}, which the server cannot parse",
                shot.id
            );
        }
    }

    #[test]
    fn the_sidecar_emits_only_uuids_the_server_will_accept() {
        // `<UUID>` is typed UUID4 there, so a v1 or a non-UUID fails the whole
        // file; a repeat aliases two shots onto one GeoJSON feature. Shot UUIDs
        // are unique per SECTION by design, so the repeat is reachable.
        let v4 = "3f1e2d4c-5b6a-4789-8abc-def012345678";
        let other_v4 = "11111111-2222-4333-9444-555555555555";
        let v1 = "6f9619ff-8b86-1d01-b42d-00cf4fc964ff";
        let wrong_variant = "3f1e2d4c-5b6a-4789-cabc-def012345678";

        assert!(speleodb_uuid(v4).is_some());
        assert!(speleodb_uuid(v1).is_none(), "not a v4");
        assert!(speleodb_uuid(wrong_variant).is_none(), "not RFC 4122");
        assert!(speleodb_uuid("not-a-uuid").is_none());
        assert!(speleodb_uuid("").is_none());
        assert!(
            speleodb_uuid("00000000-0000-0000-0000-000000000000").is_none(),
            "the nil UUID is version 0"
        );

        let mut shots = vec![
            geometry_shot(1, -1, 0.0, 0.0, 0.0, false),
            geometry_shot(2, 1, 0.0, 1.0, 5.0, false),
            geometry_shot(3, 2, 1.0, 2.0, 5.0, false),
            geometry_shot(4, 3, 2.0, 3.0, 5.0, false),
            geometry_shot(5, 4, 3.0, 4.0, 5.0, false),
        ];
        shots[1].uuid = Some(v4.to_string());
        shots[2].uuid = Some(v4.to_uppercase());
        shots[3].uuid = Some(v1.to_string());
        shots[4].uuid = Some(other_v4.to_string());

        harden_for_speleodb(&mut shots);

        assert_eq!(shots[1].uuid.as_deref(), Some(v4), "a valid v4 is kept");
        assert!(
            shots[2].uuid.is_none(),
            "the same UUID in another case is still the same feature id"
        );
        assert!(shots[3].uuid.is_none(), "a v1 fails the whole file");
        assert_eq!(shots[4].uuid.as_deref(), Some(other_v4));
    }

    /// The gate above decides WHETHER to emit; this is about WHAT gets emitted.
    ///
    /// `Uuid::parse_str` is permissive - `{braced}`, `urn:uuid:` and hyphenless all
    /// parse, and all of them are genuine v4s that sail through `speleodb_uuid`. Wire
    /// the accepted value straight through and the file carries a spelling the
    /// server's `UUID4` field will not match, failing the whole document on a value
    /// the check had just approved. That is the same class of silent, whole-file
    /// rejection the rest of this hardening exists to remove, so the canonical
    /// `8-4-4-4-12` form is what goes in.
    #[test]
    fn a_uuid_the_gate_accepts_is_written_in_the_form_the_server_expects() {
        let canonical = "3f1e2d4c-5b6a-4789-8abc-def012345678";
        for spelling in [
            format!("{{{canonical}}}"),
            format!("urn:uuid:{canonical}"),
            canonical.replace('-', ""),
            canonical.to_uppercase(),
        ] {
            assert!(
                speleodb_uuid(&spelling).is_some(),
                "{spelling} is a real v4 and the gate accepts it"
            );

            let mut shots = vec![
                geometry_shot(1, -1, 0.0, 0.0, 0.0, false),
                geometry_shot(2, 1, 0.0, 1.0, 5.0, false),
            ];
            shots[1].uuid = Some(spelling.clone());

            harden_for_speleodb(&mut shots);

            assert_eq!(
                shots[1].uuid.as_deref(),
                Some(canonical),
                "{spelling} must be written as the canonical hyphenated form"
            );
        }
    }

    #[test]
    fn an_anchor_on_the_equator_is_nudged_off_zero_for_the_sidecar() {
        // SpeleoDB anchors the map only when BOTH components exceed 1e-7, so a
        // survey on the equator or the prime meridian silently gets no map at
        // all. The nudge is a micro-degree, roughly 11 cm: a tenth of that is
        // below `format_f64`'s six decimals and would go into the file as "0.0".
        let (lat, lon) = speleodb_anchor(0.0, -87.5);
        assert_eq!(format_f64(lat), "0.000001");
        assert_eq!(format_f64(lon), "-87.5", "a real component is left alone");

        let (lat, lon) = speleodb_anchor(-0.0, 0.0);
        assert_eq!(format_f64(lat), "0.000001", "negative zero is still zero");
        assert_eq!(format_f64(lon), "0.000001");

        // Below the six decimals the sign is all that survives, and it is kept.
        assert_eq!(format_f64(speleodb_anchor(-1e-9, 1.0).0), "-0.000001");

        // Out of range parses fine there and explodes inside the projection.
        let (lat, lon) = speleodb_anchor(95.0, 200.0);
        assert_eq!(format_f64(lat), "90.0");
        assert_eq!(format_f64(lon), "180.0");

        // No datum means no anchor, in both profiles: inventing one would put
        // the survey somewhere it has never been.
        let no_datum_project = simple_project();
        let no_datum = build_export_shots(
            &no_datum_project,
            TmlProfile::SpeleoDbSidecar,
            AzimuthExport::decide(&no_datum_project),
        );
        let start = no_datum.iter().find(|s| s.shot_type == "START").unwrap();
        assert!(start.latitude.abs() < f64::EPSILON);
        assert!(start.longitude.abs() < f64::EPSILON);
    }

    #[test]
    fn the_sidecar_strips_the_section_identity_separator() {
        // SpeleoDB groups shots into sections by hashing
        // name/description/date/explorers/surveyors joined with U+220E, so the
        // character inside one of those fields re-cuts the survey.
        let mut project = simple_project();
        project.sections[0].name = Some("Main\u{220e}Line".to_string());
        project.sections[0].surveyor = Some("Alice\u{220e}Bob".to_string());

        let user = build_export_shots(&project, TmlProfile::User, AzimuthExport::decide(&project));
        assert_eq!(user[0].section, "Main\u{220e}Line");
        assert_eq!(user[0].explorer.as_deref(), Some("Alice\u{220e}Bob"));

        let sidecar = build_export_shots(
            &project,
            TmlProfile::SpeleoDbSidecar,
            AzimuthExport::decide(&project),
        );
        assert_eq!(sidecar[0].section, "Main Line");
        assert_eq!(sidecar[0].explorer.as_deref(), Some("Alice Bob"));
    }

    #[test]
    fn the_sidecar_normalizes_azimuth_for_every_shot_type() {
        // The server normalises REAL shots and passes every other type through
        // raw, so a VIRTUAL leg at -10 degrees walks off in a direction nothing
        // was surveyed in.
        let mut project = simple_project();
        project.sections[0].shots[0].azimuth_in = -10.0;
        project.sections[0].shots[0].azimuth_out = -10.0;
        project.sections[0].shots[0].kind = ShotKind::Jump;
        project.sections[0].shots[1].azimuth_in = 400.0;
        project.sections[0].shots[1].azimuth_out = 400.0;

        // Both profiles write the bearing the app draws, which the circular
        // mean already puts in [0, 360) (#576 NF-AN5-1).
        let user = build_export_shots(&project, TmlProfile::User, AzimuthExport::decide(&project));
        assert!((user[1].azimuth - 350.0).abs() < 1e-9);
        assert!((user[2].azimuth - 40.0).abs() < 1e-9);

        let sidecar = build_export_shots(
            &project,
            TmlProfile::SpeleoDbSidecar,
            AzimuthExport::decide(&project),
        );
        assert_eq!(sidecar[1].shot_type, "VIRTUAL");
        assert!((sidecar[1].azimuth - 350.0).abs() < 1e-9);
        assert!((sidecar[2].azimuth - 40.0).abs() < 1e-9);
    }

    #[test]
    fn exported_ariane_ids_are_unique_across_the_whole_document() {
        // A duplicate `<ID>` anywhere - across sections, split blocks, closures,
        // the synthesized START - fails the whole file with a DuplicateValueError.
        // Nothing renumbers here because nothing has to: ids come off one counter
        // that is never reset, which this pins on the topologies that mint the
        // most of them (a datum split, a mid-section merge, closures).
        let mut project = Project::new("Ids".to_string(), None);
        project.add_section(SectionData {
            name: Some("Root".to_string()),
            color: "#ff0000".to_string(),
            shots: (0..4)
                .map(|i| ShotData {
                    length: 5.0,
                    depth_in: f64::from(i),
                    depth_out: f64::from(i) + 1.0,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        });
        project.add_section(SectionData {
            name: Some("Child".to_string()),
            color: "#00ff00".to_string(),
            shots: (0..3)
                .map(|_| ShotData {
                    length: 2.0,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        });
        project.station_merges.push(StationMerge {
            station_a: StationRef {
                section_id: 0,
                station_index: 2,
            }
            .into(),
            station_b: StationRef {
                section_id: 1,
                station_index: 1,
            }
            .into(),
            merged_at: chrono::Utc::now(),
        });
        project.loop_closures.push(LoopClosure {
            id: 1,
            station_a: StationRef {
                section_id: 0,
                station_index: 1,
            }
            .into(),
            station_b: StationRef {
                section_id: 1,
                station_index: 3,
            }
            .into(),
            path_length: 12.0,
            misclosure_distance: 0.3,
            error_percentage: 2.5,
            horizontal_misclosure: 0.3,
            vertical_misclosure: 0.0,
            strategy: ClosureStrategy::Batch,
            is_aligned: false,
        });
        // A datum away from station 0 splits the root into two blocks.
        project.zero_datums.push(ZeroDatum {
            station_ref: StationRef {
                section_id: 0,
                station_index: 2,
            }
            .into(),
            gps: GpsCoordinates::new(20.25, -87.5, 0.0).unwrap(),
        });

        for profile in [TmlProfile::User, TmlProfile::SpeleoDbSidecar] {
            let shots = build_export_shots(&project, profile, AzimuthExport::decide(&project));
            assert!(shots.len() > 8, "the topology should mint plenty of ids");
            let unique: HashSet<i64> = shots.iter().map(|s| s.id).collect();
            assert_eq!(
                unique.len(),
                shots.len(),
                "{profile:?} emitted a duplicate ID"
            );
        }
    }

    #[test]
    fn the_user_export_keeps_every_value_the_sidecar_repairs() {
        // The sidecar's concessions are lies about the survey, small enough for
        // a map preview and not for the file a user asked for. One project that
        // trips four of them at once, exported both ways.
        let mut project = closure_on_the_bound_project();
        project.sections[0].name = Some("Main\u{220e}Line".to_string());
        project.sections[0].shots[0].uuid = Some("not-a-uuid".to_string());
        project.zero_datums.push(ZeroDatum {
            station_ref: StationRef {
                section_id: 0,
                station_index: 0,
            }
            .into(),
            gps: GpsCoordinates::new(20.25, 0.0, 0.0).unwrap(),
        });

        let dir = tempfile::TempDir::new().unwrap();
        let user_path = dir.path().join("user.tmlu");
        let sidecar_path = dir.path().join("sidecar.tmlu");
        export_tml(&project, &user_path).unwrap();
        export_tml_for_speleodb(&project, &sidecar_path).unwrap();
        let user = std::fs::read_to_string(&user_path).unwrap();
        let sidecar = std::fs::read_to_string(&sidecar_path).unwrap();

        assert!(user.contains("<UU>not-a-uuid</UU>"), "verbatim, as always");
        assert!(user.contains("Main\u{220e}Line"));
        assert!(user.contains("<LGT>0.0</LGT>"));
        assert!(
            !user.contains("<LG>1.300001</LG>"),
            "the closure tape is untouched in the user export"
        );

        assert!(!sidecar.contains("not-a-uuid"));
        assert!(!sidecar.contains('\u{220e}'));
        assert!(sidecar.contains("<LGT>0.000001</LGT>"));
        assert!(sidecar.contains("<LG>1.300001</LG>"));

        // And the sidecar is still deterministic, which the push relies on to
        // stay a 304 no-op when nothing changed.
        let again = dir.path().join("again.tmlu");
        export_tml_for_speleodb(&project, &again).unwrap();
        assert_eq!(sidecar, std::fs::read_to_string(&again).unwrap());
    }

    // ---- reversed legs keep their shot's identity (export-roundtrip-fuzz X6) ----

    /// A section split by a zero datum: the shots before the datum are
    /// re-emitted as the reversed `(pre-zero-datum)` leg.
    fn project_split_by_a_zero_datum() -> Project {
        let mut project = Project::new("Split".to_string(), None);
        project.add_section(SectionData {
            name: Some("Main".to_string()),
            color: "#ff0000".to_string(),
            shots: (0..4)
                .map(|i| ShotData {
                    length: 3.0 + f64::from(i),
                    azimuth_in: 10.0 * f64::from(i),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        });
        project.zero_datums.push(ZeroDatum {
            station_ref: StationRef {
                section_id: 0,
                station_index: 2,
            }
            .into(),
            gps: GpsCoordinates::new(20.25, -87.5, 12.0).unwrap(),
        });
        project.ensure_shot_uuids();
        project
    }

    /// A child merged MID-section: a forward block plus a reversed block.
    fn project_with_a_mid_section_merge() -> Project {
        let mut project = Project::new("Mid Merge".to_string(), None);
        project.add_section(SectionData {
            name: Some("Root".to_string()),
            color: "#ff0000".to_string(),
            shots: vec![
                ShotData {
                    length: 5.0,
                    azimuth_in: 90.0,
                    ..Default::default()
                },
                ShotData {
                    length: 5.0,
                    azimuth_in: 90.0,
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
        project.add_section(SectionData {
            name: Some("Branch".to_string()),
            color: "#00ff00".to_string(),
            shots: (0..4)
                .map(|i| ShotData {
                    length: 3.0 + f64::from(i),
                    azimuth_in: 20.0 * f64::from(i),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        });
        // Child station 2 is in the MIDDLE of its four shots, so the exporter
        // emits shots 2..4 forward and shots 0..2 reversed.
        project.station_merges.push(StationMerge {
            station_a: StationRef {
                section_id: 0,
                station_index: 2,
            }
            .into(),
            station_b: StationRef {
                section_id: 1,
                station_index: 2,
            }
            .into(),
            merged_at: chrono::Utc::now(),
        });
        project.ensure_shot_uuids();
        project
    }

    /// A child merged at its LAST station: the whole section is reversed and
    /// there is no forward block at all.
    fn project_with_an_end_merge() -> Project {
        let mut project = Project::new("End Merge".to_string(), None);
        project.add_section(SectionData {
            name: Some("Root".to_string()),
            color: "#ff0000".to_string(),
            shots: vec![
                ShotData {
                    length: 5.0,
                    azimuth_in: 90.0,
                    ..Default::default()
                },
                ShotData {
                    length: 5.0,
                    azimuth_in: 90.0,
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
        project.add_section(SectionData {
            name: Some("Tail".to_string()),
            color: "#00ff00".to_string(),
            shots: (0..3)
                .map(|i| ShotData {
                    length: 3.0 + f64::from(i),
                    azimuth_in: 10.0 * f64::from(i),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        });
        project.station_merges.push(StationMerge {
            station_a: StationRef {
                section_id: 0,
                station_index: 2,
            }
            .into(),
            station_b: StationRef {
                section_id: 1,
                station_index: 3,
            }
            .into(),
            merged_at: chrono::Utc::now(),
        });
        project.ensure_shot_uuids();
        project
    }

    fn project_uuids(project: &Project) -> HashSet<String> {
        project
            .sections
            .iter()
            .flat_map(|s| s.shots.iter().filter_map(|sh| sh.uuid.clone()))
            .collect()
    }

    /// How many PROJECT shots the export re-emitted into a reversed block,
    /// counted by the section label only the reversal constructors write and
    /// the shot types only a real project shot gets. Deliberately blind to the
    /// UUID, so it stays a statement about the SHAPE of the export and cannot
    /// pass or fail for the reason the tests below are about.
    fn reversed_leg_shot_count(shots: &[ExportShot]) -> usize {
        shots
            .iter()
            .filter(|s| {
                (s.section.ends_with("(pre-zero-datum)") || s.section.ends_with("(reversed)"))
                    && (s.shot_type == "REAL" || s.shot_type == "VIRTUAL")
            })
            .count()
    }

    /// The three shapes that make the exporter emit a reversed leg, each with
    /// the number of project shots that leg has to carry.
    ///
    /// The count is asserted HERE, once, for every consumer below, because
    /// these tests are worth nothing the moment a fixture stops reaching the
    /// reversed constructor at all: a project whose shots are all emitted
    /// forward satisfies every assertion in this section trivially, and the
    /// suite goes green over an export that no longer contains the thing under
    /// test. WHICH shots get reversed is a function of the export root, and the
    /// root is chosen from `Project::frame_datum` and the merge graph - both of
    /// which have moved before (the frame rule replaced `zero_datums[0]`) and
    /// can move again without touching a line of this module.
    fn reversal_shapes() -> Vec<(&'static str, Project)> {
        let shapes = vec![
            // 4 shots split at station 2: shots 0 and 1 come back reversed.
            ("zero-datum split", project_split_by_a_zero_datum(), 2),
            // The child's 4 shots merge at its station 2: shots 0 and 1 reversed.
            ("mid-section merge", project_with_a_mid_section_merge(), 2),
            // The child merges at its LAST station: all 3 shots reversed, and
            // there is no forward block at all.
            ("end merge", project_with_an_end_merge(), 3),
        ];
        shapes
            .into_iter()
            .map(|(label, project, reversed)| {
                let shots =
                    build_export_shots(&project, TmlProfile::User, AzimuthExport::decide(&project));
                assert_eq!(
                    reversed_leg_shot_count(&shots),
                    reversed,
                    "{label}: fixture rot - the export no longer re-emits these shots \
                     as a reversed leg, so every assertion about one is vacuous"
                );
                (label, project)
            })
            .collect()
    }

    /// Every REAL project shot reaches the file with its own UUID, whichever
    /// block it lands in.
    ///
    /// The reversed legs used to be emitted with `uuid: None`, so the reader had
    /// nothing to preserve and `Shot::with_uuid` minted a fresh one - 78 of a
    /// real 532-shot survey's shots were re-identified by a round trip through a
    /// file this app had just written. Found by `export-roundtrip-fuzz`'s X6;
    /// see that scenario's finding 4.
    #[test]
    fn every_real_shot_reaches_the_export_with_its_own_uuid() {
        for (label, project) in reversal_shapes() {
            let expected = project_uuids(&project);
            assert!(!expected.is_empty(), "{label}: fixture has no uuids");

            let shots =
                build_export_shots(&project, TmlProfile::User, AzimuthExport::decide(&project));
            let emitted: HashSet<String> = shots.iter().filter_map(|s| s.uuid.clone()).collect();
            assert_eq!(emitted, expected, "{label}: emitted uuids != project uuids");

            // The synthesized blocks - the ones that stand for no project shot -
            // still carry none.
            for shot in &shots {
                if shot.shot_type == "START" || shot.shot_type == "CLOSURE" {
                    assert!(
                        shot.uuid.is_none(),
                        "{label}: a synthesized {} block carries a uuid",
                        shot.shot_type
                    );
                }
            }
        }
    }

    /// A reversed leg is its shot's ONLY appearance, so nothing is re-minted.
    ///
    /// This is the premise the fix rests on: every constructor pairs a reversed
    /// block with a forward block over a DISJOINT index range, or replaces the
    /// forward block entirely. If one ever emitted a shot twice,
    /// `remint_duplicate_uuids` would read a forward/reverse pair as a document
    /// duplicate and re-mint one of them - silently undoing the fix for exactly
    /// the shots it was made for.
    #[test]
    fn a_reversed_leg_is_a_shots_only_appearance_so_nothing_is_reminted() {
        for (label, project) in reversal_shapes() {
            let shots =
                build_export_shots(&project, TmlProfile::User, AzimuthExport::decide(&project));
            let mut seen: HashMap<String, usize> = HashMap::new();
            for shot in &shots {
                if let Some(uuid) = &shot.uuid {
                    *seen.entry(uuid.clone()).or_default() += 1;
                }
            }
            let repeats: Vec<&String> = seen
                .iter()
                .filter(|(_, count)| **count > 1)
                .map(|(uuid, _)| uuid)
                .collect();
            assert!(
                repeats.is_empty(),
                "{label}: {} uuid(s) appear more than once in the document: {repeats:?}",
                repeats.len()
            );
            // `build_export_shots` has already run `remint_duplicate_uuids` for
            // the User profile, so this equality is what proves it did not fire:
            // a re-mint would put a uuid in the document that the project does
            // not have.
            let emitted: HashSet<String> = shots.iter().filter_map(|s| s.uuid.clone()).collect();
            assert_eq!(
                emitted,
                project_uuids(&project),
                "{label}: a uuid was re-minted"
            );
        }
    }

    /// The identity survives the whole trip: write the file, read it back with
    /// the app's own parser, and the reversed leg's shots still answer to the
    /// uuids they left with.
    #[test]
    fn a_reversed_legs_uuid_survives_an_export_and_reparse() {
        for (label, project) in reversal_shapes() {
            let expected = project_uuids(&project);

            let file = NamedTempFile::with_suffix(".tmlu").unwrap();
            export_tml(&project, file.path()).unwrap();
            let parsed = crate::parsers::tml::parse_tml(file.path()).unwrap();

            let read_back: HashSet<String> = parsed
                .sections
                .iter()
                .flat_map(|s| s.shots.iter().filter_map(|sh| sh.uuid.clone()))
                .collect();
            assert_eq!(
                read_back, expected,
                "{label}: a shot's identity did not survive export + parse_tml"
            );
        }
    }

    /// The SpeleoDB sidecar is unaffected: it still renders, and still renders
    /// the same bytes twice, which is what keeps a re-push a 304 no-op.
    ///
    /// `harden_for_speleodb` runs INSTEAD of `remint_duplicate_uuids` and drops
    /// any uuid that is not a canonical v4 or that repeats, so the extra uuids
    /// the reversed legs now carry go through the same gate every forward shot's
    /// already did.
    #[test]
    fn reversed_leg_uuids_leave_the_speleodb_sidecar_deterministic() {
        for (label, project) in reversal_shapes() {
            let dir = tempfile::tempdir().unwrap();
            let first = dir.path().join("a.tmlu");
            let second = dir.path().join("b.tmlu");
            export_tml_for_speleodb(&project, &first).unwrap();
            export_tml_for_speleodb(&project, &second).unwrap();
            assert_eq!(
                std::fs::read(&first).unwrap(),
                std::fs::read(&second).unwrap(),
                "{label}: the sidecar render is no longer deterministic"
            );
        }
    }
}
