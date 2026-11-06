//! Ariane `.tml` / `.tmlu` generator, split into a survey MODEL and a
//! RENDERER so a test can write one survey several ways.
//!
//! [`model`] builds the rows of Ariane's `SurveyData` table - START anchors,
//! chains of REAL / VIRTUAL shots under section names, branches inside and
//! across sections, CLOSURE rows - with every length in metres. [`render`]
//! writes a model as XML in a chosen [`Units`] (the `<unit>` label, the unit
//! the numbers are written in, and how an inheriting `DepthIn` is spelled),
//! as a zipped `.tml` or a plain `.tmlu`. `tests/tml_roundtrip.rs` renders one
//! model in metres and in feet and requires both to import alike (the property
//! finding HFI-1 broke: a metric file whose `-1` "inherit" marker had been
//! scaled to `-0.3048` imported every start depth as a literal -0.3 m), and
//! imports, exports and re-imports a model.
//!
//! Model knobs: section names (repeated, empty, carrying Ariane's
//! `<SectionDescription>` suffix, colliding with the importer's own
//! `(branch N)` / `(trip N)` suffixes), START anchors with and without GPS,
//! sections hanging off a station of another section, off nothing or off a row
//! the file does not have, branches inside a section, VIRTUAL / excluded rows,
//! `<Type>` in other cases or unknown, the trip (explorer in both encodings,
//! dates incl. the `2000-01-01` placeholder and unpadded ones, colours)
//! changing mid-chain, `DepthIn` inheriting at different rates or spelled
//! out (also as a value that reads like a marker), azimuths outside 0..360,
//! inclinations, LRUD, comments with the export's U+200B marker, Ariane 26
//! `UUID`s (absent, repeated), CLOSURE rows aimed at stations and at START
//! rows, entrance elevation, the magnetic-azimuth flag, file order, id bases
//! and strides, and huge row and section counts ([`count`]).
//!
//! Rendering knobs: the unit label (`m`, `ft`, their case variants, labels the
//! reader does not know), the `DepthIn` marker as `-1` or a unit-scaled
//! spelling ([`sentinel`]), number precision, long or short element names
//! (or a mix), field order, the `Id` legacy spelling, the elements Ariane
//! writes that the reader skips (`Name`, `Locked`, `Profiletype`, `Shape`
//! subtrees), unknown and deeply nested elements, XML comments, character
//! references, the declaration and encoding (UTF-8 with and without a BOM,
//! Windows-1252), line endings, and for `.tml` the archive itself (entry name
//! case, stored or deflated, extra entries).
//!
//! Hostile files ([`Faults`], about one in four): ids that are missing, not
//! integers or overflow; duplicate, self-referencing, cyclic and dangling
//! ids; numbers in odd spellings and at the edges of `f64`; unescaped text,
//! CDATA, undefined entities, a DOCTYPE; repeated and unclosed elements; UTF-16;
//! byte damage ([`damage`]); and archive faults (no `Data.xml`, a second case
//! variant of it, an empty archive, an XML file posing as a `.tml`, damage to
//! the zip bytes).

use std::fmt::Write as _;
use std::io::Write as _;

use stalagmight_core::core::VirtualShotMode;
use stalagmight_models::project::Project;
use stalagmight_parsers::parsers::tml::{convert_tml_with_options, parse_tml, TmlImportOptions};

use crate::fileset::{with_scratch_dir, FileSet};
use crate::gen::{
    count, damage, line_endings, reading, sentinel, spell, Encoding, Faults, LengthUnit,
};
use crate::oracle::{fresh_project, with_state, Violation};
use crate::source::ByteSource;

/// The most rows one model holds. The big-count knobs still reach thousands of
/// rows; this keeps several of them firing at once from building a document
/// of tens of megabytes.
pub const MAX_ROWS: usize = 6000;

/// Ariane's `DepthIn` value for "this shot starts where the previous one
/// ended", as the format defines it (not taken from the importer).
pub const INHERIT_MARKER: f64 = -1.0;

/// `<unit>` labels the reader does not know (read as metres, with a warning).
#[rustfmt::skip]
const UNKNOWN_UNITS: &[&str] = &["feet", "meters", "yd", "Fuss", "m\u{e8}tres", "ft.", "m3"];
#[rustfmt::skip]
const PEOPLE: &[&str] = &["nh", "ds", "Diver A", "J. Doe", "Ana-Maria", "\u{c9}lodie", "O'Neil"];
#[rustfmt::skip]
const BAD_DATES: &[&str] = &[
    "2024-13-40", "not a date", "2024-03-05T10:00:00", "", "0000-00-00", "99999-01-01", "1899-12-31",
];
#[rustfmt::skip]
const BAD_INTS: &[&str] = &["", "abc", "1.5", "99999999999999999999", " 7 ", "+3", "0x1F", "-"];
/// Stray content between the rows of a hostile file.
#[rustfmt::skip]
const STRAY: &[&str] = &[
    "junk text", "<SurveyData>", "</Data>", "<!-- a comment -->", "<?pi data?>", "&amp;",
    "<SRVD><ID>1</ID></SRVD>", "<SRVD/>", "<![CDATA[<SRVD>]]>",
];

/// One of `items`, owned (the empty string for an empty list).
fn one(src: &mut ByteSource<'_>, items: &[&str]) -> String {
    src.pick_or(items, "").to_string()
}

/// What a row is, as its `<Type>` says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowType {
    /// An entrance: no measurement, the root a chain hangs off.
    Start,
    Real,
    Virtual,
    /// A loop-closure marker, aimed at another row by `ClosureToID`.
    Closure,
}

impl RowType {
    /// The spelling Ariane writes.
    pub fn canonical(self) -> &'static str {
        match self {
            Self::Start => "START",
            Self::Real => "REAL",
            Self::Virtual => "VIRTUAL",
            Self::Closure => "CLOSURE",
        }
    }
}

/// A row's `DepthIn`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DepthIn {
    /// "Same as previous": written as the [`Units::marker`] of the rendering.
    Inherit,
    /// A start depth in metres, scaled like every other length.
    Value(f64),
}

