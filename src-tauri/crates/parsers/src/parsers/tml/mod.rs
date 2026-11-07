//! Ariane TML/TMLU file parser
//!
//! TML files are ZIP archives containing `Data.xml`; TMLU files are plain XML.
//! Both use the same schema but TML uses long element names while TMLU uses
//! short abbreviations.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use chrono::NaiveDate;
use quick_xml::events::Event;
use quick_xml::name::QName;
use quick_xml::Reader;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::models::config::DEFAULT_INCLINATION_DEPTH_DIFF_THRESHOLD;
use crate::models::shot::implied_pitch_deg;
use crate::models::{ImportClosure, ImportSectionConfig, ShotData, ShotKind};
use crate::parsers::station_graph::{topological_sort_configs, ConfigOrder};

use azimuth::tml_azimuth_reference;
use import_options::{build_selection_index, determine_placement, PlacementInput};
pub use import_options::{TmlImportOptions, TmlRootPlacement};

mod azimuth;
mod decode;
mod import_options;
mod linearize;
mod reading_bounds;
pub mod schema;

use schema::element as el;
pub use schema::inclination_to_pitch;
// `test-support` (never on in production builds) keeps these fixture builders
// compiled for the root crate's tests, which can no longer see a dependency's
// `#[cfg(test)]` items now that the parsers live in their own crate.
#[cfg(any(test, feature = "test-support"))]
pub mod test_helpers;
#[cfg(test)]
#[expect(clippy::print_stdout)]
mod undated_date_rv4_tests;

#[cfg(test)]
mod mock_narrowing_tests;

#[cfg(test)]
#[expect(clippy::expect_used, clippy::print_stdout)]
mod cdata_tests;

// #576 FZ-PARSE-5: readings beyond the limit read as missing.
#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::print_stdout)]
mod reading_bound_tests;

#[cfg(test)]
mod excluded_leg_depth_tests;

use linearize::{
    collapsed_trip_report, join_contiguous_chains, linearize_section, split_runs_at_trip_boundaries,
};

// ============================================================================
// IPC-visible types
// ============================================================================

/// Parsed Ariane TML/TMLU file.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../bindings/")]
pub struct TmlFile {
    pub cave_name: String,
    pub unit: TmlUnit,
    pub use_magnetic_azimuth: bool,
    pub first_start_absolute_elevation: f64,
    /// The file's `<unit>` text when it is neither a known metric nor a known
    /// imperial spelling, sanitized by `sanitize_unit_label`. The file is read
    /// as metric anyway; this drives an import warning so a silent mis-scaling
    /// is visible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub unrecognized_unit: Option<String>,
    /// Structural damage the parser silently smoothed over, when there was any.
    /// Nothing else records it: each of these is a shot that was dropped or
    /// reinterpreted, and the import receipt is the only place the user can
    /// learn it happened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub parse_anomalies: Option<TmlParseAnomalies>,
    /// Set when the file's bytes were not valid UTF-8 and had to be decoded
    /// another way (see `decode::decode_xml_bytes`). Built from a fixed vocabulary, not
    /// from file text, so it needs no sanitizing to cross IPC.
    ///
    /// `convert_tml_to_import_configs` copies it verbatim into the import
    /// warnings, which reach the receipt on the import-wizard path. The
    /// SpeleoDB adoption path in `core::state::speleodb_adopt` calls the same
    /// converter but discards its warnings, so this one is invisible there too -
    /// a pre-existing gap, not one this field introduces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub encoding_warning: Option<String>,
    /// Set when the file's "same as previous" `DepthIn` marker was written scaled
    /// from feet and read as the marker (`restore_scaled_depth_in_marker`), so the
    /// import receipt can say so (#576 HFI-1-D2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub scaled_depth_in_marker: Option<TmlScaledDepthInMarker>,
    /// Readings the parser read as missing because they are beyond the limit
    /// no real survey reaches (#576 FZ-PARSE-5-D1), one line each naming the
    /// shot ID. Shown on the import dialog before Import is pressed and copied
    /// into the import warnings; absent for a clean file. Built from shot IDs
    /// and numbers only, never file text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub warnings: Option<Vec<String>>,
    pub geo_coding: Option<TmlGeoCoding>,
    pub sections: Vec<TmlSection>,
    pub closures: Vec<TmlClosure>,
}

/// Counts of the file-level anomalies the parser recovers from instead of
/// failing on. All three are recoveries, not errors: the file still imports,
/// but with fewer or different shots than it declares.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../bindings/")]
pub struct TmlParseAnomalies {
    /// Shot elements with no usable `<ID>`, dropped by `build_raw_shot`.
    pub shots_missing_id: u32,
    /// Shots whose `<Type>` is none of REAL/VIRTUAL/START/CLOSURE and were
    /// therefore read as REAL.
    pub unknown_shot_types: u32,
    /// Shots sharing an `<ID>` with an earlier shot. The topology index is
    /// keyed by ID, so only the last shot with each ID survives it.
    pub duplicate_shot_ids: u32,
}

impl TmlParseAnomalies {
    /// Whether anything at all was recorded, so `parse_tml` can leave the
    /// field off a clean file entirely.
    fn any(self) -> bool {
        self != Self::default()
    }
}

/// [#576 HFI-1-D2] A "same as previous" `DepthIn` marker the parser read from
/// its feet-to-metre scaled spelling ([`schema::DEPTH_IN_SAME_AS_PREVIOUS_SCALED`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../bindings/")]
pub struct TmlScaledDepthInMarker {
    /// Each spelling the file used (`-0.3`, ...), once, in file order. From the
    /// fixed spelling list, never file text.
    pub written_as: Vec<String>,
    /// Shots whose `DepthIn` was read as the marker.
    pub shots: u32,
}

/// Measurement unit in the TML file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../bindings/")]
pub enum TmlUnit {
    Metric,
    Imperial,
}

/// GPS reference from the TML file.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../bindings/")]
pub struct TmlGeoCoding {
    pub latitude: f64,
    pub longitude: f64,
    pub altitude: f64,
}

/// A linearized survey section.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../bindings/")]
pub struct TmlSection {
    pub name: String,
    pub shots: Vec<TmlShot>,
    pub surveyor: Option<String>,
    #[ts(type = "string | null")]
    pub date: Option<NaiveDate>,
    pub color: String,
    pub is_branch_split: bool,
    /// Ariane ID of the first shot's FromID (for topology linking).
    #[ts(type = "number")]
    pub start_ariane_id: i64,
    /// Ariane ID of the last shot in this section.
    #[ts(type = "number")]
    pub end_ariane_id: i64,
    /// Cumulative depth of the parent shot (from_id target) for correct rebasing
    /// when a section branches mid-chain. 0.0 if the section starts from a
    /// START shot or has no parent.
    pub start_depth: f64,
}

/// A single survey shot.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../bindings/")]
pub struct TmlShot {
    #[ts(type = "number")]
    pub id: i64,
    /// Ariane's own `<UUID>`/`<UU>` for this row, verbatim - `None` when the
    /// element is absent or empty. Carried into [`crate::models::ShotData::uuid`]
    /// so an Ariane-authored file keeps its shot identity across a round trip;
    /// deliberately NOT validated or coerced to a canonical v4, since coercing
    /// would recreate the churn the carry exists to stop (#423).
    #[serde(default)]
    #[ts(optional)]
    pub uuid: Option<String>,
    #[ts(type = "number")]
    pub from_id: i64,
    pub shot_type: TmlShotType,
    pub azimuth: f64,
    pub length: f64,
    pub depth: f64,
    pub depth_in: f64,
    pub inclination: f64,
    pub left: f64,
    pub right: f64,
    pub up: f64,
    pub down: f64,
    pub comment: Option<String>,
    pub explorer: Option<String>,
    #[ts(type = "string | null")]
    pub date: Option<NaiveDate>,
    pub color: String,
    pub excluded: bool,
    pub latitude: f64,
    pub longitude: f64,
    #[ts(type = "number")]
    pub closure_to_id: i64,
    pub section: String,
}

/// Shot type in Ariane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../bindings/")]
pub enum TmlShotType {
    Real,
    Virtual,
    Start,
    Closure,
}

/// A closure linking two stations across sections, in PREVIEW coordinates.
///
/// The indices are positions in [`TmlSection::shots`] as parsed - CLOSURE and
/// excluded shots included, and no `+1` shift onto the station at the end of
/// each shot. That is deliberately NOT the convention of the
/// [`crate::models::ImportClosure`] values `convert_tml_to_import_configs`
/// builds, which index the filtered import shot list and DO shift (`shot i`
/// ends at `station i + 1`). The two are not interchangeable: this type exists
/// only so the import dialog can say how many closures the file contains
/// before anything is imported, and the import path re-resolves every closure
/// from the raw Ariane IDs rather than reading these.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../bindings/")]
pub struct TmlClosure {
    pub from_section_index: usize,
    pub from_station_index: usize,
    pub to_section_index: usize,
    pub to_station_index: usize,
}

// ============================================================================
// Internal raw shot for parsing
// ============================================================================

#[derive(Debug, Clone)]
struct RawShot {
    id: i64,
    uuid: Option<String>,
    from_id: i64,
    closure_to_id: i64,
    shot_type: TmlShotType,
    section: String,
    azimuth: f64,
    length: f64,
    depth: f64,
    depth_in: f64,
    inclination: f64,
    left: f64,
    right: f64,
    up: f64,
    down: f64,
    comment: Option<String>,
    explorer: Option<String>,
    date: Option<NaiveDate>,
    color: String,
    excluded: bool,
    latitude: f64,
    longitude: f64,
}

// ============================================================================
// Dual key mapping (TML long names ↔ TMLU short names)
// ============================================================================

/// Normalize an XML element name to a canonical key. Byte-slice form used by
/// the hot XML loop so we don't have to allocate a `String` from the quick-xml
/// element name on every Start/End/Empty event. The names come from
/// [`schema::element`], the one table the exporter writes from too.
fn normalize_key_bytes(name: &[u8]) -> Option<&'static str> {
    match name {
        el::id::LONG | schema::ID_LEGACY_SPELLING => Some("id"),
        // Ariane 26+ writes a stable per-row id here and preserves it across
        // edits; we read it so a file that leaves Ariane and comes back keeps
        // its shot identity instead of getting a fresh mint (#423).
        el::uuid::LONG | el::uuid::SHORT => Some("uuid"),
        el::from_id::LONG | el::from_id::SHORT => Some("from_id"),
        el::closure_to_id::LONG | el::closure_to_id::SHORT => Some("closure_to_id"),
        el::shot_type::LONG | el::shot_type::SHORT => Some("type"),
        el::section::LONG | el::section::SHORT => Some("section"),
        el::azimuth::LONG | el::azimuth::SHORT => Some("azimuth"),
        el::length::LONG | el::length::SHORT => Some("length"),
        el::depth::LONG | el::depth::SHORT => Some("depth"),
        el::depth_in::LONG | el::depth_in::SHORT => Some("depth_in"),
        el::inclination::LONG | el::inclination::SHORT => Some("inclination"),
        el::left::LONG | el::left::SHORT => Some("left"),
        el::right::LONG | el::right::SHORT => Some("right"),
        el::up::LONG | el::up::SHORT => Some("up"),
        el::down::LONG | el::down::SHORT => Some("down"),
        el::comment::LONG | el::comment::SHORT => Some("comment"),
        el::explorer::LONG | el::explorer::SHORT => Some("explorer"),
        el::date::LONG | el::date::SHORT => Some("date"),
        el::color::LONG | el::color::SHORT => Some("color"),
        el::excluded::LONG | el::excluded::SHORT => Some("excluded"),
        el::latitude::LONG | el::latitude::SHORT => Some("latitude"),
        el::longitude::LONG | el::longitude::SHORT => Some("longitude"),
        // `Name`/`NM` is deliberately absent: Ariane uses it as a per-shot marker
        // ("START", "CLOSURE:CLOSURE", else ""), and import takes shot type from
        // `Type`/`TY`, so nothing would read it. `Locked` and `Profiletype` are
        // write-only for the same reason.
        _ => None,
    }
}

// ============================================================================
// XML parsing
// ============================================================================

/// Parse all raw shots from XML content.
fn parse_raw_shots(xml: &str) -> Result<(Vec<RawShot>, FileMetadata)> {
    let mut reader = Reader::from_str(xml);

    let mut metadata = FileMetadata::default();
    let mut shots = Vec::new();

    // Parsing state. `current_element_key` holds the normalised field
    // name for the *currently-open* element. We resolve it once at
    // Start and reuse it at Text - that avoids re-doing the lookup
    // (and any String allocation) on the text event.  For non-shot
    // file-level elements we keep the raw bytes around so the small
    // `match` in the metadata branch can dispatch by slice.
    let mut in_cave_file = false;
    let mut in_data = false;
    let mut in_shot = false;
    let mut current_fields: HashMap<&'static str, String> = HashMap::new();
    let mut current_element_key: Option<&'static str> = None;
    let mut current_element_raw: Vec<u8> = Vec::new();
    // Shape/SH subtrees are skipped - TML's <Shape> per-shot LRUD
    // polygons are not consumed by the importer.  We use quick-xml's
    // `read_to_end` to fast-forward past them instead of looping
    // through every nested event ourselves; for a 57 MB Data.xml this
    // saves on the order of ~30 % of the parse wall-clock.

    // Hash table for the file-level metadata fields (string slot →
    // setter). Keeps the Text branch allocation-free for the common
    // shot fields and small for the file-level ones.
    let mut buf = Vec::new();

    // How many elements are open right now. quick-xml's low-level reader
    // reports a document that simply stops as a clean `Eof` (only its serde
    // layer turns that into `MissingEndTag`), so a half-written save would
    // otherwise parse as a perfectly successful empty survey. A counter,
    // not a name stack: this loop runs over Data.xml files of tens of MB and
    // the surrounding code goes out of its way to stay allocation-free.
    let mut open_elements: usize = 0;

    loop {
        // [#576 FZ-PARSE-3] A CDATA section is character data like any other
        // text: re-escaped into a text event, it reads (after the unescape the
        // text arm does) exactly as the value it wraps. Only text events were
        // read, so a CDATA length read as 0 and a CDATA comment vanished.
        let event = match reader.read_event_into(&mut buf) {
            Ok(Event::CData(c)) => c.escape().map(Event::Text).map_err(quick_xml::Error::from),
            other => other,
        };
        match event {
            Ok(Event::Start(e)) => {
                open_elements += 1;
                let name_bytes = e.name();
                let name_bytes = name_bytes.as_ref();

                match name_bytes {
                    el::cave_file::LONG => in_cave_file = true,
                    el::data::LONG => in_data = true,
                    el::survey_data::LONG | el::survey_data::SHORT => {
                        in_shot = true;
                        current_fields.clear();
                    }
                    el::shape::LONG | el::shape::SHORT => {
                        // Fast-forward past the Shape/SH subtree -
                        // quick-xml will scan its bytes without
                        // emitting events back to us.
                        let qname = QName(name_bytes);
                        if let Err(err) = reader.read_to_end(qname) {
                            bail!(
                                "XML parse error skipping <{}> at position {}: {err}",
                                std::str::from_utf8(name_bytes).unwrap_or("<non-utf8>"),
                                reader.error_position()
                            );
                        }
                        // `read_to_end` consumed this element's own End event,
                        // so the End arm below will never see it. (It fails on
                        // a truncated subtree, which is why that case already
                        // errors today.)
                        open_elements -= 1;
                    }
                    _ => {
                        if in_shot {
                            // Resolve the shot-field key once (returns
                            // a 'static slice from the lookup table);
                            // None means "field we don't care about".
                            current_element_key = normalize_key_bytes(name_bytes);
                            current_element_raw.clear();
                        } else if in_cave_file && !in_data {
                            current_element_key = None;
                            current_element_raw.clear();
                            current_element_raw.extend_from_slice(name_bytes);
                        }
                    }
                }
            }
            Ok(Event::End(e)) => {
                // quick-xml rejects an End with no matching Start before we see
                // it (`check_end_names` is on by default); saturate anyway
                // rather than wrap a good file into a false truncation report.
                open_elements = open_elements.saturating_sub(1);
                let name_bytes = e.name();
                let name_bytes = name_bytes.as_ref();

                match name_bytes {
                    el::survey_data::LONG | el::survey_data::SHORT => {
                        if in_shot {
                            // Counted as a missing ID because that is
                            // `build_raw_shot`'s ONLY fallible path today (the
                            // numeric fields all coerce). A new `bail!` in
                            // there needs its own counter, or this warning
                            // starts naming the wrong defect.
                            match build_raw_shot(&current_fields, &mut metadata.anomalies) {
                                Ok(shot) => shots.push(shot),
                                Err(e) => {
                                    tracing::warn!(
                                        reason = %e,
                                        "Skipping malformed shot element"
                                    );
                                    metadata.anomalies.shots_missing_id =
                                        metadata.anomalies.shots_missing_id.saturating_add(1);
                                }
                            }
                            in_shot = false;
                        }
                    }
                    el::data::LONG => in_data = false,
                    el::cave_file::LONG => in_cave_file = false,
                    _ => {
                        current_element_key = None;
                        current_element_raw.clear();
                    }
                }
            }
            Ok(Event::Text(e)) => {
                if in_shot {
                    // Skip the unescape + trim + alloc unless this is a
                    // known shot field.
                    if let Some(key) = current_element_key {
                        let text = e.unescape().unwrap_or_default().trim().to_string();
                        if !text.is_empty() {
                            current_fields.insert(key, text);
                        }
                    }
                } else if in_cave_file && !in_data && !current_element_raw.is_empty() {
                    let text = e.unescape().unwrap_or_default().trim().to_string();
                    if !text.is_empty() {
                        match current_element_raw.as_slice() {
                            el::cave_name::LONG => metadata.cave_name = text,
                            el::unit::LONG => metadata.unit = text,
                            el::use_magnetic_azimuth::LONG => {
                                metadata.use_magnetic_azimuth = text.eq_ignore_ascii_case("true");
                            }
                            el::first_start_absolute_elevation::LONG => {
                                // `"NaN"`, `"inf"` and `1e309` all parse
                                // happily. This value is scaled by the unit
                                // conversion and handed out on `TmlFile` and
                                // the geo-coding altitude, so a non-finite one
                                // would poison every consumer downstream of it.
                                metadata.first_start_absolute_elevation = text
                                    .parse()
                                    .ok()
                                    .filter(|v: &f64| v.is_finite())
                                    .unwrap_or(0.0);
                            }
                            _ => {}
                        }
                    }
                }
            }
            Ok(Event::Empty(e)) => {
                let name_bytes = e.name();
                let name_bytes = name_bytes.as_ref();
                // Handle self-closing shot elements like <CM/> (empty comment)
                if in_shot {
                    if let Some(key) = normalize_key_bytes(name_bytes) {
                        current_fields.insert(key, String::new());
                    }
                }
            }
            Ok(Event::Eof) => {
                if open_elements > 0 {
                    bail!(
                        "The survey XML ends at byte {} with {open_elements} element(s) \
                         still unclosed - the file appears truncated or damaged",
                        reader.buffer_position()
                    );
                }
                break;
            }
            Err(e) => bail!(
                "XML parse error at position {}: {e}",
                reader.error_position()
            ),
            _ => {}
        }
        buf.clear();
    }

    Ok((shots, metadata))
}

#[derive(Debug, Default)]
struct FileMetadata {
    cave_name: String,
    unit: String,
    use_magnetic_azimuth: bool,
    first_start_absolute_elevation: f64,
    /// Tallied by the raw-shot loop, not read from the file: the two anomalies
    /// only the element-by-element pass can see (a shot with no ID, a shot with
    /// an unknown type). `duplicate_shot_ids` is filled in later, by
    /// `parse_tml`, once the whole shot list exists.
    anomalies: TmlParseAnomalies,
}

/// The longest foreign shot id we will adopt, in BYTES (`str::len`, so a
/// multibyte value hits the cap sooner in characters). A hyphenated UUID is 36
/// bytes; 128 leaves room for a `urn:uuid:` prefix or a braced form and still
/// bounds the amplification, because the value is cloned into the project-wide
/// `shot_uuid_index` AND into every undo snapshot (`Shot::to_shot_data` copies
/// it), i.e. shots x length x undo depth.
const MAX_FOREIGN_UUID_LEN: usize = 128;

/// True if `c` cannot be written into TML element content and read back as
/// itself. Two overlapping rules, both needed:
///
/// * **Outside XML 1.0's `Char` production**, which is
///   `#x9 | #xA | #xD | [#x20-#xD7FF] | [#xE000-#xFFFD] | [#x10000-#x10FFFF]`.
///   `BytesText::new` escapes only `<>&"'`, so anything else is emitted as a
///   RAW byte and the document simply stops being XML. This half is what catches
///   U+FFFE and U+FFFF, which are excluded by the production but are NOT control
///   characters - a `char::is_control` gate let them straight through.
///   Surrogates are the production's other exclusion and cannot occur in a Rust
///   `char`.
/// * **Any remaining control character.** Tab, LF and CR are perfectly legal
///   XML, and the C1 range (U+0080-U+009F) is legal XML 1.0 text, but none of
///   them belongs in an identifier and a `\0` additionally risks truncation on
///   SQLite TEXT read-back - which would make the saved and the reloaded
///   identity differ with nothing to show for it.
fn is_unexportable_in_xml(c: char) -> bool {
    !schema::is_xml_char(c) || c.is_control()
}

/// The file's `<UUID>`/`<UU>` as an identity we are willing to store, or `None`
/// to mint a fresh one.
///
/// The VALUE is never coerced - canonicalising it would recreate the identity
/// churn #423 exists to stop - but its SHAPE is checked, because two classes of
/// input are not merely odd, they are unstorable:
///
/// * **Characters we cannot re-emit.** See [`is_unexportable_in_xml`]. Measured:
///   exporting a shot whose uuid holds U+0001 writes that byte into element
///   content. Our own quick-xml reader is lenient and reads it straight back, so
///   nothing on this side complains; the strict consumers - Ariane and
///   SpeleoDB's `openspeleo_lib` - reject the file.
/// * **Unbounded length.** See [`MAX_FOREIGN_UUID_LEN`].
///
/// Rejecting is the fail-safe direction and the asymmetry is stark: a rejected
/// value costs THAT SHOT its foreign identity and nothing else (it gets the same
/// fresh mint an uncovered row gets), while one accepted bad byte costs the
/// WHOLE exported document - a strict consumer fails the file, not the row.
fn foreign_shot_uuid(shot_id: i64, raw: Option<&str>) -> Option<String> {
    // Surrounding whitespace is trimmed BEFORE adoption - the one normalisation
    // "never coerced" does not cover. Identity is compared byte-verbatim
    // everywhere downstream, and XML readers routinely whitespace-normalise
    // element content, so storing " x " would hand back a different identity on
    // the very first round trip through a normalising consumer.
    let raw = raw?.trim();
    // Empty (or whitespace-only) means "no identity", matching the way
    // `export::tml::shot_export_uuid` already treats an empty stored value.
    if raw.is_empty() {
        return None;
    }
    if raw.len() > MAX_FOREIGN_UUID_LEN {
        tracing::warn!(
            shot_id,
            len = raw.len(),
            max = MAX_FOREIGN_UUID_LEN,
            purpose = "bound foreign shot identity (#423)",
            "Shot UUID is too long to adopt; minting a fresh identity instead"
        );
        return None;
    }
    if raw.chars().any(is_unexportable_in_xml) {
        tracing::warn!(
            shot_id,
            purpose = "bound foreign shot identity (#423)",
            "Shot UUID holds a character that cannot be re-exported as well-formed \
             XML; minting a fresh identity instead"
        );
        return None;
    }
    Some(raw.to_owned())
}

/// How the two halves of Ariane's one people-field are rejoined. `" / "` is
/// what the compound values in real files already use, so a file that arrives
/// with `"nh / ds"` leaves this app unchanged, and one that arrives in the
/// element form leaves it in the form Ariane's own users write.
const EXPLORER_SURVEYOR_SEPARATOR: &str = " / ";

/// Ariane's placeholder for a half nobody filled in. Dropped per HALF, so
/// `<Explorer>unknown</Explorer><Surveyor>ds</Surveyor>` keeps `ds` instead of
/// throwing the whole field away (#427). Case-insensitive: the placeholder is
/// a word this format writes, not a value, and `Unknown` is the same word.
fn is_explorer_placeholder(value: &str) -> bool {
    value.is_empty()
        || value.eq_ignore_ascii_case("None")
        || value.eq_ignore_ascii_case(schema::EXPLORER_PLACEHOLDER)
}

/// The text of `<tag>…</tag>` inside an already-unescaped field value, with the
/// byte offset of its OPENING tag so the caller can keep DOCUMENT order and see
/// what came before it.
fn tagged_half<'a>(raw: &'a str, tag: &str) -> Option<(usize, &'a str)> {
    let open = format!("<{tag}>");
    let opens_at = raw.find(&open)?;
    let rest = &raw[opens_at + open.len()..];
    // An unterminated half keeps everything after the open tag - the same
    // recovery the previous single-half reader did.
    Some((
        opens_at,
        rest.split_once(format!("</{tag}>").as_str())
            .map_or(rest, |(text, _)| text),
    ))
}

/// Read Ariane's `Explorer`, which is ONE element holding TWO roles.
///
/// Two encodings occur: a plain compound string (`"nh / ds"` - the form every
/// file in the #411 survey used) and an entity-escaped element pair
/// (`"<Explorer>nh</Explorer><Surveyor>ds</Surveyor>"`). Both halves are kept:
/// the surveyor half used to be dropped outright, and an `"unknown"` explorer
/// half took the surveyor down with it (#427).
///
/// Splitting a compound value into PEOPLE is [#329], not this: the halves are
/// rejoined into the single string `Section::surveyor` holds, which the TML
/// export writes straight back into `Explorer`, so the round trip is stable and
/// idempotent: a re-import splits the plain form on the same separator, applies
/// the same per-half rules, and rejoins - and anything that survived those rules
/// once survives them again unchanged. Identical halves collapse to one - "the
/// same person did both" is not two names.
///
/// Halves come out in DOCUMENT order, and any text before the first tag is a
/// half of its own: this field is a person's name in a file nobody validates,
/// so the reader keeps what is there rather than deciding which part of it was
/// meant.
///
/// [#329]: https://github.com/Stalagmight/Stalagmight/issues/329
fn parse_explorer_field(raw: &str) -> Option<String> {
    let halves: Vec<&str> = if raw.contains("<Explorer>") || raw.contains("<Surveyor>") {
        let mut found: Vec<(usize, &str)> = ["Explorer", "Surveyor"]
            .iter()
            .filter_map(|tag| tagged_half(raw, tag))
            .collect();
        // Text before the first opening tag is somebody's name that lost its
        // element, not noise to drop on the floor.
        if let Some(first_tag) = found.iter().map(|(at, _)| *at).min() {
            let lead = &raw[..first_tag];
            if !lead.trim().is_empty() {
                found.push((0, lead));
            }
        }
        found.sort_by_key(|(at, _)| *at);
        found.into_iter().map(|(_, text)| text).collect()
    } else {
        // The plain compound form gets the SAME per-half treatment, on the same
        // separator this rejoins with. Splitting it verbatim instead let
        // "unknown / ds" carry Ariane's placeholder into `Section::surveyor`
        // and back out through the export, while the element form dropped it.
        raw.split(EXPLORER_SURVEYOR_SEPARATOR).collect()
    };

    let mut kept: Vec<&str> = Vec::with_capacity(halves.len());
    for half in halves {
        let half = half.trim();
        if !is_explorer_placeholder(half) && !kept.contains(&half) {
            kept.push(half);
        }
    }

    if kept.is_empty() {
        None
    } else {
        Some(kept.join(EXPLORER_SURVEYOR_SEPARATOR))
    }
}

/// The row's survey date, strictly `%Y-%m-%d`.
///
/// [`schema::UNDATED_PLACEHOLDER`] is "no date" (#576 B2): our exporter writes
/// it for a section with none, and reading it as 1 January 2000 fabricated a
/// date AND derived a declination for it. Every 2000-01-01 reads as undated,
/// whether or not our [`schema::UNDATED_MARKER`] comment follows it: Ariane
/// drops the comment when it re-saves a file, exports from before the marker
/// never had it, and the SpeleoDB sidecar never writes it. A survey genuinely
/// dated 1 January 2000 therefore imports undated, the accepted cost of not
/// rotating every re-imported placeholder (B2-MARKER).
///
/// The parsed date is compared, not the raw text: `%Y-%m-%d` also accepts an
/// unpadded month or day, so "2000-1-1" from a tool that does not zero-pad
/// is the placeholder too (#576 RV4-R-P1).
fn shot_date(fields: &HashMap<&str, String>) -> Option<NaiveDate> {
    let raw = fields.get("date")?;
    let date = NaiveDate::parse_from_str(raw, "%Y-%m-%d").ok()?;
    let placeholder = NaiveDate::parse_from_str(schema::UNDATED_PLACEHOLDER, "%Y-%m-%d").ok();
    (Some(date) != placeholder).then_some(date)
}

/// Build a `RawShot` from parsed field map.
fn build_raw_shot(
    fields: &HashMap<&str, String>,
    anomalies: &mut TmlParseAnomalies,
) -> Result<RawShot> {
    let id = parse_i64(fields, "id").context("Missing or invalid ID")?;
    let from_id = parse_i64(fields, "from_id").unwrap_or(-1);

    // Ariane writes the type in upper case, but a file that has been through a
    // third-party tool need not: match every spelling before falling through to
    // the unknown-type path.
    let shot_type = match fields.get("type").map(String::as_str) {
        Some(t) if t.eq_ignore_ascii_case(schema::TYPE_REAL) => TmlShotType::Real,
        Some(t) if t.eq_ignore_ascii_case(schema::TYPE_VIRTUAL) => TmlShotType::Virtual,
        Some(t) if t.eq_ignore_ascii_case(schema::TYPE_START) => TmlShotType::Start,
        Some(t) if t.eq_ignore_ascii_case(schema::TYPE_CLOSURE) => TmlShotType::Closure,
        Some(other) => {
            tracing::warn!(shot_type = other, id, "Unknown shot type, treating as Real");
            anomalies.unknown_shot_types = anomalies.unknown_shot_types.saturating_add(1);
            TmlShotType::Real
        }
        None => TmlShotType::Real,
    };

    let section = strip_section_description(
        fields
            .get("section")
            .map(String::as_str)
            .unwrap_or_default(),
    );

    let color = schema::color_from_ariane(fields.get("color").map(String::as_str));

    let date = shot_date(fields);

    let comment = fields
        .get("comment")
        .filter(|c| !c.is_empty() && *c != "None")
        .cloned();
    let explorer = fields
        .get("explorer")
        .and_then(|e| parse_explorer_field(e.as_str()));

    Ok(RawShot {
        id,
        uuid: foreign_shot_uuid(id, fields.get("uuid").map(String::as_str)),
        from_id,
        closure_to_id: parse_i64(fields, "closure_to_id").unwrap_or(-1),
        shot_type,
        section,
        azimuth: parse_f64(fields, "azimuth"),
        length: parse_f64(fields, "length"),
        depth: parse_f64(fields, "depth"),
        depth_in: parse_f64_default(fields, "depth_in", schema::DEPTH_IN_SAME_AS_PREVIOUS),
        inclination: parse_f64(fields, "inclination"),
        left: parse_f64(fields, "left"),
        right: parse_f64(fields, "right"),
        up: parse_f64(fields, "up"),
        down: parse_f64(fields, "down"),
        comment,
        explorer,
        date,
        color,
        excluded: fields
            .get("excluded")
            .is_some_and(|v| v.eq_ignore_ascii_case("true")),
        latitude: parse_f64(fields, "latitude"),
        longitude: parse_f64(fields, "longitude"),
    })
}

fn parse_f64(fields: &HashMap<&str, String>, key: &str) -> f64 {
    // Rust's f64 parser accepts "NaN"/"inf"/"-inf"; treat any non-finite value as
    // missing so a corrupt/hostile .tml can't seed non-finite geometry that would
    // silently poison every downstream coordinate/alignment computation.
    fields
        .get(key)
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|v| v.is_finite())
        .unwrap_or(0.0)
}

fn parse_f64_default(fields: &HashMap<&str, String>, key: &str, default: f64) -> f64 {
    fields
        .get(key)
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|v| v.is_finite())
        .unwrap_or(default)
}

fn parse_i64(fields: &HashMap<&str, String>, key: &str) -> Result<i64> {
    fields
        .get(key)
        .ok_or_else(|| anyhow!("Missing field '{key}'"))?
        .parse::<i64>()
        .map_err(|e| anyhow!("Invalid integer for '{key}': {e}"))
}

// ============================================================================
// Section name cleanup
// ============================================================================

/// Strip `<SectionDescription>...</SectionDescription>` suffixes from section names.
///
/// Ariane encodes section descriptions as entity-escaped XML appended to the
/// section name (e.g. `"Mainline<SectionDescription>notes</SectionDescription>"`).
/// After XML unescaping this becomes part of the text content. We strip it so
/// that START shots (which carry the description) group with their REAL shots.
fn strip_section_description(section: &str) -> String {
    if let Some(idx) = section.find("<SectionDescription>") {
        section[..idx].trim_end().to_string()
    } else {
        section.to_string()
    }
}

// ============================================================================
// Sectioning and topology
// ============================================================================

/// Propagate GPS coordinates from START shots to their immediate successors.
///
/// Ariane stores GPS on START shots (section origin markers) which carry no
/// measurement data and are excluded from sections. To preserve the geo-reference,
/// copy lat/lng to the first real shot(s) that follow each START shot - but only
/// if those successors don't already have their own GPS data.
fn propagate_start_gps(raw_shots: &mut [RawShot]) {
    // Collect START shots that have GPS data
    let gps_starts: Vec<(i64, f64, f64)> = raw_shots
        .iter()
        .filter(|s| {
            s.shot_type == TmlShotType::Start
                && (s.latitude.abs() > f64::EPSILON || s.longitude.abs() > f64::EPSILON)
        })
        .map(|s| (s.id, s.latitude, s.longitude))
        .collect();

    if gps_starts.is_empty() {
        return;
    }

    let start_gps: HashMap<i64, (f64, f64)> = gps_starts
        .into_iter()
        .map(|(id, lat, lng)| (id, (lat, lng)))
        .collect();

    // For each non-START shot whose from_id points to a GPS-tagged START,
    // copy the coordinates if the successor has no GPS of its own.
    for shot in raw_shots.iter_mut() {
        if shot.shot_type == TmlShotType::Start {
            continue;
        }
        if shot.latitude.abs() > f64::EPSILON || shot.longitude.abs() > f64::EPSILON {
            continue; // already has its own GPS
        }
        if let Some(&(lat, lng)) = start_gps.get(&shot.from_id) {
            shot.latitude = lat;
            shot.longitude = lng;
        }
    }
}