/// One `SurveyData` row. Lengths and depths in metres.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub id: i64,
    pub from_id: i64,
    /// `ClosureToID`: the target of a CLOSURE row, `-1` on every other row.
    pub closure_to: i64,
    pub row_type: RowType,
    /// `<Type>` exactly as written: usually [`RowType::canonical`], sometimes
    /// another case or a type the reader does not know (read as REAL).
    pub type_text: String,
    /// `<Section>` as text (the renderer escapes it).
    pub section: String,
    pub azimuth: f64,
    pub length: f64,
    /// Station depth at the end of the row.
    pub depth: f64,
    pub depth_in: DepthIn,
    pub inclination: f64,
    /// Left, right, up, down.
    pub lrud: [f64; 4],
    pub comment: Option<String>,
    pub explorer: Option<String>,
    pub date: Option<String>,
    pub color: Option<String>,
    pub excluded: bool,
    pub latitude: f64,
    pub longitude: f64,
    pub uuid: Option<String>,
}

/// One Ariane survey, independent of how it is written down.
#[derive(Debug, Clone, PartialEq)]
pub struct TmlModel {
    pub cave_name: String,
    /// `<useMagneticAzimuth>` text; `None` leaves the element out.
    pub use_magnetic_azimuth: Option<String>,
    /// `<firstStartAbsoluteElevation>` in metres; `None` leaves it out.
    pub elevation: Option<f64>,
    /// In file order.
    pub rows: Vec<Row>,
}

/// How a rendering writes lengths.
#[derive(Debug, Clone, PartialEq)]
pub struct Units {
    /// `<unit>` text; `None` leaves the element out (read as metres).
    pub label: Option<String>,
    /// The unit every length and depth is written in.
    pub scale: LengthUnit,
    /// How an inheriting `DepthIn` is written, verbatim (NOT scaled): `-1` is
    /// the format's marker in either unit; a metric file that went through a
    /// careless feet-to-metre conversion carries `-0.3048` or a rounding of
    /// it instead (the HFI-1 class).
    pub marker: f64,
}

impl Units {
    /// A metric file as Ariane writes one.
    pub fn metres() -> Self {
        Self::metres_with_marker(INHERIT_MARKER)
    }

    /// A metric file whose inheriting `DepthIn` is spelled `marker`.
    pub fn metres_with_marker(marker: f64) -> Self {
        Self {
            label: Some("m".to_string()),
            scale: LengthUnit::Meters,
            marker,
        }
    }

    /// A feet file as Ariane writes one.
    pub fn feet() -> Self {
        Self {
            label: Some("ft".to_string()),
            scale: LengthUnit::Feet,
            marker: INHERIT_MARKER,
        }
    }

    /// Mostly a recognised label with numbers in its unit and the `-1` marker;
    /// sometimes a label the reader does not know (read as metres, warned
    /// about) and a marker in one of its unit-scaled spellings.
    pub fn pick(src: &mut ByteSource<'_>, f: Faults) -> Self {
        let scale = LengthUnit::pick(src);
        let label = match scale {
            LengthUnit::Meters => match src.below(8) {
                0 => None,
                1 => Some(""),
                2 => Some("metric"),
                3 => Some("M"),
                4 => Some(" Metric "),
                _ => Some("m"),
            },
            LengthUnit::Feet => match src.below(6) {
                0 => Some("imperial"),
                1 => Some("FT"),
                2 => Some(" ft"),
                _ => Some("ft"),
            },
        };
        let mut label = label.map(str::to_string);
        if src.chance(12) {
            label = Some(one(src, UNKNOWN_UNITS));
        }
        if f.hit(src, 16) {
            label = Some(src.text(12));
        }
        let marker = if src.chance(40) {
            sentinel(src)
        } else {
            INHERIT_MARKER
        };
        Self {
            label,
            scale,
            marker,
        }
    }
}

/// Render one Ariane import: a zipped `.tml` when `zipped`, else a `.tmlu`.
pub fn generate(src: &mut ByteSource<'_>, zipped: bool) -> FileSet {
    let f = Faults::pick(src);
    let survey = model(src, f);
    let units = Units::pick(src, f);
    render(&survey, &units, zipped, f, src)
}

// ============================================================================
// The model
// ============================================================================

/// Centimetres: what a clean file carries (Ariane writes two decimals).
fn cm(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// A length-like reading in metres: centimetres in `lo..=hi` in a clean file,
/// sometimes any reading at all (sentinels, `f64` edges, non-finite) in a
/// hostile one.
fn metres(src: &mut ByteSource<'_>, f: Faults, lo: f64, hi: f64) -> f64 {
    if f.hit(src, 24) {
        reading(src, lo, hi)
    } else {
        cm(src.f64_in(lo, hi))
    }
}

/// Explorer, date and colour: Ariane writes all three on every row.
#[derive(Debug, Clone)]
struct Trip {
    explorer: Option<String>,
    date: Option<String>,
    color: Option<String>,
}

fn person(src: &mut ByteSource<'_>) -> String {
    if src.chance(64) {
        src.ident(10)
    } else {
        one(src, PEOPLE)
    }
}

impl Trip {
    fn pick(src: &mut ByteSource<'_>, f: Faults) -> Self {
        let explorer = match src.below(10) {
            0 => None,
            1 => Some("unknown".to_string()),
            2 => Some("None".to_string()),
            3 => Some(String::new()),
            // Ariane's entity-escaped two-role encoding.
            4 => Some(format!(
                "<Explorer>{}</Explorer><Surveyor>{}</Surveyor>",
                person(src),
                person(src)
            )),
            5 => Some(format!("unknown / {}", person(src))),
            6 => Some(person(src)),
            _ => Some(format!("{} / {}", person(src), person(src))),
        };
        let year = 1990 + src.below(37);
        let month = 1 + src.below(12);
        let day = 1 + src.below(28);
        let date = match src.below(12) {
            0 => None,
            // The exporter's "no date" placeholder, read back as undated.
            1 => Some("2000-01-01".to_string()),
            // A tool that does not zero-pad.
            2 => Some(format!("{year}-{month}-{day}")),
            3 if f.on() => Some(one(src, BAD_DATES)),
            _ => Some(format!("{year:04}-{month:02}-{day:02}")),
        };
        let rgb = src.u32() & 0x00FF_FFFF;
        let color = match src.below(10) {
            0 => None,
            1 => Some(format!("0X{rgb:06X}FF")),
            // Not Ariane's format: read as the fallback colour.
            2 => Some(one(src, &["#336699", "", "0x8f61", "red", "0x12345678ff"])),
            _ => Some(format!("0x{rgb:06x}ff")),
        };
        Self {
            explorer,
            date,
            color,
        }
    }
}

impl Row {
    fn new(id: i64, from_id: i64, row_type: RowType, section: &str, trip: &Trip) -> Self {
        Self {
            id,
            from_id,
            closure_to: -1,
            row_type,
            type_text: row_type.canonical().to_string(),
            section: section.to_string(),
            azimuth: 0.0,
            length: 0.0,
            depth: 0.0,
            depth_in: DepthIn::Inherit,
            inclination: 0.0,
            lrud: [0.0; 4],
            comment: None,
            explorer: trip.explorer.clone(),
            date: trip.date.clone(),
            color: trip.color.clone(),
            excluded: false,
            latitude: 0.0,
            longitude: 0.0,
            uuid: None,
        }
    }
}

/// A row other rows may hang off, and where it ends.
#[derive(Debug, Clone, Copy)]
struct Station {
    id: i64,
    depth: f64,
    /// Index into the builder's rows.
    row: usize,
}

struct Builder {
    rows: Vec<Row>,
    next_id: i64,
    stride: i64,
    stations: Vec<Station>,
    names: Vec<String>,
}

impl Builder {
    fn new(src: &mut ByteSource<'_>, f: Faults) -> Self {
        let next_id = match src.below(8) {
            1 => 1,
            2 => 1000,
            3 => 1 << 40,
            4 if f.on() => i64::MAX - 10_000,
            _ => 0,
        };
        let stride = if src.chance(24) {
            1 + src.below(1000) as i64
        } else {
            1
        };
        Self {
            rows: Vec::new(),
            next_id,
            stride,
            stations: Vec::new(),
            names: Vec::new(),
        }
    }

    fn mint(&mut self) -> i64 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(self.stride);
        id
    }

    fn push_station(&mut self, row: Row) {
        self.stations.push(Station {
            id: row.id,
            depth: row.depth,
            row: self.rows.len(),
        });
        self.rows.push(row);
    }
}

fn section_name(src: &mut ByteSource<'_>, used: &[String]) -> String {
    const BASES: &[&str] = &[
        "Main",
        "Mainline",
        "Upstream",
        "Downstream",
        "Side passage",
        "Sump 1",
        "Galerie des \u{c9}toiles",
        "T",
    ];
    let base = if src.chance(16) {
        src.text(20)
    } else {
        let mut name = src.pick_or(BASES, "Main").to_string();
        if src.bool() {
            let _ = write!(name, " {}", src.below(40));
        }
        name
    };
    match src.below(16) {
        // A name declared twice: the importer groups both chains as one.
        0 | 1 if !used.is_empty() => src.pick(used).cloned().unwrap_or(base),
        // "Unnamed".
        2 => String::new(),
        // Ariane's description suffix, stripped on read.
        3 => format!(
            "{base}<SectionDescription>{}</SectionDescription>",
            src.text(30)
        ),
        // Collides with a suffix the importer mints itself.
        4 => format!("{base} (branch {})", 1 + src.below(3)),
        5 => format!("{base} (trip {})", 2 + src.below(3)),
        _ => base,
    }
}

fn gps(src: &mut ByteSource<'_>, f: Faults, base: (f64, f64)) -> (f64, f64) {
    if f.hit(src, 32) {
        (src.number(-180.0, 180.0), src.number(-360.0, 360.0))
    } else {
        (
            base.0 + src.f64_in(-0.01, 0.01),
            base.1 + src.f64_in(-0.01, 0.01),
        )
    }
}

fn comment(src: &mut ByteSource<'_>, f: Faults) -> String {
    match src.below(10) {
        0 => "None".to_string(),
        1 => String::new(),
        // A shot line marked the way the exporter marks one.
        2 => format!("\u{200B}{}", src.text(30)),
        3 => format!("{}\n\u{200B}{}", src.text(20), src.text(20)),
        4 => format!("  {}\n{}  ", src.text(20), src.text(20)),
        5 if f.on() => src.text(4000),
        _ => src.text(40),
    }
}

fn type_text(src: &mut ByteSource<'_>, f: Faults, row_type: RowType) -> String {
    let canonical = row_type.canonical();
    match src.below(32) {
        0 => canonical.to_lowercase(),
        1 => {
            let lower = canonical.to_lowercase();
            let mut chars = lower.chars();
            chars.next().map_or_else(String::new, |c| {
                c.to_uppercase().chain(chars).collect::<String>()
            })
        }
        // A type the reader does not know reads as REAL, so only a REAL row
        // carries one in a clean file.
        2 if row_type == RowType::Real || f.on() => src
            .pick_or(&["SPLAY", "SURFACE", "Rope", ""], "SPLAY")
            .to_string(),
        3 if f.on() => src.text(8),
        _ => canonical.to_string(),
    }
}