/// How many shots share an ID with an earlier shot.
///
/// `build_sections` keys its topology index (`shot_by_id`) on the Ariane ID, so
/// every one of these is a shot the import loses: the later shot overwrites the
/// earlier one in the index, and the earlier one's successors reattach to the
/// survivor. Counted here rather than fixed - which shot the file MEANT is not
/// knowable from the file.
fn count_duplicate_shot_ids(raw_shots: &[RawShot]) -> u32 {
    let mut seen = std::collections::HashSet::with_capacity(raw_shots.len());
    let duplicates = raw_shots.iter().filter(|s| !seen.insert(s.id)).count();
    u32::try_from(duplicates).unwrap_or(u32::MAX)
}

/// Build topology and linearize into sections.
fn build_sections(mut raw_shots: Vec<RawShot>) -> (Vec<TmlSection>, Vec<TmlClosure>) {
    if raw_shots.is_empty() {
        return (Vec::new(), Vec::new());
    }

    // Propagate GPS coordinates from START shots to their immediate successors.
    // Ariane places GPS data on START shots, which we later exclude from sections.
    propagate_start_gps(&mut raw_shots);

    // Index shots by ID
    let shot_by_id: HashMap<i64, &RawShot> = raw_shots.iter().map(|s| (s.id, s)).collect();

    // Build adjacency: from_id → list of successor shot IDs
    let mut successors: HashMap<i64, Vec<i64>> = HashMap::new();
    for shot in &raw_shots {
        if shot.from_id >= 0 {
            successors.entry(shot.from_id).or_default().push(shot.id);
        }
    }

    // Group non-START shots by section name, preserving FromID chain order
    let mut section_groups: Vec<(String, Vec<&RawShot>)> = Vec::new();
    let mut section_map: HashMap<String, usize> = HashMap::new();

    // Sort shots topologically: walk from each START shot
    let mut visited: std::collections::HashSet<i64> = std::collections::HashSet::new();
    let mut ordered_ids: Vec<i64> = Vec::new();

    // Find START shots as roots
    let mut roots: Vec<i64> = raw_shots
        .iter()
        .filter(|s| s.shot_type == TmlShotType::Start)
        .map(|s| s.id)
        .collect();
    roots.sort_unstable();

    // BFS from each root
    for root_id in &roots {
        let mut queue = std::collections::VecDeque::new();
        queue.push_back(*root_id);
        while let Some(current_id) = queue.pop_front() {
            if !visited.insert(current_id) {
                continue;
            }
            ordered_ids.push(current_id);
            if let Some(succs) = successors.get(&current_id) {
                for &succ_id in succs {
                    if !visited.contains(&succ_id) {
                        queue.push_back(succ_id);
                    }
                }
            }
        }
    }

    // Also add any unvisited shots (disconnected)
    for shot in &raw_shots {
        if visited.insert(shot.id) {
            ordered_ids.push(shot.id);
        }
    }

    // Now group ordered non-START shots by section name
    for &shot_id in &ordered_ids {
        let Some(shot) = shot_by_id.get(&shot_id) else {
            continue;
        };
        // Skip START shots - they carry no measurements
        if shot.shot_type == TmlShotType::Start {
            continue;
        }

        let section_name = if shot.section.is_empty() {
            "Unnamed".to_string()
        } else {
            shot.section.clone()
        };

        if let Some(&idx) = section_map.get(&section_name) {
            section_groups[idx].1.push(shot);
        } else {
            let idx = section_groups.len();
            section_map.insert(section_name.clone(), idx);
            section_groups.push((section_name, vec![shot]));
        }
    }

    // Within each group, linearize by walking FromID chains.
    // If branching detected (shot has multiple successors within the same section),
    // split into separate linear sections.
    let mut sections: Vec<TmlSection> = Vec::new();

    for (section_name, group_shots) in &section_groups {
        let linearized = linearize_section(section_name, group_shots, &successors, &shot_by_id);
        sections.extend(linearized);
    }

    // Sew the cuts back up where the file's own topology allows it: a chain that
    // starts exactly where another ends is the same passage under a new name
    // (#412). Must run BEFORE extract_closures, which keys closures on
    // (section index, station index) and would otherwise index the pre-merge
    // fragments.
    //
    // The join matches a chain's start against another chain's end BY SHOT ID,
    // so it needs to know whether this file's ids are actually unique - a
    // duplicate id would splice an unrelated chain onto the wrong parent.
    let joins = join_contiguous_chains(&mut sections, count_duplicate_shot_ids(&raw_shots));
    if joins > 0 {
        tracing::debug!(joins, "Joined contiguous TML chains");
    }

    // Then give each TRIP its own run: Ariane writes surveyor/date/colour on
    // every shot row, a run carries only its FIRST shot's, and the rest used to
    // be dropped without a word (#427).
    //
    // Strictly AFTER the join, which is decided on the chain ends the FILE has.
    // A cut end is not one of those - it has an in-section successor by
    // construction - and offering it to the join let an unrelated branch at that
    // junction splice onto the mainline's head, taking its name while the real
    // continuation stayed a separate run. The cost is that a cut tail cannot
    // absorb a same-trip chain continuing it under another name; that is the
    // behaviour this file already had, and the join's contract is worth more
    // than the extra merge.
    let trip_cuts = split_runs_at_trip_boundaries(&mut sections);
    if trip_cuts > 0 {
        tracing::debug!(trip_cuts, "Split TML runs where the survey trip changed");
    }

    // Extract closures: find CLOSURE shots and map to section:station indices
    let closures = extract_closures(&sections, &shot_by_id);

    (sections, closures)
}

/// Extract closure information from CLOSURE-type shots.
fn extract_closures(
    sections: &[TmlSection],
    shot_by_id: &HashMap<i64, &RawShot>,
) -> Vec<TmlClosure> {
    // Build reverse index: shot ID → (section_index, station_index)
    let mut id_to_location: HashMap<i64, (usize, usize)> = HashMap::new();
    for (si, section) in sections.iter().enumerate() {
        for (shot_idx, shot) in section.shots.iter().enumerate() {
            id_to_location.insert(shot.id, (si, shot_idx));
        }
    }

    let mut closures = Vec::new();

    for (si, section) in sections.iter().enumerate() {
        for (shot_idx, shot) in section.shots.iter().enumerate() {
            if shot.shot_type == TmlShotType::Closure && shot.closure_to_id >= 0 {
                // The closure shot itself is the "from" location
                let from_section_index = si;
                let from_station_index = shot_idx;

                // closure_to_id points to the target shot
                if let Some(&(to_si, to_shot_idx)) = id_to_location.get(&shot.closure_to_id) {
                    closures.push(TmlClosure {
                        from_section_index,
                        from_station_index,
                        to_section_index: to_si,
                        to_station_index: to_shot_idx,
                    });
                } else if shot_by_id
                    .get(&shot.closure_to_id)
                    .is_some_and(|target| target.shot_type == TmlShotType::Start)
                {
                    // The target is a START shot, which carries no measurement
                    // and so is in no section. Aim at the first section that
                    // hangs off it instead. Scanning `sections` in order (rather
                    // than the previous scan of the `id_to_location` HashMap)
                    // is what makes the answer the same on every run when
                    // several sections start from the same START shot.
                    let to_section_index = sections.iter().position(|s| {
                        s.shots
                            .first()
                            .is_some_and(|first| first.from_id == shot.closure_to_id)
                    });
                    if let Some(to_si) = to_section_index {
                        // Station 0 of the target section (first station after START)
                        closures.push(TmlClosure {
                            from_section_index,
                            from_station_index,
                            to_section_index: to_si,
                            to_station_index: 0,
                        });
                    }
                }
            }
        }
    }

    closures
}

// ============================================================================
// Unit conversion
// ============================================================================

const FEET_TO_METERS: f64 = 0.3048;

/// Apply imperial→metric conversion in place if the file uses imperial units.
///
/// Every length the importer carries forward passes through here, not just the
/// shot measurements: `start_depth` is the parent station's cumulative depth and
/// rebases the first shot of every branch section whose `DepthIn` is the `-1`
/// sentinel (see `resolve_tml_depth_in`), and the file-level absolute elevation
/// is a length too.
fn apply_unit_conversion(
    sections: &mut [TmlSection],
    first_start_absolute_elevation: &mut f64,
    unit: TmlUnit,
) {
    if unit != TmlUnit::Imperial {
        return;
    }

    *first_start_absolute_elevation *= FEET_TO_METERS;

    for section in sections.iter_mut() {
        section.start_depth *= FEET_TO_METERS;
        for shot in &mut section.shots {
            shot.length *= FEET_TO_METERS;
            shot.depth *= FEET_TO_METERS;
            if !schema::is_depth_in_sentinel(shot.depth_in) {
                shot.depth_in *= FEET_TO_METERS;
            }
            shot.left *= FEET_TO_METERS;
            shot.right *= FEET_TO_METERS;
            shot.up *= FEET_TO_METERS;
            shot.down *= FEET_TO_METERS;
        }
    }
}

/// [#576 HFI-1] Read the "same as previous" `DepthIn` marker in a METRIC file
/// whose marker went through a feet-to-metre conversion: -1.0 ft written in
/// metres ([`schema::DEPTH_IN_SAME_AS_PREVIOUS_SCALED`], `-0.3` as Ariane writes
/// it) instead of `-1.0`. Read literally, every such shot started 0.3 m above the
/// datum. Rewritten to the canonical marker in place, so every reader downstream
/// (`resolve_tml_depth_in`, the linearizer) sees the file's intent.
///
/// Per FILE, never per value, because -0.3 m is also a legal start depth: the
/// marker is written on nearly every shot of an Ariane file, so the scaled
/// spelling counts as the marker only when MORE THAN HALF of the file's shots
/// carry it. A real start depth
/// shared by half a survey's shots is not a survey - and if a flat stretch did
/// sit there, "same as previous" would read the same depth back. A feet file is
/// left alone: its marker is `-1.0`, and its -0.3 is -0.3 ft.
///
/// Returns what it read, for the import receipt (#576 HFI-1-D2).
fn restore_scaled_depth_in_marker(
    sections: &mut [TmlSection],
    unit: TmlUnit,
) -> Option<TmlScaledDepthInMarker> {
    if unit != TmlUnit::Metric {
        return None;
    }
    let total: usize = sections.iter().map(|s| s.shots.len()).sum();
    let scaled = sections
        .iter()
        .flat_map(|s| &s.shots)
        .filter(|shot| schema::is_scaled_depth_in_sentinel(shot.depth_in))
        .count();
    if scaled * 2 <= total {
        return None;
    }
    let mut written_as: Vec<String> = Vec::new();
    for shot in sections.iter_mut().flat_map(|s| &mut s.shots) {
        if let Some(spelling) = schema::scaled_depth_in_spelling(shot.depth_in) {
            let spelling = spelling.to_string();
            if !written_as.contains(&spelling) {
                written_as.push(spelling);
            }
            shot.depth_in = schema::DEPTH_IN_SAME_AS_PREVIOUS;
        }
    }
    tracing::info!(
        shots = scaled,
        total,
        "TML: DepthIn marker written as -1 ft in metres; read as same-as-previous"
    );
    Some(TmlScaledDepthInMarker {
        written_as,
        shots: u32::try_from(scaled).unwrap_or(u32::MAX),
    })
}

// ============================================================================
// Public parse entry point
// ============================================================================

/// Parse a TML or TMLU file into a `TmlFile`.
pub fn parse_tml(path: &Path) -> Result<TmlFile> {
    tracing::info!(path = ?path, "Parsing Ariane TML/TMLU file");

    let decode::LoadedXml {
        xml,
        encoding_warning,
    } = decode::load_xml(path)?;
    let (mut raw_shots, mut metadata) = parse_raw_shots(&xml)?;

    tracing::debug!(
        shot_count = raw_shots.len(),
        cave_name = %metadata.cave_name,
        unit = %metadata.unit,
        "Parsed raw shots from XML"
    );

    let (unit, unrecognized_unit) = classify_unit(&metadata.unit);
    let warnings = reading_bounds::bound_readings(
        &mut raw_shots,
        &mut metadata.first_start_absolute_elevation,
        unit,
    );

    let mut anomalies = metadata.anomalies;
    anomalies.duplicate_shot_ids = count_duplicate_shot_ids(&raw_shots);

    let (mut sections, closures) = build_sections(raw_shots);
    let scaled_depth_in_marker = restore_scaled_depth_in_marker(&mut sections, unit);

    // Apply unit conversion before returning
    let mut first_start_absolute_elevation = metadata.first_start_absolute_elevation;
    apply_unit_conversion(&mut sections, &mut first_start_absolute_elevation, unit);

    // Extract geo coding from first shot with non-zero lat/lng
    let geo_coding = sections
        .iter()
        .flat_map(|s| &s.shots)
        .find(|shot| shot.latitude.abs() > f64::EPSILON || shot.longitude.abs() > f64::EPSILON)
        .map(|shot| TmlGeoCoding {
            latitude: shot.latitude,
            longitude: shot.longitude,
            altitude: first_start_absolute_elevation,
        });

    tracing::info!(
        sections = sections.len(),
        closures = closures.len(),
        unit = ?unit,
        anomalies = ?anomalies,
        "TML parsing complete"
    );

    Ok(TmlFile {
        cave_name: metadata.cave_name,
        unit,
        use_magnetic_azimuth: metadata.use_magnetic_azimuth,
        first_start_absolute_elevation,
        unrecognized_unit,
        parse_anomalies: anomalies.any().then_some(anomalies),
        encoding_warning,
        scaled_depth_in_marker,
        warnings,
        geo_coding,
        sections,
        closures,
    })
}

/// Map Ariane's `<unit>` text onto a `TmlUnit`.
///
/// `ft`/`imperial` mean feet, `m`/`metric` and an absent element mean meters
/// (files with no `<unit>` at all exist). Anything else still reads as metric -
/// that is the safe default, every observed Ariane file is metric - but the raw
/// string comes back (sanitized) so the import can say so: silently scaling a
/// whole survey by the wrong factor leaves no trace anywhere else.
fn classify_unit(raw: &str) -> (TmlUnit, Option<String>) {
    let trimmed = raw.trim();
    match trimmed.to_lowercase().as_str() {
        "ft" | "imperial" => (TmlUnit::Imperial, None),
        "" | "m" | "metric" => (TmlUnit::Metric, None),
        _ => (TmlUnit::Metric, Some(sanitize_unit_label(trimmed))),
    }
}

/// The most of a file-controlled unit string worth keeping. Matches
/// `ImportReceipt::MAX_WARNING_LEN`, the clamp the warning built from it lands
/// under anyway.
const MAX_UNIT_LABEL_LEN: usize = 300;

/// Make a file-controlled unit string fit to cross IPC and land in a UI warning
/// line. The file is untrusted input - this app reads whatever anyone hands it -
/// and the rules are the receipt's, so the sanitizing itself lives with the
/// receipt: [`crate::models::sanitize_display_text`].
fn sanitize_unit_label(raw: &str) -> String {
    crate::models::sanitize_display_text(raw, MAX_UNIT_LABEL_LEN)
}

/// The import receipt's unit-conversion note for an Ariane file: TML carries ONE
/// unit for the whole file, so this is a straight read of it.
pub fn tml_unit_conversion_note(unit: TmlUnit) -> Option<String> {
    match unit {
        TmlUnit::Imperial => Some("Lengths converted from feet to meters".to_string()),
        TmlUnit::Metric => None,
    }
}

// ============================================================================
// Conversion to ImportSectionConfig
// ============================================================================

/// `""` or `"s"` for a count, so a warning reads naturally at 1.
fn plural_s(count: u32) -> &'static str {
    if count == 1 {
        ""
    } else {
        "s"
    }
}

/// `"was"` or `"were"` for a count.
fn was_were(count: u32) -> &'static str {
    if count == 1 {
        "was"
    } else {
        "were"
    }
}

/// Turn the parser's anomaly tallies into receipt-visible sentences.
///
/// These describe shots the file contains that the import does not, or that it
/// holds as something other than what the file said. The parser already logs
/// each one, but a log is not something the person doing the import ever sees.
///
/// The counts are PARSE-time and whole-file: they precede the exclusion filter
/// and section selection, so a coerced shot counted here may not be in the
/// import at all. The wording says "in the file" for exactly that reason.
///
/// One sentence per anomaly CLASS, never one per shot - a file with ten
/// thousand duplicate IDs must not push ten thousand warnings through IPC.
fn push_parse_anomaly_warnings(anomalies: TmlParseAnomalies, warnings: &mut Vec<String>) {
    let TmlParseAnomalies {
        shots_missing_id,
        unknown_shot_types,
        duplicate_shot_ids,
    } = anomalies;

    if shots_missing_id > 0 {
        warnings.push(format!(
            "{shots_missing_id} shot{} in the file had a missing or unreadable ID and {} skipped",
            plural_s(shots_missing_id),
            was_were(shots_missing_id)
        ));
    }
    if unknown_shot_types > 0 {
        warnings.push(format!(
            "{unknown_shot_types} shot{} in the file had an unrecognized type and {} read as {}",
            plural_s(unknown_shot_types),
            was_were(unknown_shot_types),
            if unknown_shot_types == 1 {
                "a regular shot"
            } else {
                "regular shots"
            }
        ));
    }
    if duplicate_shot_ids > 0 {
        warnings.push(format!(
            "{duplicate_shot_ids} shot{} in the file reused an ID already used by another shot; \
             only the last shot with each ID was kept",
            plural_s(duplicate_shot_ids)
        ));
    }
}

/// Convert a parsed TML file into `ImportSectionConfig` values ready for
/// `execute_import_sections`.
///
/// A file with no sections is NOT an early return: it still produces the
/// warnings that say why (an unrecognized unit, shots the parser had to drop),
/// and those are the only account the user gets of an import that landed
/// nothing.
pub fn convert_tml_to_import_configs(
    tml: &TmlFile,
    include_excluded: bool,
    comments_on_stations: bool,
) -> Result<(Vec<ImportSectionConfig>, Vec<ImportClosure>, Vec<String>)> {
    convert_tml_with_options(
        tml,
        &TmlImportOptions::basic(include_excluded, comments_on_stations),
    )
}

/// [`convert_tml_to_import_configs`] with the whole option set: the per-section
/// filter and the root-placement choice the import dialog collects (#425).
///
/// Not a second implementation - the shorter entry point above delegates
/// straight here, the same way `execute_import_sections` delegates to
/// `execute_import_sections_inner`. It exists because the callers that have no
/// user in front of them (the SpeleoDB adoption path, every round-trip test)
/// have no selection and no placement to express, and threading two `None`s
/// through them would say nothing.
pub fn convert_tml_with_options(
    tml: &TmlFile,
    opts: &TmlImportOptions,
) -> Result<(Vec<ImportSectionConfig>, Vec<ImportClosure>, Vec<String>)> {
    let include_excluded = opts.include_excluded;
    let comments_on_stations = opts.comments_on_stations;
    let mut warnings: Vec<String> = Vec::new();

    // A selection naming sections this file does not have is not fatal - it
    // simply selects nothing - but silence would leave an empty import with no
    // account of itself. Unreachable from the dialog, which builds the indices
    // from the very file it is previewing.
    let out_of_range = opts.out_of_range_selection(tml.sections.len());
    if out_of_range > 0 {
        warnings.push(format!(
            "{out_of_range} selected section{} did not exist in this file and {} ignored",
            plural_s(out_of_range as u32),
            was_were(out_of_range as u32)
        ));
    }

    if let Some(note) = &tml.encoding_warning {
        warnings.push(note.clone());
    }

    if let Some(notes) = &tml.warnings {
        warnings.extend(notes.iter().cloned());
    }

    if let Some(raw_unit) = &tml.unrecognized_unit {
        warnings.push(format!(
            "The file declares an unrecognized unit \"{raw_unit}\"; all measurements were read as meters"
        ));
    }

    if let Some(anomalies) = tml.parse_anomalies {
        push_parse_anomaly_warnings(anomalies, &mut warnings);
    }

    if let Some(marker) = &tml.scaled_depth_in_marker {
        warnings.push(format!(
            "Read the depth marker written as {} (a scaled -1) as same as previous on {} shot{}.",
            marker.written_as.join(" or "),
            marker.shots,
            plural_s(marker.shots)
        ));
    }

    // What the per-trip section cut could not express (#427).
    if let Some(note) = collapsed_trip_report(&tml.sections, include_excluded) {
        warnings.push(note);
    }

    // #438: azimuths are copied VERBATIM - the fast path this convention was
    // chosen for - and the file's `useMagneticAzimuth` becomes the section's
    // declared north reference instead of arithmetic on every shot.
    let azimuth_reference = tml_azimuth_reference(tml, &mut warnings);

    // Ariane shot ID → (section_index, station_index) for the SELECTED
    // sections, plus the ids a deselection dropped. Station index = the station
    // at the END of a shot: station 0 is the section start, shot 0 goes from
    // station 0 → 1, so shot `i` ends at station `i + 1`.
    let import_options::SelectionIndex {
        id_to_import_loc,
        deselected_ids,
    } = build_selection_index(&tml.sections, opts);

    let mut configs: Vec<ImportSectionConfig> = Vec::new();
    let mut zero_shot_sections_dropped: u32 = 0;

    for (si, section) in tml.sections.iter().enumerate() {
        let section_index = si as u32;

        // A deselected section contributes nothing at all - not a config, not a
        // dropped-section warning. The user said no; that is not an anomaly to
        // report back to them.
        if !opts.includes_section(si) {
            continue;
        }

        // Convert shots to ShotData, using start_depth for correct rebasing
        let shot_data =
            convert_shots_to_shot_data(&section.shots, section.start_depth, include_excluded);

        // Skip sections that end up with zero shots after filtering
        if shot_data.is_empty() {
            tracing::warn!(
                section = ?section.name,
                "Section has no shots after filtering, dropping from import"
            );
            zero_shot_sections_dropped += 1;
            continue;
        }

        // When excluded shots are filtered, the effective first shot may differ
        // from section.start_ariane_id (which is the raw first shot's from_id).
        // Find the from_id of the first non-excluded, non-CLOSURE shot as a
        // fallback for placement when start_ariane_id is itself excluded.
        let effective_start_id = if include_excluded {
            None
        } else {
            let eff = section
                .shots
                .iter()
                .find(|s| survives_import(s, false))
                .map(|s| s.from_id);
            // Only use as fallback if different from start_ariane_id
            eff.filter(|&id| id != section.start_ariane_id)
        };

        // Determine placement: the user's root choice, then start_ariane_id
        // (preserves original topology), then effective_start_id as fallback
        // when the original from_id was excluded and can't be resolved.
        let (placement, placement_warning) = determine_placement(
            section,
            &PlacementInput {
                section_index,
                start_ariane_id: section.start_ariane_id,
                effective_start_id,
                id_to_import_loc: &id_to_import_loc,
                deselected_ids: &deselected_ids,
                // THE definition of root: the first section this loop actually
                // creates. Every early `continue` above - a deselected section,
                // a section the shot filter emptied - is already behind us, so
                // an empty `configs` means nothing has been placed yet.
                // Deriving it from the selection instead (the "lowest selected
                // index" this used to compute) disagreed with the loop exactly
                // when the lowest-selected section was dropped for having no
                // shots, and the user's typed datum went to a section that was
                // never built.
                is_root: configs.is_empty(),
                root_placement: opts.root_placement,
                include_excluded,
                azimuth_reference,
            },
        )
        .map_err(|e| anyhow!("{e}"))?;
        if let Some(w) = placement_warning {
            warnings.push(w);
        }

        // Every shot's Comment field is decoded through the ZWSP marker
        // convention (see `models::comment_split`), on BOTH paths - the setting
        // chooses where the decoded halves land, never whether to decode.
        // Skipping the decode is what #483 was: the raw co-encoded field landed
        // verbatim in the shot comment, invisible marker and all, the station
        // half was silently lost, and the next export re-marked the
        // already-marked text so no later import could tell the halves apart.
        //
        // - comments_on_stations: marked lines stay on the shot, unmarked lines
        //   become the comment of the "to" station (shot_index + 1). This
        //   handles both Stalagmight-authored .tml files (which mark shot
        //   lines) and bare Ariane-authored ones (no markers → everything
        //   routes to the station, matching how Ariane visually centers Comment
        //   text on the station).
        // - otherwise: every line stays on the shot, in the order the field
        //   wrote them, with only the markers removed. Deliberately NOT a
        //   `split` and rejoin: `split` partitions into two groups (see its
        //   `interleaved_lines_partition` test), so rejoining would reorder a
        //   field whose marked and unmarked lines interleave - which is what an
        //   Ariane user produces by typing a note above our marked text. Their
        //   lines are their own; reordering them can change what they mean.
        let mut station_comments: Vec<(u32, String)> = Vec::new();
        let shots: Vec<ShotData> = shot_data
            .into_iter()
            .enumerate()
            .map(|(shot_idx, mut shot)| {
                if comments_on_stations {
                    let (shot_part, station_part) =
                        crate::models::comment_split::split(shot.comment.as_deref());
                    // A field of nothing but a marker decodes to an empty
                    // string, which is not the same as "no comment" to anything
                    // asking whether a shot HAS one.
                    shot.comment = shot_part
                        .map(|s| crate::models::comment_split::strip_markers(&s))
                        .filter(|s| !s.is_empty());
                    if let Some(text) = station_part
                        .map(|s| crate::models::comment_split::strip_markers(&s))
                        .filter(|s| !s.is_empty())
                    {
                        station_comments.push((shot_idx as u32 + 1, text));
                    }
                } else {
                    shot.comment = shot
                        .comment
                        .as_deref()
                        .map(crate::models::comment_split::strip_markers)
                        .filter(|s| !s.is_empty());
                }
                shot
            })
            .collect();

        configs.push(ImportSectionConfig {
            reverse: false,
            section_index,
            name: Some(section.name.clone()),
            surveyor: section.surveyor.clone(),
            date: section.date,
            color: section.color.clone(),
            direction: None, // Ariane doesn't distinguish in/out direction
            quality: 1.0,
            shots,
            placement,
            station_comments,
            azimuth_reference: Some(azimuth_reference),
        });
    }

    // When excluded-shot filtering drops entire branches, simplify names
    // like "xxx (branch 1)" back to "xxx" if only one branch survives.
    simplify_lone_branch_names(&mut configs);

    // Topologically sort configs so that any section referenced by a
    // FromImporting placement is created before its dependent. This handles
    // forward references that arise when excluded-shot filtering changes
    // which section a given section attaches to.
    topological_sort_configs(&mut configs, ConfigOrder::KeepValid);

    // Ariane IDs that name an ENTRANCE (a START shot): they are some section's
    // parent but are no shot in the file, because START shots carry no
    // measurement and never enter a section. Needed to tell the two reasons a
    // closure endpoint fails to resolve apart - "you filtered it out" and "it
    // is the entrance, which this importer has no station for".
    let shot_ids: std::collections::HashSet<i64> = tml
        .sections
        .iter()
        .flat_map(|s| &s.shots)
        .map(|s| s.id)
        .collect();
    let entrance_ids: std::collections::HashSet<i64> = tml
        .sections
        .iter()
        .map(|s| s.start_ariane_id)
        .filter(|id| *id >= 0 && !shot_ids.contains(id))
        .collect();

    // Extract loop closures by resolving CLOSURE shots through id_to_import_loc.
    // CLOSURE shots have from_id (the station the closure originates from) and
    // closure_to_id (the target station). Both must resolve to import-space
    // coordinates; closures where either endpoint was filtered out are skipped.
    let mut import_closures = Vec::new();
    let mut closures_with_excluded_endpoints: u32 = 0;
    let mut closures_to_entrance: u32 = 0;
    let mut closures_to_deselected: u32 = 0;
    for (si, section) in tml.sections.iter().enumerate() {
        // A CLOSURE row inside a section the user deselected is not theirs to
        // hear about: nothing at either end of it is being imported.
        if !opts.includes_section(si) {
            continue;
        }
        for shot in &section.shots {
            if shot.shot_type != TmlShotType::Closure || shot.closure_to_id < 0 {
                continue;
            }
            let from = id_to_import_loc.get(&shot.from_id);
            let to = id_to_import_loc.get(&shot.closure_to_id);
            if let (Some(&(from_si, from_st)), Some(&(to_si, to_st))) = (from, to) {
                import_closures.push(ImportClosure {
                    from_section_index: from_si,
                    from_station_index: from_st,
                    to_section_index: to_si,
                    to_station_index: to_st,
                });
            } else if deselected_ids.contains(&shot.from_id)
                || deselected_ids.contains(&shot.closure_to_id)
            {
                // Checked BEFORE the exclusion arm: a deselected endpoint is
                // the user's own live choice, undone by re-selecting the
                // section, and blaming their exclusion filter for it would send
                // them to the wrong toggle.
                tracing::warn!(
                    section = ?section.name,
                    "Loop closure referenced a deselected section and was skipped"
                );
                closures_to_deselected = closures_to_deselected.saturating_add(1);
            } else if (from.is_none() && entrance_ids.contains(&shot.from_id))
                || (to.is_none() && entrance_ids.contains(&shot.closure_to_id))
            {
                // Nothing was excluded here - the closure simply ends at the
                // entrance station, which the import does not model as a
                // station of any section. Counted apart so the receipt stops
                // blaming the user's exclusion filter for it.
                tracing::warn!(
                    section = ?section.name,
                    "Loop closure referenced an entrance (START) station and was skipped"
                );
                closures_to_entrance = closures_to_entrance.saturating_add(1);
            } else {
                tracing::warn!(
                    section = ?section.name,
                    "Loop closure referenced an excluded station and was skipped"
                );
                closures_with_excluded_endpoints += 1;
            }
        }
    }

    // Defense in depth for the class of bug the `is_root` comment above
    // describes. The placement is applied by exactly one section, so if this
    // conversion built no section at all it went nowhere - and a survey quietly
    // appearing at (0, 0) after the user typed coordinates is the one outcome
    // that must never be silent. Reachable today only via an empty import
    // (nothing selected, or every section filtered away), where the receipt is
    // otherwise empty and this is the only line explaining it.
    if opts.root_placement.is_some() && configs.is_empty() {
        warnings
            .push("Nothing was imported, so the placement you chose was not applied".to_string());
    }

    if zero_shot_sections_dropped > 0 {
        warnings.push(format!(
            "{zero_shot_sections_dropped} section{} had no shots after filtering and {} dropped",
            if zero_shot_sections_dropped == 1 {
                ""
            } else {
                "s"
            },
            if zero_shot_sections_dropped == 1 {
                "was"
            } else {
                "were"
            }
        ));
    }
    if closures_with_excluded_endpoints > 0 {
        warnings.push(format!(
            "{closures_with_excluded_endpoints} loop closure{} referenced excluded stations and \
             {} skipped",
            if closures_with_excluded_endpoints == 1 {
                ""
            } else {
                "s"
            },
            if closures_with_excluded_endpoints == 1 {
                "was"
            } else {
                "were"
            }
        ));
    }
    if closures_to_entrance > 0 {
        warnings.push(format!(
            "{closures_to_entrance} loop closure{} referenced an entrance station and {} not \
             reconstructed",
            plural_s(closures_to_entrance),
            was_were(closures_to_entrance)
        ));
    }
    if closures_to_deselected > 0 {
        warnings.push(format!(
            "{closures_to_deselected} loop closure{} referenced a section you did not select and \
             {} skipped",
            plural_s(closures_to_deselected),
            was_were(closures_to_deselected)
        ));
    }

    Ok((configs, import_closures, warnings))
}

/// A config's name with a TRAILING `" (branch N)"` removed, or unchanged.
///
/// The suffix has to be at the END, and `N` has to be digits. A merged run is
/// named `"A + B"` (#412) and either part can itself be a branch, giving
/// `"A (branch 2) + B"` - a plain `rfind` would strip from the middle and
/// rename that config to `"A"`, silently dropping the fact that it also holds
/// B. A file-supplied name containing the literal `" (branch "` is likewise
/// left alone unless it really ends in a branch number.
fn branch_base(name: &str) -> &str {
    let suffix = " (branch ";
    let Some(pos) = name.rfind(suffix) else {
        return name;
    };
    let inner = &name[pos + suffix.len()..];
    let Some(number) = inner.strip_suffix(')') else {
        return name;
    };
    if !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit()) {
        &name[..pos]
    } else {
        name
    }
}

/// Simplify branch-suffixed names when only one branch of a group survives.
///
/// During linearization, sections that branch get names like "Foo (branch 1)",
/// "Foo (branch 2)". When excluded-shot filtering drops all shots from some
/// branches, those empty sections are removed. If only one branch remains for
/// a given base name, strip the suffix so it's just "Foo".
fn simplify_lone_branch_names(configs: &mut [ImportSectionConfig]) {
    // Count how many configs share each base name (name without " (branch N)")
    let mut base_counts: HashMap<String, u32> = HashMap::new();
    for c in configs.iter() {
        let name = c.name.as_deref().unwrap_or("");
        *base_counts
            .entry(branch_base(name).to_string())
            .or_insert(0) += 1;
    }

    // Rename lone survivors
    for c in configs.iter_mut() {
        let name = match c.name.as_deref() {
            Some(n) => n,
            None => continue,
        };
        let base = branch_base(name);
        if base.len() != name.len() && base_counts.get(base).copied() == Some(1) {
            c.name = Some(base.to_string());
        }
    }
}