fn shot_row(
    src: &mut ByteSource<'_>,
    f: Faults,
    (id, from, depth): (i64, i64, f64),
    name: &str,
    trip: &Trip,
    inherit_odds: u8,
) -> Row {
    let row_type = if src.chance(20) {
        RowType::Virtual
    } else {
        RowType::Real
    };
    // Zero only by choice: a dry source must not make every shot zero-length
    // (which fails the whole import, docs/SPEC.md "Zero length shot").
    let length = if src.chance(2) {
        0.0
    } else {
        metres(src, f, 0.1, 25.0)
    };
    // Within reach of the tape mostly; anywhere sometimes (a gauge glitch).
    let end = if src.chance(8) {
        metres(src, f, -5.0, 150.0)
    } else {
        cm(depth + src.f64_in(-1.0, 1.0) * length.min(15.0))
    };
    let depth_in = if src.chance(inherit_odds) {
        DepthIn::Inherit
    } else {
        match src.below(8) {
            // A real start depth that reads like a marker.
            0 => DepthIn::Value(sentinel(src)),
            // Starts somewhere else than the row it hangs off ends.
            1 => DepthIn::Value(metres(src, f, -5.0, 150.0)),
            // Spelled out where it starts.
            _ => DepthIn::Value(depth),
        }
    };
    let inclination = if src.chance(24) {
        (src.f64_in(-90.0, 90.0) * 10.0).round() / 10.0
    } else {
        0.0
    };
    let mut lrud = [0.0; 4];
    if !src.chance(96) {
        for v in &mut lrud {
            *v = if src.chance(48) {
                0.0
            } else {
                metres(src, f, 0.0, 20.0)
            };
        }
    }
    let azimuth = if src.chance(16) {
        src.pick_or(&[-45.0, 360.0, 370.5, -0.0, 720.0, -359.9, 359.95], 0.0)
    } else {
        (src.f64_in(0.0, 360.0) * 10.0).round() / 10.0
    };
    let comment = src.chance(24).then(|| comment(src, f));
    let (latitude, longitude) = if src.chance(4) {
        (src.f64_in(-60.0, 60.0), src.f64_in(-170.0, 170.0))
    } else {
        (0.0, 0.0)
    };
    Row {
        type_text: type_text(src, f, row_type),
        azimuth,
        length,
        depth: end,
        depth_in,
        inclination,
        lrud,
        comment,
        excluded: src.chance(10),
        latitude,
        longitude,
        ..Row::new(id, from, row_type, name, trip)
    }
}

fn section(src: &mut ByteSource<'_>, f: Faults, b: &mut Builder, first: bool, base: (f64, f64)) {
    let name = section_name(src, &b.names);
    b.names.push(name.clone());
    let mut trip = Trip::pick(src, f);
    // Ariane writes the marker on nearly every shot; some files spell most
    // start depths out instead.
    let inherit_odds = match src.below(8) {
        0 => 48,
        1 => 150,
        _ => 240,
    };
    let (mut from, mut depth) = if first || b.stations.is_empty() || src.chance(20) {
        // A new entrance.
        let id = b.mint();
        let depth = metres(src, f, 0.0, 6.0);
        let (latitude, longitude) = if src.chance(64) {
            (0.0, 0.0)
        } else {
            gps(src, f, base)
        };
        b.push_station(Row {
            depth,
            depth_in: DepthIn::Value(depth),
            latitude,
            longitude,
            ..Row::new(id, -1, RowType::Start, &name, &trip)
        });
        (id, depth)
    } else if src.chance(10) {
        // An orphan: hangs off nothing, or off a row the file does not have.
        let from = if src.bool() {
            -1
        } else {
            b.next_id.saturating_add(1_000_000)
        };
        (from, metres(src, f, 0.0, 40.0))
    } else {
        src.pick(&b.stations).map_or((-1, 0.0), |s| (s.id, s.depth))
    };
    let own = b.stations.len();
    let shots = 1 + count(src, 12, 1500);
    for _ in 0..shots {
        if b.rows.len() >= MAX_ROWS {
            break;
        }
        // A branch inside the section: carry on from an earlier station of it.
        if b.stations.len() > own && src.chance(12) {
            let pick = own + src.below(b.stations.len() - own);
            if let Some(s) = b.stations.get(pick) {
                (from, depth) = (s.id, s.depth);
            }
        }
        // Another team or another day: the importer cuts the run here.
        if src.chance(5) {
            trip = Trip::pick(src, f);
        }
        let id = b.mint();
        let row = shot_row(src, f, (id, from, depth), &name, &trip, inherit_odds);
        (from, depth) = (row.id, row.depth);
        b.push_station(row);
    }
}

fn closures(src: &mut ByteSource<'_>, f: Faults, b: &mut Builder) {
    for _ in 0..count(src, 3, 60) {
        if b.rows.len() >= MAX_ROWS || b.stations.len() < 2 {
            break;
        }
        let Some(from) = src.pick(&b.stations).copied() else {
            break;
        };
        let target = match src.below(16) {
            // A START row, which no section holds.
            0 => b
                .rows
                .iter()
                .find(|r| r.row_type == RowType::Start)
                .map_or(-1, |r| r.id),
            1 if f.on() => src.pick_or(&[-1, i64::MAX, from.id, b.next_id], -1),
            _ => src.pick(&b.stations).map_or(-1, |s| s.id),
        };
        let target_depth = b
            .stations
            .iter()
            .find(|s| s.id == target)
            .map_or(from.depth, |s| s.depth);
        let Some(origin) = b.rows.get(from.row) else {
            break;
        };
        let trip = Trip {
            explorer: origin.explorer.clone(),
            date: origin.date.clone(),
            color: origin.color.clone(),
        };
        let section = origin.section.clone();
        let id = b.mint();
        b.rows.push(Row {
            closure_to: target,
            length: cm((target_depth - from.depth).abs()),
            depth: target_depth,
            ..Row::new(id, from.id, RowType::Closure, &section, &trip)
        });
    }
}

/// A v4-shaped id (Ariane 26+ writes one per row).
fn uuid_v4(src: &mut ByteSource<'_>) -> String {
    format!(
        "{:08x}-{:04x}-4{:03x}-{:x}{:03x}-{:012x}",
        src.u32(),
        src.u16(),
        src.u16() & 0x0FFF,
        8 + src.below(4),
        src.u16() & 0x0FFF,
        src.u64() & 0xFFFF_FFFF_FFFF
    )
}

fn uuids(src: &mut ByteSource<'_>, f: Faults, rows: &mut [Row]) {
    // Ariane before 26 writes none; 26 writes one per row.
    let style = src.below(4);
    if style == 0 {
        return;
    }
    let mut previous: Option<String> = None;
    for row in &mut *rows {
        if style == 2 && src.bool() {
            continue;
        }
        let uuid = if previous.is_some() && src.chance(4) {
            // A repeat: the same file imported twice, then exported.
            previous.clone()
        } else if f.hit(src, 8) {
            Some(match src.below(4) {
                0 => String::new(),
                1 => "a".repeat(129),
                2 => "a\u{1}b".to_string(),
                _ => src.text(40),
            })
        } else {
            Some(uuid_v4(src))
        };
        previous.clone_from(&uuid);
        row.uuid = uuid;
    }
}

fn shuffle<T>(src: &mut ByteSource<'_>, items: &mut [T]) {
    for i in (1..items.len()).rev() {
        items.swap(i, src.below(i + 1));
    }
}

/// Ids the file's topology cannot trust: duplicates, self references, cycles,
/// dangling references and ids at the edges of `i64`.
fn topology_faults(src: &mut ByteSource<'_>, rows: &mut [Row]) {
    let n = rows.len();
    if n == 0 {
        return;
    }
    for _ in 0..count(src, 3, 64) {
        let other = rows.get(src.below(n)).map_or(0, |r| r.id);
        let Some(row) = rows.get_mut(src.below(n)) else {
            continue;
        };
        match src.below(7) {
            0 => row.id = other,
            1 => row.from_id = row.id,
            // Possibly a later row: a cycle.
            2 => row.from_id = other,
            3 => row.from_id = src.pick_or(&[i64::MAX, i64::MIN, -2, 1 << 40], -2),
            4 => row.closure_to = row.id,
            5 => row.id = src.pick_or(&[i64::MAX, i64::MIN, -1, 0], 0),
            _ => row.row_type = RowType::Start,
        }
    }
}

/// Build one survey. `f` decides whether it may carry hostile topology and
/// values (see the module docs); a clean model is a survey Ariane could have
/// written, apart from its semantic choices.
pub fn model(src: &mut ByteSource<'_>, f: Faults) -> TmlModel {
    let cave_name = if src.chance(32) {
        src.text(24)
    } else {
        format!("Cave {}", src.ident(8))
    };
    let use_magnetic_azimuth = match src.below(8) {
        0 => None,
        1 => Some("TRUE".to_string()),
        2 if f.on() => Some(src.text(8)),
        3 | 4 => Some("true".to_string()),
        _ => Some("false".to_string()),
    };
    let elevation = match src.below(6) {
        0 => None,
        // Ariane's "not recorded".
        1 => Some(0.0),
        _ => Some(metres(src, f, -400.0, 4000.0)),
    };
    let base = (src.f64_in(-60.0, 60.0), src.f64_in(-170.0, 170.0));
    let mut b = Builder::new(src, f);
    let sections = 1 + count(src, 5, 400);
    for k in 0..sections {
        if b.rows.len() >= MAX_ROWS {
            break;
        }
        section(src, f, &mut b, k == 0, base);
    }
    closures(src, f, &mut b);
    let mut rows = b.rows;
    uuids(src, f, &mut rows);
    // File order is not survey order.
    match src.below(16) {
        14 => rows.reverse(),
        15 => shuffle(src, &mut rows),
        _ => {}
    }
    if f.on() {
        topology_faults(src, &mut rows);
    }
    TmlModel {
        cave_name,
        use_magnetic_azimuth,
        elevation,
        rows,
    }
}

// ============================================================================
// The renderer
// ============================================================================

/// One element as `.tml` (long) and `.tmlu` (short) spell it. Written out here
/// rather than taken from the importer's schema table, so a misspelling there
/// is not mirrored here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct El(&'static str, &'static str);

const SURVEY_DATA: El = El("SurveyData", "SRVD");
const SHAPE: El = El("Shape", "SH");
const AZIMUTH: El = El("Azimut", "AZ");
const CLOSURE_TO: El = El("ClosureToID", "CID");
const COLOR: El = El("Color", "CL");
const COMMENT: El = El("Comment", "CM");
const DATE: El = El("Date", "DT");
const DEPTH: El = El("Depth", "DP");
const DEPTH_IN: El = El("DepthIn", "DPI");
const DOWN: El = El("Down", "D");
const EXCLUDED: El = El("Excluded", "EXC");
const EXPLORER: El = El("Explorer", "EX");
const FROM_ID: El = El("FromID", "FRID");
const ID: El = El("ID", "ID");
const ID_LEGACY: El = El("Id", "Id");
const INCLINATION: El = El("Inclination", "INC");
const LATITUDE: El = El("Latitude", "LT");
const LEFT: El = El("Left", "L");
const LENGTH: El = El("Length", "LG");
const LOCKED: El = El("Locked", "LCK");
const LONGITUDE: El = El("Longitude", "LGT");
const NAME: El = El("Name", "NM");
const PROFILE_TYPE: El = El("Profiletype", "PT");
const RIGHT: El = El("Right", "R");
const SECTION: El = El("Section", "SC");
const TYPE: El = El("Type", "TY");
const UP: El = El("Up", "U");
const UUID: El = El("UUID", "UU");

/// How numbers are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Numbers {
    /// Ariane writes two decimals.
    Fixed(usize),
    /// Rust's shortest round-trip form.
    Shortest,
    /// Six decimals, trailing zeros trimmed (the exporter's form).
    Trimmed,
}

impl Numbers {
    fn pick(src: &mut ByteSource<'_>) -> Self {
        match src.below(16) {
            0..=7 => Self::Fixed(2),
            8 => Self::Fixed(3),
            9 => Self::Fixed(4),
            10 => Self::Fixed(6),
            11 | 12 => Self::Shortest,
            _ => Self::Trimmed,
        }
    }
}

/// Whether XML 1.0 can carry `c` at all.
fn is_xml_char(c: char) -> bool {
    matches!(c,
        '\u{9}' | '\u{A}' | '\u{D}'
        | '\u{20}'..='\u{D7FF}'
        | '\u{E000}'..='\u{FFFD}'
        | '\u{10000}'..='\u{10FFFF}')
}