/// Resolve a TML shot's entry depth, handling the `-1.0` "same as previous" sentinel.
///
/// `prev_depth` is the depth the previous leg in the file ends at, whether that
/// leg was kept or dropped, CLOSURE rows skipped (`None` for the first shot in a
/// section), `start_depth` the parent station's cumulative depth.
///
/// The `-1.0` is the FILE FORMAT's marker, not this parser's invention, so a
/// literal `-1.0` in the file means "inherit" even though -1 m is a perfectly
/// legal depth one metre above the datum. `export::tml::emitted_depth_in` is the
/// other half of that contract: it nudges a genuine -1.0 to -1.000001 before
/// writing, precisely so this reader does not swallow it. Making a literal
/// `-1.0` measured here would misread every ordinary Ariane file, where the
/// sentinel is spelled out on nearly every shot.
fn resolve_tml_depth_in(shot: &TmlShot, prev_depth: Option<f64>, start_depth: f64) -> f64 {
    if schema::is_depth_in_sentinel(shot.depth_in) {
        prev_depth.unwrap_or(start_depth)
    } else {
        shot.depth_in
    }
}

/// Resolve a TML shot's `pitch_in`, treating a wire `0.0` inclination as
/// "not recorded" exactly when reading it as `Some(0.0)` would fabricate an
/// `INCLINATION_DEPTH_MISMATCH` warning.
///
/// `schema::pitch_to_inclination` collapses both `None` (never measured)
/// and `Some(0.0)` (genuinely level) to the same wire `0.0`, so import cannot
/// tell them apart from the inclination field alone (#482). Gate on the real
/// condition instead of a proxy: compute what the shot's own geometry implies
/// the inclination should have been ([`implied_pitch_deg`], the SAME
/// computation `core::validation`'s cross-check uses), and compare it
/// against the SAME default threshold the validator would - a wire `0.0`
/// only turns into `None` when a recorded `Some(0.0)` would have exceeded
/// that threshold and been flagged. `DEFAULT_INCLINATION_DEPTH_DIFF_THRESHOLD`
/// (the config default), not the user's live `ValidationConfig`: the import
/// mapping must be deterministic, not move if someone edits the threshold.
///
/// Two consequences are deliberate residual losses, not bugs:
/// - A genuinely recorded `Some(0.0)` on a STEEP shot (implied pitch beyond
///   the threshold) is erased to `None` by one round trip. The wire cannot
///   distinguish it from an absent reading, and the warning it would have
///   produced was never earnable back from a lossy `0.0` in the first place.
/// - Corrupt geometry - wire `0.0` paired with a genuinely wrong depth or
///   length that implies a steep pitch - imports silently, with no warning.
///   No importer-side rule can recover that warning without re-fabricating
///   #482's false positives for every ordinary depth-only survey.
///
/// A near-level shot (implied pitch under the threshold) keeps `Some(0.0)`:
/// unlike the steep case, that reading would not have warned anyway, so
/// there is no ambiguity worth resolving in the importer's favor.
///
/// Only fires on an exact wire `0.0`: any other inclination value came from
/// somewhere and is trusted as recorded, per [`inclination_to_pitch`].
fn tml_pitch_in(inclination: f64, length: f64, depth_in: f64, depth_out: f64) -> Option<f64> {
    if inclination == 0.0 {
        let implied = implied_pitch_deg(length, depth_in, depth_out);
        if implied.abs() > DEFAULT_INCLINATION_DEPTH_DIFF_THRESHOLD {
            return None;
        }
    }
    Some(inclination_to_pitch(inclination))
}

/// Canonical mapping of a single Ariane TML shot to Stalagmight's `ShotData`.
///
/// This is the SINGLE source of truth for the TML to Stalagmight field mapping
/// (azimuth `rem_euclid(360)`, depth in/out, inclination to `pitch_in`, LRUD 0
/// to None, VIRTUAL to a virtual [`ShotKind`]). `depth_in` must already be
/// resolved via [`resolve_tml_depth_in`].
pub fn normalize_tml_shot(shot: &TmlShot, depth_in: f64) -> ShotData {
    // LRUD: pass through, using None for zero values
    let lrud_opt = |v: f64| {
        if v.abs() < f64::EPSILON {
            None
        } else {
            Some(v)
        }
    };

    // Ariane stores azimuths as arbitrary angles (can be negative or >360).
    // Normalize to [0, 360) for Stalagmight's data model.
    //
    // The shared helper, not a bare `rem_euclid`: for a tiny negative input `rem_euclid` returns
    // exactly `360.0` (`(-1e-14f64).rem_euclid(360.0) == 360.0`), which is OUT of range and trips
    // `INVALID_AZIMUTH` - a warning `warning_is_ignorable` refuses to suppress. The v14 migration
    // establishes "every stored azimuth is in [0, 360)" for existing files, and it is gated on
    // `current < 14`, so a value written here afterwards is never revisited.
    let az = crate::models::normalize_azimuth(shot.azimuth);

    ShotData {
        // The file's own shot id, carried so the round trip re-emits it (#423).
        // The TML importer is the only one that sets this; every other importer
        // leaves it `None` and gets a fresh mint, exactly as before.
        uuid: shot.uuid.clone(),
        azimuth_in: az,
        azimuth_out: az, // No backsight in Ariane
        length: shot.length,
        depth_in,
        depth_out: shot.depth,
        pitch_in: tml_pitch_in(shot.inclination, shot.length, depth_in, shot.depth),
        pitch_out: None, // No backsight inclination
        left: lrud_opt(shot.left),
        right: lrud_opt(shot.right),
        up: lrud_opt(shot.up),
        down: lrud_opt(shot.down),
        comment: shot.comment.clone(),
        // Ariane has a single VIRTUAL category, so the parser can only decide
        // virtual-vs-not. Which virtual kind it is comes from the chain
        // classifier at import finalize (#27 q10/q19): a virtual chain bounded
        // by real survey on both ends is a Jump, anything else a Reference.
        // Reference is the provisional value because it is the conservative
        // one - it keeps the leg out of gas and penetration until the graph
        // proves it is a swum shortcut.
        kind: if shot.shot_type == TmlShotType::Virtual {
            ShotKind::Reference
        } else {
            ShotKind::Normal
        },
        marker_index: None,
        temperature: None,
    }
}

/// Whether the import builds `shot` - THE rule, used everywhere the importer
/// decides which rows become shots (#576 E17). CLOSURE rows never do: they are
/// loop-closure markers, not survey measurements, whatever tape reading they
/// carry (our own exporter gives them the vertical gap between the two nodes
/// they tie), and their topology is captured separately. Excluded rows do only
/// when the user ticks "include excluded shots".
///
/// The import dialog's preview applies the same rule in TypeScript
/// (`survivesImport`, src/utils/tmlImportStats.ts) so it can recompute on every
/// toggle without a round trip; the two are pinned by the same truth table,
/// here (`survives_import_truth_table`) and in
/// src/utils/__tests__/tmlImportStats.test.ts.
pub(crate) fn survives_import(shot: &TmlShot, include_excluded: bool) -> bool {
    shot.shot_type != TmlShotType::Closure && (include_excluded || !shot.excluded)
}

/// Convert TML shots to `ShotData` for import.
///
/// Depth handling: Ariane `DP` is absolute station depth (depth gauge reading).
/// For each shot, `depth_in` = DPI if available, otherwise the DP of the raw
/// leg before it, dropped or not (or `start_depth` for the first shot).
/// `depth_out` = this shot's DP.
/// Depths are stored as absolute values - no rebasing is applied.
fn convert_shots_to_shot_data(
    shots: &[TmlShot],
    start_depth: f64,
    include_excluded: bool,
) -> Vec<ShotData> {
    if shots.is_empty() {
        return Vec::new();
    }

    let real_shots: Vec<&TmlShot> = shots
        .iter()
        .filter(|s| survives_import(s, include_excluded))
        .collect();

    if real_shots.is_empty() {
        return Vec::new();
    }

    // A "same as previous" `DepthIn` is the depth the file records where the
    // shot starts: the end of the leg before it, kept or dropped. A dropped
    // leg moves where the next kept shot starts, never that station's depth -
    // a dropped lead (#576 NF-TML-2) or a leg mid-section (#576 W27, the tml
    // metres-vs-feet seed 363281). A CLOSURE row is no leg.
    let mut result = Vec::with_capacity(real_shots.len());
    let mut leg_end_depth = None;
    for shot in shots {
        if survives_import(shot, include_excluded) {
            let depth_in = resolve_tml_depth_in(shot, leg_end_depth, start_depth);
            result.push(normalize_tml_shot(shot, depth_in));
        }
        if shot.shot_type != TmlShotType::Closure {
            leg_end_depth = Some(shot.depth);
        }
    }

    result
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr,
    clippy::print_stdout
)]
mod tests {
    use super::linearize::{MAX_TRIP_CUTS_PER_FILE, MAX_TRIP_CUTS_PER_RUN};
    use super::test_helpers::*;
    use super::*;
    use crate::models::test_support::require_env_file;
    use crate::models::ImportPlacement;
    use crate::parsers::station_graph::dependency_order;

    /// #576 B2-MARKER: the placeholder 2000-01-01 reads as "no date" whether
    /// or not our marker comment follows it - a file Ariane re-saved drops the
    /// comment, an export from before the marker never had it, and the
    /// SpeleoDB sidecar never writes it. The marker never erases any other
    /// date, and any other comment changes nothing.
    #[test]
    fn the_placeholder_date_reads_as_undated_with_or_without_the_marker() {
        // Every row hangs off the START, so no two sections chain end to start
        // (#412 would sew same-trip neighbours into one).
        let row = |id: i64, section: &str, date_and_comment: &str| {
            tml_shot_xml(id, 0, "REAL", section)
                .replace("<Date>2024-06-15</Date>", date_and_comment)
        };
        let marker = format!("<!--{}-->", schema::UNDATED_MARKER);
        let xml = wrap_xml(
            &[
                tml_shot_xml(0, -1, "START", "Start"),
                row(
                    1,
                    "Marked",
                    &format!("<Date>2000-01-01</Date>\n    {marker}"),
                ),
                row(2, "Unmarked", "<Date>2000-01-01</Date>"),
                row(3, "Real", &format!("<Date>2019-02-03</Date>{marker}")),
                row(4, "Other", "<Date>2000-01-01</Date><!-- hand edited -->"),
            ]
            .join("\n"),
        );

        let tml = parse_synthetic_tmlu(&xml);
        let date_of = |name: &str| {
            tml.sections
                .iter()
                .find(|s| s.name == name)
                .unwrap_or_else(|| panic!("no section {name}"))
                .date
        };
        assert_eq!(date_of("Marked"), None);
        assert_eq!(date_of("Unmarked"), None);
        assert_eq!(date_of("Real"), NaiveDate::from_ymd_opt(2019, 2, 3));
        assert_eq!(date_of("Other"), None);
    }

    /// The reader keys every per-shot element the exporter writes from the same
    /// `schema::element` table, in BOTH spellings, onto the field it means
    /// (#576 B4). The three elements the exporter writes for Ariane's sake and
    /// the import has no use for map to nothing, explicitly.
    #[test]
    fn every_schema_shot_element_reads_back_as_its_field_in_both_spellings() {
        let expected: &[(schema::ElementName, Option<&str>)] = &[
            (el::azimuth::NAME, Some("azimuth")),
            (el::closure_to_id::NAME, Some("closure_to_id")),
            (el::color::NAME, Some("color")),
            (el::comment::NAME, Some("comment")),
            (el::date::NAME, Some("date")),
            (el::depth::NAME, Some("depth")),
            (el::depth_in::NAME, Some("depth_in")),
            (el::down::NAME, Some("down")),
            (el::excluded::NAME, Some("excluded")),
            (el::explorer::NAME, Some("explorer")),
            (el::from_id::NAME, Some("from_id")),
            (el::id::NAME, Some("id")),
            (el::inclination::NAME, Some("inclination")),
            (el::latitude::NAME, Some("latitude")),
            (el::left::NAME, Some("left")),
            (el::length::NAME, Some("length")),
            (el::locked::NAME, None),
            (el::longitude::NAME, Some("longitude")),
            (el::name::NAME, None),
            (el::profiletype::NAME, None),
            (el::right::NAME, Some("right")),
            (el::section::NAME, Some("section")),
            (el::shot_type::NAME, Some("type")),
            (el::up::NAME, Some("up")),
            (el::uuid::NAME, Some("uuid")),
        ];
        for (element, key) in expected {
            for spelling in [element.long, element.short] {
                assert_eq!(
                    normalize_key_bytes(spelling.as_bytes()),
                    *key,
                    "<{spelling}> must read as {key:?}"
                );
            }
        }
        // Every per-shot element of the schema is accounted for above: the
        // file-level ones and the skipped `Shape` are the only others.
        let file_level = [
            el::cave_file::NAME,
            el::cave_name::NAME,
            el::unit::NAME,
            el::use_magnetic_azimuth::NAME,
            el::first_start_absolute_elevation::NAME,
            el::data::NAME,
            el::survey_data::NAME,
            el::shape::NAME,
        ];
        assert_eq!(
            schema::element::ALL.len(),
            expected.len() + file_level.len(),
            "a new schema element needs a row in this table"
        );
        assert_eq!(normalize_key_bytes(b"Id"), Some("id"));
    }

    /// A tiny NEGATIVE azimuth must normalise to `0.0`, not `360.0`.
    ///
    /// `rem_euclid` alone returns exactly `360.0` for such an input, which is OUT of the
    /// `[0, 360)` range the data model requires: it trips `INVALID_AZIMUTH`, a warning
    /// `warning_is_ignorable` refuses to suppress. The v14 migration establishes that invariant
    /// for existing files and is gated on `current < 14`, so anything this importer writes
    /// afterwards is never revisited - the importer has to get it right itself.
    #[test]
    fn a_tiny_negative_azimuth_normalises_to_zero_not_360() {
        assert_eq!(
            (-1e-14f64).rem_euclid(360.0),
            360.0,
            "premise: the bare operation this guards against really does return 360"
        );

        let mut shot = sample_tml_shot();
        shot.azimuth = -1e-14;
        let out = normalize_tml_shot(&shot, 0.0);
        assert_eq!(out.azimuth_in, 0.0);
        assert_eq!(out.azimuth_out, 0.0);

        // And the ordinary wraps still behave.
        for (input, expected) in [(-90.0, 270.0), (450.0, 90.0), (360.0, 0.0), (12.5, 12.5)] {
            shot.azimuth = input;
            assert_eq!(normalize_tml_shot(&shot, 0.0).azimuth_in, expected);
        }
    }

    // -------------------------------------------------------------------
    // Foreign shot identity (#423)
    // -------------------------------------------------------------------

    /// The gate is XML 1.0's `Char` production, not the control range. U+FFFE and
    /// U+FFFF are excluded by the production and are NOT control characters, so a
    /// `char::is_control` gate adopted them - and `BytesText::new` escapes only
    /// `<>&"'`, so they went into element content raw and the document stopped
    /// being XML. One accepted bad byte costs the WHOLE exported file (a strict
    /// consumer rejects the document, not the row), so this rejects rather than
    /// repairs, and the shot gets an ordinary fresh mint.
    #[test]
    fn foreign_shot_uuid_rejects_characters_xml_forbids() {
        const GOOD: &str = "beefbeef-0000-4000-8000-000000000001";
        assert_eq!(
            foreign_shot_uuid(1, Some(GOOD)),
            Some(GOOD.to_owned()),
            "a well-formed id is adopted verbatim, never coerced"
        );

        // Excluded by `Char`, but NOT control characters - the gap this closes.
        assert_eq!(foreign_shot_uuid(1, Some("a\u{FFFF}b")), None);
        assert_eq!(foreign_shot_uuid(1, Some("a\u{FFFE}b")), None);
        // C0, C1, and NUL: control characters, rejected as before.
        assert_eq!(foreign_shot_uuid(1, Some("a\u{1}b")), None);
        assert_eq!(foreign_shot_uuid(1, Some("a\u{9F}b")), None);
        assert_eq!(foreign_shot_uuid(1, Some("a\u{0}b")), None);
        // Tab/LF/CR are legal XML but not legal in an identifier.
        assert_eq!(foreign_shot_uuid(1, Some("a\tb")), None);
        // Absent, empty and whitespace-only all mean "no identity".
        assert_eq!(foreign_shot_uuid(1, None), None);
        assert_eq!(foreign_shot_uuid(1, Some("")), None);
        assert_eq!(foreign_shot_uuid(1, Some("   ")), None);
        // Surrounding whitespace is trimmed, not adopted: identity compares
        // byte-verbatim downstream, and a whitespace-normalising XML consumer
        // would hand " x " back as "x" - a different identity - on the first
        // round trip.
        assert_eq!(
            foreign_shot_uuid(1, Some(&format!("  {GOOD}\n"))),
            Some(GOOD.to_owned())
        );
        // Length is bounded: at the cap is fine, one past it is not.
        let at_cap = "a".repeat(MAX_FOREIGN_UUID_LEN);
        assert_eq!(foreign_shot_uuid(1, Some(&at_cap)), Some(at_cap.clone()));
        assert_eq!(
            foreign_shot_uuid(1, Some(&"a".repeat(MAX_FOREIGN_UUID_LEN + 1))),
            None
        );
        // U+FFFD and astral characters ARE in `Char` and stay adopted, so the
        // gate cannot quietly widen into "reject anything unusual".
        assert_eq!(
            foreign_shot_uuid(1, Some("a\u{FFFD}b")),
            Some("a\u{FFFD}b".to_owned())
        );
        assert_eq!(
            foreign_shot_uuid(1, Some("a\u{1F300}b")),
            Some("a\u{1F300}b".to_owned())
        );
    }

    // -------------------------------------------------------------------
    // Color conversion tests
    // -------------------------------------------------------------------

    #[test]
    fn test_color_conversion_standard() {
        assert_eq!(schema::color_from_ariane(Some("0x8f6112ff")), "#8f6112");
        assert_eq!(schema::color_from_ariane(Some("0xFF0000FF")), "#FF0000");
    }

    #[test]
    fn parse_rejects_non_finite_numeric_fields() {
        // Rust's f64 parser accepts "NaN"/"Infinity"; a corrupt/hostile .tml must
        // not seed non-finite geometry. parse_f64 coerces them to the field default
        // (0.0) so nothing non-finite reaches a RawShot.
        let xml = wrap_xml(
            r#"<SurveyData>
    <ID>1</ID>
    <FromID>0</FromID>
    <ClosureToID>-1</ClosureToID>
    <Type>REAL</Type>
    <Section>S</Section>
    <Azimut>NaN</Azimut>
    <Length>Infinity</Length>
    <Depth>-Infinity</Depth>
    <DepthIn>-1.0</DepthIn>
    <Inclination>NaN</Inclination>
    <Left>1.0</Left>
    <Right>2.0</Right>
    <Up>3.0</Up>
    <Down>4.0</Down>
    <Comment></Comment>
    <Explorer>Tester</Explorer>
    <Date>2024-06-15</Date>
    <Color>0x8f6112ff</Color>
    <Excluded>false</Excluded>
    <Latitude>0.0</Latitude>
    <Longitude>0.0</Longitude>
</SurveyData>"#,
        );
        let (shots, _meta) = parse_raw_shots(&xml).unwrap();
        let s = shots.iter().find(|s| s.id == 1).expect("shot 1 parsed");
        assert!(s.azimuth.is_finite(), "azimuth NaN must be coerced");
        assert!(s.length.is_finite(), "length inf must be coerced");
        assert!(s.depth.is_finite(), "depth -inf must be coerced");
        assert!(s.inclination.is_finite(), "inclination NaN must be coerced");
        assert_eq!(s.length, 0.0);
    }

    #[test]
    fn test_color_conversion_fallback() {
        assert_eq!(schema::color_from_ariane(None), "#4ecdc4");
        assert_eq!(schema::color_from_ariane(Some("")), "#4ecdc4");
        assert_eq!(schema::color_from_ariane(Some("invalid")), "#4ecdc4");
        assert_eq!(schema::color_from_ariane(Some("0xZZZZZZZZ")), "#4ecdc4");
        assert_eq!(schema::color_from_ariane(Some("0x123")), "#4ecdc4"); // too short
    }

    // -------------------------------------------------------------------
    // XML parsing: TML long keys
    // -------------------------------------------------------------------

    #[test]
    fn test_parse_tml_long_keys() {
        let xml = wrap_xml(
            &[
                tml_shot_xml(0, -1, "START", "Start"),
                tml_shot_xml(1, 0, "REAL", "Section 1"),
            ]
            .join("\n"),
        );

        let (shots, meta) = parse_raw_shots(&xml).unwrap_or_else(|_| {
            std::process::abort(); // unreachable in tests
            #[expect(unreachable_code)]
            {
                (Vec::new(), FileMetadata::default())
            }
        });

        // Must not panic-unwrap, use assert
        assert_eq!(meta.cave_name, "Test Cave");
        assert_eq!(meta.unit, "m");
        assert!(meta.use_magnetic_azimuth);
        assert!((meta.first_start_absolute_elevation - 100.0).abs() < f64::EPSILON);
        assert_eq!(shots.len(), 2);
        assert_eq!(shots[0].shot_type, TmlShotType::Start);
        assert_eq!(shots[1].shot_type, TmlShotType::Real);
        assert!((shots[1].azimuth - 90.0).abs() < f64::EPSILON);
        assert!((shots[1].length - 10.0).abs() < f64::EPSILON);
    }

    // -------------------------------------------------------------------
    // XML parsing: TMLU short keys
    // -------------------------------------------------------------------

    #[test]
    fn test_parse_tmlu_short_keys() {
        let xml = wrap_xml(
            &[
                tmlu_shot_xml(0, -1, "START", "Start"),
                tmlu_shot_xml(1, 0, "REAL", "Sec A"),
            ]
            .join("\n"),
        );

        let (shots, _) =
            parse_raw_shots(&xml).unwrap_or_else(|_| (Vec::new(), FileMetadata::default()));

        assert_eq!(shots.len(), 2);
        assert_eq!(shots[1].section, "Sec A");
        assert!((shots[1].azimuth - 45.0).abs() < f64::EPSILON);
        assert!((shots[1].length - 5.0).abs() < f64::EPSILON);
    }

    // -------------------------------------------------------------------
    // START shot exclusion
    // -------------------------------------------------------------------