/// `raw` as element text: markup escaped, characters XML cannot carry
/// replaced, non-ASCII optionally as character references.
fn escape(raw: &str, char_refs: bool) -> String {
    let mut out = String::with_capacity(raw.len() + 8);
    for c in raw.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c if !is_xml_char(c) => out.push('\u{FFFD}'),
            c if char_refs && !c.is_ascii() => {
                let _ = write!(out, "&#x{:X};", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out
}

/// Text Windows-1252 / Latin-1 bytes carry unchanged (no C1 controls, which
/// the two code pages read differently, nothing past U+00FF).
fn latin1_safe(text: &str) -> bool {
    text.chars().all(|c| {
        let u = u32::from(c);
        u < 0x80 || (0xA0..=0xFF).contains(&u)
    })
}

/// One rendering's choices.
struct Cx<'m> {
    units: &'m Units,
    f: Faults,
    long: bool,
    mixed: bool,
    numbers: Numbers,
    pretty: bool,
    shuffle: bool,
    extras: bool,
    char_refs: bool,
    legacy_id: bool,
}

impl Cx<'_> {
    fn name(&self, src: &mut ByteSource<'_>, el: El) -> &'static str {
        let long = if self.mixed { src.bool() } else { self.long };
        if long {
            el.0
        } else {
            el.1
        }
    }

    fn nl(&self, out: &mut String, depth: usize) {
        if self.pretty {
            out.push('\n');
            for _ in 0..depth {
                out.push_str("    ");
            }
        }
    }

    fn num(&self, src: &mut ByteSource<'_>, v: f64) -> String {
        if self.f.hit(src, 12) {
            return spell(src, v);
        }
        match self.numbers {
            Numbers::Fixed(d) => format!("{v:.d$}"),
            Numbers::Shortest => format!("{v}"),
            Numbers::Trimmed => {
                let six = format!("{v:.6}");
                let trimmed = six.trim_end_matches('0');
                if trimmed.ends_with('.') {
                    format!("{trimmed}0")
                } else {
                    trimmed.to_string()
                }
            }
        }
    }

    /// A length or depth, in the rendering's unit.
    fn len(&self, src: &mut ByteSource<'_>, metres: f64) -> String {
        self.num(src, self.units.scale.from_meters(metres))
    }

    fn int(&self, src: &mut ByteSource<'_>, v: i64) -> String {
        if self.f.hit(src, 6) {
            return one(src, BAD_INTS);
        }
        v.to_string()
    }

    fn text(&self, src: &mut ByteSource<'_>, raw: &str) -> String {
        if self.f.hit(src, 24) {
            return match src.below(4) {
                0 => raw.to_string(),
                1 if !raw.contains("]]>") => format!("<![CDATA[{raw}]]>"),
                2 => format!("{}&bogus;", escape(raw, false)),
                _ => format!("&#0;{}", escape(raw, false)),
            };
        }
        escape(raw, self.char_refs)
    }
}

/// Unknown elements the reader must skip, sometimes nested very deep. The
/// names collide with nothing the reader knows.
fn nested_unknown(src: &mut ByteSource<'_>, cx: &Cx<'_>, out: &mut String) {
    let depth = if src.chance(16) {
        200 + src.below(3000)
    } else {
        1 + src.below(4)
    };
    let prefix = if src.chance(32) { "ns:" } else { "" };
    for i in 0..depth {
        let _ = write!(out, "<{prefix}x{} a=\"{i}\">", i % 7);
    }
    let text = src.text(12);
    out.push_str(&cx.text(src, &text));
    for i in (0..depth).rev() {
        if cx.f.hit(src, 2) {
            continue; // an unbalanced close, rarely
        }
        let _ = write!(out, "</{prefix}x{}>", i % 7);
    }
}

/// Ariane's per-row cross-section polygon: a subtree the reader skips whole,
/// holding element names it would otherwise read.
fn shape(src: &mut ByteSource<'_>, cx: &Cx<'_>, out: &mut String) {
    let name = cx.name(src, SHAPE);
    let _ = write!(out, "<{name}>");
    for _ in 0..count(src, 3, 40) {
        let v = src.f64_in(-5.0, 5.0);
        let x = cx.num(src, v);
        let _ = write!(out, "<PT><X>{x}</X><AZ>999</AZ><LG>-1</LG></PT>");
    }
    if !cx.f.hit(src, 2) {
        let _ = write!(out, "</{name}>");
    }
}

fn row_fields(src: &mut ByteSource<'_>, cx: &Cx<'_>, row: &Row) -> Vec<(El, String)> {
    let f = cx.f;
    let mut fields: Vec<(El, String)> = Vec::with_capacity(32);
    fields.push((AZIMUTH, cx.num(src, row.azimuth)));
    fields.push((CLOSURE_TO, cx.int(src, row.closure_to)));
    if let Some(color) = &row.color {
        fields.push((COLOR, cx.text(src, color)));
    }
    match &row.comment {
        Some(comment) => fields.push((COMMENT, cx.text(src, comment))),
        None if src.bool() => fields.push((COMMENT, String::new())),
        None => {}
    }
    if let Some(date) = &row.date {
        fields.push((DATE, cx.text(src, date)));
    }
    fields.push((DEPTH, cx.len(src, row.depth)));
    let depth_in = match row.depth_in {
        DepthIn::Inherit => cx.num(src, cx.units.marker),
        DepthIn::Value(v) => cx.len(src, v),
    };
    fields.push((DEPTH_IN, depth_in));
    fields.push((DOWN, cx.len(src, row.lrud[3])));
    let excluded = if f.hit(src, 8) {
        src.pick_or(&["1", "yes", "", "maybe"], "1")
    } else if row.excluded {
        src.pick_or(&["true", "true", "TRUE", "True"], "true")
    } else {
        "false"
    };
    fields.push((EXCLUDED, excluded.to_string()));
    if let Some(explorer) = &row.explorer {
        fields.push((EXPLORER, cx.text(src, explorer)));
    }
    fields.push((FROM_ID, cx.int(src, row.from_id)));
    if !f.hit(src, 4) {
        let id = if cx.legacy_id { ID_LEGACY } else { ID };
        fields.push((id, cx.int(src, row.id)));
    }
    fields.push((INCLINATION, cx.num(src, row.inclination)));
    fields.push((LATITUDE, cx.num(src, row.latitude)));
    fields.push((LEFT, cx.len(src, row.lrud[0])));
    fields.push((LENGTH, cx.len(src, row.length)));
    if cx.extras {
        let locked = if row.row_type == RowType::Start {
            "true"
        } else {
            "false"
        };
        fields.push((LOCKED, locked.to_string()));
    }
    fields.push((LONGITUDE, cx.num(src, row.longitude)));
    if cx.extras {
        let name = match row.row_type {
            RowType::Start => "START",
            RowType::Closure => "CLOSURE:CLOSURE",
            RowType::Real | RowType::Virtual => "",
        };
        fields.push((NAME, name.to_string()));
        fields.push((PROFILE_TYPE, "VERTICAL".to_string()));
    }
    fields.push((RIGHT, cx.len(src, row.lrud[1])));
    fields.push((SECTION, cx.text(src, &row.section)));
    fields.push((TYPE, cx.text(src, &row.type_text)));
    fields.push((UP, cx.len(src, row.lrud[2])));
    if let Some(uuid) = &row.uuid {
        fields.push((UUID, cx.text(src, uuid)));
    }
    if cx.shuffle {
        shuffle(src, &mut fields);
    }
    // A repeated element: the reader keeps the last one.
    if f.hit(src, 6) {
        if let Some(&(el, _)) = fields.get(src.below(fields.len())) {
            let junk = src.text(8);
            fields.push((el, junk));
        }
    }
    fields
}

fn row_xml(src: &mut ByteSource<'_>, cx: &Cx<'_>, row: &Row, out: &mut String) {
    let fields = row_fields(src, cx, row);
    cx.nl(out, 2);
    let survey_data = cx.name(src, SURVEY_DATA);
    let _ = write!(out, "<{survey_data}>");
    for (el, value) in &fields {
        cx.nl(out, 3);
        let name = cx.name(src, *el);
        if value.is_empty() && src.bool() {
            let _ = write!(out, "<{name}/>");
        } else {
            let _ = write!(out, "<{name}>{value}</{name}>");
        }
        // The exporter's note after a placeholder date: an XML comment.
        if *el == DATE && row.date.as_deref() == Some("2000-01-01") && src.bool() {
            out.push_str("<!--stalagmight:no-survey-date-->");
        }
    }
    if cx.extras && src.chance(64) {
        cx.nl(out, 3);
        shape(src, cx, out);
    }
    if src.chance(6) {
        cx.nl(out, 3);
        nested_unknown(src, cx, out);
    }
    cx.nl(out, 2);
    if !cx.f.hit(src, 2) {
        let _ = write!(out, "</{survey_data}>");
    }
}

fn metadata(src: &mut ByteSource<'_>, cx: &Cx<'_>, survey: &TmlModel, out: &mut String) {
    let mut meta = vec![format!(
        "<caveName>{}</caveName>",
        cx.text(src, &survey.cave_name)
    )];
    if let Some(label) = &cx.units.label {
        meta.push(format!("<unit>{}</unit>", cx.text(src, label)));
    }
    if let Some(flag) = &survey.use_magnetic_azimuth {
        meta.push(format!(
            "<useMagneticAzimuth>{}</useMagneticAzimuth>",
            cx.text(src, flag)
        ));
    }
    if let Some(elevation) = survey.elevation {
        meta.push(format!(
            "<firstStartAbsoluteElevation>{}</firstStartAbsoluteElevation>",
            cx.len(src, elevation)
        ));
    }
    if src.chance(32) {
        meta.push("<Version>26.3</Version><!-- written by fuzz -->".to_string());
    }
    if cx.shuffle {
        shuffle(src, &mut meta);
    }
    for element in meta {
        cx.nl(out, 1);
        out.push_str(&element);
    }
    if src.chance(24) {
        cx.nl(out, 1);
        nested_unknown(src, cx, out);
    }
}

fn declaration(src: &mut ByteSource<'_>, encoding: Encoding, out: &mut String) {
    let label = match encoding {
        Encoding::Cp1252 => {
            if src.bool() {
                "windows-1252"
            } else {
                "ISO-8859-1"
            }
        }
        Encoding::Utf16LeBom | Encoding::Utf16BeBom => "UTF-16",
        Encoding::Utf8 | Encoding::Utf8Bom => "UTF-8",
    };
    match src.below(8) {
        0 => return,
        1 => out.push_str("<?xml version=\"1.0\"?>"),
        _ => {
            let _ = write!(
                out,
                "<?xml version=\"1.0\" encoding=\"{label}\" standalone=\"yes\"?>"
            );
        }
    }
    out.push('\n');
}

fn stray(src: &mut ByteSource<'_>, out: &mut String) {
    out.push_str(src.pick_or(STRAY, ""));
}

/// Pack `xml` the way a `.tml` holds it: a zip archive with `Data.xml`.
fn archive(src: &mut ByteSource<'_>, f: Faults, xml: Vec<u8>) -> Vec<u8> {
    let entry = if f.hit(src, 32) {
        src.pick_or(
            &["Data.xm", "sub/Data.xml", "Data.xml.bak", "Daten.xml"],
            "x",
        )
    } else {
        // Archives written on a case-insensitive filesystem.
        src.pick_or(
            &["Data.xml", "Data.xml", "data.xml", "DATA.XML"],
            "Data.xml",
        )
    };
    let deflate = !src.chance(64);
    let mut entries: Vec<(&str, Vec<u8>)> = Vec::new();
    if src.chance(24) {
        let extra = src.pick_or(&["Thumbs.db", "meta.xml", "__MACOSX/._Data.xml"], "x");
        entries.push((extra, src.bytes(64).to_vec()));
    }
    let raw_xml = f.hit(src, 12).then(|| xml.clone());
    entries.push((entry, xml));
    // A second case variant: the exact spelling wins.
    if f.hit(src, 16) && entry == "Data.xml" {
        entries.push(("data.xml", src.bytes(64).to_vec()));
    }
    if f.hit(src, 8) {
        entries.clear();
    }
    let zipped = zip_bytes(&entries, deflate);
    // An XML file posing as a `.tml`; a writer failure keeps the XML too.
    let bytes = match (raw_xml, zipped) {
        (Some(raw), _) => raw,
        (None, Some(zipped)) => zipped,
        (None, None) => entries.pop().map_or_else(Vec::new, |(_, xml)| xml),
    };
    if f.on() {
        damage(src, bytes)
    } else {
        bytes
    }
}

fn zip_bytes(entries: &[(&str, Vec<u8>)], deflate: bool) -> Option<Vec<u8>> {
    let method = if deflate {
        zip::CompressionMethod::Deflated
    } else {
        zip::CompressionMethod::Stored
    };
    // A fixed timestamp: the same decisions must render the same bytes.
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(method)
        .last_modified_time(zip::DateTime::default());
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, body) in entries {
        writer.start_file(*name, options).ok()?;
        writer.write_all(body).ok()?;
    }
    writer.finish().ok().map(std::io::Cursor::into_inner)
}

/// Write `survey` in `units` as `survey.tml` (zipped) or `survey.tmlu`.
///
/// With `f` off, every choice `src` makes is one the reader must not care
/// about (spelling, order, encoding, container): text XML cannot carry is
/// replaced, and a Windows-1252 rendering whose text is not all Latin-1 falls
/// back to UTF-8 rather than lose a character. The decisions a value consumes
/// do not depend on the value, so two renderings of one model from the same
/// `src` bytes differ in the numbers alone.
pub fn render(
    survey: &TmlModel,
    units: &Units,
    zipped: bool,
    f: Faults,
    src: &mut ByteSource<'_>,
) -> FileSet {
    // `.tml` writes long names and `.tmlu` short ones; the reader takes either.
    let long = if src.chance(24) { !zipped } else { zipped };
    let cx = Cx {
        units,
        f,
        long,
        mixed: src.chance(8),
        numbers: Numbers::pick(src),
        pretty: !src.chance(48),
        shuffle: src.chance(24),
        extras: src.chance(128),
        char_refs: src.chance(16),
        legacy_id: src.chance(16),
    };
    let encoding = if f.on() {
        Encoding::pick(src)
    } else {
        match src.below(16) {
            0 => Encoding::Utf8Bom,
            1 => Encoding::Cp1252,
            _ => Encoding::Utf8,
        }
    };
    let mut out = String::new();
    declaration(src, encoding, &mut out);
    if f.hit(src, 8) {
        out.push_str("<!DOCTYPE CaveFile [<!ENTITY e \"&#x26;&#38;\">]>\n");
    }
    out.push_str("<CaveFile>");
    metadata(src, &cx, survey, &mut out);
    cx.nl(&mut out, 1);
    out.push_str("<Data>");
    for row in &survey.rows {
        row_xml(src, &cx, row, &mut out);
        if f.hit(src, 2) {
            stray(src, &mut out);
        }
        if src.chance(2) {
            out.push_str("<!-- between rows -->");
        }
    }
    cx.nl(&mut out, 1);
    out.push_str("</Data>");
    cx.nl(&mut out, 0);
    if !f.hit(src, 4) {
        out.push_str("</CaveFile>");
    }
    out.push('\n');
    let text = line_endings(src, &out);
    let encoding = if encoding == Encoding::Cp1252 && !f.on() && !latin1_safe(&text) {
        Encoding::Utf8
    } else {
        encoding
    };
    let mut bytes = encoding.encode(&text);
    if f.on() {
        bytes = damage(src, bytes);
    }
    if zipped {
        FileSet::single("survey.tml", archive(src, f, bytes))
    } else {
        FileSet::single("survey.tmlu", bytes)
    }
}