    #[test]
    fn test_start_shots_excluded_from_sections() {
        let xml = wrap_xml(
            &[
                tml_shot_xml(0, -1, "START", "Start"),
                tml_shot_xml(1, 0, "REAL", "Section 1"),
                tml_shot_xml(2, 1, "REAL", "Section 1"),
            ]
            .join("\n"),
        );

        let (raw_shots, _) =
            parse_raw_shots(&xml).unwrap_or_else(|_| (Vec::new(), FileMetadata::default()));

        let (sections, _) = build_sections(raw_shots);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].shots.len(), 2);
        // No START shot in the section
        for shot in &sections[0].shots {
            assert_ne!(shot.shot_type, TmlShotType::Start);
        }
    }

    // -------------------------------------------------------------------
    // GPS propagation from START shots
    // -------------------------------------------------------------------

    #[test]
    fn test_start_gps_propagated_to_first_real_shot() {
        // START shot has GPS, its successor REAL shot should inherit it
        let xml = wrap_xml(
            &[
                tml_shot_xml_gps(0, -1, "START", "Entrance", 20.2, -87.5),
                tml_shot_xml(1, 0, "REAL", "Entrance"),
                tml_shot_xml(2, 1, "REAL", "Entrance"),
            ]
            .join("\n"),
        );

        let (raw_shots, _) =
            parse_raw_shots(&xml).unwrap_or_else(|_| (Vec::new(), FileMetadata::default()));

        let (sections, _) = build_sections(raw_shots);
        assert_eq!(sections.len(), 1);
        // First shot should have GPS from the START shot
        let first = &sections[0].shots[0];
        assert!((first.latitude - 20.2).abs() < f64::EPSILON);
        assert!((first.longitude - (-87.5)).abs() < f64::EPSILON);
        // Second shot should NOT have GPS (not a direct successor of START)
        let second = &sections[0].shots[1];
        assert!(second.latitude.abs() < f64::EPSILON);
        assert!(second.longitude.abs() < f64::EPSILON);
    }

    #[test]
    fn test_start_gps_not_overwritten_if_successor_has_own_gps() {
        // Successor already has GPS - should keep its own
        let xml = wrap_xml(
            &[
                tml_shot_xml_gps(0, -1, "START", "Entrance", 20.2, -87.5),
                tml_shot_xml_gps(1, 0, "REAL", "Entrance", 19.0, -88.0),
            ]
            .join("\n"),
        );

        let (raw_shots, _) =
            parse_raw_shots(&xml).unwrap_or_else(|_| (Vec::new(), FileMetadata::default()));

        let (sections, _) = build_sections(raw_shots);
        let first = &sections[0].shots[0];
        assert!((first.latitude - 19.0).abs() < f64::EPSILON);
        assert!((first.longitude - (-88.0)).abs() < f64::EPSILON);
    }

    #[test]
    fn test_start_gps_produces_zero_datum_placement() {
        // When START has GPS and it propagates, section 0 should get ZeroDatum
        let xml = wrap_xml(
            &[
                tml_shot_xml_gps(0, -1, "START", "Entrance", 20.2, -87.5),
                tml_shot_xml(1, 0, "REAL", "Entrance"),
                tml_shot_xml(2, 1, "REAL", "Entrance"),
            ]
            .join("\n"),
        );

        let (raw_shots, metadata) =
            parse_raw_shots(&xml).unwrap_or_else(|_| (Vec::new(), FileMetadata::default()));
        let (sections, closures) = build_sections(raw_shots);

        // geo_coding should be populated from propagated GPS
        let geo_coding = sections
            .iter()
            .flat_map(|s| &s.shots)
            .find(|shot| shot.latitude.abs() > f64::EPSILON || shot.longitude.abs() > f64::EPSILON)
            .map(|shot| TmlGeoCoding {
                latitude: shot.latitude,
                longitude: shot.longitude,
                altitude: metadata.first_start_absolute_elevation,
            });

        let tml = TmlFile {
            cave_name: metadata.cave_name,
            unit: TmlUnit::Metric,
            use_magnetic_azimuth: true,
            first_start_absolute_elevation: 100.0,
            unrecognized_unit: None,
            parse_anomalies: None,
            encoding_warning: None,
            scaled_depth_in_marker: None,
            warnings: None,
            geo_coding,
            sections,
            closures,
        };

        let (configs, _, _) = convert_tml_to_import_configs(&tml, false, false)
            .unwrap_or_else(|_| (Vec::new(), Vec::new(), Vec::new()));
        assert_eq!(configs.len(), 1);
        match &configs[0].placement {
            ImportPlacement::ZeroDatum {
                coordinates,
                station_index,
            } => {
                assert!((coordinates.latitude - 20.2).abs() < f64::EPSILON);
                assert!((coordinates.longitude - (-87.5)).abs() < f64::EPSILON);
                assert_eq!(*station_index, 0);
            }
            other => {
                assert!(false, "Expected ZeroDatum, got {other:?}");
            }
        }
    }

    #[test]
    fn test_geo_coding_extracted_from_propagated_gps() {
        // TmlFile.geo_coding should be populated from GPS that was on a START shot
        let xml = wrap_xml(
            &[
                tml_shot_xml_gps(0, -1, "START", "Entrance", 20.2, -87.5),
                tml_shot_xml(1, 0, "REAL", "Entrance"),
            ]
            .join("\n"),
        );

        let (raw_shots, _) =
            parse_raw_shots(&xml).unwrap_or_else(|_| (Vec::new(), FileMetadata::default()));
        let (sections, _) = build_sections(raw_shots);

        // geo_coding extraction (same logic as parse_tml)
        let geo_coding = sections
            .iter()
            .flat_map(|s| &s.shots)
            .find(|shot| shot.latitude.abs() > f64::EPSILON || shot.longitude.abs() > f64::EPSILON)
            .map(|shot| TmlGeoCoding {
                latitude: shot.latitude,
                longitude: shot.longitude,
                altitude: 100.0,
            });

        assert!(geo_coding.is_some());
        if let Some(gc) = &geo_coding {
            assert!((gc.latitude - 20.2).abs() < f64::EPSILON);
            assert!((gc.longitude - (-87.5)).abs() < f64::EPSILON);
        }
    }

    // -------------------------------------------------------------------
    // VIRTUAL shot marking
    // -------------------------------------------------------------------

    #[test]
    fn test_virtual_shot_detected() {
        let xml = wrap_xml(
            &[
                tml_shot_xml(0, -1, "START", "Start"),
                tml_shot_xml(1, 0, "VIRTUAL", "Section 1"),
            ]
            .join("\n"),
        );

        let (raw_shots, _) =
            parse_raw_shots(&xml).unwrap_or_else(|_| (Vec::new(), FileMetadata::default()));

        let (sections, _) = build_sections(raw_shots);
        assert_eq!(sections[0].shots[0].shot_type, TmlShotType::Virtual);
    }

    // -------------------------------------------------------------------
    // Depth conversion
    // -------------------------------------------------------------------

    #[test]
    fn test_depth_conversion_linear_chain() {
        // 3 shots with cumulative depths 0, 5, 12
        let xml = wrap_xml(&format!(
            r#"
            <SurveyData>
                <ID>0</ID><FromID>-1</FromID><ClosureToID>-1</ClosureToID>
                <Type>START</Type><Section>Start</Section>
                <Azimut>0</Azimut><Length>0</Length><Depth>0</Depth><DepthIn>-1.0</DepthIn>
                <Inclination>0</Inclination><Left>0</Left><Right>0</Right><Up>0</Up><Down>0</Down>
                <Color>0x000000ff</Color><Excluded>false</Excluded><Latitude>0</Latitude><Longitude>0</Longitude>
            </SurveyData>
            <SurveyData>
                <ID>1</ID><FromID>0</FromID><ClosureToID>-1</ClosureToID>
                <Type>REAL</Type><Section>S1</Section>
                <Azimut>90</Azimut><Length>10</Length><Depth>5.0</Depth><DepthIn>-1.0</DepthIn>
                <Inclination>0</Inclination><Left>1</Left><Right>1</Right><Up>1</Up><Down>1</Down>
                <Color>0x000000ff</Color><Excluded>false</Excluded><Latitude>0</Latitude><Longitude>0</Longitude>
            </SurveyData>
            <SurveyData>
                <ID>2</ID><FromID>1</FromID><ClosureToID>-1</ClosureToID>
                <Type>REAL</Type><Section>S1</Section>
                <Azimut>180</Azimut><Length>8</Length><Depth>12.0</Depth><DepthIn>-1.0</DepthIn>
                <Inclination>-5</Inclination><Left>2</Left><Right>2</Right><Up>2</Up><Down>2</Down>
                <Color>0x000000ff</Color><Excluded>false</Excluded><Latitude>0</Latitude><Longitude>0</Longitude>
            </SurveyData>
            "#
        ));

        let (raw_shots, _) =
            parse_raw_shots(&xml).unwrap_or_else(|_| (Vec::new(), FileMetadata::default()));
        let (sections, _) = build_sections(raw_shots);
        let shot_data = convert_shots_to_shot_data(&sections[0].shots, 0.0, false);

        // Shot 1: depth_in=0 (from START, base=0), depth_out=5
        assert!((shot_data[0].depth_in - 0.0).abs() < f64::EPSILON);
        assert!((shot_data[0].depth_out - 5.0).abs() < f64::EPSILON);

        // Shot 2: depth_in=5 (prev shot's DP), depth_out=12
        assert!((shot_data[1].depth_in - 5.0).abs() < f64::EPSILON);
        assert!((shot_data[1].depth_out - 12.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_depth_conversion_explicit_dpi() {
        // Shot with explicit DepthIn != -1.0
        let xml = wrap_xml(&format!(
            r#"
            <SurveyData>
                <ID>0</ID><FromID>-1</FromID><ClosureToID>-1</ClosureToID>
                <Type>START</Type><Section>Start</Section>
                <Azimut>0</Azimut><Length>0</Length><Depth>0</Depth><DepthIn>-1.0</DepthIn>
                <Inclination>0</Inclination><Left>0</Left><Right>0</Right><Up>0</Up><Down>0</Down>
                <Color>0x000000ff</Color><Excluded>false</Excluded><Latitude>0</Latitude><Longitude>0</Longitude>
            </SurveyData>
            <SurveyData>
                <ID>1</ID><FromID>0</FromID><ClosureToID>-1</ClosureToID>
                <Type>REAL</Type><Section>S1</Section>
                <Azimut>90</Azimut><Length>10</Length><Depth>5.0</Depth><DepthIn>2.0</DepthIn>
                <Inclination>0</Inclination><Left>0</Left><Right>0</Right><Up>0</Up><Down>0</Down>
                <Color>0x000000ff</Color><Excluded>false</Excluded><Latitude>0</Latitude><Longitude>0</Longitude>
            </SurveyData>
            "#
        ));

        let (raw_shots, _) =
            parse_raw_shots(&xml).unwrap_or_else(|_| (Vec::new(), FileMetadata::default()));
        let (sections, _) = build_sections(raw_shots);
        let shot_data = convert_shots_to_shot_data(&sections[0].shots, 0.0, false);

        // DPI=2.0 used as absolute depth_in; depth_out = 5.0 (absolute, no rebasing)
        assert!((shot_data[0].depth_in - 2.0).abs() < f64::EPSILON);
        assert!((shot_data[0].depth_out - 5.0).abs() < f64::EPSILON);
    }

    // -------------------------------------------------------------------
    // Unit conversion
    // -------------------------------------------------------------------

    #[test]
    fn test_imperial_conversion() {
        let mut sections = vec![TmlSection {
            name: "Test".to_string(),
            shots: vec![TmlShot {
                uuid: None,
                id: 1,
                from_id: 0,
                shot_type: TmlShotType::Real,
                azimuth: 90.0,
                length: 10.0, // feet
                depth: 5.0,   // feet
                depth_in: -1.0,
                inclination: 0.0,
                left: 3.0,
                right: 4.0,
                up: 2.0,
                down: 1.0,
                comment: None,
                explorer: None,
                date: None,
                color: "#000000".to_string(),
                excluded: false,
                latitude: 0.0,
                longitude: 0.0,
                closure_to_id: -1,
                section: "Test".to_string(),
            }],
            surveyor: None,
            date: None,
            color: "#000000".to_string(),
            is_branch_split: false,
            start_ariane_id: 0,
            end_ariane_id: 1,
            start_depth: 12.0,
        }];

        let mut elevation = 150.0;
        apply_unit_conversion(&mut sections, &mut elevation, TmlUnit::Imperial);

        let shot = &sections[0].shots[0];
        assert!((shot.length - 10.0 * FEET_TO_METERS).abs() < 1e-6);
        assert!((shot.depth - 5.0 * FEET_TO_METERS).abs() < 1e-6);
        assert!((shot.left - 3.0 * FEET_TO_METERS).abs() < 1e-6);
        assert!((shot.right - 4.0 * FEET_TO_METERS).abs() < 1e-6);
        // depth_in == -1.0 should NOT be converted
        assert!((shot.depth_in - (-1.0)).abs() < f64::EPSILON);
        // The branch rebase depth and the file elevation are lengths too.
        assert!((sections[0].start_depth - 12.0 * FEET_TO_METERS).abs() < 1e-6);
        assert!((elevation - 150.0 * FEET_TO_METERS).abs() < 1e-6);
    }

    #[test]
    fn test_metric_unchanged() {
        let mut sections = vec![TmlSection {
            name: "Test".to_string(),
            shots: vec![TmlShot {
                uuid: None,
                id: 1,
                from_id: 0,
                shot_type: TmlShotType::Real,
                azimuth: 90.0,
                length: 10.0,
                depth: 5.0,
                depth_in: -1.0,
                inclination: 0.0,
                left: 3.0,
                right: 4.0,
                up: 2.0,
                down: 1.0,
                comment: None,
                explorer: None,
                date: None,
                color: "#000000".to_string(),
                excluded: false,
                latitude: 0.0,
                longitude: 0.0,
                closure_to_id: -1,
                section: "Test".to_string(),
            }],
            surveyor: None,
            date: None,
            color: "#000000".to_string(),
            is_branch_split: false,
            start_ariane_id: 0,
            end_ariane_id: 1,
            start_depth: 12.0,
        }];

        let mut elevation = 150.0;
        apply_unit_conversion(&mut sections, &mut elevation, TmlUnit::Metric);

        let shot = &sections[0].shots[0];
        assert!((shot.length - 10.0).abs() < f64::EPSILON);
        assert!((shot.depth - 5.0).abs() < f64::EPSILON);
        assert!((sections[0].start_depth - 12.0).abs() < f64::EPSILON);
        assert!((elevation - 150.0).abs() < f64::EPSILON);
    }

    /// A branch section's first shot carries the `-1` DepthIn sentinel, so its
    /// entry depth comes from the parent station - a value read straight out of
    /// the file, in FEET on an imperial one. Leave it unconverted and every
    /// branch starts ~3.28x too deep, which is geometry, not presentation.
    #[test]
    fn imperial_branch_section_starts_at_the_converted_parent_depth() {
        let shots = [
            tml_shot_xml_depth(0, -1, "START", "Start", 0.0, -1.0),
            tml_shot_xml_depth(1, 0, "REAL", "Main", 10.0, -1.0),
            tml_shot_xml_depth(2, 1, "REAL", "Main", 20.0, -1.0),
            // Second child of shot 1: branches off a station 10 ft down.
            tml_shot_xml_depth(3, 1, "REAL", "Side", 30.0, -1.0),
        ]
        .join("\n");
        let tml = parse_synthetic_tmlu(&wrap_xml_with("ft", 0.0, &shots));

        let side = tml
            .sections
            .iter()
            .find(|s| s.name.starts_with("Side"))
            .expect("branch section");
        assert!((side.start_depth - 10.0 * FEET_TO_METERS).abs() < 1e-6);

        let (configs, _, _) = convert_tml_to_import_configs(&tml, false, false).unwrap();
        let side_config = configs
            .iter()
            .find(|c| c.name.as_deref().is_some_and(|n| n.starts_with("Side")))
            .expect("branch config");
        assert!((side_config.shots[0].depth_in - 10.0 * FEET_TO_METERS).abs() < 1e-6);
        assert!((side_config.shots[0].depth_out - 30.0 * FEET_TO_METERS).abs() < 1e-6);
    }

    /// `firstStartAbsoluteElevation` is a length, so an imperial file states it
    /// in feet - both on `TmlFile` and on the geo-coding altitude derived from
    /// it.
    #[test]
    fn imperial_elevation_is_converted() {
        let shots = [
            tml_shot_xml_gps(0, -1, "START", "Start", 20.5, -87.3),
            tml_shot_xml_depth(1, 0, "REAL", "Main", 10.0, -1.0),
        ]
        .join("\n");
        let tml = parse_synthetic_tmlu(&wrap_xml_with("ft", 500.0, &shots));

        assert_eq!(tml.unit, TmlUnit::Imperial);
        assert!((tml.first_start_absolute_elevation - 500.0 * FEET_TO_METERS).abs() < 1e-6);
        let geo = tml
            .geo_coding
            .expect("geo coding from the GPS-bearing shot");
        assert!((geo.altitude - 500.0 * FEET_TO_METERS).abs() < 1e-6);
    }

    /// The metric twin: nothing is scaled, so the elevation is passed through.
    #[test]
    fn metric_elevation_is_passed_through() {
        let shots = [
            tml_shot_xml_gps(0, -1, "START", "Start", 20.5, -87.3),
            tml_shot_xml_depth(1, 0, "REAL", "Main", 10.0, -1.0),
        ]
        .join("\n");
        let tml = parse_synthetic_tmlu(&wrap_xml_with("m", 500.0, &shots));

        assert_eq!(tml.unit, TmlUnit::Metric);
        assert!((tml.first_start_absolute_elevation - 500.0).abs() < f64::EPSILON);
    }

    /// An unknown `<unit>` still reads as metric - but it says so, because a
    /// wrong scale factor leaves no other trace in the imported data.
    #[test]
    fn an_unrecognized_unit_warns_and_falls_back_to_metric() {
        let shots = [
            tml_shot_xml_depth(0, -1, "START", "Start", 0.0, -1.0),
            tml_shot_xml_depth(1, 0, "REAL", "Main", 10.0, -1.0),
        ]
        .join("\n");
        let tml = parse_synthetic_tmlu(&wrap_xml_with("furlongs", 0.0, &shots));

        assert_eq!(tml.unit, TmlUnit::Metric);
        assert_eq!(tml.unrecognized_unit.as_deref(), Some("furlongs"));

        let (_, _, warnings) = convert_tml_to_import_configs(&tml, false, false).unwrap();
        assert!(
            warnings.iter().any(|w| w.contains("furlongs")),
            "the raw unit string must reach the import receipt, got {warnings:?}"
        );
    }

    /// The units we do understand, and an absent `<unit>`, stay silent - a
    /// warning on every ordinary metric file would be noise.
    #[test]
    fn known_and_absent_units_do_not_warn() {
        for raw in ["m", "M", "metric", "ft", "FT", "imperial", "", "  "] {
            let (_, unrecognized) = classify_unit(raw);
            assert_eq!(unrecognized, None, "unit {raw:?} should not warn");
        }
        assert_eq!(classify_unit("ft").0, TmlUnit::Imperial);
        // The other imperial spelling, which only this test exercises.
        assert_eq!(classify_unit("imperial").0, TmlUnit::Imperial);
        assert_eq!(classify_unit("  Imperial ").0, TmlUnit::Imperial);
        assert_eq!(classify_unit("").0, TmlUnit::Metric);
    }

    /// The unit string is file-controlled and ends up in an IPC field and a UI
    /// warning line, so it is clamped and stripped of anything that can rewrite
    /// the message around it - bidi overrides reorder the rendered text, and
    /// control characters break the line.
    #[test]
    fn a_hostile_unit_string_is_clamped_and_stripped() {
        let hostile = format!(
            "furl\u{202e}ongs\u{7}\n{}",
            "A".repeat(4000 /* well past the 300-char clamp */)
        );
        let (unit, unrecognized) = classify_unit(&hostile);

        assert_eq!(unit, TmlUnit::Metric);
        let label = unrecognized.expect("a hostile unit is still an unknown unit");
        assert_eq!(label.chars().count(), MAX_UNIT_LABEL_LEN);
        assert!(
            !label.chars().any(|c| c.is_control()
                || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')),
            "control and bidi characters must not survive"
        );

        let mut tml = parse_synthetic_tmlu(&wrap_xml_with(
            "m",
            0.0,
            &[
                tml_shot_xml_depth(0, -1, "START", "Start", 0.0, -1.0),
                tml_shot_xml_depth(1, 0, "REAL", "Main", 10.0, -1.0),
            ]
            .join("\n"),
        ));
        tml.unrecognized_unit = Some(label);
        let (_, _, warnings) = convert_tml_to_import_configs(&tml, false, false).unwrap();
        assert!(
            !warnings
                .iter()
                .any(|w| w.chars().any(|c| c.is_control() || c == '\u{202e}')),
            "the warning line must stay control-free, got {warnings:?}"
        );
    }

    /// `"NaN"` parses. The elevation is scaled by the unit conversion and handed
    /// out on `TmlFile` and the geo-coding altitude, so a non-finite one has to
    /// die at the parse, not downstream.
    #[test]
    fn a_non_finite_elevation_falls_back_to_zero() {
        let shots = [
            tml_shot_xml_gps(0, -1, "START", "Start", 20.5, -87.3),
            tml_shot_xml_depth(1, 0, "REAL", "Main", 10.0, -1.0),
        ]
        .join("\n");

        for raw in ["NaN", "inf", "-inf", "1e309"] {
            let tml = parse_synthetic_tmlu(&wrap_xml_with("ft", raw, &shots));
            assert!(
                tml.first_start_absolute_elevation.abs() < f64::EPSILON,
                "elevation {raw:?} must not survive the parse"
            );
            let geo = tml.geo_coding.expect("geo coding");
            assert!(
                geo.altitude.abs() < f64::EPSILON,
                "altitude {raw:?} must not survive"
            );
        }
    }

    // -------------------------------------------------------------------
    // Structural anomalies the parser recovers from (#411)
    // -------------------------------------------------------------------

    /// A shot element with no `<ID>` is dropped. Before this it was dropped
    /// with nothing but a `tracing::warn!` - the import reported a clean
    /// success one shot short of the file.
    #[test]
    fn a_shot_with_no_id_is_counted_and_named_in_the_receipt() {
        let idless = r"<SurveyData>
    <FromID>1</FromID>
    <Type>REAL</Type>
    <Section>Main</Section>
    <Azimut>90.0</Azimut>
    <Length>10.0</Length>
</SurveyData>";
        let xml = wrap_xml(
            &[
                tml_shot_xml(0, -1, "START", "Start"),
                tml_shot_xml(1, 0, "REAL", "Main"),
                idless.to_string(),
            ]
            .join("\n"),
        );
        let tml = parse_synthetic_tmlu(&xml);

        let anomalies = tml
            .parse_anomalies
            .expect("a dropped shot must be recorded on the parse result");
        assert_eq!(anomalies.shots_missing_id, 1);
        // Only the ID-less shot is lost; the well-formed one still imports.
        assert_eq!(tml.sections.iter().map(|s| s.shots.len()).sum::<usize>(), 1);

        let (_, _, warnings) = convert_tml_to_import_configs(&tml, false, false).unwrap();
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("1 shot in the file had a missing or unreadable ID")),
            "the dropped shot must reach the import receipt, got {warnings:?}"
        );
    }

    /// An unknown `<Type>` is read as REAL. That is a defensible recovery, but
    /// it is a reinterpretation of the file and the receipt has to say so.
    #[test]
    fn an_unknown_shot_type_is_counted_and_named_in_the_receipt() {
        let xml = wrap_xml(
            &[
                tml_shot_xml(0, -1, "START", "Start"),
                tml_shot_xml(1, 0, "SPLAY", "Main"),
                tml_shot_xml(2, 1, "SPLAY", "Main"),
            ]
            .join("\n"),
        );
        let tml = parse_synthetic_tmlu(&xml);

        let anomalies = tml
            .parse_anomalies
            .expect("coerced shot types must be recorded on the parse result");
        assert_eq!(anomalies.unknown_shot_types, 2);
        // Behaviour is unchanged - they are still imported as Real.
        assert_eq!(tml.sections[0].shots[0].shot_type, TmlShotType::Real);

        let (_, _, warnings) = convert_tml_to_import_configs(&tml, false, false).unwrap();
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("2 shots in the file had an unrecognized type")),
            "the coerced types must reach the import receipt, got {warnings:?}"
        );
    }

    /// Two shots sharing an ID collapse: the topology index is keyed by ID, so
    /// the second silently replaces the first. Behaviour kept, silence not.
    #[test]
    fn duplicate_shot_ids_are_counted_and_named_in_the_receipt() {
        let xml = wrap_xml(
            &[
                tml_shot_xml(0, -1, "START", "Start"),
                tml_shot_xml(1, 0, "REAL", "Main"),
                tml_shot_xml(1, 1, "REAL", "Main"),
            ]
            .join("\n"),
        );
        let tml = parse_synthetic_tmlu(&xml);

        let anomalies = tml
            .parse_anomalies
            .expect("a collapsed ID must be recorded on the parse result");
        assert_eq!(anomalies.duplicate_shot_ids, 1);

        let (_, _, warnings) = convert_tml_to_import_configs(&tml, false, false).unwrap();
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("1 shot in the file reused an ID")),
            "the collapsed ID must reach the import receipt, got {warnings:?}"
        );
    }

    /// One sentence per anomaly CLASS, whatever the count. A per-shot warning
    /// here would put a thousand strings on the wire for a file that damaged
    /// itself a thousand times, and `cap_warnings` would then bury every other
    /// warning behind them.
    #[test]
    fn a_thousand_duplicate_ids_still_produce_exactly_one_warning() {
        let mut shots = vec![tml_shot_xml(0, -1, "START", "Start")];
        shots.extend((0..1000).map(|_| tml_shot_xml(1, 0, "REAL", "Main")));
        let tml = parse_synthetic_tmlu(&wrap_xml(&shots.join("\n")));

        let anomalies = tml
            .parse_anomalies
            .expect("collapsed IDs must be recorded on the parse result");
        assert_eq!(anomalies.duplicate_shot_ids, 999);

        let (_, _, warnings) = convert_tml_to_import_configs(&tml, false, false).unwrap();
        let duplicate_warnings = warnings
            .iter()
            .filter(|w| w.contains("reused an ID"))
            .count();
        assert_eq!(
            duplicate_warnings, 1,
            "one warning per class, got {warnings:?}"
        );
        assert!(warnings.iter().any(|w| w.contains("999 shots in the file")));
    }

    /// A clean file says nothing - these warnings have to stay rare enough to
    /// be worth reading.
    #[test]
    fn a_clean_file_records_no_anomalies() {
        let xml = wrap_xml(
            &[
                tml_shot_xml(0, -1, "START", "Start"),
                tml_shot_xml(1, 0, "REAL", "Main"),
                tml_shot_xml(2, 1, "VIRTUAL", "Main"),
            ]
            .join("\n"),
        );
        let tml = parse_synthetic_tmlu(&xml);
        assert_eq!(tml.parse_anomalies, None);

        let (_, _, warnings) = convert_tml_to_import_configs(&tml, false, false).unwrap();
        assert!(warnings.is_empty(), "clean file warned: {warnings:?}");
    }

    /// A file the parser could make NO section out of used to leave
    /// `convert_tml_to_import_configs` on the first line, dropping the very
    /// warnings that explain the emptiness. It is the one import where the
    /// warnings are the entire result.
    #[test]
    fn a_file_with_no_sections_still_reports_its_warnings() {
        // START shots carry no measurement and form no section, so a file of
        // nothing but a START parses to zero sections.
        let xml = wrap_xml_with("furlongs", 0.0, &tml_shot_xml(0, -1, "START", "Start"));
        let tml = parse_synthetic_tmlu(&xml);
        assert!(tml.sections.is_empty(), "premise: no sections were built");

        let (configs, closures, warnings) =
            convert_tml_to_import_configs(&tml, false, false).unwrap();
        assert!(configs.is_empty());
        assert!(closures.is_empty());
        assert!(
            warnings.iter().any(|w| w.contains("furlongs")),
            "an import that lands nothing must still say why, got {warnings:?}"
        );
    }

    /// A closure whose target is the ENTRANCE resolves to no station: START
    /// shots never enter a section. It was counted into "referenced excluded
    /// stations", which sends the user looking through an exclusion filter
    /// that had nothing to do with it.
    #[test]
    fn a_closure_to_the_entrance_is_not_blamed_on_excluded_stations() {
        let make_shot = |id, from_id, shot_type, closure_to_id| TmlShot {
            id,
            from_id,
            shot_type,
            closure_to_id,
            ..sample_tml_shot()
        };

        let tml = TmlFile {
            cave_name: "Test".to_string(),
            unit: TmlUnit::Metric,
            use_magnetic_azimuth: false,
            first_start_absolute_elevation: 0.0,
            unrecognized_unit: None,
            parse_anomalies: None,
            encoding_warning: None,
            scaled_depth_in_marker: None,
            warnings: None,
            geo_coding: None,
            sections: vec![TmlSection {
                name: "Main".to_string(),
                shots: vec![
                    make_shot(1, 99, TmlShotType::Real, -1),
                    make_shot(2, 1, TmlShotType::Real, -1),
                    // Closes back onto shot 99 - the START this section hangs
                    // off, which is in no section and so in no import.
                    make_shot(3, 2, TmlShotType::Closure, 99),
                ],
                surveyor: None,
                date: None,
                color: "#ff0000".to_string(),
                is_branch_split: false,
                start_ariane_id: 99,
                end_ariane_id: 2,
                start_depth: 0.0,
            }],
            closures: Vec::new(),
        };

        let (_, closures, warnings) = convert_tml_to_import_configs(&tml, false, false).unwrap();
        assert!(closures.is_empty(), "the closure cannot be reconstructed");
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("referenced an entrance station")),
            "expected the entrance-station reason, got {warnings:?}"
        );
        assert!(
            !warnings.iter().any(|w| w.contains("excluded stations")),
            "nothing was excluded here, so nothing may say so: {warnings:?}"
        );
    }

    /// The preview's closure extraction aims a START-targeted closure at the
    /// first section hanging off that START. It used to pick one by scanning a
    /// HashMap, so the answer changed from run to run whenever more than one
    /// section qualified. Repeated because that is what a random iteration
    /// order looks like from the outside.
    #[test]
    fn a_closure_to_a_start_shot_resolves_to_the_same_section_every_run() {
        let xml = wrap_xml(
            &[
                tml_shot_xml(0, -1, "START", "Start"),
                tml_shot_xml(1, 0, "REAL", "A"),
                tml_shot_xml(2, 0, "REAL", "B"),
                {
                    let mut closure = tml_shot_xml(3, 2, "CLOSURE", "B");
                    closure = closure.replace(
                        "<ClosureToID>-1</ClosureToID>",
                        "<ClosureToID>0</ClosureToID>",
                    );
                    closure
                },
            ]
            .join("\n"),
        );

        for run in 0..25 {
            let tml = parse_synthetic_tmlu(&xml);
            let closure =
                tml.closures.first().copied().unwrap_or_else(|| {
                    panic!("run {run}: the START-targeted closure must resolve")
                });
            assert_eq!(
                tml.sections[closure.to_section_index].name, "A",
                "run {run}: the target section must be the first one hanging off the START"
            );
        }
    }

    // -------------------------------------------------------------------
    // Branching detection
    // -------------------------------------------------------------------

    #[test]
    fn test_linear_no_split() {
        let xml = wrap_xml(
            &[
                tml_shot_xml(0, -1, "START", "Start"),
                tml_shot_xml(1, 0, "REAL", "Section 1"),
                tml_shot_xml(2, 1, "REAL", "Section 1"),
                tml_shot_xml(3, 2, "REAL", "Section 1"),
            ]
            .join("\n"),
        );

        let (raw_shots, _) =
            parse_raw_shots(&xml).unwrap_or_else(|_| (Vec::new(), FileMetadata::default()));
        let (sections, _) = build_sections(raw_shots);

        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].name, "Section 1");
        assert!(!sections[0].is_branch_split);
        assert_eq!(sections[0].shots.len(), 3);
    }

    #[test]
    fn test_branch_detection_and_split() {
        // Shot 1 has two successors (2 and 3) in the same section
        let xml = wrap_xml(
            &[
                tml_shot_xml(0, -1, "START", "Start"),
                tml_shot_xml(1, 0, "REAL", "Section 1"),
                tml_shot_xml(2, 1, "REAL", "Section 1"),
                tml_shot_xml(3, 1, "REAL", "Section 1"), // branches from same shot
            ]
            .join("\n"),
        );

        let (raw_shots, _) =
            parse_raw_shots(&xml).unwrap_or_else(|_| (Vec::new(), FileMetadata::default()));
        let (sections, _) = build_sections(raw_shots);

        // Should split into 2 branches
        assert!(sections.len() >= 2);
        assert!(sections.iter().all(|s| s.is_branch_split));
        // Mainline naming rule (#412): the chain the continuation heuristic
        // keeps walking retains the section's own name; only the deferred
        // side chains get "(branch N)" suffixes.
        assert_eq!(sections[0].name, "Section 1");
        assert!(sections[1].name.contains("branch"));
    }

    #[test]
    fn test_branch_leftovers_preserve_parent_child_topology() {
        // Interleaved order that used to trigger fallback flattening:
        // 2 branches to (3, 6), and 3 branches to (4, 5). The 6→7→8 chain
        // must stay connected and must not absorb shot 5.
        let xml = wrap_xml(
            &[
                tml_shot_xml(0, -1, "START", "Start"),
                tml_shot_xml(1, 0, "REAL", "Section 1"),
                tml_shot_xml(2, 1, "REAL", "Section 1"),
                tml_shot_xml(3, 2, "REAL", "Section 1"),
                tml_shot_xml(6, 2, "VIRTUAL", "Section 1"),
                tml_shot_xml(4, 3, "REAL", "Section 1"),
                tml_shot_xml(5, 3, "REAL", "Section 1"),
                tml_shot_xml(7, 6, "REAL", "Section 1"),
                tml_shot_xml(8, 7, "REAL", "Section 1"),
            ]
            .join("\n"),
        );

        let (raw_shots, _) =
            parse_raw_shots(&xml).unwrap_or_else(|_| (Vec::new(), FileMetadata::default()));
        let (sections, _) = build_sections(raw_shots);

        let mut id_to_section: HashMap<i64, usize> = HashMap::new();
        let mut seen = std::collections::HashSet::new();
        for (si, sec) in sections.iter().enumerate() {
            for shot in &sec.shots {
                assert!(
                    seen.insert(shot.id),
                    "shot {} emitted multiple times",
                    shot.id
                );
                id_to_section.insert(shot.id, si);
            }
        }

        for id in [1_i64, 2, 3, 4, 5, 6, 7, 8] {
            assert!(id_to_section.contains_key(&id), "missing shot {id}");
        }

        let sec_6 = *id_to_section.get(&6).unwrap();
        let sec_7 = *id_to_section.get(&7).unwrap();
        let sec_8 = *id_to_section.get(&8).unwrap();
        let sec_5 = *id_to_section.get(&5).unwrap();

        assert_eq!(sec_6, sec_7, "6 and 7 must stay in the same branch");
        assert_eq!(sec_7, sec_8, "7 and 8 must stay in the same branch");
        assert_ne!(
            sec_5, sec_6,
            "shot 5 must not be merged into the 6→7→8 chain"
        );

        let chain_6_ids: Vec<i64> = sections[sec_6].shots.iter().map(|s| s.id).collect();
        assert_eq!(chain_6_ids, vec![6, 7, 8], "expected preserved 6→7→8 chain");
        assert_eq!(sections[sec_6].start_ariane_id, 2);
        assert_eq!(sections[sec_5].start_ariane_id, 3);
    }

    /// #412: a short side stub written FIRST in the file must not steal the
    /// mainline. Greatest total downstream length picks the continuation.
    #[test]
    fn branch_continuation_longest_downstream_wins() {
        let xml = wrap_xml(
            &[
                tml_shot_xml(0, -1, "START", "Start"),
                tml_shot_xml_az_len(1, 0, "Section 1", 90.0, 10.0),
                // Two-shot side stub, written before the continuation.
                tml_shot_xml_az_len(10, 1, "Section 1", 0.0, 2.0),
                tml_shot_xml_az_len(11, 10, "Section 1", 0.0, 2.0),
                // The continuation: far more downstream length.
                tml_shot_xml_az_len(2, 1, "Section 1", 90.0, 10.0),
                tml_shot_xml_az_len(3, 2, "Section 1", 90.0, 10.0),
                tml_shot_xml_az_len(4, 3, "Section 1", 90.0, 10.0),
            ]
            .join("\n"),
        );

        let (raw_shots, _) =
            parse_raw_shots(&xml).unwrap_or_else(|_| (Vec::new(), FileMetadata::default()));
        let (sections, _) = build_sections(raw_shots);

        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].name, "Section 1");
        let mainline_ids: Vec<i64> = sections[0].shots.iter().map(|s| s.id).collect();
        assert_eq!(
            mainline_ids,
            vec![1, 2, 3, 4],
            "mainline must continue past the stub"
        );
        assert_eq!(sections[1].name, "Section 1 (branch 2)");
        let stub_ids: Vec<i64> = sections[1].shots.iter().map(|s| s.id).collect();
        assert_eq!(stub_ids, vec![10, 11]);
    }

    /// #412: when both branches carry comparable downstream length (within
    /// 20% of the longest), azimuth continuity decides - the successor whose
    /// heading is closest to the incoming shot's continues, even when it is
    /// written later in the file and has higher ids.
    #[test]
    fn branch_continuation_azimuth_tie_break() {
        let xml = wrap_xml(
            &[
                tml_shot_xml(0, -1, "START", "Start"),
                tml_shot_xml_az_len(1, 0, "Section 1", 90.0, 10.0),
                // Written first, slightly longer (20 vs 18: comparable),
                // but turns 60 degrees off the incoming heading.
                tml_shot_xml_az_len(5, 1, "Section 1", 150.0, 10.0),
                tml_shot_xml_az_len(6, 5, "Section 1", 150.0, 10.0),
                // Written later, higher ids, but holds the heading.
                tml_shot_xml_az_len(7, 1, "Section 1", 92.0, 9.0),
                tml_shot_xml_az_len(8, 7, "Section 1", 92.0, 9.0),
            ]
            .join("\n"),
        );

        let (raw_shots, _) =
            parse_raw_shots(&xml).unwrap_or_else(|_| (Vec::new(), FileMetadata::default()));
        let (sections, _) = build_sections(raw_shots);

        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].name, "Section 1");
        let mainline_ids: Vec<i64> = sections[0].shots.iter().map(|s| s.id).collect();
        assert_eq!(
            mainline_ids,
            vec![1, 7, 8],
            "azimuth continuity must pick the continuation"
        );
        let side_ids: Vec<i64> = sections[1].shots.iter().map(|s| s.id).collect();
        assert_eq!(side_ids, vec![5, 6]);
    }

    /// #412 shape: a long mainline with side stubs written before each
    /// continuation. File order used to cut the mainline at every T; the
    /// downstream-length rule must keep one long mainline plus N stubs.
    #[test]
    fn branch_continuation_mainline_with_many_ts() {
        let xml = wrap_xml(
            &[
                tml_shot_xml(0, -1, "START", "Start"),
                tml_shot_xml_az_len(1, 0, "Main", 90.0, 10.0),
                tml_shot_xml_az_len(2, 1, "Main", 90.0, 10.0),
                tml_shot_xml_az_len(3, 2, "Main", 90.0, 10.0),
                tml_shot_xml_az_len(20, 3, "Main", 0.0, 2.0),
                tml_shot_xml_az_len(21, 20, "Main", 0.0, 2.0),
                tml_shot_xml_az_len(4, 3, "Main", 90.0, 10.0),
                tml_shot_xml_az_len(5, 4, "Main", 90.0, 10.0),
                tml_shot_xml_az_len(6, 5, "Main", 90.0, 10.0),
                tml_shot_xml_az_len(30, 6, "Main", 180.0, 2.0),
                tml_shot_xml_az_len(31, 30, "Main", 180.0, 2.0),
                tml_shot_xml_az_len(7, 6, "Main", 90.0, 10.0),
                tml_shot_xml_az_len(8, 7, "Main", 90.0, 10.0),
                tml_shot_xml_az_len(40, 8, "Main", 0.0, 2.0),
                tml_shot_xml_az_len(41, 40, "Main", 0.0, 2.0),
                tml_shot_xml_az_len(9, 8, "Main", 90.0, 10.0),
            ]
            .join("\n"),
        );

        let (raw_shots, _) =
            parse_raw_shots(&xml).unwrap_or_else(|_| (Vec::new(), FileMetadata::default()));
        let (sections, _) = build_sections(raw_shots);

        assert_eq!(sections.len(), 4, "one mainline + three stubs");
        assert_eq!(sections[0].name, "Main");
        let mainline_ids: Vec<i64> = sections[0].shots.iter().map(|s| s.id).collect();
        assert_eq!(
            mainline_ids,
            vec![1, 2, 3, 4, 5, 6, 7, 8, 9],
            "the mainline must survive every T uncut"
        );
        for section in &sections[1..] {
            assert!(
                section.name.contains("(branch "),
                "stub name: {}",
                section.name
            );
            assert_eq!(section.shots.len(), 2, "each stub is two shots");
        }
    }

    /// Same file, same sections: the continuation choice must not depend on
    /// anything with unstable iteration order.
    #[test]
    fn branch_continuation_is_deterministic() {
        let shots = [
            tml_shot_xml(0, -1, "START", "Start"),
            tml_shot_xml_az_len(1, 0, "Section 1", 90.0, 10.0),
            tml_shot_xml_az_len(10, 1, "Section 1", 0.0, 2.0),
            tml_shot_xml_az_len(2, 1, "Section 1", 92.0, 9.0),
            tml_shot_xml_az_len(3, 2, "Section 1", 88.0, 9.0),
            tml_shot_xml_az_len(20, 2, "Section 1", 150.0, 10.0),
            tml_shot_xml_az_len(21, 20, "Section 1", 150.0, 10.0),
        ]
        .join("\n");
        let xml = wrap_xml(&shots);

        let fingerprint = |tml: &TmlFile| -> String {
            tml.sections
                .iter()
                .map(|s| {
                    format!(
                        "{}|{}|{:?}\n",
                        s.name,
                        s.start_ariane_id,
                        s.shots.iter().map(|shot| shot.id).collect::<Vec<_>>()
                    )
                })
                .collect()
        };

        // Hard-coded, not just self-consistent: repeating the same parse proves
        // stability, but a rule that changed which successor continues the
        // mainline would be stably WRONG and still pass. At shot 2 the walk
        // takes 20 (20.0 m downstream) over 3 (9.0 m), and the two deferred
        // chains keep the mid-chain parents that make them unjoinable.
        const EXPECTED: &str = "Section 1|0|[1, 2, 20, 21]\n\
                                Section 1 (branch 2)|1|[10]\n\
                                Section 1 (branch 3)|2|[3]\n";

        let first = fingerprint(&parse_synthetic_tmlu(&xml));
        assert_eq!(
            first, EXPECTED,
            "the continuation rule picked a new mainline"
        );
        for _ in 0..5 {
            assert_eq!(
                fingerprint(&parse_synthetic_tmlu(&xml)),
                first,
                "section list must be byte-identical on every parse"
            );
        }
    }

    /// A section with several roots gives its own name to the LONGEST root,
    /// not to whichever root the file happened to write first (#412).
    ///
    /// `choose_continuation` only decides junctions inside a chain; the roots
    /// themselves were walked in file order, so an entrance stub written first
    /// took the section's name and the real mainline was demoted to
    /// "(branch 2)" - the same file-order bias the continuation rule exists to
    /// remove, one level up.
    #[test]
    fn the_longest_root_keeps_the_section_name() {
        let shots = [
            // Written first, but a dead-end stub.
            tml_shot_xml_az_len(1, -1, "Sump", 90.0, 2.0),
            // Written second, and the actual passage.
            tml_shot_xml_az_len(2, -1, "Sump", 90.0, 30.0),
            tml_shot_xml_az_len(3, 2, "Sump", 90.0, 30.0),
            tml_shot_xml_az_len(4, 3, "Sump", 90.0, 30.0),
        ]
        .join("\n");
        let tml = parse_synthetic_tmlu(&wrap_xml(&shots));

        let named: Vec<(&str, Vec<i64>)> = tml
            .sections
            .iter()
            .map(|s| (s.name.as_str(), s.shots.iter().map(|sh| sh.id).collect()))
            .collect();
        assert_eq!(
            named,
            vec![("Sump", vec![2, 3, 4]), ("Sump (branch 2)", vec![1]),],
            "the 90 m root is the mainline; the 2 m stub is the branch"
        );
    }

    /// A CLOSURE shot must never win a junction away from real survey (#412).
    ///
    /// Closures are loop markers that `convert_shots_to_shot_data` filters out
    /// of the import, and our own exporter gives them the vertical gap between
    /// the nodes they tie, so their `Length` is not always zero. Letting that
    /// length into the continuation choice makes the mainline "continue" into
    /// an edge that then vanishes: the run stops dead at the junction and the
    /// real continuation is demoted to a branch.
    #[test]
    fn a_closure_edge_never_takes_the_junction_from_real_survey() {
        let closure_xml = |id: i64, from_id: i64, length: f64| {
            format!(
                r"<SurveyData>
    <ID>{id}</ID>
    <FromID>{from_id}</FromID>
    <ClosureToID>1</ClosureToID>
    <Type>CLOSURE</Type>
    <Section>Main</Section>
    <Azimut>90.0</Azimut>
    <Length>{length}</Length>
    <Depth>5.0</Depth>
    <DepthIn>-1.0</DepthIn>
    <Inclination>0.0</Inclination>
    <Left>0.0</Left>
    <Right>0.0</Right>
    <Up>0.0</Up>
    <Down>0.0</Down>
    <Comment></Comment>
    <Explorer>Tester</Explorer>
    <Date>2024-06-15</Date>
    <Color>0x8f6112ff</Color>
    <Excluded>false</Excluded>
    <Latitude>0.0</Latitude>
    <Longitude>0.0</Longitude>
</SurveyData>"
            )
        };
        let long_closure = closure_xml(4, 2, 500.0);
        let shots = [
            tml_shot_xml_az_len(1, -1, "Main", 90.0, 10.0),
            tml_shot_xml_az_len(2, 1, "Main", 90.0, 10.0),
            // The real continuation, far shorter than the closure's 500 m.
            tml_shot_xml_az_len(3, 2, "Main", 90.0, 10.0),
            long_closure.to_owned(),
        ]
        .join("\n");
        let tml = parse_synthetic_tmlu(&wrap_xml(&shots));

        let mainline = tml
            .sections
            .iter()
            .find(|s| s.name == "Main")
            .expect("the mainline keeps the section name");
        assert!(
            mainline.shots.iter().any(|s| s.id == 3),
            "the real continuation must stay on the mainline; got {:?}",
            mainline.shots.iter().map(|s| s.id).collect::<Vec<_>>()
        );

        // And the run the user actually imports is not truncated at the junction.
        let (configs, _, _) = convert_tml_to_import_configs(&tml, false, false).expect("convert");
        assert_eq!(configs.len(), 1, "a closure must not fragment the section");
        assert_eq!(
            configs[0].shots.len(),
            3,
            "all three real shots import as one run"
        );

        // Zero-weighting the closure's length is not on its own enough. When
        // the real continuation ALSO has no downstream length - a zero-length
        // station marker - both candidates survive elimination and the choice
        // falls through to azimuth, where a straight-ahead closure beats a real
        // shot that turns. Only refusing closures as candidates covers this.
        let degenerate = [
            tml_shot_xml_az_len(1, -1, "Main", 90.0, 10.0),
            tml_shot_xml_az_len(2, 1, "Main", 90.0, 10.0),
            // Real, but zero length and turning hard away from the heading.
            tml_shot_xml_az_len(3, 2, "Main", 180.0, 0.0),
            closure_xml(4, 2, 0.0),
        ]
        .join("\n");
        let tml = parse_synthetic_tmlu(&wrap_xml(&degenerate));
        let mainline = tml
            .sections
            .iter()
            .find(|s| s.name == "Main")
            .expect("the mainline keeps the section name");
        assert!(
            mainline.shots.iter().any(|s| s.id == 3),
            "a closure must not win on azimuth either; got {:?}",
            mainline.shots.iter().map(|s| s.id).collect::<Vec<_>>()
        );

        // Nor may a closure's length reach the choice INDIRECTLY, by inflating
        // a real ancestor's downstream total. Here both candidates at shot 1
        // are real, so refusing closures as candidates does nothing: only
        // giving the closure zero weight keeps the 200 m branch ahead of the
        // 5 m one that happens to carry a 500 m loop marker.
        let inflated = [
            tml_shot_xml_az_len(1, -1, "Main", 90.0, 10.0),
            tml_shot_xml_az_len(2, 1, "Main", 90.0, 5.0),
            closure_xml(5, 2, 500.0),
            tml_shot_xml_az_len(3, 1, "Main", 180.0, 100.0),
            tml_shot_xml_az_len(4, 3, "Main", 180.0, 100.0),
        ]
        .join("\n");
        let tml = parse_synthetic_tmlu(&wrap_xml(&inflated));
        let mainline = tml
            .sections
            .iter()
            .find(|s| s.name == "Main")
            .expect("the mainline keeps the section name");
        assert_eq!(
            mainline.shots.iter().map(|s| s.id).collect::<Vec<_>>(),
            vec![1, 3, 4],
            "the 200 m real branch is the mainline, not the 5 m one with a closure on it"
        );
    }

    #[test]
    fn test_malformed_cycle_and_self_loop_are_guarded() {
        // Malformed graph with a 2<->3 cycle and a self-loop at 4.
        // Parser should still terminate and emit each shot exactly once.
        let xml = wrap_xml(
            &[
                tml_shot_xml(0, -1, "START", "Start"),
                tml_shot_xml(1, 0, "REAL", "Section 1"),
                tml_shot_xml(2, 3, "REAL", "Section 1"),
                tml_shot_xml(3, 2, "REAL", "Section 1"),
                tml_shot_xml(4, 4, "REAL", "Section 1"),
            ]
            .join("\n"),
        );

        let (raw_shots, _) =
            parse_raw_shots(&xml).unwrap_or_else(|_| (Vec::new(), FileMetadata::default()));
        let (sections, _) = build_sections(raw_shots);

        let ids: Vec<i64> = sections
            .iter()
            .flat_map(|s| s.shots.iter().map(|shot| shot.id))
            .collect();
        let unique: std::collections::HashSet<i64> = ids.iter().copied().collect();

        assert_eq!(ids.len(), 4, "all non-START shots should be emitted");
        assert_eq!(unique.len(), 4, "no shot should be emitted more than once");
        assert!(unique.contains(&1));
        assert!(unique.contains(&2));
        assert!(unique.contains(&3));
        assert!(unique.contains(&4));
    }

    // -------------------------------------------------------------------
    // Closure extraction
    // -------------------------------------------------------------------

    #[test]
    fn test_closure_mapping() {
        let xml = wrap_xml(&format!(
            r#"
            <SurveyData>
                <ID>0</ID><FromID>-1</FromID><ClosureToID>-1</ClosureToID>
                <Type>START</Type><Section>Start</Section>
                <Azimut>0</Azimut><Length>0</Length><Depth>0</Depth><DepthIn>-1.0</DepthIn>
                <Inclination>0</Inclination><Left>0</Left><Right>0</Right><Up>0</Up><Down>0</Down>
                <Color>0x000000ff</Color><Excluded>false</Excluded><Latitude>0</Latitude><Longitude>0</Longitude>
            </SurveyData>
            <SurveyData>
                <ID>1</ID><FromID>0</FromID><ClosureToID>-1</ClosureToID>
                <Type>REAL</Type><Section>Sec A</Section>
                <Azimut>90</Azimut><Length>10</Length><Depth>5</Depth><DepthIn>-1.0</DepthIn>
                <Inclination>0</Inclination><Left>1</Left><Right>1</Right><Up>1</Up><Down>1</Down>
                <Color>0x000000ff</Color><Excluded>false</Excluded><Latitude>0</Latitude><Longitude>0</Longitude>
            </SurveyData>
            <SurveyData>
                <ID>2</ID><FromID>1</FromID><ClosureToID>-1</ClosureToID>
                <Type>REAL</Type><Section>Sec B</Section>
                <Azimut>180</Azimut><Length>8</Length><Depth>3</Depth><DepthIn>-1.0</DepthIn>
                <Inclination>0</Inclination><Left>1</Left><Right>1</Right><Up>1</Up><Down>1</Down>
                <Color>0x000000ff</Color><Excluded>false</Excluded><Latitude>0</Latitude><Longitude>0</Longitude>
            </SurveyData>
            <SurveyData>
                <ID>3</ID><FromID>2</FromID><ClosureToID>1</ClosureToID>
                <Type>CLOSURE</Type><Section>Sec B</Section>
                <Azimut>270</Azimut><Length>5</Length><Depth>5</Depth><DepthIn>-1.0</DepthIn>
                <Inclination>0</Inclination><Left>0</Left><Right>0</Right><Up>0</Up><Down>0</Down>
                <Color>0x000000ff</Color><Excluded>false</Excluded><Latitude>0</Latitude><Longitude>0</Longitude>
            </SurveyData>
            "#
        ));

        let (raw_shots, _) =
            parse_raw_shots(&xml).unwrap_or_else(|_| (Vec::new(), FileMetadata::default()));
        let (sections, closures) = build_sections(raw_shots);

        assert_eq!(closures.len(), 1);
        let cl = &closures[0];
        // Closure shot 3 is in Sec B, pointing to shot 1 in Sec A
        assert_eq!(
            sections[cl.to_section_index].shots[cl.to_station_index].id,
            1
        );
    }

    #[test]
    fn test_closure_to_id_negative_ignored() {
        let xml = wrap_xml(
            &[
                tml_shot_xml(0, -1, "START", "Start"),
                tml_shot_xml(1, 0, "REAL", "Section 1"),
            ]
            .join("\n"),
        );

        let (raw_shots, _) =
            parse_raw_shots(&xml).unwrap_or_else(|_| (Vec::new(), FileMetadata::default()));
        let (_, closures) = build_sections(raw_shots);

        assert!(closures.is_empty());
    }

    // -------------------------------------------------------------------
    // Empty / error cases
    // -------------------------------------------------------------------

    #[test]
    fn test_empty_data() {
        let xml = wrap_xml("");
        let (raw_shots, _) =
            parse_raw_shots(&xml).unwrap_or_else(|_| (Vec::new(), FileMetadata::default()));
        let (sections, closures) = build_sections(raw_shots);
        assert!(sections.is_empty());
        assert!(closures.is_empty());
    }

    /// A document that stops mid-element - a half-written or interrupted save -
    /// must be an error, not a survey with nothing in it.
    ///
    /// Without the fix `Event::Eof` just breaks the loop: this returns
    /// `Ok(([], _))`, the import reports "0 sections - 0 shots", walks the user
    /// into creating a project, and lands them in an empty one with no hint
    /// that the file is damaged.
    #[test]
    fn a_document_truncated_mid_element_is_an_error() {
        let err = parse_raw_shots("<CaveFile><caveName>X</caveName><Data><SurveyData><ID>1</ID>")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("truncated or damaged"),
            "the error must say the file is damaged, got {err:?}"
        );
        assert!(
            err.contains("3 element(s)"),
            "the error must count what is still open (CaveFile, Data, SurveyData), got {err:?}"
        );
    }

    /// The truncation can also land on an element boundary: every element seen
    /// is well formed and the last shot is complete, yet `CaveFile` and `Data`
    /// never close. This is why the signal is unclosed depth at EOF and not
    /// "the last element was cut in half" or "no shots were found" - both would
    /// wave this file through, and it can carry any number of good shots.
    #[test]
    fn a_document_truncated_between_elements_is_an_error() {
        let xml = format!(
            "<CaveFile><caveName>X</caveName><Data>\n{}",
            tml_shot_xml(1, -1, "REAL", "A")
        );
        let err = parse_raw_shots(&xml).unwrap_err().to_string();
        assert!(
            err.contains("truncated or damaged"),
            "the error must say the file is damaged, got {err:?}"
        );
        assert!(
            err.contains("2 element(s)"),
            "CaveFile and Data are the two left open, got {err:?}"
        );
    }

    /// A file that genuinely holds no shots is not damaged, and must keep
    /// reaching the zero-section receipt instead of being refused. Both
    /// spellings of an empty `<Data>` count.
    #[test]
    fn a_complete_document_with_no_shots_still_parses() {
        let self_closing_data =
            "<CaveFile><caveName>Test Cave</caveName><unit>m</unit><Data/></CaveFile>";
        for xml in [wrap_xml(""), self_closing_data.to_string()] {
            let (shots, meta) = parse_raw_shots(&xml)
                .unwrap_or_else(|e| panic!("an empty but complete file must parse, got {e}"));
            assert!(shots.is_empty(), "no shots, but no error either");
            assert_eq!(meta.cave_name, "Test Cave");
        }
    }

    /// The `<Shape>` subtree is skipped with `read_to_end`, which swallows its
    /// own End event - so the depth counter has to be corrected for it.
    ///
    /// Without that correction every ordinary Ariane file (they all carry
    /// per-shot Shape polygons) would come back as "truncated or damaged".
    #[test]
    fn shape_subtrees_do_not_read_as_truncation() {
        let shot = tml_shot_xml(1, -1, "REAL", "A").replace(
            "</SurveyData>",
            "<Shape><hasProfileAzimut>false</hasProfileAzimut>\
             <RadiusCollection><RadiusVector><angle>0.0</angle></RadiusVector>\
             </RadiusCollection></Shape></SurveyData>",
        );
        assert!(
            shot.contains("<Shape>"),
            "premise: the shot carries a Shape"
        );

        let tml = parse_synthetic_tmlu(&wrap_xml(&shot));
        assert_eq!(tml.sections.len(), 1);
        assert_eq!(tml.sections[0].shots.len(), 1);
    }

    #[test]
    fn test_nonexistent_file() {
        let result = parse_tml(Path::new("/nonexistent/file.tml"));
        assert!(result.is_err());
    }

    /// `<Type>` spellings other than Ariane's upper case must still be
    /// recognised. Without the fix `start` falls through to the unknown-type
    /// path and is imported as an ordinary shot, inventing a second section.
    #[test]
    fn shot_type_spellings_are_case_insensitive() {
        let xml = wrap_xml(
            &[
                tml_shot_xml(0, -1, "start", "Start"),
                tml_shot_xml(1, 0, "Real", "A"),
                tml_shot_xml(2, 1, "virtual", "A"),
            ]
            .join("\n"),
        );

        let tml = parse_synthetic_tmlu(&xml);
        assert_eq!(tml.sections.len(), 1, "the START shot must not become one");
        assert_eq!(tml.sections[0].shots.len(), 2);
        assert_eq!(tml.sections[0].shots[1].shot_type, TmlShotType::Virtual);
    }

    /// The two boolean elements are compared against the literal `"true"`;
    /// without the fix `TRUE` reads as false and an excluded shot is imported.
    #[test]
    fn boolean_element_spellings_are_case_insensitive() {
        let excluded = tml_shot_xml(2, 1, "REAL", "A")
            .replace("<Excluded>false</Excluded>", "<Excluded>TRUE</Excluded>");
        let xml = wrap_xml(
            &[
                tml_shot_xml(0, -1, "START", "Start"),
                tml_shot_xml(1, 0, "REAL", "A"),
                excluded,
            ]
            .join("\n"),
        )
        .replace(
            "<useMagneticAzimuth>true</useMagneticAzimuth>",
            "<useMagneticAzimuth>True</useMagneticAzimuth>",
        );

        let tml = parse_synthetic_tmlu(&xml);
        assert!(tml.use_magnetic_azimuth);
        assert!(tml.sections[0].shots[1].excluded);
    }

    /// `-1.0` is the FORMAT's "same as the previous shot" marker, not this
    /// parser's private default: `export::tml::emitted_depth_in` nudges a
    /// genuine -1.0 m depth to -1.000001 so it survives this reader. Reading a
    /// literal -1.0 as a measured depth would misread every ordinary Ariane
    /// file, so pin both halves of that contract here.
    #[test]
    fn a_literal_minus_one_depth_in_stays_the_sentinel_and_the_nudged_value_does_not() {
        let xml = wrap_xml(
            &[
                tml_shot_xml_depth(0, -1, "START", "Start", 0.0, -1.0),
                tml_shot_xml_depth(1, 0, "REAL", "A", 7.0, -1.0),
                tml_shot_xml_depth(2, 1, "REAL", "A", 9.0, -1.000_001),
            ]
            .join("\n"),
        );

        let tml = parse_synthetic_tmlu(&xml);
        let section = &tml.sections[0];
        let shots = convert_shots_to_shot_data(&section.shots, section.start_depth, false);

        assert!(
            shots[0].depth_in.abs() < f64::EPSILON,
            "the sentinel inherits, got {}",
            shots[0].depth_in
        );
        assert!(
            (shots[1].depth_in - (-1.000_001)).abs() < 1e-9,
            "the nudged value is a measurement, got {}",
            shots[1].depth_in
        );
    }

    /// The `DepthIn` of each imported shot of `xml`'s first section.
    fn imported_depth_ins(xml: &str) -> Vec<f64> {
        let tml = parse_synthetic_tmlu(xml);
        let section = &tml.sections[0];
        convert_shots_to_shot_data(&section.shots, section.start_depth, false)
            .iter()
            .map(|s| s.depth_in)
            .collect()
    }

    /// [#576 HFI-1] A METRIC file whose "same as previous" marker went through a
    /// feet-to-metre conversion carries -1 ft written in metres: -0.3048, which
    /// Ariane (two decimals, trailing zero trimmed) writes as `-0.3`, on nearly
    /// every shot. Read literally, every one of those shots started 0.3 m above the
    /// datum - impossible geometry the validator flagged without naming the cause.
    /// Each spelling a conversion can leave (rounded or truncated to 1-4 decimals)
    /// is the marker there.
    #[test]
    fn a_metric_marker_scaled_from_feet_still_means_same_as_previous() {
        for marker in [-0.3, -0.304, -0.305, -0.3048] {
            let xml = wrap_xml(
                &[
                    tml_shot_xml_depth(0, -1, "START", "Start", 0.0, marker),
                    tml_shot_xml_depth(1, 0, "REAL", "A", 7.0, marker),
                    tml_shot_xml_depth(2, 1, "REAL", "A", 9.0, marker),
                    tml_shot_xml_depth(3, 2, "REAL", "A", 12.0, marker),
                ]
                .join("\n"),
            );
            let depth_ins = imported_depth_ins(&xml);
            assert_eq!(depth_ins, [0.0, 7.0, 9.0], "marker spelled {marker}");
        }
    }

    /// [#576 HFI-1] ...but only where it IS the file's marker. A real start depth
    /// of -0.3 m (a station 0.3 m above the datum) on one shot of a file that
    /// marks "same as previous" with -1.0 stays a measurement, and a feet file's
    /// -0.3 is -0.3 ft.
    #[test]
    fn a_real_start_depth_that_spells_the_scaled_marker_stays_a_measurement() {
        let rows = |unit: &str| {
            wrap_xml_with(
                unit,
                100.0,
                &[
                    tml_shot_xml_depth(0, -1, "START", "Start", 0.0, -1.0),
                    tml_shot_xml_depth(1, 0, "REAL", "A", -0.2, -1.0),
                    tml_shot_xml_depth(2, 1, "REAL", "A", 2.0, -0.3),
                    tml_shot_xml_depth(3, 2, "REAL", "A", 3.0, -1.0),
                ]
                .join("\n"),
            )
        };
        let metric = imported_depth_ins(&rows("m"));
        assert_eq!(metric, [0.0, -0.3, 2.0], "metric");
        let feet = imported_depth_ins(&rows("ft"));
        assert!(
            (feet[1] - (-0.3 * FEET_TO_METERS)).abs() < 1e-9,
            "feet: {feet:?}"
        );

        // A marker is on nearly every shot: on half of them, and no more, it is not.
        let half = wrap_xml(
            &[
                tml_shot_xml_depth(0, -1, "START", "Start", 0.0, -1.0),
                tml_shot_xml_depth(1, 0, "REAL", "A", 1.0, -0.3),
                tml_shot_xml_depth(2, 1, "REAL", "A", 2.0, -1.0),
            ]
            .join("\n"),
        );
        assert_eq!(imported_depth_ins(&half), [-0.3, 1.0], "half the shots");
    }

    /// [#576 HFI-1-D2] Reading the scaled marker as "same as previous" is a
    /// reinterpretation of the file, so the import receipt says so in one line,
    /// naming the spelling and how many shots it was read on. A file whose marker
    /// is the plain -1.0 (or that has no scaled spelling at all) gets no line.
    #[test]
    fn the_receipt_says_a_scaled_depth_marker_was_read_as_same_as_previous() {
        let receipt = |rows: &[String]| {
            let tml = parse_synthetic_tmlu(&wrap_xml(&rows.join("\n")));
            let (_, _, warnings) = convert_tml_to_import_configs(&tml, false, false).unwrap();
            warnings
                .into_iter()
                .filter(|w| w.contains("depth marker"))
                .collect::<Vec<_>>()
        };
        // The START row opens the section; the three REAL rows are its shots.
        let scaled = |marker: f64| {
            vec![
                tml_shot_xml_depth(0, -1, "START", "Start", 0.0, marker),
                tml_shot_xml_depth(1, 0, "REAL", "A", 7.0, marker),
                tml_shot_xml_depth(2, 1, "REAL", "A", 9.0, marker),
                tml_shot_xml_depth(3, 2, "REAL", "A", 12.0, marker),
            ]
        };
        let lines = receipt(&scaled(-0.3));
        println!("HFI-1-D2 marker -0.3 on every shot: {lines:?}");
        assert_eq!(
            lines,
            ["Read the depth marker written as -0.3 (a scaled -1) as same as previous on 3 shots."]
        );
        assert_eq!(
            receipt(&scaled(-0.3048)),
            ["Read the depth marker written as -0.3048 (a scaled -1) as same as previous on 3 shots."]
        );
        // Two spellings in one file: both named, in file order, in the one line.
        let mut mixed = scaled(-0.305);
        mixed[3] = tml_shot_xml_depth(3, 2, "REAL", "A", 12.0, -0.3);
        assert_eq!(
            receipt(&mixed),
            ["Read the depth marker written as -0.305 or -0.3 (a scaled -1) as same as previous on 3 shots."]
        );
        let plain = receipt(&scaled(-1.0));
        println!("HFI-1-D2 marker -1.0 on every shot: {plain:?}");
        assert!(plain.is_empty(), "the plain marker is no news: {plain:?}");
    }

    // -------------------------------------------------------------------
    // Multi-section topology
    // -------------------------------------------------------------------

    /// Two declared sections running in SERIES are one linear passage, so they
    /// import as one config (#412).
    ///
    /// This is the whole of the cross-section join: B's first shot hangs off
    /// A's LAST shot, so B's station 0 already IS A's final station and one
    /// polyline holds both. The names are not thrown away - the merged run is
    /// called "Section A + Section B", because nothing else in an
    /// `ImportSectionConfig` can carry the fact that the surveyor named these
    /// separately.
    ///
    /// A file whose sections branch instead of running in series is untouched;
    /// `test_branch_leftovers_preserve_parent_child_topology` covers that.
    #[test]
    fn sections_running_in_series_import_as_one_named_run() {
        let xml = wrap_xml(
            &[
                tml_shot_xml(0, -1, "START", "Start"),
                tml_shot_xml(1, 0, "REAL", "Section A"),
                tml_shot_xml(2, 1, "REAL", "Section A"),
                tml_shot_xml(3, 2, "REAL", "Section B"),
                tml_shot_xml(4, 3, "REAL", "Section B"),
            ]
            .join("\n"),
        );

        let (raw_shots, _) =
            parse_raw_shots(&xml).unwrap_or_else(|_| (Vec::new(), FileMetadata::default()));
        let (sections, _) = build_sections(raw_shots);

        assert_eq!(sections.len(), 1, "series sections join into one run");
        assert_eq!(sections[0].name, "Section A + Section B");
        assert_eq!(
            sections[0].shots.iter().map(|s| s.id).collect::<Vec<_>>(),
            vec![1, 2, 3, 4],
            "the join keeps file order, so the run stays a valid import sequence"
        );
        assert_eq!(
            sections[0].start_ariane_id, 0,
            "the merged run still starts where the parent chain did"
        );
        assert_eq!(
            sections[0].end_ariane_id, 4,
            "and ends where the absorbed chain did"
        );
    }

    // -------------------------------------------------------------------
    // Import config conversion
    // -------------------------------------------------------------------

    /// Cross-section `FromImporting` placement, on a section that BRANCHES off
    /// mid-chain.
    ///
    /// Section B hangs off shot 1, which is mid-Section-A, so the two stay
    /// separate configs and the topology has to be carried by placement - the
    /// case this test exists for. (Were B parented on A's LAST shot the two
    /// would join into one run instead; see
    /// `sections_running_in_series_import_as_one_named_run`.)
    #[test]
    fn test_convert_to_import_configs() {
        let xml = wrap_xml(
            &[
                tml_shot_xml(0, -1, "START", "Start"),
                tml_shot_xml(1, 0, "REAL", "Section A"),
                tml_shot_xml(2, 1, "REAL", "Section A"),
                tml_shot_xml(3, 1, "REAL", "Section B"),
            ]
            .join("\n"),
        );

        let (raw_shots, metadata) =
            parse_raw_shots(&xml).unwrap_or_else(|_| (Vec::new(), FileMetadata::default()));
        let (sections, closures) = build_sections(raw_shots);

        let tml = TmlFile {
            cave_name: metadata.cave_name,
            unit: TmlUnit::Metric,
            use_magnetic_azimuth: true,
            first_start_absolute_elevation: 100.0,
            unrecognized_unit: None,
            parse_anomalies: None,
            encoding_warning: None,
            scaled_depth_in_marker: None,
            warnings: None,
            geo_coding: None,
            sections,
            closures,
        };

        let (configs, _, _) = convert_tml_to_import_configs(&tml, false, false)
            .unwrap_or_else(|_| (Vec::new(), Vec::new(), Vec::new()));
        assert_eq!(configs.len(), 2);

        // First section → Origin (no GPS)
        assert!(matches!(configs[0].placement, ImportPlacement::Origin));
        assert_eq!(configs[0].section_index, 0);

        // Second section → FromImporting (linked via topology)
        // Section B's first shot (ID=3) has FromID=1, the 1st shot in Section A.
        // Shot ID=1 is at import shot index 0, so it ends at station 1.
        match &configs[1].placement {
            ImportPlacement::FromImporting {
                import_station,
                target_section_index,
                target_station_index,
            } => {
                assert_eq!(
                    *import_station, 0,
                    "import_station should be 0 (first station)"
                );
                assert_eq!(*target_section_index, 0, "should link to section 0");
                assert_eq!(
                    *target_station_index, 1,
                    "should link to station 1 (end of shot ID=1)"
                );
            }
            other => panic!("Expected FromImporting, got {:?}", other),
        }
    }

    // -------------------------------------------------------------------
    // File-level parsing (TML ZIP)
    // -------------------------------------------------------------------

    /// The whole `.tml` path end to end: ZIP wrapper, long element names, and
    /// the field mapping every consumer downstream reads. Synthetic on purpose -
    /// survey fixtures are not committed, and the two tests this replaces
    /// returned early when their fixture was missing, which it always was.
    #[test]
    fn a_zip_wrapped_tml_parses_into_the_mapped_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("synthetic.tml");
        let xml = wrap_xml(
            &[
                tml_shot_xml(0, -1, "START", "Start"),
                tml_shot_xml(1, 0, "REAL", "Section A"),
                tml_shot_xml(2, 1, "REAL", "Section A"),
            ]
            .join("\n"),
        );
        write_tml_archive(&path, "Data.xml", xml.as_bytes());

        let tml = parse_tml(&path).unwrap();
        assert_eq!(tml.cave_name, "Test Cave");
        assert_eq!(tml.unit, TmlUnit::Metric);
        assert!(tml.use_magnetic_azimuth);
        assert!((tml.first_start_absolute_elevation - 100.0).abs() < f64::EPSILON);
        assert_eq!(tml.sections.len(), 1);

        let section = &tml.sections[0];
        assert_eq!(section.name, "Section A");
        assert_eq!(section.shots.len(), 2);
        assert_eq!(section.shots[0].shot_type, TmlShotType::Real);
        assert_eq!(section.shots[0].color, "#8f6112");
        assert_eq!(section.shots[0].explorer.as_deref(), Some("Tester"));
        assert!((section.shots[0].azimuth - 90.0).abs() < f64::EPSILON);
        assert!((section.shots[0].length - 10.0).abs() < f64::EPSILON);
        assert!((section.shots[0].depth - 5.0).abs() < f64::EPSILON);
    }

    /// The same document as a plain `.tmlu` on disk: the extension picks the
    /// reader, and no ZIP is involved.
    #[test]
    fn a_plain_tmlu_parses_the_same_way() {
        let tml = parse_synthetic_tmlu(&wrap_xml(
            &[
                tml_shot_xml(0, -1, "START", "Start"),
                tml_shot_xml(1, 0, "REAL", "Section A"),
            ]
            .join("\n"),
        ));

        assert_eq!(tml.cave_name, "Test Cave");
        assert_eq!(tml.sections.len(), 1);
        assert_eq!(tml.sections[0].shots.len(), 1);
        assert!((tml.sections[0].shots[0].length - 10.0).abs() < f64::EPSILON);
    }

    // -------------------------------------------------------------------
    // Shot data conversion details
    // -------------------------------------------------------------------

    #[test]
    fn test_virtual_shot_in_shot_data() {
        let shots = vec![TmlShot {
            uuid: None,
            id: 1,
            from_id: 0,
            shot_type: TmlShotType::Virtual,
            azimuth: 90.0,
            length: 10.0,
            depth: 5.0,
            depth_in: -1.0,
            inclination: 0.0,
            left: 0.0,
            right: 0.0,
            up: 0.0,
            down: 0.0,
            comment: None,
            explorer: None,
            date: None,
            color: "#000000".to_string(),
            excluded: false,
            latitude: 0.0,
            longitude: 0.0,
            closure_to_id: -1,
            section: "Test".to_string(),
        }];

        let shot_data = convert_shots_to_shot_data(&shots, 0.0, false);
        assert_eq!(shot_data[0].kind, ShotKind::Reference);
    }

    #[test]
    fn test_lrud_zero_becomes_none() {
        let shots = vec![TmlShot {
            uuid: None,
            id: 1,
            from_id: 0,
            shot_type: TmlShotType::Real,
            azimuth: 90.0,
            length: 10.0,
            depth: 5.0,
            depth_in: -1.0,
            inclination: 0.0,
            left: 0.0,
            right: 2.5,
            up: 0.0,
            down: 1.0,
            comment: None,
            explorer: None,
            date: None,
            color: "#000000".to_string(),
            excluded: false,
            latitude: 0.0,
            longitude: 0.0,
            closure_to_id: -1,
            section: "Test".to_string(),
        }];

        let shot_data = convert_shots_to_shot_data(&shots, 0.0, false);
        assert!(shot_data[0].left.is_none());
        assert!((shot_data[0].right.unwrap_or(0.0) - 2.5).abs() < f64::EPSILON);
        assert!(shot_data[0].up.is_none());
        assert!((shot_data[0].down.unwrap_or(0.0) - 1.0).abs() < f64::EPSILON);
    }

    /// #482: a wire `0.0` inclination on a shot with a real depth change
    /// (the shape a depth-only, depth-gauge survey exports as) must import
    /// as `pitch_in: None`, not a fabricated `Some(0.0)`. Without this,
    /// re-importing an exported depth-only project trips
    /// `INCLINATION_DEPTH_MISMATCH` on every such shot.
    #[test]
    fn test_zero_inclination_with_depth_change_imports_as_no_pitch() {
        let shots = vec![TmlShot {
            depth: 5.0,
            depth_in: 0.0,
            inclination: 0.0,
            ..sample_tml_shot()
        }];

        let shot_data = convert_shots_to_shot_data(&shots, 0.0, false);
        assert_eq!(shot_data[0].pitch_in, None);
    }

    /// #482 companion: a wire `0.0` inclination with (near-)zero depth
    /// change is indistinguishable from a genuinely level, measured shot
    /// and stays `Some(0.0)` - the ambiguous case is resolved in favour of
    /// the harmless reading.
    #[test]
    fn test_zero_inclination_with_zero_depth_change_stays_some_zero() {
        let shots = vec![TmlShot {
            depth: 0.0,
            depth_in: 0.0,
            inclination: 0.0,
            ..sample_tml_shot()
        }];

        let shot_data = convert_shots_to_shot_data(&shots, 0.0, false);
        assert_eq!(shot_data[0].pitch_in, Some(0.0));
    }

    /// #482 boundary: `tml_pitch_in` pins the comparison as strictly-greater
    /// (`>`), matching the validator's own `INCLINATION_DEPTH_MISMATCH` test
    /// (`(recorded - implied).abs() > threshold`). Exactly at the default
    /// threshold, a recorded `Some(0.0)` would NOT have been flagged, so the
    /// wire `0.0` keeps reading as `Some(0.0)`; just a hair over, it does.
    #[test]
    fn test_zero_inclination_threshold_boundary_is_strictly_greater() {
        let length = 10.0;
        // dz for which implied_pitch_deg(length, 0.0, dz) == the threshold exactly.
        let dz_at_threshold = length * DEFAULT_INCLINATION_DEPTH_DIFF_THRESHOLD.to_radians().sin();
        assert!(
            (implied_pitch_deg(length, 0.0, dz_at_threshold)
                - DEFAULT_INCLINATION_DEPTH_DIFF_THRESHOLD)
                .abs()
                < 1e-9,
            "fixture must land on the threshold, not near it"
        );

        assert_eq!(
            tml_pitch_in(0.0, length, 0.0, dz_at_threshold),
            Some(0.0),
            "exactly at the threshold, a recorded 0.0 would not have been flagged"
        );

        let just_under = dz_at_threshold - 1e-4;
        assert_eq!(
            tml_pitch_in(0.0, length, 0.0, just_under),
            Some(0.0),
            "just under the threshold keeps the harmless reading"
        );

        let just_over = dz_at_threshold + 1e-4;
        assert_eq!(
            tml_pitch_in(0.0, length, 0.0, just_over),
            None,
            "just over the threshold, Some(0.0) would have fabricated a mismatch"
        );
    }

    /// #482: an ASCENDING depth-only shot (negative dz) must map to `None`
    /// the same as a descending one - the comparison is against the implied
    /// pitch's MAGNITUDE, not its sign.
    #[test]
    fn test_zero_inclination_ascending_steep_shot_imports_as_no_pitch() {
        let length = 10.0;
        let steep_ascend = -(length
            * (DEFAULT_INCLINATION_DEPTH_DIFF_THRESHOLD + 10.0)
                .to_radians()
                .sin());
        assert_eq!(tml_pitch_in(0.0, length, steep_ascend.abs(), 0.0), None);
    }

    #[test]
    fn test_azimuth_normalization() {
        // Ariane stores azimuths as arbitrary angles (negative, >360).
        // convert_shots_to_shot_data must normalize to 0..360.
        let make_shot = |az: f64| TmlShot {
            uuid: None,
            id: 1,
            from_id: 0,
            shot_type: TmlShotType::Real,
            azimuth: az,
            length: 10.0,
            depth: 0.0,
            depth_in: -1.0,
            inclination: 0.0,
            left: 0.0,
            right: 0.0,
            up: 0.0,
            down: 0.0,
            comment: None,
            explorer: None,
            date: None,
            color: "#000000".to_string(),
            excluded: false,
            latitude: 0.0,
            longitude: 0.0,
            closure_to_id: -1,
            section: "Test".to_string(),
        };

        // Negative azimuth → normalized
        let data = convert_shots_to_shot_data(&[make_shot(-124.0)], 0.0, false);
        assert!((data[0].azimuth_in - 236.0).abs() < 1e-6); // -124 + 360 = 236

        // Very negative → normalized
        let data = convert_shots_to_shot_data(&[make_shot(-690.0)], 0.0, false);
        assert!((data[0].azimuth_in - 30.0).abs() < 1e-6); // -690 + 720 = 30

        // >360 → normalized
        let data = convert_shots_to_shot_data(&[make_shot(450.0)], 0.0, false);
        assert!((data[0].azimuth_in - 90.0).abs() < 1e-6); // 450 - 360 = 90

        // Already valid → unchanged
        let data = convert_shots_to_shot_data(&[make_shot(102.0)], 0.0, false);
        assert!((data[0].azimuth_in - 102.0).abs() < 1e-6);

        // Zero → unchanged
        let data = convert_shots_to_shot_data(&[make_shot(0.0)], 0.0, false);
        assert!(data[0].azimuth_in.abs() < 1e-6);

        // azimuth_out should match azimuth_in
        let data = convert_shots_to_shot_data(&[make_shot(-124.0)], 0.0, false);
        assert!((data[0].azimuth_out - data[0].azimuth_in).abs() < f64::EPSILON);
    }

    #[test]
    fn test_excluded_shots_filtered_by_default() {
        let shots = vec![
            TmlShot {
                uuid: None,
                id: 1,
                from_id: 0,
                shot_type: TmlShotType::Real,
                azimuth: 90.0,
                length: 10.0,
                depth: 5.0,
                depth_in: -1.0,
                inclination: 0.0,
                left: 1.0,
                right: 1.0,
                up: 1.0,
                down: 1.0,
                comment: None,
                explorer: None,
                date: None,
                color: "#000000".to_string(),
                excluded: false,
                latitude: 0.0,
                longitude: 0.0,
                closure_to_id: -1,
                section: "Test".to_string(),
            },
            TmlShot {
                uuid: None,
                id: 2,
                from_id: 1,
                shot_type: TmlShotType::Real,
                azimuth: 180.0,
                length: 8.0,
                depth: 10.0,
                depth_in: -1.0,
                inclination: 0.0,
                left: 1.0,
                right: 1.0,
                up: 1.0,
                down: 1.0,
                comment: None,
                explorer: None,
                date: None,
                color: "#000000".to_string(),
                excluded: true, // This shot is excluded
                latitude: 0.0,
                longitude: 0.0,
                closure_to_id: -1,
                section: "Test".to_string(),
            },
            TmlShot {
                uuid: None,
                id: 3,
                from_id: 2,
                shot_type: TmlShotType::Real,
                azimuth: 270.0,
                length: 5.0,
                depth: 15.0,
                depth_in: -1.0,
                inclination: 0.0,
                left: 1.0,
                right: 1.0,
                up: 1.0,
                down: 1.0,
                comment: None,
                explorer: None,
                date: None,
                color: "#000000".to_string(),
                excluded: false,
                latitude: 0.0,
                longitude: 0.0,
                closure_to_id: -1,
                section: "Test".to_string(),
            },
        ];

        // Default: excluded shots filtered out
        let data = convert_shots_to_shot_data(&shots, 0.0, false);
        assert_eq!(data.len(), 2);
        assert!((data[0].azimuth_in - 90.0).abs() < f64::EPSILON);
        assert!((data[1].azimuth_in - 270.0).abs() < f64::EPSILON);

        // include_excluded=true: all shots kept
        let data = convert_shots_to_shot_data(&shots, 0.0, true);
        assert_eq!(data.len(), 3);
        assert!((data[0].azimuth_in - 90.0).abs() < f64::EPSILON);
        assert!((data[1].azimuth_in - 180.0).abs() < f64::EPSILON);
        assert!((data[2].azimuth_in - 270.0).abs() < f64::EPSILON);
    }

    /// #576 NF-TML-2: when leading excluded shots are dropped, the first
    /// imported shot starts where they END - the station `dropped_lead_offset`
    /// carries the datum to - so a "same as previous" `DepthIn` resolves to the
    /// last dropped shot's depth, not the parent section's start depth. A
    /// leading CLOSURE row is no leg and does not count.
    #[test]
    fn a_first_kept_shot_starts_at_the_depth_the_dropped_lead_ends() {
        let shot = |id: i64, depth: f64, excluded: bool, shot_type: TmlShotType| TmlShot {
            uuid: None,
            id,
            from_id: id - 1,
            shot_type,
            azimuth: 90.0,
            length: 20.0,
            depth,
            depth_in: schema::DEPTH_IN_SAME_AS_PREVIOUS,
            inclination: 0.0,
            left: 0.0,
            right: 0.0,
            up: 0.0,
            down: 0.0,
            comment: None,
            explorer: None,
            date: None,
            color: "#000000".to_string(),
            excluded,
            latitude: 0.0,
            longitude: 0.0,
            closure_to_id: -1,
            section: "Test".to_string(),
        };
        let shots = [
            shot(1, 4.0, true, TmlShotType::Real),
            shot(2, 9.0, true, TmlShotType::Real),
            shot(3, 30.0, false, TmlShotType::Closure),
            shot(4, 12.0, false, TmlShotType::Real),
            shot(5, 15.0, false, TmlShotType::Real),
        ];
        let data = convert_shots_to_shot_data(&shots, 1.5, false);
        let depths: Vec<(f64, f64)> = data.iter().map(|d| (d.depth_in, d.depth_out)).collect();
        eprintln!("dropped lead ends at 9.0, parent start 1.5: kept (in, out) = {depths:?}");
        assert_eq!(depths, [(9.0, 12.0), (12.0, 15.0)]);

        // Nothing dropped: the parent's start depth, as before.
        let data = convert_shots_to_shot_data(&shots[3..], 1.5, false);
        assert!((data[0].depth_in - 1.5).abs() < f64::EPSILON);
        // Kept with include_excluded: every shot chains from the one before.
        let data = convert_shots_to_shot_data(&shots, 1.5, true);
        assert!((data[0].depth_in - 1.5).abs() < f64::EPSILON);
        assert!((data[2].depth_in - 9.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_all_excluded_section_dropped_from_configs() {
        // Section where every shot is excluded → should be dropped entirely
        let make_shot = |id, from_id, excluded| TmlShot {
            uuid: None,
            id,
            from_id,
            shot_type: TmlShotType::Real,
            azimuth: 90.0,
            length: 10.0,
            depth: 0.0,
            depth_in: -1.0,
            inclination: 0.0,
            left: 0.0,
            right: 0.0,
            up: 0.0,
            down: 0.0,
            comment: None,
            explorer: None,
            date: None,
            color: "#000000".to_string(),
            excluded,
            latitude: 0.0,
            longitude: 0.0,
            closure_to_id: -1,
            section: "S".to_string(),
        };

        let tml = TmlFile {
            cave_name: "Test".to_string(),
            unit: TmlUnit::Metric,
            use_magnetic_azimuth: false,
            first_start_absolute_elevation: 0.0,
            unrecognized_unit: None,
            parse_anomalies: None,
            encoding_warning: None,
            scaled_depth_in_marker: None,
            warnings: None,
            geo_coding: None,
            sections: vec![
                TmlSection {
                    name: "Good".to_string(),
                    shots: vec![make_shot(1, 0, false)],
                    surveyor: None,
                    date: None,
                    color: "#ff0000".to_string(),
                    is_branch_split: false,
                    start_ariane_id: 0,
                    end_ariane_id: 1,
                    start_depth: 0.0,
                },
                TmlSection {
                    name: "AllExcluded".to_string(),
                    shots: vec![make_shot(2, 1, true), make_shot(3, 2, true)],
                    surveyor: None,
                    date: None,
                    color: "#00ff00".to_string(),
                    is_branch_split: false,
                    start_ariane_id: 1,
                    end_ariane_id: 3,
                    start_depth: 0.0,
                },
            ],
            closures: Vec::new(),
        };

        // With excluded filtered out, section "AllExcluded" has 0 shots → dropped
        let (configs, _closures, _) = convert_tml_to_import_configs(&tml, false, false).unwrap();
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].name.as_deref(), Some("Good"));

        // With excluded included, both sections present
        let (configs, _closures, _) = convert_tml_to_import_configs(&tml, true, false).unwrap();
        assert_eq!(configs.len(), 2);
    }

    #[test]
    fn test_zero_shot_section_and_filtered_closure_surface_receipt_warnings() {
        // Same all-excluded-section shape as the test above, plus a CLOSURE shot
        // whose target station was excluded (and so never made it into
        // `id_to_import_loc`), exercising both converter-level warnings at once.
        let make_shot = |id, from_id, shot_type, excluded, closure_to_id| TmlShot {
            uuid: None,
            id,
            from_id,
            shot_type,
            azimuth: 90.0,
            length: 10.0,
            depth: 0.0,
            depth_in: -1.0,
            inclination: 0.0,
            left: 0.0,
            right: 0.0,
            up: 0.0,
            down: 0.0,
            comment: None,
            explorer: None,
            date: None,
            color: "#000000".to_string(),
            excluded,
            latitude: 0.0,
            longitude: 0.0,
            closure_to_id,
            section: "S".to_string(),
        };

        let tml = TmlFile {
            cave_name: "Test".to_string(),
            unit: TmlUnit::Metric,
            use_magnetic_azimuth: false,
            first_start_absolute_elevation: 0.0,
            unrecognized_unit: None,
            parse_anomalies: None,
            encoding_warning: None,
            scaled_depth_in_marker: None,
            warnings: None,
            geo_coding: None,
            sections: vec![
                TmlSection {
                    name: "Good".to_string(),
                    shots: vec![
                        make_shot(1, 0, TmlShotType::Real, false, -1),
                        make_shot(2, 1, TmlShotType::Real, false, -1),
                        // Closure from Good's station 2 to shot 4, which is
                        // excluded and filtered out of `id_to_import_loc`.
                        make_shot(5, 2, TmlShotType::Closure, false, 4),
                    ],
                    surveyor: None,
                    date: None,
                    color: "#ff0000".to_string(),
                    is_branch_split: false,
                    start_ariane_id: 0,
                    end_ariane_id: 2,
                    start_depth: 0.0,
                },
                TmlSection {
                    name: "AllExcluded".to_string(),
                    shots: vec![
                        make_shot(3, 2, TmlShotType::Real, true, -1),
                        make_shot(4, 3, TmlShotType::Real, true, -1),
                    ],
                    surveyor: None,
                    date: None,
                    color: "#00ff00".to_string(),
                    is_branch_split: false,
                    start_ariane_id: 2,
                    end_ariane_id: 4,
                    start_depth: 0.0,
                },
            ],
            closures: Vec::new(),
        };

        let (configs, closures, warnings) =
            convert_tml_to_import_configs(&tml, false, false).unwrap();
        assert_eq!(configs.len(), 1, "the all-excluded section must be dropped");
        assert!(
            closures.is_empty(),
            "the closure whose endpoint was excluded must be skipped"
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("had no shots after filtering")),
            "expected a zero-shot-section warning, got: {warnings:?}"
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("referenced excluded stations")),
            "expected a filtered-closure warning, got: {warnings:?}"
        );
    }

    #[test]
    fn test_lone_branch_name_simplified() {
        // Two branches of "Gallery": branch 2 is all-excluded → dropped.
        // Branch 1 should lose the "(branch 1)" suffix.
        let make_shot = |id, from_id, excluded| TmlShot {
            uuid: None,
            id,
            from_id,
            shot_type: TmlShotType::Real,
            azimuth: 90.0,
            length: 10.0,
            depth: 0.0,
            depth_in: -1.0,
            inclination: 0.0,
            left: 0.0,
            right: 0.0,
            up: 0.0,
            down: 0.0,
            comment: None,
            explorer: None,
            date: None,
            color: "#000000".to_string(),
            excluded,
            latitude: 0.0,
            longitude: 0.0,
            closure_to_id: -1,
            section: "S".to_string(),
        };

        let tml = TmlFile {
            cave_name: "Test".to_string(),
            unit: TmlUnit::Metric,
            use_magnetic_azimuth: false,
            first_start_absolute_elevation: 0.0,
            unrecognized_unit: None,
            parse_anomalies: None,
            encoding_warning: None,
            scaled_depth_in_marker: None,
            warnings: None,
            geo_coding: None,
            sections: vec![
                TmlSection {
                    name: "Gallery (branch 1)".to_string(),
                    shots: vec![make_shot(1, 0, false), make_shot(2, 1, false)],
                    surveyor: None,
                    date: None,
                    color: "#ff0000".to_string(),
                    is_branch_split: true,
                    start_ariane_id: 0,
                    end_ariane_id: 2,
                    start_depth: 0.0,
                },
                TmlSection {
                    name: "Gallery (branch 2)".to_string(),
                    shots: vec![make_shot(3, 0, true), make_shot(4, 3, true)],
                    surveyor: None,
                    date: None,
                    color: "#00ff00".to_string(),
                    is_branch_split: true,
                    start_ariane_id: 0,
                    end_ariane_id: 4,
                    start_depth: 0.0,
                },
            ],
            closures: Vec::new(),
        };

        // Excluded filtered: branch 2 dropped, branch 1 renamed to "Gallery"
        let (configs, _closures, _) = convert_tml_to_import_configs(&tml, false, false).unwrap();
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].name.as_deref(), Some("Gallery"));

        // Both included: both branches keep their suffixes
        let (configs, _closures, _) = convert_tml_to_import_configs(&tml, true, false).unwrap();
        assert_eq!(configs.len(), 2);
        assert_eq!(configs[0].name.as_deref(), Some("Gallery (branch 1)"));
        assert_eq!(configs[1].name.as_deref(), Some("Gallery (branch 2)"));
    }

    #[test]
    fn test_excluded_leading_shots_placement() {
        // Real-world scenario modeled after Ariane data:
        //   Section A (index 0): shots 1,2 - main line
        //   Section C (index 1): shot 10 (from_id=2) - virtual jump chain
        //   Section B (index 2): excluded shot 5 (from_id=3, where 3 is excluded),
        //     then non-excluded shot 6 (from_id=10 → section C's last shot)
        //
        // start_ariane_id for B = 3 (excluded shot, not in id_to_import_loc).
        // Placement must fall back to effective_start_id = shot 6's from_id = 10
        // (in section C), and topological sort must reorder C before B even though
        // C has a higher raw index than expected.
        let make_shot = |id: i64, from_id: i64, excluded: bool| TmlShot {
            uuid: None,
            id,
            from_id,
            shot_type: TmlShotType::Real,
            azimuth: 90.0,
            length: 10.0,
            depth: 0.0,
            depth_in: -1.0,
            inclination: 0.0,
            left: 0.0,
            right: 0.0,
            up: 0.0,
            down: 0.0,
            comment: None,
            explorer: None,
            date: None,
            color: "#000000".to_string(),
            excluded,
            latitude: 0.0,
            longitude: 0.0,
            closure_to_id: -1,
            section: "S".to_string(),
        };

        let tml = TmlFile {
            cave_name: "Test".to_string(),
            unit: TmlUnit::Metric,
            use_magnetic_azimuth: false,
            first_start_absolute_elevation: 0.0,
            unrecognized_unit: None,
            parse_anomalies: None,
            encoding_warning: None,
            scaled_depth_in_marker: None,
            warnings: None,
            geo_coding: None,
            sections: vec![
                // Section A (index 0): shots 1,2
                TmlSection {
                    name: "A".to_string(),
                    shots: vec![make_shot(1, -1, false), make_shot(2, 1, false)],
                    surveyor: None,
                    date: None,
                    color: "#ff0000".to_string(),
                    is_branch_split: false,
                    start_ariane_id: -1,
                    end_ariane_id: 2,
                    start_depth: 0.0,
                },
                // Section B (index 1): excluded shot 5 (from_id=3, excluded),
                // then non-excluded shot 6 (from_id=10 → section C)
                TmlSection {
                    name: "B".to_string(),
                    shots: vec![
                        make_shot(5, 3, true),   // excluded, from_id=3 is also excluded
                        make_shot(6, 10, false), // kept, from_id=10 is in section C
                    ],
                    surveyor: None,
                    date: None,
                    color: "#0000ff".to_string(),
                    is_branch_split: false,
                    start_ariane_id: 3, // raw first shot's from_id (excluded, not in map)
                    end_ariane_id: 6,
                    start_depth: 0.0,
                },
                // Section C (index 2): shot 10 (from_id=2) - comes after B in raw order
                TmlSection {
                    name: "C".to_string(),
                    shots: vec![make_shot(10, 2, false)],
                    surveyor: None,
                    date: None,
                    color: "#00ff00".to_string(),
                    is_branch_split: false,
                    start_ariane_id: 2,
                    end_ariane_id: 10,
                    start_depth: 0.0,
                },
            ],
            closures: Vec::new(),
        };

        let (configs, _closures, _) = convert_tml_to_import_configs(&tml, false, false).unwrap();
        assert_eq!(configs.len(), 3);

        // After topological sort: C must come before B because B depends on C.
        // Expected order: A, C, B
        assert_eq!(configs[0].name.as_deref(), Some("A"));
        assert_eq!(configs[1].name.as_deref(), Some("C"));
        assert_eq!(configs[2].name.as_deref(), Some("B"));

        // Section B should attach to section C's shot 10
        let b_config = &configs[2];
        match &b_config.placement {
            ImportPlacement::FromImporting {
                import_station,
                target_section_index,
                target_station_index,
            } => {
                assert_eq!(*import_station, 0);
                assert_eq!(
                    *target_section_index, 2,
                    "should reference section C (raw index 2)"
                );
                assert_eq!(
                    *target_station_index, 1,
                    "should attach to station 1 (shot 10)"
                );
            }
            other => panic!("Expected FromImporting, got {:?}", other),
        }
    }

    #[test]
    fn test_strip_section_description() {
        assert_eq!(strip_section_description("Mainline"), "Mainline");
        assert_eq!(
            strip_section_description(
                "Reel to Mainline Cx<SectionDescription>reel to mainline connection</SectionDescription>"
            ),
            "Reel to Mainline Cx"
        );
        assert_eq!(
            strip_section_description(
                "Eastern Bypass Cavern-side<SectionDescription>notes</SectionDescription>"
            ),
            "Eastern Bypass Cavern-side"
        );
        assert_eq!(strip_section_description(""), "");
    }

    #[test]
    fn test_section_description_grouping() {
        // START shot has SectionDescription suffix, REAL shots don't.
        // After stripping, they should group into the same section.
        let xml = wrap_xml(&format!(
            "{}\n{}\n{}",
            r#"<SurveyData><ID>0</ID><FromID>-1</FromID><Type>START</Type>
                <Section>MySec&lt;SectionDescription&gt;notes&lt;/SectionDescription&gt;</Section>
                <Azimut>0</Azimut><Length>0</Length><Depth>0</Depth><DepthIn>0</DepthIn>
                <Inclination>0</Inclination><Left>0</Left><Right>0</Right><Up>0</Up><Down>0</Down>
                <Color>0x000000ff</Color><Excluded>false</Excluded><Latitude>0</Latitude>
                <Longitude>0</Longitude><ClosureToID>-1</ClosureToID></SurveyData>"#,
            r#"<SurveyData><ID>1</ID><FromID>0</FromID><Type>REAL</Type>
                <Section>MySec</Section>
                <Azimut>90</Azimut><Length>10</Length><Depth>5</Depth><DepthIn>-1</DepthIn>
                <Inclination>0</Inclination><Left>1</Left><Right>2</Right><Up>3</Up><Down>4</Down>
                <Color>0x000000ff</Color><Excluded>false</Excluded><Latitude>0</Latitude>
                <Longitude>0</Longitude><ClosureToID>-1</ClosureToID></SurveyData>"#,
            r#"<SurveyData><ID>2</ID><FromID>1</FromID><Type>REAL</Type>
                <Section>MySec</Section>
                <Azimut>180</Azimut><Length>15</Length><Depth>8</Depth><DepthIn>-1</DepthIn>
                <Inclination>0</Inclination><Left>1</Left><Right>2</Right><Up>3</Up><Down>4</Down>
                <Color>0x000000ff</Color><Excluded>false</Excluded><Latitude>0</Latitude>
                <Longitude>0</Longitude><ClosureToID>-1</ClosureToID></SurveyData>"#,
        ));

        let (raw_shots, _) =
            parse_raw_shots(&xml).unwrap_or_else(|_| (Vec::new(), FileMetadata::default()));
        let (sections, _) = build_sections(raw_shots);

        // Should be 1 section (MySec) with 2 shots (START excluded)
        assert_eq!(
            sections.len(),
            1,
            "Expected 1 section, got {}: {:?}",
            sections.len(),
            sections.iter().map(|s| &s.name).collect::<Vec<_>>()
        );
        assert_eq!(sections[0].name, "MySec");
        assert_eq!(sections[0].shots.len(), 2);
    }

    /// Test mid-section branching topology (not just end-of-section).
    /// Verifies that when section C branches from the MIDDLE of section A,
    /// the target_station_index correctly points to the station at the END
    /// of the referenced shot.
    #[test]
    fn test_mid_section_branching_topology() {
        // A: START → shot1 → shot2 → shot3 → shot4
        // B: branches from shot2 (mid-section)
        let xml = wrap_xml(
            &[
                tml_shot_xml(0, -1, "START", "Start"),
                tml_shot_xml(1, 0, "REAL", "Section A"),
                tml_shot_xml(2, 1, "REAL", "Section A"),
                tml_shot_xml(3, 2, "REAL", "Section A"),
                tml_shot_xml(4, 3, "REAL", "Section A"),
                tml_shot_xml(5, 2, "REAL", "Section B"), // branches from shot 2 (mid-section A)
                tml_shot_xml(6, 5, "REAL", "Section B"),
            ]
            .join("\n"),
        );

        let (raw_shots, metadata) =
            parse_raw_shots(&xml).unwrap_or_else(|_| (Vec::new(), FileMetadata::default()));
        let (sections, closures) = build_sections(raw_shots);

        let tml = TmlFile {
            cave_name: metadata.cave_name,
            unit: TmlUnit::Metric,
            use_magnetic_azimuth: true,
            first_start_absolute_elevation: 100.0,
            unrecognized_unit: None,
            parse_anomalies: None,
            encoding_warning: None,
            scaled_depth_in_marker: None,
            warnings: None,
            geo_coding: None,
            sections,
            closures,
        };

        let (configs, _closures, _) = convert_tml_to_import_configs(&tml, false, false).unwrap();
        assert_eq!(configs.len(), 2, "Should have 2 sections");

        // Section A: 4 shots → 5 stations (0, 1, 2, 3, 4)
        // Shot ID=2 at import_idx=1 → station_after=2
        assert_eq!(configs[0].shots.len(), 4);
        assert!(matches!(configs[0].placement, ImportPlacement::Origin));

        // Section B: from_id=2 → end of shot ID=2 = station 2 of section A
        match &configs[1].placement {
            ImportPlacement::FromImporting {
                import_station,
                target_section_index,
                target_station_index,
            } => {
                assert_eq!(*import_station, 0);
                assert_eq!(*target_section_index, 0);
                assert_eq!(
                    *target_station_index, 2,
                    "Should link to station 2 (end of shot ID=2 at import_idx=1)"
                );
            }
            other => panic!("Expected FromImporting, got {:?}", other),
        }
    }

    /// End-to-end coordinate verification on a real TML file: parse it, import
    /// all sections into a Project, then check every station against an
    /// independent walk of its section's shots, and every merged pair for
    /// co-location. The file is the caller's survey, never committed; it comes
    /// from `TML_COORDS_PATH`, and a run without it fails naming the variable
    /// (#576 NF-PRIVATE-PATHS, NF-W4M-4).
    ///
    /// Run with:  TML_COORDS_PATH=/path/to/ariane.tml cargo test --manifest-path src-tauri/Cargo.toml -p stalagmight-parsers --lib a_real_tml_import_matches_an_independent_traversal -- --ignored
    #[test]
    #[ignore = "needs a private Ariane .tml; set TML_COORDS_PATH"]
    fn a_real_tml_import_matches_an_independent_traversal() {
        use crate::models::{ImportPlacement, Project, SectionData, StationRef};

        // 1. Parse
        let path = require_env_file("TML_COORDS_PATH");
        let tml = parse_tml(&path).expect("Failed to parse TML");

        // 2. Convert to import configs (exclude excluded shots)
        let (configs, closures, _) = convert_tml_to_import_configs(&tml, false, false).unwrap();
        assert!(!configs.is_empty(), "Should produce import configs");

        // 3. Create a project and import all sections
        let mut project = Project::new("Test Import".into(), None);
        let mut created_ids: HashMap<u32, u32> = HashMap::new();

        for config in &configs {
            let section_data = SectionData {
                name: config.name.clone(),
                surveyor: config.surveyor.clone(),
                date: config.date,
                color: config.color.clone(),
                direction: config.direction,
                quality: config.quality,
                shots: config.shots.clone(),
            };
            let section_id = project.add_section(section_data).id;
            created_ids.insert(config.section_index, section_id);

            match &config.placement {
                ImportPlacement::Origin => {}
                ImportPlacement::ZeroDatum { station_index, .. } => {
                    // First datum station goes to origin; offset so that station is at (0,0)
                    let station_ref = StationRef::new(section_id, *station_index);
                    let current = project
                        .compute_station_raw_coords(station_ref)
                        .expect("station coords for zero datum");
                    let offset_x = -current.x;
                    let offset_y = -current.y;
                    if offset_x.abs() > 1e-9 || offset_y.abs() > 1e-9 {
                        project.set_section_offset(section_id, (offset_x, offset_y));
                    }
                }
                ImportPlacement::FromExisting { .. } => {
                    panic!("Unexpected FromExisting in fresh import");
                }
                ImportPlacement::FromImporting {
                    import_station,
                    target_section_index,
                    target_station_index,
                } => {
                    let target_section_id = *created_ids
                        .get(target_section_index)
                        .expect("target section should exist");
                    let import_ref = StationRef::new(section_id, *import_station);
                    let target_ref = StationRef::new(target_section_id, *target_station_index);
                    project
                        .merge_stations(import_ref, target_ref)
                        .expect("merge should succeed");
                }
                ImportPlacement::Manual { .. } | ImportPlacement::ManualSnap { .. } => {}
            }
        }

        // 4. Verify per-section relative coordinates
        //    For each section, compute station coords independently from shot data
        //    and compare against compute_station_raw_coords (relative to station 0).
        for section in &project.sections {
            let base = project
                .compute_station_raw_coords(StationRef::new(section.id, 0))
                .expect("base station coords");

            let mut expected_x = 0.0_f64;
            let mut expected_y = 0.0_f64;

            // Through the section's own reference, not the raw reading (#438).
            // The two coincide for a fixture whose correction is zero, which is
            // exactly why the raw form would rot: the day this oracle is pointed
            // at a georeferenced file it would silently start comparing a
            // magnetic walk against a corrected one and report the declination
            // as a coordinate bug.
            let azimuth_reference = project.azimuth_reference_for(section);
            for (i, shot) in section.shots.iter().enumerate() {
                let azi_rad = shot.average_azimuth_true_rad(azimuth_reference);
                let h = shot.horizontal_length();
                expected_x += h * azi_rad.sin();
                expected_y += h * azi_rad.cos();

                let station_idx = (i + 1) as u32;
                let actual = project
                    .compute_station_raw_coords(StationRef::new(section.id, station_idx))
                    .expect("station coords");

                let rel_x = actual.x - base.x;
                let rel_y = actual.y - base.y;

                assert!(
                    (rel_x - expected_x).abs() < 1e-6 && (rel_y - expected_y).abs() < 1e-6,
                    "Section '{}' station {}: expected rel ({:.6}, {:.6}), got ({:.6}, {:.6})",
                    section.name.as_deref().unwrap_or("?"),
                    station_idx,
                    expected_x,
                    expected_y,
                    rel_x,
                    rel_y,
                );
            }
        }

        // 5. Verify all merged stations are co-located (XY within tolerance)
        //    This validates the full import + merge + offset pipeline.
        let merge_tolerance = 1e-6;
        for merge in &project.station_merges {
            let sa = project
                .resolve_key(&merge.station_a)
                .unwrap_or_else(|| panic!("Resolve merge station_a {:?}", merge.station_a));
            let sb = project
                .resolve_key(&merge.station_b)
                .unwrap_or_else(|| panic!("Resolve merge station_b {:?}", merge.station_b));
            let ca = project
                .compute_station_raw_coords(sa)
                .unwrap_or_else(|| panic!("Coords for merge station_a {:?}", merge.station_a));
            let cb = project
                .compute_station_raw_coords(sb)
                .unwrap_or_else(|| panic!("Coords for merge station_b {:?}", merge.station_b));
            assert!(
                (ca.x - cb.x).abs() < merge_tolerance && (ca.y - cb.y).abs() < merge_tolerance,
                "Merged stations {:?} and {:?} not co-located: ({:.6},{:.6}) vs ({:.6},{:.6})",
                merge.station_a,
                merge.station_b,
                ca.x,
                ca.y,
                cb.x,
                cb.y,
            );
        }

        eprintln!(
            "TML coordinate check passed: {} sections, {} merges, {} closures, {} total stations verified",
            project.sections.len(),
            project.station_merges.len(),
            closures.len(),
            project.sections.iter().map(|s| s.shots.len() + 1).sum::<usize>(),
        );
    }

    // ------------------------------------------------------------------
    // Comment co-encoding (#483): the marker is an on-disk encoding and must
    // never reach the project, on EITHER setting.
    // ------------------------------------------------------------------

    /// One section, one shot, whose TML `<Comment>` carries `field`.
    fn tml_with_shot_comment(field: Option<String>) -> TmlFile {
        let mut shot = sample_tml_shot();
        shot.comment = field;
        TmlFile {
            cave_name: "Test".to_string(),
            unit: TmlUnit::Metric,
            use_magnetic_azimuth: false,
            first_start_absolute_elevation: 0.0,
            unrecognized_unit: None,
            parse_anomalies: None,
            encoding_warning: None,
            scaled_depth_in_marker: None,
            warnings: None,
            geo_coding: None,
            sections: vec![TmlSection {
                name: "S".to_string(),
                shots: vec![shot],
                surveyor: None,
                date: None,
                color: "#ff0000".to_string(),
                is_branch_split: false,
                start_ariane_id: 0,
                end_ariane_id: 1,
                start_depth: 0.0,
            }],
            closures: Vec::new(),
        }
    }

    /// Import `field` with the given `comments_on_stations`, returning the one
    /// shot's comment and the section's station comments.
    fn import_comment(
        field: Option<String>,
        comments_on_stations: bool,
    ) -> (Option<String>, Vec<(u32, String)>) {
        let tml = tml_with_shot_comment(field);
        let (configs, _, _) =
            convert_tml_to_import_configs(&tml, false, comments_on_stations).unwrap();
        let config = configs.into_iter().next().unwrap();
        let comment = config.shots.into_iter().next().unwrap().comment;
        (comment, config.station_comments)
    }

    /// The #483 headline: with comments kept on shots, the decode was skipped
    /// entirely, so the raw co-encoded field landed verbatim - invisible marker
    /// included - and the station half was silently destroyed.
    #[test]
    fn comments_on_shots_decodes_rather_than_leaking_the_raw_field() {
        let field =
            crate::models::comment_split::merge(Some("broken tape"), Some("junction with main"));
        let (comment, station_comments) = import_comment(field, false);

        let comment = comment.expect("the shot must keep the comment text");
        assert!(
            !comment.contains(crate::models::comment_split::SHOT_MARKER),
            "the ZWSP marker leaked into the project: {comment:?}"
        );
        // Both halves survive, in the order the field carried them - the user
        // asked for comments on shots, so nothing is routed to a station.
        assert_eq!(comment, "broken tape\njunction with main");
        assert!(station_comments.is_empty());
    }

    /// The other half of the same call site, so a future edit cannot fix the
    /// shots path by breaking the default one.
    #[test]
    fn comments_on_stations_still_partitions_the_field() {
        let field =
            crate::models::comment_split::merge(Some("broken tape"), Some("junction with main"));
        let (comment, station_comments) = import_comment(field, true);

        assert_eq!(comment.as_deref(), Some("broken tape"));
        // Station index is the shot's "to" station: shot 0 ends at station 1.
        assert_eq!(
            station_comments,
            vec![(1, "junction with main".to_string())]
        );
    }

    /// The corruption that made this P1 rather than cosmetic: a second
    /// export/import cycle used to re-mark already-marked text, after which no
    /// import could tell the halves apart. Decoding on ingestion makes the
    /// shots path a fixed point.
    /// An Ariane user typing a note ABOVE our marked text produces a field
    /// whose marked and unmarked lines interleave. Their line order is theirs -
    /// partitioning the field into groups would silently rewrite it.
    #[test]
    fn interleaved_lines_keep_their_order_on_the_shots_path() {
        let marker = crate::models::comment_split::SHOT_MARKER;
        let field = Some(format!("sump at 40 m\n{marker}do not pass\nsilt below"));
        let (comment, station_comments) = import_comment(field, false);

        assert_eq!(
            comment.as_deref(),
            Some("sump at 40 m\ndo not pass\nsilt below")
        );
        assert!(station_comments.is_empty());
    }

    /// A field carrying nothing but a marker decodes to an empty string, which
    /// is not the same as having no comment at all.
    #[test]
    fn a_marker_only_field_yields_no_comment_rather_than_an_empty_one() {
        let field = Some(crate::models::comment_split::SHOT_MARKER.to_string());
        for comments_on_stations in [false, true] {
            let (comment, station_comments) = import_comment(field.clone(), comments_on_stations);
            assert_eq!(comment, None);
            assert!(station_comments.is_empty());
        }
    }

    #[test]
    fn a_second_cycle_on_the_shots_path_is_stable() {
        let marker = crate::models::comment_split::SHOT_MARKER;
        // The shot half carries an EMBEDDED marker too, so this test also reds
        // if the strip is dropped - decoding alone would leave that one behind
        // and the second cycle would not match the first.
        let field = crate::models::comment_split::merge(
            Some(&format!("broken{marker} tape")),
            Some("junction with main"),
        );
        let (first, _) = import_comment(field, false);

        // Export again: the whole comment is now shot-side, so it is marked once.
        let reexported = crate::models::comment_split::merge(first.as_deref(), None);
        let (second, station_comments) = import_comment(reexported, false);

        assert_eq!(second, first);
        assert!(station_comments.is_empty());
    }

    /// A marker that is not line-leading has no meaning and cannot be typed, so
    /// it is foreign junk - it must not ride into the project, where it is
    /// invisible in the UI and leaks verbatim into CSV and JSON export.
    #[test]
    fn an_embedded_marker_never_reaches_the_project() {
        let marker = crate::models::comment_split::SHOT_MARKER;
        let field = Some(format!("note with{marker}embedded zwsp"));

        for comments_on_stations in [false, true] {
            let (comment, station_comments) = import_comment(field.clone(), comments_on_stations);
            let text = comment
                .or_else(|| station_comments.first().map(|(_, t)| t.clone()))
                .expect("the text must land somewhere");
            assert_eq!(text, "note withembedded zwsp");
        }
    }

    /// An Ariane-authored file carries no markers at all. On the shots path the
    /// text must stay on the shot - decoding must not start routing plain
    /// comments to stations against the user's chosen setting.
    #[test]
    fn unmarked_ariane_text_stays_on_the_shot_when_comments_stay_on_shots() {
        let (comment, station_comments) = import_comment(Some("muddy water".to_string()), false);
        assert_eq!(comment.as_deref(), Some("muddy water"));
        assert!(station_comments.is_empty());
    }

    /// Why each emitted chain starts where it does (#412).
    ///
    /// The point of the split is that only ONE of these classes could ever be
    /// merged away into a neighbouring config: a chain whose parent shot is the
    /// LAST shot of the chain that contains it. Every other class attaches
    /// mid-chain, and a `models::Section` is a strictly linear polyline
    /// (`Section::shots` is consumed positionally; every cross-reference into a
    /// section is a single `station_index`), so a mid-chain attachment has no
    /// representation inside one config.
    #[derive(Default, Debug, PartialEq, Eq)]
    struct ChainOrigins {
        /// Parent is a START shot, so there is no parent chain at all.
        entrance_root: usize,
        /// Parent shot is in a DIFFERENT declared section.
        cross_section_root: usize,
        /// Parent shot is in the same declared section AND ends its chain -
        /// the only in-section merge a linear config could express.
        in_section_parent_is_chain_end: usize,
        /// Parent shot is in the same declared section but sits mid-chain.
        in_section_parent_mid_chain: usize,
        /// Of `cross_section_root`, those whose parent ends its chain.
        cross_root_at_parent_chain_end: usize,
        /// Chains that share a start station with an earlier chain: a "V" that
        /// one reversed-then-concatenated linear run could express.
        sibling_v_merge_savings: usize,
        /// Of `cross_root_at_parent_chain_end`, those whose surveyor, date and
        /// colour all match the chain they would be appended to. A config
        /// carries ONE of each, so merging a mismatched pair silently
        /// reattributes one leg's trip metadata to the other's.
        cross_root_at_chain_end_metadata_match: usize,
        /// Same guard for the sibling-V pairs.
        sibling_v_metadata_match: usize,
    }

    fn classify_chain_origins(tml: &TmlFile) -> ChainOrigins {
        let base_of = |n: &str| match n.rfind(" (branch ") {
            Some(i) if n.ends_with(')') => n[..i].to_string(),
            _ => n.to_string(),
        };
        let mut shot_section: HashMap<i64, String> = HashMap::new();
        let mut ends_a_chain: std::collections::HashSet<i64> = std::collections::HashSet::new();
        let mut chains_per_start: HashMap<i64, usize> = HashMap::new();
        // Which chain ENDS at a given shot id, so a candidate can be compared
        // against the exact chain it would be appended to.
        let mut chain_ending_at: HashMap<i64, usize> = HashMap::new();
        let mut chains_by_start: HashMap<i64, Vec<usize>> = HashMap::new();
        for (idx, section) in tml.sections.iter().enumerate() {
            for shot in &section.shots {
                shot_section.insert(shot.id, shot.section.clone());
            }
            if let Some(last) = section.shots.last() {
                ends_a_chain.insert(last.id);
                chain_ending_at.insert(last.id, idx);
            }
            *chains_per_start.entry(section.start_ariane_id).or_insert(0) += 1;
            chains_by_start
                .entry(section.start_ariane_id)
                .or_default()
                .push(idx);
        }
        let trip_metadata_matches = |a: usize, b: usize| {
            let (x, y) = (&tml.sections[a], &tml.sections[b]);
            x.surveyor == y.surveyor && x.date == y.date && x.color == y.color
        };

        let mut out = ChainOrigins {
            sibling_v_merge_savings: chains_per_start
                .values()
                .filter(|&&n| n >= 2)
                .sum::<usize>()
                - chains_per_start.values().filter(|&&n| n >= 2).count(),
            ..ChainOrigins::default()
        };
        for (idx, section) in tml.sections.iter().enumerate() {
            let parent_ends_chain = ends_a_chain.contains(&section.start_ariane_id);
            match shot_section.get(&section.start_ariane_id) {
                None => out.entrance_root += 1,
                Some(parent_section) if *parent_section != base_of(&section.name) => {
                    out.cross_section_root += 1;
                    if parent_ends_chain {
                        out.cross_root_at_parent_chain_end += 1;
                        if chain_ending_at
                            .get(&section.start_ariane_id)
                            .is_some_and(|&parent| trip_metadata_matches(parent, idx))
                        {
                            out.cross_root_at_chain_end_metadata_match += 1;
                        }
                    }
                }
                Some(_) if parent_ends_chain => out.in_section_parent_is_chain_end += 1,
                Some(_) => out.in_section_parent_mid_chain += 1,
            }
        }

        // Sibling-V pairs that also agree on trip metadata.
        let mut starts: Vec<&i64> = chains_by_start.keys().collect();
        starts.sort_unstable();
        for start in starts {
            let group = &chains_by_start[start];
            if group.len() < 2 {
                continue;
            }
            let head = group[0];
            out.sibling_v_metadata_match += group[1..]
                .iter()
                .filter(|&&other| trip_metadata_matches(head, other))
                .count();
        }
        out
    }

    /// The structural reason #412 cannot be fixed by merging deferred branch
    /// chains back into their parent's config.
    ///
    /// Synthetic multi-root, multi-T topology, entirely in-code:
    ///
    /// ```text
    ///   section "Main":   1 -> 2 -> 3 -> 4 -> 5      (mainline, from entrance)
    ///                          |         `-> 8 -> 9  (T at 4)
    ///                          `-> 6 -> 7            (T at 2)
    ///   section "Side":  10 -> 11                    (root parented into Main)
    /// ```
    ///
    /// The continuation rule sends the walk through the LONGER downstream leg
    /// at each T, so shot 2 and shot 4 both stay mid-mainline. That is not an
    /// accident of this fixture: `collect_linear_chain` only defers a successor
    /// when it continues into a sibling of that successor, so the junction shot
    /// always has a chain step after it. Hence, for EVERY deferred chain, the
    /// parent shot is mid-chain - and a config is a linear polyline, so there
    /// is nowhere in the parent config to put it.
    ///
    /// This test fails the moment that stops being true, which is exactly when
    /// a contiguous re-join becomes implementable.
    #[test]
    fn every_deferred_branch_attaches_mid_chain_so_no_config_can_absorb_it() {
        let shots = [
            tml_shot_xml_az_len(1, -1, "Main", 90.0, 10.0),
            tml_shot_xml_az_len(2, 1, "Main", 90.0, 10.0),
            tml_shot_xml_az_len(3, 2, "Main", 90.0, 10.0),
            tml_shot_xml_az_len(4, 3, "Main", 90.0, 10.0),
            tml_shot_xml_az_len(5, 4, "Main", 90.0, 40.0),
            tml_shot_xml_az_len(6, 2, "Main", 180.0, 5.0),
            tml_shot_xml_az_len(7, 6, "Main", 180.0, 5.0),
            tml_shot_xml_az_len(8, 4, "Main", 0.0, 5.0),
            tml_shot_xml_az_len(9, 8, "Main", 0.0, 5.0),
            tml_shot_xml_az_len(10, 3, "Side", 270.0, 8.0),
            tml_shot_xml_az_len(11, 10, "Side", 270.0, 8.0),
        ]
        .join("\n");
        let tml = parse_synthetic_tmlu(&wrap_xml(&shots));

        let origins = classify_chain_origins(&tml);
        assert_eq!(
            origins.in_section_parent_is_chain_end, 0,
            "a deferred in-section branch never attaches at a chain END, so the \
             'merge into the parent chain's config' rule has zero candidates; \
             origins were {origins:?}"
        );
        assert_eq!(
            origins.in_section_parent_mid_chain, 2,
            "both Ts (at shot 2 and shot 4) defer a chain that hangs off a \
             mid-mainline shot; origins were {origins:?}"
        );
        assert_eq!(
            origins.cross_section_root, 1,
            "\"Side\" is parented into \"Main\", the boundary #412 leaves alone"
        );

        // The mainline really did survive both junctions: that is what the
        // continuation heuristic buys, and it is what keeps the parent shot
        // mid-chain rather than at a chain end.
        let mainline = tml
            .sections
            .iter()
            .find(|s| s.name == "Main")
            .expect("the mainline keeps the section's own name");
        assert_eq!(
            mainline.shots.iter().map(|s| s.id).collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 5],
            "the walk takes the longest downstream leg at each T"
        );

        // Every shot is emitted exactly once across the fragments, and the
        // fragment count is exactly the linear-chain count - there is no
        // representation that could make it smaller without changing
        // `models::Section` from a polyline into a tree.
        let mut ids: Vec<i64> = tml
            .sections
            .iter()
            .flat_map(|s| &s.shots)
            .map(|s| s.id)
            .collect();
        ids.sort_unstable();
        assert_eq!(ids, (1..=11).collect::<Vec<_>>());
        assert_eq!(tml.sections.len(), 4, "Main + 2 deferred Ts + Side");
    }

    /// The contiguous join drops the fragment count by exactly the chains that
    /// run in SERIES, and leaves every mid-chain branch alone (#412).
    ///
    /// Synthetic multi-root, multi-T topology, entirely in-code:
    ///
    /// ```text
    ///   "Trunk":  1 -> 2 -> 3         mainline, from the entrance
    ///                  `-> 6 -> 7     T at 2: deferred, parent is MID-chain
    ///   "Deep":   4 -> 5              parent is 3, Trunk's LAST shot -> JOINS
    ///   "Stub":   8 -> 9              parent is 2, MID-Trunk -> stays split
    /// ```
    ///
    /// So 4 chains become 3, and the one that merged is the only one whose
    /// parent ended a chain. A closure spanning the join proves the endpoints
    /// are re-indexed against the MERGED run rather than the pre-merge
    /// fragments.
    #[test]
    fn the_join_absorbs_series_chains_and_only_those() {
        let closure_5_to_7 = r"<SurveyData>
    <ID>10</ID>
    <FromID>5</FromID>
    <ClosureToID>7</ClosureToID>
    <Type>CLOSURE</Type>
    <Section>Deep</Section>
    <Azimut>0.0</Azimut>
    <Length>0.0</Length>
    <Depth>5.0</Depth>
    <DepthIn>-1.0</DepthIn>
    <Inclination>0.0</Inclination>
    <Left>0.0</Left>
    <Right>0.0</Right>
    <Up>0.0</Up>
    <Down>0.0</Down>
    <Comment></Comment>
    <Explorer>Tester</Explorer>
    <Date>2024-06-15</Date>
    <Color>0x8f6112ff</Color>
    <Excluded>false</Excluded>
    <Latitude>0.0</Latitude>
    <Longitude>0.0</Longitude>
</SurveyData>"
            .to_owned();
        let shots = [
            tml_shot_xml_az_len(1, -1, "Trunk", 90.0, 10.0),
            tml_shot_xml_az_len(2, 1, "Trunk", 90.0, 10.0),
            // Longer downstream than shot 6, so the mainline continues here and
            // shot 2 stays mid-chain.
            tml_shot_xml_az_len(3, 2, "Trunk", 90.0, 40.0),
            tml_shot_xml_az_len(4, 3, "Deep", 90.0, 10.0),
            tml_shot_xml_az_len(5, 4, "Deep", 90.0, 10.0),
            tml_shot_xml_az_len(6, 2, "Trunk", 180.0, 5.0),
            tml_shot_xml_az_len(7, 6, "Trunk", 180.0, 5.0),
            tml_shot_xml_az_len(8, 2, "Stub", 270.0, 5.0),
            tml_shot_xml_az_len(9, 8, "Stub", 270.0, 5.0),
            closure_5_to_7,
        ]
        .join("\n");
        let tml = parse_synthetic_tmlu(&wrap_xml(&shots));

        let names: Vec<&str> = tml.sections.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["Trunk + Deep", "Trunk (branch 2)", "Stub"],
            "only the series chain is absorbed, and it says so in the name"
        );

        let merged = &tml.sections[0];
        assert_eq!(
            merged
                .shots
                .iter()
                .filter(|s| s.shot_type != TmlShotType::Closure)
                .map(|s| s.id)
                .collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 5],
            "the merged run is one strictly linear import sequence, in file order"
        );
        assert_eq!(merged.start_ariane_id, -1, "still starts where Trunk did");
        assert_eq!(merged.end_ariane_id, 10, "and ends where Deep did");

        // Nothing was lost or duplicated by the join, and every emitted run is
        // still a polyline - the invariant every consumer of a TmlSection
        // assumes and nothing else in the pipeline checks.
        let mut ids: Vec<i64> = tml
            .sections
            .iter()
            .flat_map(|s| &s.shots)
            .map(|s| s.id)
            .collect();
        ids.sort_unstable();
        assert_eq!(ids, (1..=10).collect::<Vec<_>>());
        assert_eq!(linearize::first_linearity_violation(&tml.sections), None);

        // The one remaining candidate class is gone: nothing else runs in
        // series, so a second pass would find nothing to do.
        let origins = classify_chain_origins(&tml);
        assert_eq!(
            origins.cross_root_at_parent_chain_end, 0,
            "the join is a fixed point; origins were {origins:?}"
        );

        // Closure endpoints index the MERGED run. Shot 5 is the 5th importable
        // shot of section 0, so it ends at station 5 - a number that only comes
        // out right if the merge happened before the closure was resolved.
        let (configs, closures, _) =
            convert_tml_to_import_configs(&tml, false, false).expect("convert");
        assert_eq!(configs.len(), 3);
        assert_eq!(
            configs[0].shots.len(),
            5,
            "the CLOSURE shot is not imported"
        );
        assert_eq!(closures.len(), 1);
        assert_eq!(
            (
                closures[0].from_section_index,
                closures[0].from_station_index
            ),
            (0, 5),
            "the closure's near end is re-indexed against the merged run"
        );
        assert_eq!(
            (closures[0].to_section_index, closures[0].to_station_index),
            (1, 2),
            "and its far end still points at the untouched branch"
        );
    }

    /// Section names and shot ids for the join-refusal fixtures: "A" is a
    /// two-shot chain from the entrance, "B" continues from A's LAST shot.
    /// Every refusal test below is this same shape with one thing changed, so
    /// what the test is about is the difference and nothing else.
    ///
    /// B's tail inherits `b_head`'s trip so that B stays ONE trip whatever a
    /// test does to its head: a per-trip cut (#427) would otherwise split B
    /// itself, and a join-refusal test would be reporting that instead of the
    /// refusal it is about.
    fn series_a_b(a_tail: TripShot<'_>, b_head: TripShot<'_>) -> TmlFile {
        let (explorer, date, color) = (b_head.explorer, b_head.date, b_head.color);
        let shots = [
            TripShot {
                id: 1,
                from_id: -1,
                section: "A",
                depth: 5.0,
                ..TripShot::default()
            }
            .xml(),
            a_tail.xml(),
            b_head.xml(),
            TripShot {
                id: 4,
                from_id: 3,
                section: "B",
                depth: 12.0,
                explorer,
                date,
                color,
                ..TripShot::default()
            }
            .xml(),
        ]
        .join("\n");
        parse_synthetic_tmlu(&wrap_xml(&shots))
    }

    fn section_names(tml: &TmlFile) -> Vec<&str> {
        tml.sections.iter().map(|s| s.name.as_str()).collect()
    }

    /// The seam has to be a station the import actually keeps.
    ///
    /// `convert_shots_to_shot_data` drops CLOSURE shots and excluded ones with
    /// the same filter, so joining onto an EXCLUDED tail would resolve the
    /// absorbed leg's first `depth_in` from the previous SURVIVING station -
    /// translating the whole leg - and the result would change with the
    /// include-excluded toggle, which section splitting must be invariant to.
    #[test]
    fn the_join_refuses_a_seam_the_import_would_filter_out() {
        let tml = series_a_b(
            TripShot {
                id: 2,
                from_id: 1,
                section: "A",
                excluded: true,
                depth: 9.0,
                ..TripShot::default()
            },
            // DepthIn -1 means "derive from the previous station", which is
            // exactly the value the seam would corrupt.
            TripShot {
                id: 3,
                from_id: 2,
                section: "B",
                depth: 11.0,
                depth_in: -1.0,
                ..TripShot::default()
            },
        );

        assert_eq!(
            section_names(&tml),
            vec!["A", "B"],
            "an excluded tail is not a station, so it is not a seam"
        );

        let (configs, _, _) = convert_tml_to_import_configs(&tml, false, false).expect("convert");
        assert_eq!(configs.len(), 2);
        // B's first shot derives its depth from the EXCLUDED shot 2 (9.0) via
        // start_depth, not from shot 1 (5.0) which is what a join through the
        // excluded seam would have left behind.
        assert!(
            (configs[1].shots[0].depth_in - 9.0).abs() < f64::EPSILON,
            "the absorbed leg must not be translated; got {}",
            configs[1].shots[0].depth_in
        );
    }

    /// A config carries ONE surveyor, date and colour, so absorbing a chain
    /// surveyed by someone else on another day would silently reattribute it.
    #[test]
    fn the_join_refuses_chains_from_a_different_trip() {
        let tml = series_a_b(
            TripShot {
                id: 2,
                from_id: 1,
                section: "A",
                depth: 9.0,
                ..TripShot::default()
            },
            TripShot {
                id: 3,
                from_id: 2,
                section: "B",
                explorer: "Somebody Else",
                date: "2025-02-02",
                depth: 11.0,
                ..TripShot::default()
            },
        );

        assert_eq!(
            section_names(&tml),
            vec!["A", "B"],
            "different trip metadata must not be merged away"
        );
        assert_eq!(tml.sections[1].surveyor.as_deref(), Some("Somebody Else"));
    }

    /// One declared Ariane section, two trips: a chain surveyed by a second
    /// team on a second day used to import entirely under the FIRST shot's
    /// surveyor, date and colour, with the rest dropped silently (#427).
    #[test]
    fn a_run_that_changes_trip_mid_chain_is_cut_into_one_section_per_trip() {
        let second_trip = TripShot {
            section: "A",
            explorer: "Ada Lovelace",
            date: "2025-03-04",
            color: "0x11223344",
            ..TripShot::default()
        };
        let shots = [
            TripShot {
                id: 1,
                from_id: -1,
                section: "A",
                depth: 5.0,
                ..TripShot::default()
            }
            .xml(),
            TripShot {
                id: 2,
                from_id: 1,
                section: "A",
                depth: 9.0,
                ..TripShot::default()
            }
            .xml(),
            TripShot {
                id: 3,
                from_id: 2,
                depth: 11.0,
                ..second_trip
            }
            .xml(),
            TripShot {
                id: 4,
                from_id: 3,
                depth: 12.0,
                ..second_trip
            }
            .xml(),
        ]
        .join("\n");
        let tml = parse_synthetic_tmlu(&wrap_xml(&shots));

        assert_eq!(section_names(&tml), vec!["A", "A (trip 2)"]);
        assert_eq!(tml.sections[0].surveyor.as_deref(), Some("Tester"));
        assert_eq!(tml.sections[1].surveyor.as_deref(), Some("Ada Lovelace"));
        assert_eq!(
            tml.sections[1].date,
            NaiveDate::from_ymd_opt(2025, 3, 4),
            "the second trip keeps its own date, not the chain head's"
        );
        assert_ne!(
            tml.sections[0].color, tml.sections[1].color,
            "and its own colour"
        );
        assert_eq!(tml.sections[0].shots.len(), 2);
        assert_eq!(tml.sections[1].shots.len(), 2);

        let (configs, _, warnings) =
            convert_tml_to_import_configs(&tml, false, false).expect("cvt");
        assert_eq!(configs.len(), 2);
        assert!(
            matches!(
                configs[1].placement,
                ImportPlacement::FromImporting {
                    import_station: 0,
                    target_section_index: 0,
                    target_station_index: 2,
                }
            ),
            "the cut part hangs off the station the cut was made at: {:?}",
            configs[1].placement
        );
        assert!(
            (configs[1].shots[0].depth_in - 9.0).abs() < f64::EPSILON,
            "and rebases its first DepthIn on that station's depth, not on the \
             run's original start; got {}",
            configs[1].shots[0].depth_in
        );

        // Including excluded shots removes `determine_placement`'s
        // `effective_start_id` fallback, so this run resolves through the cut
        // part's OWN start id or not at all.
        let (unfiltered, _, _) = convert_tml_to_import_configs(&tml, true, false).expect("cvt");
        assert!(
            matches!(
                unfiltered[1].placement,
                ImportPlacement::FromImporting {
                    target_section_index: 0,
                    target_station_index: 2,
                    ..
                }
            ),
            "the cut part carries its own start id: {:?}",
            unfiltered[1].placement
        );
        assert!(
            !warnings.iter().any(|w| w.contains("recorded a different")),
            "nothing was collapsed, so nothing to report: {warnings:?}"
        );
    }

    /// A trip that changes across a shot the import filters out has no station
    /// seam to cut at - the child's `from_id` would resolve to a shot that is
    /// not in the import, placing it at the origin. So it is reported instead,
    /// and only when the toggle actually imports the shot that lost its values.
    #[test]
    fn a_trip_change_with_no_station_seam_is_reported_rather_than_cut() {
        let shots = [
            TripShot {
                id: 1,
                from_id: -1,
                section: "A",
                depth: 5.0,
                ..TripShot::default()
            }
            .xml(),
            TripShot {
                id: 2,
                from_id: 1,
                section: "A",
                excluded: true,
                explorer: "Ada Lovelace",
                date: "2025-03-04",
                depth: 9.0,
                ..TripShot::default()
            }
            .xml(),
            TripShot {
                id: 3,
                from_id: 2,
                section: "A",
                depth: 11.0,
                ..TripShot::default()
            }
            .xml(),
        ]
        .join("\n");
        let tml = parse_synthetic_tmlu(&wrap_xml(&shots));

        assert_eq!(
            section_names(&tml),
            vec!["A"],
            "an excluded shot is not a station, so it is no seam to cut at"
        );

        let (_, _, kept) = convert_tml_to_import_configs(&tml, false, false).expect("cvt");
        assert!(
            !kept.iter().any(|w| w.contains("recorded a different")),
            "the shot that disagrees is not being imported, so nothing was lost: {kept:?}"
        );

        let (_, _, included) = convert_tml_to_import_configs(&tml, true, false).expect("cvt");
        let note = included
            .iter()
            .find(|w| w.contains("recorded a different"))
            .expect("importing it does lose its values, which the receipt must say");
        assert!(
            note.starts_with(
                "1 shot at 1 trip change in 1 section recorded a different surveyor or date"
            ),
            "the report names the counts and the fields: {note}"
        );
    }

    /// A chain of `count` shots under one declared name, whose `trip_of`
    /// picks each shot's `(explorer, date)`. Ids and `from_id`s run 1..=count.
    fn one_chain(count: i64, trip_of: impl Fn(i64) -> (String, String)) -> TmlFile {
        let shots: Vec<String> = (1..=count)
            .map(|id| {
                let (explorer, date) = trip_of(id);
                TripShot {
                    id,
                    from_id: id - 1,
                    section: "A",
                    explorer: &explorer,
                    date: &date,
                    depth: id as f64,
                    ..TripShot::default()
                }
                .xml()
            })
            .collect();
        parse_synthetic_tmlu(&wrap_xml(&shots.join("\n")))
    }

    /// A date per shot ROW - a data-entry timestamp, or a crafted file - would
    /// mint one section per shot from one declared name. The cut is capped, and
    /// what the cap refuses to cut is reported rather than dropped.
    #[test]
    fn the_cut_is_capped_and_the_rest_is_reported() {
        let rows = MAX_TRIP_CUTS_PER_RUN as i64 + 11;
        // 28-day "months" keep every generated date valid without date arithmetic.
        let tml = one_chain(rows, |id| {
            (
                "Tester".to_owned(),
                format!("2024-{:02}-{:02}", 1 + id / 28, 1 + id % 28),
            )
        });

        assert_eq!(
            tml.sections.len(),
            MAX_TRIP_CUTS_PER_RUN + 1,
            "one part per cut, plus the head - never one per shot"
        );
        assert_eq!(
            tml.sections[MAX_TRIP_CUTS_PER_RUN].shots.len(),
            11,
            "everything past the cap stays on the last part"
        );
        assert_eq!(linearize::first_linearity_violation(&tml.sections), None);

        let (configs, _, warnings) =
            convert_tml_to_import_configs(&tml, false, false).expect("cvt");
        assert_eq!(configs.len(), MAX_TRIP_CUTS_PER_RUN + 1);
        let note = warnings
            .iter()
            .find(|w| w.contains("recorded a different"))
            .expect("the uncut trips must reach the receipt");
        assert!(
            note.starts_with("10 shots at 10 trip changes in 1 section recorded a different date"),
            "the report names what the cap left behind: {note}"
        );
    }

    /// The per-run cap alone bounds the wrong denominator: a file chooses how
    /// many RUNS it has as well as how many trips each holds. Sections are the
    /// numerator of the `section_id * FEATURE_ID_STRIDE` shot-id encode, which
    /// holds only below 42_950 sections, so the budget is file-wide too.
    #[test]
    fn the_cut_budget_is_file_wide_not_only_per_run() {
        // Enough runs that the per-run cap alone would let them all through:
        // 20 x 256 = 5120 cuts against a 4096 budget.
        let runs = 20;
        let per_run = MAX_TRIP_CUTS_PER_RUN as i64 + 1;
        let mut shots: Vec<String> = Vec::new();
        for run in 0..runs {
            for seq in 0..per_run {
                let id = run * 1000 + seq + 1;
                shots.push(
                    TripShot {
                        id,
                        // Each run starts from the entrance, so they are
                        // separate runs rather than one chain.
                        from_id: if seq == 0 { -1 } else { id - 1 },
                        section: "A",
                        explorer: "Tester",
                        date: &format!("2024-{:02}-{:02}", 1 + seq / 28, 1 + seq % 28),
                        depth: seq as f64,
                        ..TripShot::default()
                    }
                    .xml(),
                );
            }
        }
        let tml = parse_synthetic_tmlu(&wrap_xml(&shots.join("\n")));

        let cuts = tml.sections.len() - runs as usize;
        assert_eq!(
            cuts,
            MAX_TRIP_CUTS_PER_FILE,
            "the file-wide budget is spent exactly once, however many runs ask \
             for it (got {} sections over {runs} runs)",
            tml.sections.len()
        );
        assert!(
            tml.sections.len() < 42_950,
            "and the result stays inside the shot-id encode's section range"
        );
        assert_eq!(linearize::first_linearity_violation(&tml.sections), None);

        let (_, _, warnings) = convert_tml_to_import_configs(&tml, false, false).expect("cvt");
        let note = warnings
            .iter()
            .find(|w| w.contains("recorded a different"))
            .expect("what the budget refused to cut must reach the receipt");
        assert!(
            note.contains("trip changes in"),
            "and it names how many places were collapsed: {note}"
        );
    }

    /// The budget also never spends id space the file has already used. Driven
    /// as a pure function because the interesting inputs are tens of thousands
    /// of runs, which is a fixture nobody should build to assert arithmetic.
    #[test]
    fn the_cut_budget_never_spends_id_space_the_file_already_used() {
        assert_eq!(linearize::cut_budget(0), MAX_TRIP_CUTS_PER_FILE);
        // 42_949 - 4_096: the last run count that can still afford a full budget.
        assert_eq!(linearize::cut_budget(38_853), MAX_TRIP_CUTS_PER_FILE);
        assert_eq!(linearize::cut_budget(38_854), MAX_TRIP_CUTS_PER_FILE - 1);
        assert_eq!(linearize::cut_budget(42_949), 0);
        assert_eq!(
            linearize::cut_budget(usize::MAX),
            0,
            "a file already past the boundary gets no cuts, and no underflow"
        );

        // The property the constants exist for: a shot id is
        // `section_id * FEATURE_ID_STRIDE + seq`, so the sections a file ends
        // up with must stay inside the u32 encode.
        for runs in [0, 1, 100, 38_000, 38_853, 40_000, 42_948, 42_949, 60_000] {
            let total = runs + linearize::cut_budget(runs);
            assert!(
                total <= 42_949 || runs > 42_949,
                "{runs} runs + {} cuts = {total} sections, past the id space",
                linearize::cut_budget(runs)
            );
        }
    }

    /// Three trips in one run: the parts are cut back to front and each one
    /// rebases on the PREVIOUS part's tail, so a two-part test cannot see a
    /// chaining mistake.
    #[test]
    fn a_run_holding_three_trips_is_cut_into_three_rebased_parts() {
        let tml = one_chain(6, |id| {
            let trip = (id - 1) / 2 + 1; // 1,1,2,2,3,3
            (format!("Team {trip}"), format!("2025-0{trip}-01"))
        });

        assert_eq!(
            section_names(&tml),
            vec!["A", "A (trip 2)", "A (trip 3)"],
            "the suffix counts parts, and the head keeps the declared name"
        );
        let ids: Vec<Vec<i64>> = tml
            .sections
            .iter()
            .map(|s| s.shots.iter().map(|shot| shot.id).collect())
            .collect();
        assert_eq!(ids, vec![vec![1, 2], vec![3, 4], vec![5, 6]]);
        assert_eq!(tml.sections[2].surveyor.as_deref(), Some("Team 3"));

        let (configs, _, warnings) =
            convert_tml_to_import_configs(&tml, false, false).expect("cvt");
        assert!(
            !warnings.iter().any(|w| w.contains("recorded a different")),
            "every trip got its own section, so nothing collapsed: {warnings:?}"
        );
        // `one_chain` gives shot N depth N, so part 3 must rebase on shot 4.
        assert!(
            (configs[2].shots[0].depth_in - 4.0).abs() < f64::EPSILON,
            "part 3 rebases on part 2's tail, not on part 1's or the run's start; got {}",
            configs[2].shots[0].depth_in
        );
        assert!(
            matches!(
                configs[2].placement,
                ImportPlacement::FromImporting {
                    target_section_index: 1,
                    target_station_index: 2,
                    ..
                }
            ),
            "and hangs off part 2's last station: {:?}",
            configs[2].placement
        );
    }

    /// A run that is ALREADY a "(branch N)" split is cut like any other, and
    /// stays a branch: the wizard's branch marker is about topology, which the
    /// cut does not touch.
    #[test]
    fn cutting_a_branch_run_keeps_its_branch_marker_and_its_own_parent() {
        let mainline: Vec<String> = (1..=3)
            .map(|id| tml_shot_xml_az_len(id, id - 1, "Main", 90.0, 10.0))
            .collect();
        // A stub off shot 1, whose second shot is a second trip.
        let stub = [
            tml_shot_xml_az_len(20, 1, "Main", 0.0, 2.0),
            tml_shot_xml_az_len(21, 20, "Main", 0.0, 2.0)
                .replace("<Explorer>Tester</Explorer>", "<Explorer>Ada</Explorer>"),
        ];
        let xml = wrap_xml(
            &[
                vec![tml_shot_xml(0, -1, "START", "Start")],
                mainline,
                stub.to_vec(),
            ]
            .concat()
            .join("\n"),
        );
        let tml = parse_synthetic_tmlu(&xml);

        assert_eq!(
            section_names(&tml),
            vec!["Main", "Main (branch 2)", "Main (branch 2) (trip 2)"]
        );
        assert!(
            tml.sections[2].is_branch_split,
            "a cut part of a branch is still that branch"
        );
        assert_eq!(tml.sections[2].surveyor.as_deref(), Some("Ada"));

        let (configs, _, _) = convert_tml_to_import_configs(&tml, false, false).expect("cvt");
        let cut = configs
            .iter()
            .position(|c| c.name.as_deref() == Some("Main (branch 2) (trip 2)"))
            .expect("the cut part is imported");
        let branch = configs
            .iter()
            .position(|c| c.name.as_deref() == Some("Main (branch 2)"))
            .expect("so is the branch it came from");
        assert!(
            matches!(
                configs[cut].placement,
                ImportPlacement::FromImporting { target_section_index, target_station_index: 1, .. }
                    if target_section_index as usize == branch
            ),
            "the cut part hangs off the BRANCH's own tail, not the mainline: {:?}",
            configs[cut].placement
        );
    }

    /// A file may itself declare a section called "A (trip 2)". Nothing in the
    /// import keys on a section NAME - grouping keys on the file's own
    /// `<Section>` text before any suffix exists, placement and closures key on
    /// Ariane ids, and `simplify_lone_branch_names` only rewrites "(branch N)"
    /// names - so the collision is accepted and shown as-is, exactly as a file
    /// declaring two sections with one name always has been.
    #[test]
    fn a_declared_name_that_collides_with_a_cut_suffix_is_accepted() {
        let shots = [
            TripShot {
                id: 1,
                from_id: -1,
                section: "A",
                depth: 5.0,
                ..TripShot::default()
            }
            .xml(),
            TripShot {
                id: 2,
                from_id: 1,
                section: "A",
                explorer: "Ada",
                depth: 9.0,
                ..TripShot::default()
            }
            .xml(),
            TripShot {
                id: 3,
                from_id: 2,
                section: "A (trip 2)",
                explorer: "Ada",
                depth: 11.0,
                ..TripShot::default()
            }
            .xml(),
        ]
        .join("\n");
        let tml = parse_synthetic_tmlu(&wrap_xml(&shots));

        assert_eq!(
            section_names(&tml),
            vec!["A", "A (trip 2)", "A (trip 2)"],
            "the minted suffix collides with the declared name, and both stand"
        );
        let ids: Vec<Vec<i64>> = tml
            .sections
            .iter()
            .map(|s| s.shots.iter().map(|shot| shot.id).collect())
            .collect();
        assert_eq!(ids, vec![vec![1], vec![2], vec![3]]);

        let (configs, _, _) = convert_tml_to_import_configs(&tml, false, false).expect("cvt");
        assert_eq!(configs.len(), 3, "each is its own config");
        assert!(
            matches!(
                configs[2].placement,
                ImportPlacement::FromImporting {
                    target_section_index: 1,
                    target_station_index: 1,
                    ..
                }
            ),
            "the declared section resolves onto the CUT part by Ariane id, not \
             onto whichever section shares its name: {:?}",
            configs[2].placement
        );
    }

    /// A run whose HEAD shot is excluded must not be named after it: under the
    /// default toggle that shot is not imported, so every shot that IS imported
    /// disagreed with its own section and the receipt reported a loss that had
    /// not happened.
    #[test]
    fn a_run_takes_its_trip_from_the_first_shot_that_can_be_a_station() {
        let shots = [
            TripShot {
                id: 1,
                from_id: -1,
                section: "A",
                excluded: true,
                explorer: "T1",
                date: "2025-01-01",
                depth: 5.0,
                ..TripShot::default()
            }
            .xml(),
            TripShot {
                id: 2,
                from_id: 1,
                section: "A",
                explorer: "T2",
                date: "2025-02-02",
                depth: 9.0,
                ..TripShot::default()
            }
            .xml(),
            TripShot {
                id: 3,
                from_id: 2,
                section: "A",
                explorer: "T2",
                date: "2025-02-02",
                depth: 11.0,
                ..TripShot::default()
            }
            .xml(),
        ]
        .join("\n");
        let tml = parse_synthetic_tmlu(&wrap_xml(&shots));

        assert_eq!(section_names(&tml), vec!["A"]);
        assert_eq!(
            tml.sections[0].surveyor.as_deref(),
            Some("T2"),
            "the excluded head names nothing"
        );
        let (_, _, warnings) = convert_tml_to_import_configs(&tml, false, false).expect("cvt");
        assert!(
            !warnings.iter().any(|w| w.contains("recorded a different")),
            "and no phantom collapse is reported: {warnings:?}"
        );
    }

    /// One uncuttable boundary that strands a long tail is ONE place the file
    /// lost a value, not one per shot: the report counts trip CHANGES beside
    /// the shots, so a 3-shot stretch does not read as three findings.
    #[test]
    fn a_collapsed_stretch_is_reported_as_one_trip_change() {
        let on_trip = |id: i64, excluded: bool, explorer: &str| {
            TripShot {
                id,
                from_id: id - 1,
                section: "A",
                excluded,
                explorer,
                depth: id as f64,
                ..TripShot::default()
            }
            .xml()
        };
        let shots = [
            on_trip(1, false, "Team One"),
            // No station seam here, so the three shots after it cannot be cut off.
            on_trip(2, true, "Team One"),
            on_trip(3, false, "Team Two"),
            on_trip(4, false, "Team Two"),
            on_trip(5, false, "Team Two"),
        ]
        .join("\n");
        let tml = parse_synthetic_tmlu(&wrap_xml(&shots));

        assert_eq!(section_names(&tml), vec!["A"]);
        assert_eq!(tml.sections[0].surveyor.as_deref(), Some("Team One"));

        let (_, _, warnings) = convert_tml_to_import_configs(&tml, false, false).expect("cvt");
        let note = warnings
            .iter()
            .find(|w| w.contains("recorded a different"))
            .expect("three shots did lose their surveyor");
        assert!(
            note.starts_with("3 shots at 1 trip change in 1 section recorded a different surveyor"),
            "three shots on ONE collapsed trip is one change: {note}"
        );
    }

    /// A mainline running through a T, with a stub written at the junction.
    /// `retrip` moves everything past the T onto a second trip.
    fn mainline_through_a_t(retrip: bool) -> TmlFile {
        let retripped = |xml: String| {
            if !retrip {
                return xml;
            }
            let out = xml.replace("<Explorer>Tester</Explorer>", "<Explorer>Ada</Explorer>");
            assert_ne!(out, xml, "the fixture's explorer element moved");
            out
        };
        let xml = wrap_xml(
            &[
                tml_shot_xml(0, -1, "START", "Start"),
                tml_shot_xml_az_len(1, 0, "Main", 90.0, 10.0),
                tml_shot_xml_az_len(2, 1, "Main", 90.0, 10.0),
                tml_shot_xml_az_len(20, 2, "Main", 0.0, 2.0),
                tml_shot_xml_az_len(21, 20, "Main", 0.0, 2.0),
                retripped(tml_shot_xml_az_len(3, 2, "Main", 90.0, 10.0)),
                retripped(tml_shot_xml_az_len(4, 3, "Main", 90.0, 10.0)),
            ]
            .join("\n"),
        );
        parse_synthetic_tmlu(&xml)
    }

    /// The #504 mainline must be untouched by the cut when the file records one
    /// trip - and cut in exactly one place when it does not.
    #[test]
    fn the_trip_cut_leaves_a_single_trip_mainline_whole() {
        let one_trip = mainline_through_a_t(false);
        assert_eq!(
            section_names(&one_trip),
            vec!["Main", "Main (branch 2)"],
            "the mainline survives its T uncut and the stub stays a branch"
        );
        assert_eq!(
            one_trip.sections[0]
                .shots
                .iter()
                .map(|s| s.id)
                .collect::<Vec<_>>(),
            vec![1, 2, 3, 4]
        );

        let two_trips = mainline_through_a_t(true);
        assert_eq!(
            section_names(&two_trips),
            vec!["Main", "Main (trip 2)", "Main (branch 2)"],
            "one cut, where the trip changed"
        );
        assert_eq!(two_trips.sections[1].surveyor.as_deref(), Some("Ada"));
        assert_eq!(
            two_trips.sections[1]
                .shots
                .iter()
                .map(|s| s.id)
                .collect::<Vec<_>>(),
            vec![3, 4]
        );
    }

    /// The cut runs AFTER the join, because a cut end is not a chain end: it
    /// has an in-section successor by construction. Offering it to the join let
    /// the stub at the junction splice onto the mainline's head - taking the
    /// mainline's name, with the real continuation left as a separate run.
    #[test]
    fn a_cut_end_is_not_offered_to_the_join_as_a_chain_end() {
        let tml = mainline_through_a_t(true);

        assert!(
            !tml.sections.iter().any(|s| s.name.contains(" + ")),
            "nothing in this file is a series continuation: {:?}",
            section_names(&tml)
        );
        assert_eq!(
            tml.sections[2]
                .shots
                .iter()
                .map(|s| s.id)
                .collect::<Vec<_>>(),
            vec![20, 21],
            "the stub stays its own branch instead of being absorbed"
        );
        assert_eq!(linearize::first_linearity_violation(&tml.sections), None);
    }

    /// The cost of that order, pinned so it is a decision and not a surprise: a
    /// chain continuing a CUT tail under its own name is no longer joinable to
    /// it, because the join saw the run before the cut and compared its head.
    #[test]
    fn a_chain_continuing_a_cut_tail_stays_its_own_section() {
        let second_trip = TripShot {
            explorer: "Ada Lovelace",
            date: "2025-03-04",
            ..TripShot::default()
        };
        let shots = [
            TripShot {
                id: 1,
                from_id: -1,
                section: "A",
                depth: 5.0,
                ..TripShot::default()
            }
            .xml(),
            TripShot {
                id: 2,
                from_id: 1,
                section: "A",
                depth: 9.0,
                ..second_trip
            }
            .xml(),
            TripShot {
                id: 3,
                from_id: 2,
                section: "B",
                depth: 11.0,
                ..second_trip
            }
            .xml(),
        ]
        .join("\n");
        let tml = parse_synthetic_tmlu(&wrap_xml(&shots));

        assert_eq!(section_names(&tml), vec!["A", "A (trip 2)", "B"]);
        assert_eq!(tml.sections[1].surveyor.as_deref(), Some("Ada Lovelace"));
        assert_eq!(tml.sections[2].surveyor.as_deref(), Some("Ada Lovelace"));
        assert_eq!(linearize::first_linearity_violation(&tml.sections), None);
    }

    /// Ariane's `Explorer` is one element holding two roles. The surveyor half
    /// used to be dropped, and an `unknown` explorer half took the surveyor
    /// down with it (#427).
    #[test]
    fn both_halves_of_the_combined_explorer_field_survive() {
        let surveyor_of = |explorer: &str| {
            let xml = wrap_xml(
                &TripShot {
                    id: 1,
                    from_id: -1,
                    section: "A",
                    explorer,
                    ..TripShot::default()
                }
                .xml(),
            );
            parse_synthetic_tmlu(&xml).sections[0].surveyor.clone()
        };
        let elements = |explorer: &str, surveyor: &str| {
            format!("&lt;Explorer&gt;{explorer}&lt;/Explorer&gt;&lt;Surveyor&gt;{surveyor}&lt;/Surveyor&gt;")
        };

        assert_eq!(
            surveyor_of(&elements("nh", "ds")).as_deref(),
            Some("nh / ds"),
            "both halves, joined the way real files write a compound value"
        );
        assert_eq!(
            surveyor_of(&elements("unknown", "ds")).as_deref(),
            Some("ds"),
            "an unfilled explorer half must not take the surveyor with it"
        );
        assert_eq!(
            surveyor_of(&elements("nh", "unknown")).as_deref(),
            Some("nh")
        );
        assert_eq!(surveyor_of(&elements("unknown", "unknown")), None);
        assert_eq!(
            surveyor_of(&elements("nh", "nh")).as_deref(),
            Some("nh"),
            "one person in both roles is one name"
        );
        assert_eq!(
            surveyor_of("nh / ds").as_deref(),
            Some("nh / ds"),
            "and the plain compound form - what the export writes back - is \
             returned unchanged, so the round trip is idempotent"
        );
        assert_eq!(
            surveyor_of("&lt;Surveyor&gt;ds&lt;/Surveyor&gt;&lt;Explorer&gt;nh&lt;/Explorer&gt;")
                .as_deref(),
            Some("ds / nh"),
            "halves come out in DOCUMENT order, not in the order this reader \
             happens to look for the tags"
        );
        assert_eq!(
            surveyor_of("bare&lt;Surveyor&gt;ds&lt;/Surveyor&gt;").as_deref(),
            Some("bare / ds"),
            "a name that lost its element is still a name"
        );
        assert_eq!(
            surveyor_of(&elements("Unknown", "ds")).as_deref(),
            Some("ds"),
            "the placeholder is a word, so its capitalisation does not decide"
        );
        assert_eq!(surveyor_of("NONE"), None);

        // The PLAIN compound form gets the same per-half rules. It used to be
        // kept verbatim, so Ariane's placeholder rode into the section surveyor
        // and back out through the export.
        assert_eq!(
            surveyor_of("nh / ds").as_deref(),
            Some("nh / ds"),
            "a compound with no placeholder half is unchanged"
        );
        assert_eq!(surveyor_of("unknown / ds").as_deref(), Some("ds"));
        assert_eq!(surveyor_of("ds / unknown").as_deref(), Some("ds"));
        assert_eq!(surveyor_of("Unknown / None").as_deref(), None);
        // Idempotence, on the value this app itself writes back out: whatever
        // survives the filter must survive it again.
        for once in ["nh / ds", "unknown / ds", "ds / unknown", "nh"] {
            let first = surveyor_of(once);
            let second = first.as_deref().and_then(surveyor_of);
            assert_eq!(second, first, "re-reading {once:?} must not change it");
        }
    }

    /// With duplicate shot ids in the file, a chain END no longer names a
    /// unique parent, so an unrelated chain could splice onto the wrong one.
    /// Joining is disabled wholesale rather than guessing.
    #[test]
    fn the_join_is_disabled_when_the_file_reuses_shot_ids() {
        let shots = [
            TripShot {
                id: 1,
                from_id: -1,
                section: "A",
                depth: 5.0,
                ..TripShot::default()
            }
            .xml(),
            TripShot {
                id: 2,
                from_id: 1,
                section: "A",
                depth: 9.0,
                ..TripShot::default()
            }
            .xml(),
            TripShot {
                id: 3,
                from_id: 2,
                section: "B",
                depth: 11.0,
                ..TripShot::default()
            }
            .xml(),
            TripShot {
                id: 4,
                from_id: 3,
                section: "B",
                depth: 12.0,
                ..TripShot::default()
            }
            .xml(),
            // The reused id is deliberately OFF the A/B seam: without the
            // guard, A and B still join happily, so this fixture isolates the
            // guard rather than some incidental effect of the duplicate on the
            // topology index.
            TripShot {
                id: 9,
                from_id: -1,
                section: "C",
                depth: 3.0,
                ..TripShot::default()
            }
            .xml(),
            TripShot {
                id: 9,
                from_id: -1,
                section: "D",
                depth: 3.0,
                ..TripShot::default()
            }
            .xml(),
        ]
        .join("\n");
        let tml = parse_synthetic_tmlu(&wrap_xml(&shots));

        assert_eq!(
            tml.parse_anomalies.map(|a| a.duplicate_shot_ids),
            Some(1),
            "the fixture must actually trip the duplicate-id tally"
        );
        assert!(
            !section_names(&tml).iter().any(|n| n.contains(" + ")),
            "no join may happen on a file with reused ids; got {:?}",
            section_names(&tml)
        );
    }

    /// A refused join hands the child back as a fresh RUN ROOT, so the child
    /// still performs its own legal joins downstream.
    ///
    /// Chain A -> B -> C, where A/B repeat a UUID but B/C do not. Backing off
    /// A/B must not also cost B/C: the child re-enters as a root and absorbs C.
    /// Previously the child was left marked as somebody's tail, so the main
    /// pass skipped it and a trailing sweep emitted it - and C with it - as
    /// separate singletons, logged as a cycle neither was part of.
    ///
    /// This also covers the only way a config now attaches to the LAST station
    /// of another: a chain that WOULD have joined and backed off.
    #[test]
    fn a_refused_join_still_lets_the_child_absorb_its_own_tail() {
        const SHARED: &str = "bebebebe-0000-4000-8000-000000000001";
        let shots = [
            TripShot {
                id: 1,
                from_id: -1,
                section: "A",
                uuid: Some(SHARED),
                depth: 5.0,
                ..TripShot::default()
            }
            .xml(),
            TripShot {
                id: 2,
                from_id: 1,
                section: "A",
                depth: 9.0,
                ..TripShot::default()
            }
            .xml(),
            // Same UUID as shot 1: legal across sections, so A/B backs off.
            TripShot {
                id: 3,
                from_id: 2,
                section: "B",
                uuid: Some(SHARED),
                depth: 11.0,
                ..TripShot::default()
            }
            .xml(),
            TripShot {
                id: 4,
                from_id: 3,
                section: "B",
                depth: 12.0,
                ..TripShot::default()
            }
            .xml(),
            TripShot {
                id: 5,
                from_id: 4,
                section: "C",
                depth: 14.0,
                ..TripShot::default()
            }
            .xml(),
            TripShot {
                id: 6,
                from_id: 5,
                section: "C",
                depth: 16.0,
                ..TripShot::default()
            }
            .xml(),
        ]
        .join("\n");
        let tml = parse_synthetic_tmlu(&wrap_xml(&shots));

        assert_eq!(
            section_names(&tml),
            vec!["A", "B + C"],
            "the backed-off chain re-enters as a root and keeps its own tail"
        );
        assert_eq!(linearize::first_linearity_violation(&tml.sections), None);

        let (configs, _, _) = convert_tml_to_import_configs(&tml, false, false).expect("convert");
        assert_eq!(configs.len(), 2);
        assert!(
            matches!(configs[0].placement, ImportPlacement::Origin),
            "the file's first chain still anchors the import"
        );
        // A has two shots, so its LAST station is 2 - the placement shape that
        // only a refused join can produce.
        match &configs[1].placement {
            ImportPlacement::FromImporting {
                import_station,
                target_section_index,
                target_station_index,
            } => {
                assert_eq!(
                    (
                        *import_station,
                        *target_section_index,
                        *target_station_index
                    ),
                    (0, 0, 2),
                    "B attaches at A's final station"
                );
            }
            other => panic!("expected FromImporting, got {other:?}"),
        }
    }

    /// A merged name is bounded and sanitised, and the branch-suffix stripper
    /// does not eat half of one.
    ///
    /// Section names come from the file, so an attacker-controlled name can be
    /// arbitrarily long and can carry control characters; three of them
    /// concatenated flows to the wizard's fragment list and to log lines. And
    /// `simplify_lone_branch_names` used to `rfind(" (branch ")` anywhere in
    /// the name, so `"A (branch 2) + B"` collapsed to `"A"` - quietly losing
    /// the fact that the config also holds B.
    #[test]
    fn merged_names_are_bounded_and_survive_branch_simplification() {
        assert_eq!(branch_base("Foo (branch 2)"), "Foo");
        assert_eq!(
            branch_base("Foo (branch 2) + Bar"),
            "Foo (branch 2) + Bar",
            "a merged name keeps every part it holds"
        );
        assert_eq!(
            branch_base("Sump (branch note)"),
            "Sump (branch note)",
            "only a real trailing branch NUMBER is a suffix"
        );
        assert_eq!(branch_base("Plain"), "Plain");

        // Five sections in series under long, control-character-bearing names.
        let long = "N".repeat(400);
        let shots: Vec<String> = (1..=5)
            .map(|i| {
                TripShot {
                    id: i,
                    from_id: if i == 1 { -1 } else { i - 1 },
                    section: &format!("{long}\u{7}{i}"),
                    depth: 5.0 * f64::from(u32::try_from(i).unwrap_or(0)),
                    ..TripShot::default()
                }
                .xml()
            })
            .collect();
        let tml = parse_synthetic_tmlu(&wrap_xml(&shots.join("\n")));

        assert_eq!(tml.sections.len(), 1, "all five run in series");
        let name = &tml.sections[0].name;
        assert!(
            !name.chars().any(char::is_control),
            "control characters must not reach a merged label"
        );
        assert!(
            name.chars().count() <= 203,
            "merged name is capped; got {} chars",
            name.chars().count()
        );
        assert!(
            name.ends_with("..."),
            "an over-long merged name is truncated visibly; got {name:?}"
        );
    }

    /// The series join must not manufacture a duplicate shot UUID.
    ///
    /// Ariane shot ids are unique per SECTION, not per file, so two shots in
    /// different sections may legally carry the same `<UUID>` (#423) - there is
    /// a core test, `the_same_uuid_in_two_sections_of_one_file_is_left_alone`,
    /// that requires exactly that to survive an import. Joining those two
    /// sections would turn the legal repeat into a duplicate WITHIN one
    /// section, and `ensure_unique_shot_uuids` keeps the FIRST and re-mints the
    /// second - changing the identity of a shot that had one from the file and
    /// moving every station cross-reference anchored on it.
    ///
    /// So the join yields: the two chains stay separate even though they run in
    /// series. Fewer fragments is not worth a silently re-minted identity.
    #[test]
    fn the_join_backs_off_rather_than_duplicate_a_shot_uuid() {
        const SHARED: &str = "bebebebe-0000-4000-8000-000000000001";
        let with_uuid = |id: i64, from_id: i64, section: &str, uuid: &str| {
            format!(
                r"<SurveyData>
    <ID>{id}</ID>
    <FromID>{from_id}</FromID>
    <UUID>{uuid}</UUID>
    <ClosureToID>-1</ClosureToID>
    <Type>REAL</Type>
    <Section>{section}</Section>
    <Azimut>90.0</Azimut>
    <Length>10.0</Length>
    <Depth>5.0</Depth>
    <DepthIn>-1.0</DepthIn>
    <Inclination>0.0</Inclination>
    <Left>0.0</Left>
    <Right>0.0</Right>
    <Up>0.0</Up>
    <Down>0.0</Down>
    <Comment></Comment>
    <Explorer>Tester</Explorer>
    <Date>2024-06-15</Date>
    <Color>0x8f6112ff</Color>
    <Excluded>false</Excluded>
    <Latitude>0.0</Latitude>
    <Longitude>0.0</Longitude>
</SurveyData>"
            )
        };

        // Series topology: shot 3's parent is shot 2, the last shot of "A".
        // Without the UUID clash these two WOULD join.
        let clashing = [
            with_uuid(1, -1, "A", SHARED),
            tml_shot_xml_az_len(2, 1, "A", 90.0, 10.0),
            with_uuid(3, 2, "B", SHARED),
            tml_shot_xml_az_len(4, 3, "B", 90.0, 10.0),
        ]
        .join("\n");
        let tml = parse_synthetic_tmlu(&wrap_xml(&clashing));
        assert_eq!(
            tml.sections
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            vec!["A", "B"],
            "a cross-section UUID repeat is legal, so the join must back off"
        );

        // The identical topology with distinct UUIDs still joins, so the guard
        // is the UUID clash and not the shape of the fixture.
        let distinct = [
            with_uuid(1, -1, "A", SHARED),
            tml_shot_xml_az_len(2, 1, "A", 90.0, 10.0),
            with_uuid(3, 2, "B", "bebebebe-0000-4000-8000-000000000002"),
            tml_shot_xml_az_len(4, 3, "B", 90.0, 10.0),
        ]
        .join("\n");
        let joined = parse_synthetic_tmlu(&wrap_xml(&distinct));
        assert_eq!(
            joined
                .sections
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            vec!["A + B"],
            "with no clash the same shape joins"
        );
    }

    /// Re-joining must not silently break the two things every emitted config
    /// carries besides its shots: the closure indices and the depth rebase.
    ///
    /// Locks them against the topology above, so a future merge attempt that
    /// moves shots between configs has to keep both correct.
    #[test]
    fn closures_and_depth_rebasing_are_keyed_to_the_emitting_fragment() {
        let shots = [
            tml_shot_xml_az_len(1, -1, "Main", 90.0, 10.0),
            tml_shot_xml_az_len(2, 1, "Main", 90.0, 10.0),
            tml_shot_xml_az_len(3, 2, "Main", 90.0, 40.0),
            tml_shot_xml_az_len(4, 2, "Main", 180.0, 5.0),
            // A closure from the deferred branch's tip back to the mainline.
            format!(
                r"<SurveyData>
    <ID>5</ID>
    <FromID>4</FromID>
    <ClosureToID>3</ClosureToID>
    <Type>CLOSURE</Type>
    <Section>Main</Section>
    <Azimut>0.0</Azimut>
    <Length>0.0</Length>
    <Depth>5.0</Depth>
    <DepthIn>-1.0</DepthIn>
    <Inclination>0.0</Inclination>
    <Left>0.0</Left>
    <Right>0.0</Right>
    <Up>0.0</Up>
    <Down>0.0</Down>
    <Comment></Comment>
    <Explorer>Tester</Explorer>
    <Date>2024-06-15</Date>
    <Color>0x8f6112ff</Color>
    <Excluded>false</Excluded>
    <Latitude>0.0</Latitude>
    <Longitude>0.0</Longitude>
</SurveyData>"
            ),
        ]
        .join("\n");
        let tml = parse_synthetic_tmlu(&wrap_xml(&shots));

        let branch = tml
            .sections
            .iter()
            .find(|s| s.name != "Main")
            .expect("the T defers one branch");
        assert_eq!(
            branch.start_ariane_id, 2,
            "the branch records the mid-mainline shot it hangs off"
        );
        assert_eq!(
            branch.start_depth, 5.0,
            "start_depth rebases onto the PARENT shot's cumulative depth, not 0"
        );

        let (configs, closures, _) =
            convert_tml_to_import_configs(&tml, false, false).expect("convert");
        assert_eq!(configs.len(), 2, "mainline + the deferred branch");
        assert_eq!(closures.len(), 1, "the CLOSURE shot resolves to both ends");
        let closure = &closures[0];
        // Mainline is [1, 2, 3]; shot 3 is its 3rd importable shot, so it ends
        // at station 3. The branch is [4]; shot 4 ends at its station 1. Exact
        // pairs, not merely "different": a closure that resolved to the right
        // sections and the wrong stations would pass an inequality check.
        assert_eq!(
            (closure.from_section_index, closure.from_station_index),
            (1, 1),
            "the closure's near end is the branch tip"
        );
        assert_eq!(
            (closure.to_section_index, closure.to_station_index),
            (0, 3),
            "and its far end is the mainline station it ties back to"
        );
    }

    /// #412 regression bounds against the committed Naharon TML.
    ///
    /// The env-gated harness below measures whatever file you point it at; this
    /// pins the numbers for one that ships with the repo, so a change that
    /// re-fragments the importer fails in CI rather than in somebody's import.
    ///
    /// The bounds are one-sided on purpose - fewer, longer fragments may always
    /// pass - so a later merge class can only make them easier to meet. The
    /// fixture is committed (e2e/fixtures/naharon_sources/), so a missing file
    /// is a hard failure, not a silent skip: a skip here would make the test
    /// unfailable exactly when it matters.
    ///
    /// The numbers are set from what this file ACTUALLY produces, and they are
    /// deliberately thin: the metadata gate on the join (a chain is only
    /// absorbed when it agrees on surveyor, date and colour) leaves this file
    /// with one join, so the margin over the un-joined baseline is exactly one
    /// config and one shot of median. Both bounds still flip if the join is
    /// removed - un-joined this file is 18 configs with a median of 13 - which
    /// is what makes them worth asserting. The two <=2-shot stubs are chains a
    /// same-trip file would have absorbed and this one may not, so they are
    /// bounded rather than forbidden.
    #[test]
    fn the_naharon_tml_imports_as_few_long_fragments() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../e2e/fixtures/naharon_sources/2025-12-09_23h05_naharon_resurvey.tml");
        assert!(
            path.exists(),
            "committed fixture is missing: {}",
            path.display()
        );

        let tml = parse_tml(&path).expect("the committed fixture parses");
        let (configs, _, _) =
            convert_tml_to_import_configs(&tml, false, false).expect("and converts");

        let mut sizes: Vec<usize> = configs.iter().map(|c| c.shots.len()).collect();
        sizes.sort_unstable();
        let median = sizes.get(sizes.len() / 2).copied().unwrap_or(0);
        let short = sizes.iter().filter(|&&n| n <= 2).count();

        assert!(
            configs.len() <= 17,
            "the file's 14 declared names should not explode into fragments; got {} configs (sizes {sizes:?})",
            configs.len()
        );
        assert!(
            median >= 14,
            "the median fragment should be a usable run, not a stub; got {median} (sizes {sizes:?})"
        );
        assert!(
            short <= 2,
            "a fragment of <=2 shots is the #412 complaint itself, and this file \
             should not grow more of them; got {short} (sizes {sizes:?})"
        );

        // Whatever the counts, every emitted run must still be a polyline.
        assert_eq!(linearize::first_linearity_violation(&tml.sections), None);
    }

    /// Baseline harness for #412: how many linear sections the importer
    /// produces from a real file, and how small the fragments are.
    ///
    /// Run with:
    ///   TML_MEASURE_PATH=/path/to/file.tml cargo test measure_tml_fragmentation -- --ignored --nocapture
    #[test]
    #[ignore = "measurement harness for #412, not a check; set TML_MEASURE_PATH"]
    fn measure_tml_fragmentation() {
        let path = require_env_file("TML_MEASURE_PATH");
        let tml = parse_tml(&path).expect("parse");
        let file_names: std::collections::BTreeSet<String> = tml
            .sections
            .iter()
            .map(|s| {
                let n = s.name.as_str();
                match n.rfind(" (branch ") {
                    Some(i) if n.ends_with(')') => n[..i].to_string(),
                    _ => n.to_string(),
                }
            })
            .collect();
        let origins = classify_chain_origins(&tml);
        eprintln!(
            "ORIGIN sections={} entrance_root={} cross_section_root={} \
             in_section_parent_is_chain_end={} in_section_parent_mid_chain={} \
             cross_root_at_parent_chain_end={} sibling_v_merge_savings={} \
             cross_root_at_chain_end_metadata_match={} sibling_v_metadata_match={}",
            tml.sections.len(),
            origins.entrance_root,
            origins.cross_section_root,
            origins.in_section_parent_is_chain_end,
            origins.in_section_parent_mid_chain,
            origins.cross_root_at_parent_chain_end,
            origins.sibling_v_merge_savings,
            origins.cross_root_at_chain_end_metadata_match,
            origins.sibling_v_metadata_match,
        );

        let (configs, closures, warnings) =
            convert_tml_to_import_configs(&tml, false, false).expect("convert");
        let mut sizes: Vec<usize> = configs.iter().map(|c| c.shots.len()).collect();
        sizes.sort_unstable();
        let tiny = sizes.iter().filter(|&&n| n <= 2).count();
        eprintln!(
            "FRAG {}: file_section_names={} configs={} closures={} warnings={} shots_min={} shots_median={} shots_max={} fragments_le2={}",
            path.display(),
            file_names.len(),
            configs.len(),
            closures.len(),
            warnings.len(),
            sizes.first().copied().unwrap_or(0),
            sizes.get(sizes.len() / 2).copied().unwrap_or(0),
            sizes.last().copied().unwrap_or(0),
            tiny,
        );
    }

    /// The section order as it was computed before #576 B27(d): the cycle tail
    /// found by `order.contains` for every index. Kept as the oracle for
    /// [`dependency_order`].
    fn dependency_order_by_contains(dep: &[Option<usize>]) -> Vec<usize> {
        let n = dep.len();
        let mut in_degree = vec![0u32; n];
        let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (i, d) in dep.iter().enumerate() {
            if let Some(j) = d {
                in_degree[i] += 1;
                dependents[*j].push(i);
            }
        }
        let mut queue: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
        for (i, &deg) in in_degree.iter().enumerate() {
            if deg == 0 {
                queue.push_back(i);
            }
        }
        let mut order: Vec<usize> = Vec::with_capacity(n);
        while let Some(i) = queue.pop_front() {
            order.push(i);
            for &dep_i in &dependents[i] {
                in_degree[dep_i] -= 1;
                if in_degree[dep_i] == 0 {
                    queue.push_back(dep_i);
                }
            }
        }
        if order.len() < n {
            for i in 0..n {
                if !order.contains(&i) {
                    order.push(i);
                }
            }
        }
        order
    }

    /// Same order as the oracle on random dependency graphs, cycles included
    /// (a crafted file's `FromImporting` targets can form one).
    #[test]
    fn dependency_order_matches_the_contains_scan() {
        let mut state = 0x9e37_79b9_7f4a_7c15_u64;
        let mut below = |bound: u64| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state % bound
        };
        let mut with_cycle = 0;
        for case in 0..500 {
            let n = 1 + below(80) as usize;
            let dep: Vec<Option<usize>> = (0..n)
                .map(|_| (below(3) > 0).then(|| below(n as u64) as usize))
                .collect();
            let fast = dependency_order(&dep);
            let slow = dependency_order_by_contains(&dep);
            assert_eq!(fast, slow, "case {case}: {dep:?}");
            let mut sorted = fast.clone();
            sorted.sort_unstable();
            assert_eq!(sorted, (0..n).collect::<Vec<_>>(), "a permutation");
            with_cycle += usize::from(dep.iter().enumerate().any(|(i, d)| *d == Some(i)));
        }
        assert!(with_cycle > 50, "{with_cycle}");
    }

    /// Timing probe, not a test: `cargo test -p stalagmight-parsers --lib
    /// dependency_order_timing -- --ignored --nocapture`.
    #[test]
    #[ignore = "timing probe; run by hand"]
    fn dependency_order_timing() {
        for n in [10_000, 20_000, 40_000] {
            // One cycle through every section: Kahn's queue never starts, so
            // the whole order is the fallback tail.
            let dep: Vec<Option<usize>> = (0..n).map(|i| Some((i + 1) % n)).collect();
            let start = std::time::Instant::now();
            let fast = dependency_order(&dep);
            let fast_ms = start.elapsed().as_millis();
            let start = std::time::Instant::now();
            let slow = dependency_order_by_contains(&dep);
            let slow_ms = start.elapsed().as_millis();
            assert_eq!(fast, slow);
            eprintln!("{n} sections on one cycle: bitmap {fast_ms} ms, contains scan {slow_ms} ms");
        }
    }

    /// THE rule for which Ariane rows the import builds (#576 E17), as a truth
    /// table: (type, excluded, include-excluded toggle) -> imported. The SAME
    /// table pins the TypeScript mirror the import dialog previews with,
    /// `survivesImport` in src/utils/__tests__/tmlImportStats.test.ts - change
    /// one, change both.
    #[test]
    fn survives_import_truth_table() {
        let table: &[(TmlShotType, bool, bool, bool)] = &[
            (TmlShotType::Real, false, false, true),
            (TmlShotType::Real, false, true, true),
            (TmlShotType::Real, true, false, false),
            (TmlShotType::Real, true, true, true),
            (TmlShotType::Virtual, false, false, true),
            (TmlShotType::Virtual, false, true, true),
            (TmlShotType::Virtual, true, false, false),
            (TmlShotType::Virtual, true, true, true),
            (TmlShotType::Closure, false, false, false),
            (TmlShotType::Closure, false, true, false),
            (TmlShotType::Closure, true, false, false),
            (TmlShotType::Closure, true, true, false),
        ];
        for &(shot_type, excluded, include_excluded, imported) in table {
            let shot = TmlShot {
                shot_type,
                excluded,
                ..sample_tml_shot()
            };
            assert_eq!(
                survives_import(&shot, include_excluded),
                imported,
                "{shot_type:?} excluded={excluded} include_excluded={include_excluded}"
            );
        }
    }
}