// ============================================================================
// The app's import, for tests that need the project it builds
// ============================================================================

/// Import `files` into a fresh project the way the app's Ariane import does -
/// parse, convert with `opts`, apply with the file's entrance elevation - and
/// return the project, for tests that compare what an import BUILT (the
/// oracle only checks invariants of it). `Ok(Err(reason))` when the file, the
/// conversion or the apply was refused.
pub fn import_project(
    files: &FileSet,
    opts: &TmlImportOptions,
) -> Result<Result<Project, String>, Violation> {
    with_scratch_dir(|dir| -> Result<Result<Project, String>, Violation> {
        let path = files
            .materialize(dir)
            .map_err(|e| Violation(format!("harness: cannot stage files: {e:#}")))?;
        let file = match parse_tml(&path) {
            Ok(file) => file,
            Err(e) => return Ok(Err(format!("parse: {e:#}"))),
        };
        let (sections, closures, _) = match convert_tml_with_options(&file, opts) {
            Ok(converted) => converted,
            Err(e) => return Ok(Err(format!("convert: {e:#}"))),
        };
        let elevation =
            stalagmight_models::ariane_elevation_or_absent(file.first_start_absolute_elevation);
        with_state(|state| {
            fresh_project(state, "RoundTrip", &dir.join("roundtrip.stalx"))?;
            let result = state.execute_tml_import_sections(
                sections,
                closures,
                None,
                Vec::new(),
                Vec::new(),
                VirtualShotMode::default(),
                elevation,
            );
            if !result.success {
                return Ok(Err(format!("apply: {}", result.error.unwrap_or_default())));
            }
            state
                .get_project_state()
                .map(Ok)
                .ok_or_else(|| Violation("import succeeded but no project is open".to_string()))
        })
    })
    .map_err(|e| Violation(format!("harness: scratch dir: {e:#}")))?
}

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
    use super::{generate, model, render, DepthIn, Units};
    use crate::gen::Faults;
    use crate::source::{seed_bytes, ByteSource};

    #[test]
    fn a_clean_model_renders_the_same_survey_in_both_units_but_the_numbers() {
        let bytes = seed_bytes(3, 2048);
        let survey = model(&mut ByteSource::new(&bytes), Faults::none());
        assert!(survey.rows.len() >= 2, "an entrance and a shot at least");
        let style = seed_bytes(4, 256);
        let m = render(
            &survey,
            &Units::metres(),
            false,
            Faults::none(),
            &mut ByteSource::new(&style),
        );
        let ft = render(
            &survey,
            &Units::feet(),
            false,
            Faults::none(),
            &mut ByteSource::new(&style),
        );
        assert_eq!(m.entry_name(), "survey.tmlu");
        assert_ne!(m, ft);
        let text = String::from_utf8_lossy(&m.files.first().unwrap().1).into_owned();
        assert!(text.contains("CaveFile"), "{text}");
    }

    #[test]
    fn a_dry_source_renders_one_entrance_and_one_shot() {
        let survey = model(&mut ByteSource::new(&[]), Faults::none());
        assert_eq!(survey.rows.len(), 2);
        assert!(matches!(
            survey.rows.get(1).map(|r| r.depth_in),
            Some(DepthIn::Value(_) | DepthIn::Inherit)
        ));
        let zipped = generate(&mut ByteSource::new(&[]), true);
        let bytes = &zipped.files.first().unwrap().1;
        assert_eq!(bytes.get(..2), Some(&b"PK"[..]), "a .tml is a zip archive");
    }
}
