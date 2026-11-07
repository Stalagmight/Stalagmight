//! DMP file parser for mNemo device data
//!
//! Ported from pytool_mnemo_lib Python implementation.
//! Supports DMP versions 2-5.

use anyhow::{anyhow, bail, Context, Result};
use chrono::{NaiveDate, NaiveDateTime};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;
use ts_rs::TS;

use crate::parsers::encoding::{read_utf8, FileFamily};

// ============================================================================
// Constants
// ============================================================================

/// Supported DMP file versions
const SUPPORTED_VERSIONS: &[u8] = &[2, 3, 4, 5];

/// Section start magic bytes (version > 2)
const SECTION_MAGIC: [u8; 3] = [68, 89, 101];

/// Shot start magic bytes (version >= 5)
const SHOT_START_MAGIC: [u8; 3] = [57, 67, 77];

/// Shot end magic bytes (version >= 5)
const SHOT_END_MAGIC: [u8; 3] = [95, 25, 35];

/// Get shot size in bytes for a given version
fn shot_size(version: u8) -> usize {
    match version {
        2 => 16,
        3 => 21,
        4 => 29,
        5 => 35,
        _ => 0,
    }
}

/// End-of-section patterns for splitting sections
fn end_of_section_patterns(version: u8) -> Vec<Vec<u8>> {
    match version {
        2 => vec![vec![3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]],
        5 => {
            // Normal end sequence: shot_start_magic + type(3) + zeros + shot_end_magic
            let mut normal = vec![57, 67, 77, 3];
            normal.extend(vec![0u8; 28]);
            normal.extend(&[95, 25, 35]);

            // Buggy end sequence (sometimes legal)
            let mut buggy = vec![57, 67, 77, 3, 7, 8, 7, 8];
            buggy.extend(vec![0u8; 24]);
            buggy.extend(&[95, 25, 35]);

            vec![normal, buggy]
        }
        3 | 4 => {
            // Versions 3 and 4 use similar pattern to v2 but with different shot sizes
            let size = shot_size(version);
            let mut pattern = vec![3];
            pattern.extend(vec![0u8; size - 1]);
            vec![pattern]
        }
        _ => vec![],
    }
}

// ============================================================================
// IntBuffer - Sequential binary reading
// ============================================================================

/// Buffer for sequential reading of integer data
struct IntBuffer {
    data: Vec<u8>,
    cursor: usize,
}

impl IntBuffer {
    fn new(data: Vec<u8>) -> Self {
        Self { data, cursor: 0 }
    }

    /// Read a single byte and advance cursor
    fn read(&mut self) -> Result<u8> {
        if self.cursor >= self.data.len() {
            bail!("Reading beyond buffer at position {}", self.cursor);
        }
        let value = self.data[self.cursor];
        self.cursor += 1;
        Ok(value)
    }

    /// Read n bytes and advance cursor
    fn read_n(&mut self, n: usize) -> Result<Vec<u8>> {
        if self.cursor + n > self.data.len() {
            bail!(
                "Reading {} bytes beyond buffer at position {}",
                n,
                self.cursor
            );
        }
        let values = self.data[self.cursor..self.cursor + n].to_vec();
        self.cursor += n;
        Ok(values)
    }

    /// Read signed 16-bit integer from two bytes (high byte first, low byte second).
    ///
    /// The DMP file stores each byte as a signed decimal (-128..127) in the CSV.
    /// After parsing into `Vec<u8>`, bytes are unsigned (0..255).  We combine them
    /// into a `u16` then reinterpret as `i16` for correct two's complement sign
    /// extension -- matching Python `readInt16BE` and Dart `getInt16(Endian.big)`.
    fn read_int16_be(&mut self) -> Result<i32> {
        let hi = self.read()?;
        let lo = self.read()?;
        let raw = (u16::from(hi) << 8) | u16::from(lo);
        Ok(i32::from(raw as i16))
    }

    /// Check if we have at least n bytes remaining
    fn has_remaining(&self, n: usize) -> bool {
        self.cursor + n <= self.data.len()
    }

    /// Bytes not read yet
    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.cursor)
    }
}

// ============================================================================
// Data Types
// ============================================================================

/// Parsed DMP file
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct DmpFile {
    /// File format version (2-5)
    pub version: u8,
    /// Sections in the file
    pub sections: Vec<DmpSection>,
    /// What could not be read, one sentence each (#576 B13): a section that
    /// failed to parse, a section that stops at a damaged record, a last section
    /// with no end-of-section marker. Empty for a clean file. The import wizard
    /// shows them before import and hands them to `import_sections`, where they
    /// lead the receipt like the other importers' converter warnings. At most
    /// [`MAX_DMP_WARNINGS`], then one "and N more" line.
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// A section from a DMP file
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct DmpSection {
    /// DMP version for this section
    pub version: u8,
    /// 3-character section identifier
    pub name: String,
    /// Survey direction
    pub direction: DmpDirection,
    /// Survey date and time
    #[ts(type = "string")]
    pub date: NaiveDateTime,
    /// Shots in this section
    pub shots: Vec<DmpShot>,
    /// Deterministic content hash (SHA256 of section name + direction + standard shot measurements).
    /// Excludes date/time so duplicate detection works across device exports.
    pub content_hash: String,
}

/// Survey direction
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "lowercase")]
#[ts(rename_all = "lowercase")]
pub enum DmpDirection {
    In,
    Out,
}

impl TryFrom<u8> for DmpDirection {
    type Error = anyhow::Error;

    fn try_from(value: u8) -> Result<Self> {
        match value {
            0 => Ok(DmpDirection::In),
            1 => Ok(DmpDirection::Out),
            _ => bail!("Invalid direction value: {value}"),
        }
    }
}

/// Shot type from mNemo
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
pub enum DmpShotType {
    /// Cross-section A
    CsA,
    /// Cross-section B
    CsB,
    /// Standard shot
    Standard,
    /// End of survey marker
    EndOfSurvey,
}

impl TryFrom<u8> for DmpShotType {
    type Error = anyhow::Error;

    fn try_from(value: u8) -> Result<Self> {
        match value {
            0 => Ok(DmpShotType::CsA),
            1 => Ok(DmpShotType::CsB),
            2 => Ok(DmpShotType::Standard),
            3 => Ok(DmpShotType::EndOfSurvey),
            _ => bail!("Invalid shot type value: {value}"),
        }
    }
}

/// A shot from a DMP file.
///
/// Every field is what the mNemo wrote, decoded, with one deliberate
/// reinterpretation: `pitch_in` / `pitch_out` are flipped from the device's
/// positive-UP frame to the model's positive-DOWN one by `parse_section`, so a
/// `DmpShot` that has left the parser is already in the same frame as
/// `ShotData` and as the `depth_in` / `depth_out` beside it.  That single
/// normalisation point is what keeps the TypeScript import dialog (which copies
/// the parsed pitch into `ShotData` verbatim) and the overlap merge against
/// existing project shots from disagreeing about the sign.
///
/// [`compute_section_hash`] runs *before* the flip, over the device's own
/// bytes, so re-import deduplication still recognises sections imported before
/// issue #296.  See `docs/inclination.md`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct DmpShot {
    /// Shot type
    pub shot_type: DmpShotType,
    /// Heading when entering (degrees)
    pub heading_in: f64,
    /// Heading when exiting (degrees)
    pub heading_out: f64,
    /// Length in meters
    pub length: f64,
    /// Depth at start (meters)
    pub depth_in: f64,
    /// Depth at end (meters)
    pub depth_out: f64,
    /// Pitch when entering (degrees), model frame: positive is DOWN
    pub pitch_in: f64,
    /// Pitch when exiting (degrees), model frame: positive is DOWN
    pub pitch_out: f64,
    /// Left passage dimension (meters), v4+. A v2/v3 record has no LRUD and
    /// all four read 0.0 there; the import wizard checks
    /// `DmpSection::version` and imports them as not measured (#576
    /// NF-AN4-3), as it does a v2 temperature.
    pub left: f64,
    /// Right passage dimension (meters), v4+
    pub right: f64,
    /// Up passage dimension (meters), v4+
    pub up: f64,
    /// Down passage dimension (meters), v4+
    pub down: f64,
    /// Water temperature (celsius), v3+
    pub temperature: f64,
    /// Time: hour, v3+
    pub hour: u8,
    /// Time: minute, v3+
    pub minute: u8,
    /// Time: second, v3+
    pub second: u8,
    /// Physical marker index
    pub marker_index: u8,
    /// User comment (not parsed from DMP format; defaults to None)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

// ============================================================================
// Content Hashing
// ============================================================================

/// Flip a pitch between the device's positive-UP frame and the model's
/// positive-DOWN one.
///
/// Zero is mapped to itself rather than to `-0.0`, so a flat shot serialises as
/// `0` and compares equal to a hand-entered zero.
fn negate_pitch(pitch: f64) -> f64 {
    if pitch == 0.0 {
        0.0
    } else {
        -pitch
    }
}

/// Compute a deterministic content hash for a DMP section.
///
/// Hashes section name, direction, and each Standard shot's measurements
/// (heading_in/out, length, depth_in/out, pitch_in/out, LRUD, temperature,
/// marker_index).  Date/time is deliberately excluded so the same section
/// exported at different times produces an identical hash.
fn compute_section_hash(name: &str, direction: DmpDirection, shots: &[DmpShot]) -> String {
    let mut hasher = Sha256::new();

    // Section identity
    hasher.update(name.as_bytes());
    hasher.update(&[match direction {
        DmpDirection::In => 0u8,
        DmpDirection::Out => 1u8,
    }]);

    // Only Standard shots contribute to the content hash
    for shot in shots
        .iter()
        .filter(|s| s.shot_type == DmpShotType::Standard)
    {
        hasher.update(shot.heading_in.to_le_bytes());
        hasher.update(shot.heading_out.to_le_bytes());
        hasher.update(shot.length.to_le_bytes());
        hasher.update(shot.depth_in.to_le_bytes());
        hasher.update(shot.depth_out.to_le_bytes());
        hasher.update(shot.pitch_in.to_le_bytes());
        hasher.update(shot.pitch_out.to_le_bytes());
        hasher.update(shot.left.to_le_bytes());
        hasher.update(shot.right.to_le_bytes());
        hasher.update(shot.up.to_le_bytes());
        hasher.update(shot.down.to_le_bytes());
        hasher.update(shot.temperature.to_le_bytes());
        hasher.update(&[shot.marker_index]);
    }

    format!("{:x}", hasher.finalize())
}

// ============================================================================
// Parsing Functions
// ============================================================================

/// DMP data cut at its end-of-section markers.
struct SplitSections<'a> {
    /// Every slice that ends in an end-of-section marker, in file order.
    sections: Vec<Vec<u8>>,
    /// Whatever follows the last marker: the trailer a device writes after its
    /// last section (an ASCII `MN2Over`, or a lone zero byte, in the dumps seen
    /// so far), or a last section whose marker never arrived.
    tail: &'a [u8],
}

/// Split DMP data into sections by end-of-section markers
fn split_into_sections(data: &[u8], version: u8) -> SplitSections<'_> {
    let patterns = end_of_section_patterns(version);
    let pattern_len = patterns.first().map_or(0, Vec::len);

    // No patterns (unsupported version), or data shorter than a pattern: no
    // section can end in this data.
    if pattern_len == 0 || data.len() < pattern_len {
        return SplitSections {
            sections: vec![],
            tail: data,
        };
    }

    let mut sections = Vec::new();
    let mut start_idx = 0;

    for current_idx in 0..=data.len() - pattern_len {
        let window = &data[current_idx..current_idx + pattern_len];

        // Check if window matches any end pattern
        if patterns.iter().any(|p| p.as_slice() == window) {
            let end_idx = current_idx + pattern_len;
            sections.push(data[start_idx..end_idx].to_vec());
            start_idx = end_idx;
        }
    }

    SplitSections {
        sections,
        tail: &data[start_idx..],
    }
}

/// Whether `tail` (the bytes after the last end-of-section marker) begins a
/// section - a version byte, then the section magic for v3+ - rather than being
/// a device trailer. A dump cut short inside its last section looks like this;
/// a clean one never does.
fn starts_a_section(tail: &[u8], version: u8) -> bool {
    let Some((&first, rest)) = tail.split_first() else {
        return false;
    };
    if first != version {
        return false;
    }
    if version <= 2 {
        return true;
    }
    let seen = rest.len().min(SECTION_MAGIC.len());
    rest[..seen] == SECTION_MAGIC[..seen]
}

/// Parse a shot from buffer based on version
fn parse_shot(buffer: &mut IntBuffer, version: u8) -> Result<DmpShot> {
    // Magic values check (version >= 5)
    if version >= 5 {
        let magic = buffer.read_n(3)?;
        if magic != SHOT_START_MAGIC {
            bail!("Invalid shot start magic: {magic:?}, expected {SHOT_START_MAGIC:?}");
        }
    }

    // Shot type
    let shot_type = DmpShotType::try_from(buffer.read()?)?;

    // Core shot data (all versions)
    let heading_in = f64::from(buffer.read_int16_be()?) / 10.0;
    let heading_out = f64::from(buffer.read_int16_be()?) / 10.0;
    let length = (f64::from(buffer.read_int16_be()?) / 100.0).max(0.0);
    let depth_in = (f64::from(buffer.read_int16_be()?) / 100.0).max(0.0);
    let depth_out = (f64::from(buffer.read_int16_be()?) / 100.0).max(0.0);
    let pitch_in = f64::from(buffer.read_int16_be()?) / 10.0;
    let pitch_out = f64::from(buffer.read_int16_be()?) / 10.0;

    // LRUD (version >= 4)
    let (left, right, up, down) = if version >= 4 {
        (
            f64::from(buffer.read_int16_be()?) / 100.0,
            f64::from(buffer.read_int16_be()?) / 100.0,
            f64::from(buffer.read_int16_be()?) / 100.0,
            f64::from(buffer.read_int16_be()?) / 100.0,
        )
    } else {
        (0.0, 0.0, 0.0, 0.0)
    };

    // Temperature and time (version >= 3). They are the 5 bytes a v3 shot
    // (21) has over a v2 one (16), and the reference writer emits them from
    // v3 on (pytool_mnemo_lib `Shot._generate_dmp`, `if version >= 3`). Its
    // reader gates them on `>= 4` and so takes the temperature's high byte as
    // a v3 marker; the writer layout is the one that accounts for every byte.
    let (temperature, hour, minute, second) = if version >= 3 {
        (
            f64::from(buffer.read_int16_be()?) / 10.0,
            buffer.read()?,
            buffer.read()?,
            buffer.read()?,
        )
    } else {
        (0.0, 0, 0, 0)
    };
    // That v3 layout has no device dump behind it, so a v3 time that is not a
    // time of day reads as absent: 0:00:00, what v2 records and what the
    // temporal rerank skips, instead of a time that rolls into another day.
    // The hour and minute bounds are the ones `parse_section` holds the
    // section's own time to.
    let (hour, minute, second) = if version == 3 && (hour >= 24 || minute >= 60 || second >= 60) {
        (0, 0, 0)
    } else {
        (hour, minute, second)
    };

    // Marker index (all versions)
    let marker_index = buffer.read()?;

    // Magic values check end (version >= 5)
    if version >= 5 {
        let magic = buffer.read_n(3)?;
        if magic != SHOT_END_MAGIC {
            bail!("Invalid shot end magic: {magic:?}, expected {SHOT_END_MAGIC:?}");
        }
    }

    Ok(DmpShot {
        shot_type,
        heading_in,
        heading_out,
        length,
        depth_in,
        depth_out,
        pitch_in,
        pitch_out,
        left,
        right,
        up,
        down,
        temperature,
        hour,
        minute,
        second,
        marker_index,
        comment: None,
    })
}

/// Why reading a section's shots stopped before its bytes ran out.
enum SectionStop {
    /// A record failed to parse; the parse error.
    DamagedRecord(String),
    /// Fewer bytes than one whole record were left over at the end.
    PartialRecord,
}

/// A parsed section, and whether reading it stopped before its bytes ran out
/// (then the section holds only the shots read before that point).
struct ParsedSection {
    section: DmpSection,
    stopped_early: Option<SectionStop>,
}

/// Standard shots in `section` - the legs an import creates.
fn standard_shot_count(section: &DmpSection) -> usize {
    section
        .shots
        .iter()
        .filter(|shot| shot.shot_type == DmpShotType::Standard)
        .count()
}

/// Parse a section from raw section data
fn parse_section(data: Vec<u8>) -> Result<ParsedSection> {
    let mut buffer = IntBuffer::new(data);

    // Version byte
    let version = buffer.read()?;
    if !SUPPORTED_VERSIONS.contains(&version) {
        bail!("Unsupported DMP version: {version}");
    }

    // Magic values (version > 2)
    if version > 2 {
        let magic = buffer.read_n(3)?;
        if magic != SECTION_MAGIC {
            bail!("Invalid section magic: {magic:?}, expected {SECTION_MAGIC:?}");
        }
    }

    // Date: year (offset from 2000), month, day, hour, minute
    let year = i32::from(buffer.read()?) + 2000;
    let month = u32::from(buffer.read()?);
    let day = u32::from(buffer.read()?);
    let hour = u32::from(buffer.read()?);
    let minute = u32::from(buffer.read()?);

    // Validate date components
    if !(2000..2100).contains(&year) {
        bail!("Invalid year: {year}");
    }
    if !(1..=12).contains(&month) {
        bail!("Invalid month: {month}");
    }
    if !(1..=31).contains(&day) {
        bail!("Invalid day: {day}");
    }
    if hour >= 24 {
        bail!("Invalid hour: {hour}");
    }
    if minute >= 60 {
        bail!("Invalid minute: {minute}");
    }

    let date = NaiveDate::from_ymd_opt(year, month, day)
        .ok_or_else(|| anyhow!("Invalid date: {year}-{month}-{day}"))?
        .and_hms_opt(hour, minute, 0)
        .ok_or_else(|| anyhow!("Invalid time: {hour}:{minute}"))?;

    // Name: 3 ASCII characters
    let name_bytes = buffer.read_n(3)?;
    let name = String::from_utf8(name_bytes.clone())
        .unwrap_or_else(|_| format!("{}{}{}", name_bytes[0], name_bytes[1], name_bytes[2]));

    // Direction
    let direction = DmpDirection::try_from(buffer.read()?)?;

    // Parse shots. Each one is read from its own `shot_len`-byte record, as
    // the reference reader does, so the cursor always lands on the next record
    // boundary whatever `parse_shot` reads out of the current one.
    //
    // A record that does not parse ends the section there rather than being
    // skipped: a DMP section is a chain of legs, so dropping one would hang every
    // later shot off the wrong station. Where it stopped is returned, so
    // `parse_dmp` can say so (#576 B13).
    let shot_len = shot_size(version);
    let mut shots = Vec::new();
    let mut damaged_record = None;

    while buffer.has_remaining(shot_len) {
        let mut record = IntBuffer::new(buffer.read_n(shot_len)?);
        match parse_shot(&mut record, version) {
            Ok(shot) => shots.push(shot),
            Err(e) => {
                tracing::debug!(
                    reason = %e,
                    purpose = "parse section shots for import",
                    "Stopping shot parsing after terminal or invalid data"
                );
                damaged_record = Some(e.to_string());
                break;
            }
        }
    }
    let stopped_early = match damaged_record {
        Some(reason) => Some(SectionStop::DamagedRecord(reason)),
        None if buffer.remaining() > 0 => Some(SectionStop::PartialRecord),
        None => None,
    };

    if shots.is_empty() {
        bail!("Section has no shots");
    }

    let content_hash = compute_section_hash(&name, direction, &shots);

    // The mNemo records pitch positive-UP; every other frame in the codebase
    // (and the `depth_in` / `depth_out` carried beside it) is positive-DOWN.
    // Normalise here, once, so that every consumer of a `DmpShot` sees the
    // model frame: the import preview table, the `ShotData` the wizard sends,
    // and the overlap merge against existing project shots alike.  This runs *after*
    // `compute_section_hash` deliberately, so the hash keeps being taken over
    // the device's own bytes and re-import deduplication still recognises
    // sections imported before issue #296.
    for shot in &mut shots {
        shot.pitch_in = negate_pitch(shot.pitch_in);
        shot.pitch_out = negate_pitch(shot.pitch_out);
    }

    Ok(ParsedSection {
        section: DmpSection {
            version,
            name,
            direction,
            date,
            shots,
            content_hash,
        },
        stopped_early,
    })
}

/// Most warnings a [`DmpFile`] carries before folding the rest into one line -
/// the import receipt's own cap, so a file that breaks every section cannot
/// flood the preview either.
const MAX_DMP_WARNINGS: usize = 50;

/// A section name as it may appear in a warning: the three bytes are file
/// content, so control characters and bidi overrides are dropped.
fn display_name(section: &DmpSection) -> String {
    crate::models::sanitize_display_text(&section.name, 16)
}

fn shots_phrase(count: usize) -> String {
    if count == 1 {
        "1 shot".to_string()
    } else {
        format!("{count} shots")
    }
}

/// Parse a DMP file
///
/// DMP files are text files with semicolon-separated integers.
pub fn parse_dmp(path: &Path) -> Result<DmpFile> {
    // Verify the file exists
    if !path.exists() {
        bail!("File not found: {}", path.display());
    }

    // Verify extension
    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");

    if extension.to_lowercase() != "dmp" {
        bail!("File is not a DMP file: {}", path.display());
    }

    // Read through the shared capped reader (#576 B27(g)): a regular file of
    // at most the `.dmp` family's 50 MB, checked before anything is read.
    let content = read_utf8(path, FileFamily::Dmp)?;

    // Parse semicolon-separated integers
    let data: Vec<u8> = content
        .trim()
        .split(';')
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.trim()
                .parse::<i32>()
                .map(|v| v as u8)
                .with_context(|| format!("Invalid integer in DMP file: '{s}'"))
        })
        .collect::<Result<Vec<_>>>()?;

    if data.is_empty() {
        bail!("DMP file is empty");
    }

    // Get version from first byte
    let version = data[0];
    if !SUPPORTED_VERSIONS.contains(&version) {
        bail!("Unsupported DMP version: {version}. Supported: {SUPPORTED_VERSIONS:?}");
    }

    tracing::info!(
        path = ?path,
        version = version,
        purpose = "prepare import preview",
        "Parsing DMP file for import preview"
    );

    // Split into sections
    let split = split_into_sections(&data, version);
    // A last section whose end marker never arrived (a download cut short) is
    // read like any other, up to its last whole record, and said so (#576 B13).
    let unterminated = starts_a_section(split.tail, version).then(|| split.tail.to_vec());
    if split.sections.is_empty() && unterminated.is_none() {
        bail!("No sections found in DMP file");
    }

    // Parse each section. Nothing that fails to read is dropped silently: every
    // loss becomes a warning the wizard shows before import and the receipt
    // carries after it (#576 B13). Sections are named by their 1-based position
    // in the file as well as their name, which may be part of what is damaged.
    let mut sections = Vec::new();
    let mut warnings = Vec::new();
    let candidates = split
        .sections
        .into_iter()
        .map(|bytes| (bytes, true))
        .chain(unterminated.map(|bytes| (bytes, false)));
    for (i, (section_bytes, terminated)) in candidates.enumerate() {
        let position = i + 1;
        match parse_section(section_bytes) {
            Ok(ParsedSection {
                section,
                stopped_early,
            }) => {
                tracing::debug!(
                    section_index = i,
                    section_name = %section.name,
                    shot_count = section.shots.len(),
                    purpose = "build import preview",
                    "Parsed section for import preview"
                );
                let name = display_name(&section);
                let read = shots_phrase(standard_shot_count(&section));
                if !terminated {
                    warnings.push(format!(
                        "Section \"{name}\" (#{position} in the file) has no end-of-section \
                         marker, so the download may have been cut short: {read} were read, \
                         the rest of the file was not."
                    ));
                } else if let Some(stop) = stopped_early {
                    let why = match stop {
                        SectionStop::DamagedRecord(e) => format!("a damaged record ({e})"),
                        SectionStop::PartialRecord => "an incomplete record".to_string(),
                    };
                    warnings.push(format!(
                        "Section \"{name}\" (#{position} in the file) stops at {why} after \
                         {read}; the rest of that section was not read."
                    ));
                }
                sections.push(section);
            }
            Err(e) => {
                tracing::warn!(
                    section_index = i,
                    purpose = "build import preview",
                    reason = %e,
                    "Failed to parse section for import preview"
                );
                warnings.push(if terminated {
                    format!(
                        "Section #{position} in the file could not be read and was left out: {e}."
                    )
                } else {
                    format!(
                        "The file ends with a section (#{position} in the file) that has no \
                         end-of-section marker and could not be read: {e}. The download may \
                         have been cut short."
                    )
                });
            }
        }
    }
    if sections.is_empty() {
        bail!(
            "No valid sections found in DMP file. {}",
            warnings.join(" ")
        );
    }
    if warnings.len() > MAX_DMP_WARNINGS {
        let more = warnings.len() - MAX_DMP_WARNINGS;
        warnings.truncate(MAX_DMP_WARNINGS);
        warnings.push(format!(
            "... and {more} more warning{}",
            if more == 1 { "" } else { "s" }
        ));
    }

    tracing::info!(
        section_count = sections.len(),
        total_shots = sections.iter().map(|s| s.shots.len()).sum::<usize>(),
        purpose = "complete import preview",
        "DMP file parsed for import preview"
    );

    Ok(DmpFile {
        version,
        sections,
        warnings,
    })
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::print_stdout,
    clippy::float_cmp,
    clippy::expect_used,
    clippy::panic
)]
mod tests {
    use super::*;
    use crate::models::test_support::require_file;
    use crate::models::ShotKind;
    use chrono::Datelike;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_int_buffer_read() {
        let mut buffer = IntBuffer::new(vec![1, 2, 3, 4, 5]);
        assert_eq!(buffer.read().unwrap(), 1);
        assert_eq!(buffer.read().unwrap(), 2);
        assert_eq!(buffer.read_n(2).unwrap(), vec![3, 4]);
        assert_eq!(buffer.read().unwrap(), 5);
        assert!(buffer.read().is_err());
    }

    #[test]
    fn test_int_buffer_read_int16_be_positive() {
        let mut buffer = IntBuffer::new(vec![0, 100]);
        assert_eq!(buffer.read_int16_be().unwrap(), 100);

        let mut buffer = IntBuffer::new(vec![1, 0]);
        assert_eq!(buffer.read_int16_be().unwrap(), 256);

        let mut buffer = IntBuffer::new(vec![14, 16]); // 14 * 256 + 16 = 3600
        assert_eq!(buffer.read_int16_be().unwrap(), 3600);
    }

    #[test]
    fn test_int_buffer_read_int16_be_negative() {
        // -50: two's complement 0xFFCE = [0xFF, 0xCE] = [255, 206]
        let mut buffer = IntBuffer::new(vec![255, 206]);
        assert_eq!(buffer.read_int16_be().unwrap(), -50);

        // -1: 0xFFFF = [255, 255]
        let mut buffer = IntBuffer::new(vec![255, 255]);
        assert_eq!(buffer.read_int16_be().unwrap(), -1);

        // -17: 0xFFEF = [255, 239]
        // From real v5 fixture: pitch bytes "-1;-17" → u8 [255, 239]
        let mut buffer = IntBuffer::new(vec![255, 239]);
        assert_eq!(buffer.read_int16_be().unwrap(), -17);

        // -256: 0xFF00 = [255, 0]
        let mut buffer = IntBuffer::new(vec![255, 0]);
        assert_eq!(buffer.read_int16_be().unwrap(), -256);

        // -285: 0xFED3 = [254, 211]
        // From real v2 fixture: pitch "-2;-29" → u8 [254, 227]
        // Python: (-2 * 256) + (-29 & 0xFF) = -512 + 227 = -285
        let mut buffer = IntBuffer::new(vec![254, 227]);
        assert_eq!(buffer.read_int16_be().unwrap(), -285);
    }

    #[test]
    fn test_int_buffer_read_int16_be_boundaries() {
        // Zero
        let mut buffer = IntBuffer::new(vec![0, 0]);
        assert_eq!(buffer.read_int16_be().unwrap(), 0);

        // +1
        let mut buffer = IntBuffer::new(vec![0, 1]);
        assert_eq!(buffer.read_int16_be().unwrap(), 1);

        // i16::MAX = 32767 = 0x7FFF
        let mut buffer = IntBuffer::new(vec![127, 255]);
        assert_eq!(buffer.read_int16_be().unwrap(), 32767);

        // i16::MIN = -32768 = 0x8000
        let mut buffer = IntBuffer::new(vec![128, 0]);
        assert_eq!(buffer.read_int16_be().unwrap(), -32768);

        // Boundary between positive and negative: 32767 → -32768
        let mut buffer = IntBuffer::new(vec![127, 255]); // 32767
        assert_eq!(buffer.read_int16_be().unwrap(), 32767);
        let mut buffer = IntBuffer::new(vec![128, 0]); // -32768
        assert_eq!(buffer.read_int16_be().unwrap(), -32768);
    }

    #[test]
    fn test_int_buffer_read_int16_be_sequential() {
        // Read multiple int16s from one buffer: +900, -50, +150
        // 900  = 0x0384 = [3, 132]
        // -50  = 0xFFCE = [255, 206]
        // 150  = 0x0096 = [0, 150]
        let mut buffer = IntBuffer::new(vec![3, 132, 255, 206, 0, 150]);
        assert_eq!(buffer.read_int16_be().unwrap(), 900);
        assert_eq!(buffer.read_int16_be().unwrap(), -50);
        assert_eq!(buffer.read_int16_be().unwrap(), 150);
    }

    /// Cross-validate against Python pytool_mnemo_lib readInt16BE.
    /// Python stores signed ints in the buffer; the formula
    ///   (lsb * 256) + (msb & 0xFF)
    /// naturally produces signed results because lsb can be negative.
    /// Our Rust version stores u8s and sign-extends via `as i16`.
    /// Both must produce identical results for the same on-disk bytes.
    #[test]
    fn test_int16_be_matches_python_reference() {
        /// Simulate Python readInt16BE: file values are signed ints.
        fn python_read_int16_be(signed_hi: i32, signed_lo: i32) -> i32 {
            (signed_hi * 256) + (signed_lo & 0xFF)
        }

        /// Convert signed file value to u8 (same as `parse::<i32>().map(|v| v as u8)`)
        fn to_u8(signed: i32) -> u8 {
            signed as u8
        }

        // Test cases: (signed_hi_from_file, signed_lo_from_file)
        let cases: &[(i32, i32)] = &[
            (0, 100),   // +100
            (0, 0),     // 0
            (3, 132),   // +900 (heading)
            (1, 244),   // +500 (length)
            (0, 50),    // +50 (pitch +5.0°)
            (-1, -17),  // -17 (pitch -1.7°, from real v5 fixture)
            (-1, -18),  // -18 (pitch -1.8°, from real v5 fixture)
            (-1, -50),  // -50 (pitch -5.0°)
            (-1, -27),  // -27 (pitch -2.7°, from real v2 fixture)
            (-1, -1),   // -1  (pitch -0.1°, from real v2 fixture)
            (-1, -73),  // -73 (pitch -7.3°, from real v2 fixture)
            (-2, -29),  // -285 (pitch -28.5°, steep descent in v2)
            (-2, 64),   // -448 (pitch -44.8°, from real v2 fixture)
            (127, 255), // +32767 (i16::MAX)
            (-128, 0),  // -32768 (i16::MIN)
        ];

        for &(hi, lo) in cases {
            let python_result = python_read_int16_be(hi, lo);
            let mut buffer = IntBuffer::new(vec![to_u8(hi), to_u8(lo)]);
            let rust_result = buffer.read_int16_be().unwrap();
            assert_eq!(
                rust_result, python_result,
                "Mismatch for file bytes ({hi}, {lo}): Rust={rust_result}, Python={python_result}"
            );
        }
    }

    #[test]
    fn test_shot_type_conversion() {
        assert_eq!(DmpShotType::try_from(0).unwrap(), DmpShotType::CsA);
        assert_eq!(DmpShotType::try_from(1).unwrap(), DmpShotType::CsB);
        assert_eq!(DmpShotType::try_from(2).unwrap(), DmpShotType::Standard);
        assert_eq!(DmpShotType::try_from(3).unwrap(), DmpShotType::EndOfSurvey);
        assert!(DmpShotType::try_from(4).is_err());
    }

    #[test]
    fn test_direction_conversion() {
        assert_eq!(DmpDirection::try_from(0).unwrap(), DmpDirection::In);
        assert_eq!(DmpDirection::try_from(1).unwrap(), DmpDirection::Out);
        assert!(DmpDirection::try_from(2).is_err());
    }

    #[test]
    fn test_split_into_sections_v2() {
        // Create minimal v2 section data with end-of-section marker
        let mut data = vec![2]; // version
        data.extend(vec![0u8; 20]); // padding/header
        data.extend(vec![3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]); // end marker

        let sections = split_into_sections(&data, 2).sections;
        assert_eq!(sections.len(), 1);
    }

    #[test]
    fn test_non_existent_file() {
        let result = parse_dmp(Path::new("/nonexistent/file.dmp"));
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_dmp_file() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(b"").unwrap();

        let path = file.path().with_extension("dmp");
        std::fs::copy(file.path(), &path).unwrap();

        let result = parse_dmp(&path);
        assert!(result.is_err());

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_shot_size() {
        assert_eq!(shot_size(2), 16);
        assert_eq!(shot_size(3), 21);
        assert_eq!(shot_size(4), 29);
        assert_eq!(shot_size(5), 35);
        assert_eq!(shot_size(1), 0);
    }

    #[test]
    fn test_dmp_shot_comment_serialization() {
        // Test that None comment is omitted from JSON (not serialized as null)
        let shot_no_comment = DmpShot {
            shot_type: DmpShotType::Standard,
            heading_in: 45.0,
            heading_out: 46.0,
            length: 10.0,
            depth_in: 2.0,
            depth_out: 3.0,
            pitch_in: 0.0,
            pitch_out: 0.0,
            left: 1.0,
            right: 1.0,
            up: 1.0,
            down: 1.0,
            temperature: 18.0,
            hour: 10,
            minute: 30,
            second: 0,
            marker_index: 0,
            comment: None,
        };

        let json = serde_json::to_string(&shot_no_comment).unwrap();
        // With skip_serializing_if, None should be omitted entirely
        assert!(
            !json.contains("comment"),
            "None comment should be omitted from JSON"
        );

        // Test that Some comment is serialized
        let shot_with_comment = DmpShot {
            comment: Some("Test comment".to_string()),
            ..shot_no_comment
        };

        let json = serde_json::to_string(&shot_with_comment).unwrap();
        assert!(
            json.contains(r#""comment":"Test comment""#),
            "Some comment should be serialized"
        );
    }

    // Integration tests with real DMP files
    #[test]
    fn test_parse_v2_dmp() {
        let test_file =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../e2e/fixtures/test_v2.dmp");
        assert!(
            test_file.exists(),
            "Fixture missing: {}",
            test_file.display()
        );

        let result = parse_dmp(&test_file);
        assert!(result.is_ok(), "Failed to parse v2 DMP: {:?}", result.err());

        let dmp = result.unwrap();
        assert_eq!(dmp.version, 2);
        assert!(!dmp.sections.is_empty(), "No sections found");

        // Check first section has shots
        let first_section = &dmp.sections[0];
        assert!(
            !first_section.shots.is_empty(),
            "First section has no shots"
        );

        // Print summary for debugging
        println!(
            "v2 DMP: {} sections, {} total shots",
            dmp.sections.len(),
            dmp.sections.iter().map(|s| s.shots.len()).sum::<usize>()
        );
        for (i, section) in dmp.sections.iter().enumerate() {
            let standard_shots = section
                .shots
                .iter()
                .filter(|s| s.shot_type == DmpShotType::Standard)
                .count();
            println!(
                "  Section {}: name='{}', dir={:?}, shots={} (standard={})",
                i,
                section.name,
                section.direction,
                section.shots.len(),
                standard_shots
            );
        }
    }

    #[test]
    fn test_parse_v5_dmp() {
        let test_file =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../e2e/fixtures/test_v5.dmp");
        assert!(
            test_file.exists(),
            "Fixture missing: {}",
            test_file.display()
        );

        let result = parse_dmp(&test_file);
        assert!(result.is_ok(), "Failed to parse v5 DMP: {:?}", result.err());

        let dmp = result.unwrap();
        assert_eq!(dmp.version, 5);
        assert!(!dmp.sections.is_empty(), "No sections found");

        // Check first section has shots
        let first_section = &dmp.sections[0];
        assert!(
            !first_section.shots.is_empty(),
            "First section has no shots"
        );

        // Print summary for debugging
        println!(
            "v5 DMP: {} sections, {} total shots",
            dmp.sections.len(),
            dmp.sections.iter().map(|s| s.shots.len()).sum::<usize>()
        );
        for (i, section) in dmp.sections.iter().enumerate() {
            let standard_shots = section
                .shots
                .iter()
                .filter(|s| s.shot_type == DmpShotType::Standard)
                .count();
            println!(
                "  Section {}: name='{}', dir={:?}, shots={} (standard={})",
                i,
                section.name,
                section.direction,
                section.shots.len(),
                standard_shots
            );
        }
    }

    /// Ported from the WDIO `dmp-integration` spec (#120): the v5 parser must be
    /// resilient to malformed end-of-section markers (the `_buggy_EOS` fixture)
    /// and still recover sections instead of bailing out.
    #[test]
    fn test_parse_v5_dmp_buggy_eos_is_resilient() {
        let test_file = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../e2e/fixtures/test_v5_buggy_EOS.dmp");
        assert!(
            test_file.exists(),
            "Fixture missing: {}",
            test_file.display()
        );

        let dmp =
            parse_dmp(&test_file).expect("v5 DMP with malformed EOS markers should still parse");
        assert!(
            !dmp.sections.is_empty(),
            "buggy-EOS DMP should still yield sections"
        );
    }

    /// Ported from the WDIO `dmp-integration` spec (#120): a corrupted DMP must
    /// fail loudly (Err), never silently "succeed" with empty/garbage data.
    #[test]
    fn test_parse_corrupted_dmp_errors() {
        let test_file =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../e2e/fixtures/test_corrupted.dmp");
        assert!(
            test_file.exists(),
            "Fixture missing: {}",
            test_file.display()
        );

        assert!(
            parse_dmp(&test_file).is_err(),
            "corrupted DMP must return Err, not parse successfully"
        );
    }

    // ============================================================================
    // Edge Case Tests (fn-37-rxr.1)
    // ============================================================================

    /// Helper to create a DMP file with given content
    fn create_dmp_file(content: &str) -> (NamedTempFile, std::path::PathBuf) {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(content.as_bytes()).unwrap();
        let path = file.path().with_extension("dmp");
        std::fs::copy(file.path(), &path).unwrap();
        (file, path)
    }

    /// Helper to build v3 format section data as semicolon-separated string
    /// V3 format: version(1) + magic(3) + date(5) + name(3) + direction(1) + shots(21 bytes each) + EOS marker
    /// A v3 shot, as the reference writer (pytool_mnemo_lib `Shot._generate_dmp`)
    /// lays it out: type(1) + 7*int16(14) + int16 temperature(2) + hour/minute/second(3) + marker(1)
    fn build_v3_section_data() -> String {
        let mut values: Vec<i32> = Vec::new();

        // Version 3
        values.push(3);

        // Section magic (version > 2)
        values.extend(&[68, 89, 101]);

        // Date: year offset from 2000 (24 = 2024), month, day, hour, minute
        values.extend(&[24, 6, 15, 10, 30]);

        // Name: "TST" as ASCII
        values.extend(&[84, 83, 84]);

        // Direction: 0 = In
        values.push(0);

        // Shot 1 (v3 = 21 bytes: type + 7*int16 + int16 temp + hour/minute/second + marker)
        // type=2 (standard), heading_in=450 (45.0°), heading_out=460 (46.0°)
        // length=1000 (10.00m), depth_in=200 (2.00m), depth_out=300 (3.00m)
        // pitch_in=0, pitch_out=0, temperature=185 (18.5 C), time 10:30:15, marker=1
        // int16 BE format: (val >> 8, val & 0xFF)
        values.push(2); // shot type
        values.extend(&[1, 194]); // heading_in 450: (1 << 8) + 194 = 450
        values.extend(&[1, 204]); // heading_out 460
        values.extend(&[3, 232]); // length 1000: (3 << 8) + 232 = 1000
        values.extend(&[0, 200]); // depth_in 200
        values.extend(&[1, 44]); // depth_out 300: (1 << 8) + 44 = 300
        values.extend(&[0, 0]); // pitch_in 0
        values.extend(&[0, 0]); // pitch_out 0
        values.extend(&[0, 185]); // temperature 185 (18.5 C)
        values.extend(&[10, 30, 15]); // hour, minute, second
        values.push(1); // marker_index

        // End-of-section marker for v3 (type=3 followed by zeros, total 21 bytes)
        values.push(3); // EOS type
        values.extend(vec![0; 20]); // zeros to complete 21-byte shot

        values
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(";")
    }

    /// Helper to build v4 format section data as semicolon-separated string
    /// V4 format: version(1) + magic(3) + date(5) + name(3) + direction(1) + shots(29 bytes each) + EOS marker
    fn build_v4_section_data() -> String {
        let mut values: Vec<i32> = Vec::new();

        // Version 4
        values.push(4);

        // Section magic (version > 2)
        values.extend(&[68, 89, 101]);

        // Date: year offset from 2000 (24 = 2024), month, day, hour, minute
        values.extend(&[24, 8, 20, 14, 45]);

        // Name: "V4S" as ASCII
        values.extend(&[86, 52, 83]);

        // Direction: 1 = Out
        values.push(1);

        // Shot 1 (v4 = 29 bytes: type + 7*int16 + 4*int16 LRUD + int16 temp + 3 time bytes + marker)
        // type=2 (standard)
        values.push(2);

        // heading_in=900 (90.0°)
        values.extend(&[3, 132]); // 900 = (3 << 8) + 132
                                  // heading_out=910 (91.0°)
        values.extend(&[3, 142]); // 910
                                  // length=500 (5.00m)
        values.extend(&[1, 244]); // 500 = (1 << 8) + 244
                                  // depth_in=100 (1.00m)
        values.extend(&[0, 100]);
        // depth_out=150 (1.50m)
        values.extend(&[0, 150]);
        // pitch_in=50 (5.0°)
        values.extend(&[0, 50]);
        // pitch_out=-50 (-5.0°) - signed as two's complement
        values.extend(&[255, 206]); // -50 in BE int16

        // LRUD values (v4+): left=100 (1.00m), right=150 (1.50m), up=200 (2.00m), down=80 (0.80m)
        values.extend(&[0, 100]); // left
        values.extend(&[0, 150]); // right
        values.extend(&[0, 200]); // up
        values.extend(&[0, 80]); // down

        // Temperature: 185 (18.5°C)
        values.extend(&[0, 185]);

        // Time: hour=14, minute=45, second=30
        values.extend(&[14, 45, 30]);

        // Marker index
        values.push(2);

        // End-of-section marker for v4 (type=3 followed by zeros, total 29 bytes)
        values.push(3); // EOS type
        values.extend(vec![0; 28]); // zeros to complete 29-byte shot

        values
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(";")
    }

    /// Test 4.1: Verify DMP version 3 files (21-byte shots) are correctly parsed without LRUD data
    #[test]
    fn test_parse_dmp_v3_format() {
        let content = build_v3_section_data();
        let (_file, path) = create_dmp_file(&content);

        let result = parse_dmp(&path);
        std::fs::remove_file(&path).ok();

        assert!(result.is_ok(), "Failed to parse v3 DMP: {:?}", result.err());

        let dmp = result.unwrap();
        assert_eq!(dmp.version, 3, "Version should be 3");
        assert_eq!(dmp.sections.len(), 1, "Should have 1 section");

        let section = &dmp.sections[0];
        assert_eq!(section.version, 3, "Section version should be 3");
        assert_eq!(section.name, "TST", "Section name should be TST");
        assert_eq!(
            section.direction,
            DmpDirection::In,
            "Direction should be In"
        );

        // Check date parsing
        assert_eq!(section.date.year(), 2024, "Year should be 2024");
        assert_eq!(section.date.month(), 6, "Month should be 6");
        assert_eq!(section.date.day(), 15, "Day should be 15");

        // Should have at least one shot (excluding EOS)
        assert!(!section.shots.is_empty(), "Should have shots");

        // V3 does not have LRUD data - check that LRUD values are zero
        let shot = &section.shots[0];
        assert_eq!(shot.left, 0.0, "V3 should have no left LRUD");
        assert_eq!(shot.right, 0.0, "V3 should have no right LRUD");
        assert_eq!(shot.up, 0.0, "V3 should have no up LRUD");
        assert_eq!(shot.down, 0.0, "V3 should have no down LRUD");

        // But basic shot data should be parsed
        assert!(shot.heading_in > 0.0, "Should have heading_in");
        assert!(shot.length > 0.0, "Should have length");
    }

    /// A v3 section of three Standard shots plus the EOS record, each shot in
    /// the 21-byte layout `build_v3_section_data` documents. Every shot
    /// carries the same `temperature_tenths`; headings, lengths, depths,
    /// minutes and markers differ per shot so a drifted read shows up.
    fn build_v3_three_shot_section(temperature_tenths: i16) -> String {
        fn push_i16(v: &mut Vec<i32>, val: i16) {
            let bytes = val.to_be_bytes(); // [hi, lo]
            v.push(i32::from(bytes[0]));
            v.push(i32::from(bytes[1]));
        }

        let mut values: Vec<i32> = vec![3, 68, 89, 101]; // version + section magic
        values.extend(&[24, 6, 15, 10, 30]); // date: 2024-06-15 10:30
        values.extend(&[65, 65, 65]); // name "AAA"
        values.push(0); // direction In

        for i in 0..3_i16 {
            values.push(2); // Standard
            push_i16(&mut values, 450 + 10 * i); // heading_in
            push_i16(&mut values, 455 + 10 * i); // heading_out
            push_i16(&mut values, 1000 + 100 * i); // length
            push_i16(&mut values, 200 + 10 * i); // depth_in
            push_i16(&mut values, 210 + 10 * i); // depth_out
            push_i16(&mut values, 0); // pitch_in
            push_i16(&mut values, 0); // pitch_out
            push_i16(&mut values, temperature_tenths);
            values.extend(&[10, 31 + i32::from(i), 5]); // hour, minute, second
            values.push(i32::from(i) + 1); // marker_index
        }

        values.push(3); // EOS
        values.extend(vec![0; 20]);

        values
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(";")
    }

    /// Every v3 shot must consume its whole 21-byte record (#576 B29). The parser
    /// read only 16 bytes of it (temperature and time were gated on v4), so the
    /// next shot started on the temperature's low byte: at 24.5 C (raw 245)
    /// that is not a shot type and the section stopped after one shot, and at
    /// 25.6 C (raw 256) it decoded as a run of garbage shots.
    fn assert_v3_three_shot_section_parses(temperature_tenths: i16) {
        let temperature = f64::from(temperature_tenths) / 10.0;
        let content = build_v3_three_shot_section(temperature_tenths);
        let (_file, path) = create_dmp_file(&content);
        let result = parse_dmp(&path);
        std::fs::remove_file(&path).ok();

        let dmp = result.expect("synthetic v3 section must parse");
        assert_eq!(dmp.sections.len(), 1, "one section expected");
        let shots = &dmp.sections[0].shots;

        println!(
            "v3 section at {temperature} C: parsed shots: {} (expected 4: 3 Standard + EndOfSurvey)",
            shots.len()
        );
        for (i, s) in shots.iter().enumerate() {
            println!(
                "  shot {i}: type={:?} heading_in={} heading_out={} length={} depth_in={} \
                 depth_out={} temperature={} time={:02}:{:02}:{:02} marker={}",
                s.shot_type,
                s.heading_in,
                s.heading_out,
                s.length,
                s.depth_in,
                s.depth_out,
                s.temperature,
                s.hour,
                s.minute,
                s.second,
                s.marker_index
            );
        }

        assert_eq!(
            shots.len(),
            4,
            "parsed shots: {} (expected 4: 3 Standard + EndOfSurvey)",
            shots.len()
        );
        // Expected values are decoded exactly as the parser decodes them.
        for (i, shot) in (0_u8..).zip(shots.iter().take(3)) {
            let k = i16::from(i);
            assert_eq!(shot.shot_type, DmpShotType::Standard, "shot {i} type");
            assert_eq!(shot.heading_in, f64::from(450 + 10 * k) / 10.0, "shot {i}");
            assert_eq!(shot.heading_out, f64::from(455 + 10 * k) / 10.0, "shot {i}");
            assert_eq!(shot.length, f64::from(1000 + 100 * k) / 100.0, "shot {i}");
            assert_eq!(shot.depth_in, f64::from(200 + 10 * k) / 100.0, "shot {i}");
            assert_eq!(shot.depth_out, f64::from(210 + 10 * k) / 100.0, "shot {i}");
            assert_eq!(shot.temperature, temperature, "shot {i} temperature");
            assert_eq!(
                (shot.hour, shot.minute, shot.second),
                (10, 31 + i, 5),
                "shot {i} time"
            );
            assert_eq!(shot.marker_index, i + 1, "shot {i} marker_index");
        }
        assert_eq!(shots[3].shot_type, DmpShotType::EndOfSurvey, "last record");
    }

    #[test]
    fn v3_shots_at_24_5_c_each_consume_the_whole_21_byte_record() {
        assert_v3_three_shot_section_parses(245);
    }

    #[test]
    fn v3_shots_at_25_6_c_each_consume_the_whole_21_byte_record() {
        assert_v3_three_shot_section_parses(256);
    }

    /// The v3 per-shot layout is inferred from the reference writer alone, so
    /// a v3 hour/minute/second that is not a time of day must read as absent
    /// (0:00:00, what v2 records and what `deriveDmpTiming` skips) rather than
    /// feed the temporal rerank a time that rolls into another day.
    #[test]
    fn a_v3_shot_time_outside_the_day_is_read_as_absent() {
        // (hour, minute, second) per Standard shot, and what parsing must give.
        let cases: [((u8, u8, u8), (u8, u8, u8)); 6] = [
            ((10, 31, 5), (10, 31, 5)),
            ((23, 59, 59), (23, 59, 59)),
            ((24, 0, 0), (0, 0, 0)),
            ((10, 60, 0), (0, 0, 0)),
            ((10, 31, 60), (0, 0, 0)),
            ((200, 7, 9), (0, 0, 0)),
        ];
        let mut data: Vec<u8> = vec![3, 68, 89, 101]; // version + section magic
        data.extend([24, 6, 15, 10, 30]); // date: 2024-06-15 10:30
        data.extend(b"AAA"); // name
        data.push(0); // direction In
        for ((hour, minute, second), _) in cases {
            data.push(2); // Standard
            data.extend([1, 194, 1, 204, 3, 232, 0, 200, 1, 44, 0, 0, 0, 0]); // 7 * int16
            data.extend([0, 185]); // temperature 18.5 C
            data.extend([hour, minute, second]);
            data.push(1); // marker_index
        }
        data.push(3); // EndOfSurvey
        data.extend([0; 20]);

        let section = parse_section(data)
            .expect("synthetic v3 section must parse")
            .section;
        let times: Vec<(u8, u8, u8)> = section
            .shots
            .iter()
            .filter(|s| s.shot_type == DmpShotType::Standard)
            .map(|s| (s.hour, s.minute, s.second))
            .collect();
        for (((raw_h, raw_m, raw_s), _), &(h, m, s)) in cases.iter().zip(&times) {
            let used = if (h, m, s) == (0, 0, 0) {
                "absent"
            } else {
                "used"
            };
            println!(
                "v3 shot raw time {raw_h:02}:{raw_m:02}:{raw_s:02} -> parsed {h:02}:{m:02}:{s:02} ({used})"
            );
        }
        let expected: Vec<(u8, u8, u8)> = cases.iter().map(|&(_, want)| want).collect();
        assert_eq!(times, expected);
        // Only the time is dropped; the rest of each shot reads as before.
        assert!(section
            .shots
            .iter()
            .filter(|s| s.shot_type == DmpShotType::Standard)
            .all(|s| s.temperature == 18.5 && s.length == 10.0 && s.marker_index == 1));
    }

    /// The out-of-day clamp above is for v3 only, whose layout has no device
    /// dump behind it. A v4 shot's time is read as recorded, even when it is not
    /// a time of day.
    #[test]
    fn a_v4_shot_time_outside_the_day_keeps_its_raw_time() {
        let cases: [(u8, u8, u8); 5] = [
            (10, 31, 5),
            (24, 0, 0),
            (10, 60, 0),
            (10, 31, 60),
            (200, 7, 9),
        ];
        let mut data: Vec<u8> = vec![4, 68, 89, 101]; // version + section magic
        data.extend([24, 6, 15, 10, 30]); // date: 2024-06-15 10:30
        data.extend(b"AAA"); // name
        data.push(0); // direction In
        for (hour, minute, second) in cases {
            data.push(2); // Standard
            data.extend([1, 194, 1, 204, 3, 232, 0, 200, 1, 44, 0, 0, 0, 0]); // 7 * int16
            data.extend([0, 100, 0, 150, 0, 200, 0, 80]); // LRUD 4 * int16
            data.extend([0, 185]); // temperature 18.5 C
            data.extend([hour, minute, second]);
            data.push(1); // marker_index
        }
        data.push(3); // EndOfSurvey
        data.extend([0; 28]);

        let section = parse_section(data)
            .expect("synthetic v4 section must parse")
            .section;
        let times: Vec<(u8, u8, u8)> = section
            .shots
            .iter()
            .filter(|s| s.shot_type == DmpShotType::Standard)
            .map(|s| (s.hour, s.minute, s.second))
            .collect();
        for ((raw_h, raw_m, raw_s), &(h, m, s)) in cases.iter().zip(&times) {
            println!(
                "v4 shot raw time {raw_h:02}:{raw_m:02}:{raw_s:02} -> parsed {h:02}:{m:02}:{s:02}"
            );
        }
        assert_eq!(times, cases.to_vec());
    }

    /// A `.dmp` is semicolon-separated integers, and the version is the first
    /// of them. A UTF-8 BOM glues U+FEFF onto that integer, so a file saved
    /// through a Windows text editor fails to import at all.
    #[test]
    fn a_utf8_bom_does_not_break_a_dmp_file() {
        let content = build_v3_section_data();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bom.dmp");
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(content.as_bytes());
        std::fs::write(&path, bytes).unwrap();

        let dmp = parse_dmp(&path).expect("a marked DMP must import like an unmarked one");
        assert_eq!(dmp.version, 3);
        assert_eq!(dmp.sections.len(), 1);
        assert_eq!(dmp.sections[0].name, "TST");
    }

    /// Test 4.2: Verify DMP version 4 files (29-byte shots) correctly extract LRUD data and direction field
    #[test]
    fn test_parse_dmp_v4_with_lrud() {
        let content = build_v4_section_data();
        let (_file, path) = create_dmp_file(&content);

        let result = parse_dmp(&path);
        std::fs::remove_file(&path).ok();

        assert!(result.is_ok(), "Failed to parse v4 DMP: {:?}", result.err());

        let dmp = result.unwrap();
        assert_eq!(dmp.version, 4, "Version should be 4");
        assert_eq!(dmp.sections.len(), 1, "Should have 1 section");

        let section = &dmp.sections[0];
        assert_eq!(section.version, 4, "Section version should be 4");
        assert_eq!(section.name, "V4S", "Section name should be V4S");
        assert_eq!(
            section.direction,
            DmpDirection::Out,
            "Direction should be Out"
        );

        // Check date parsing
        assert_eq!(section.date.year(), 2024, "Year should be 2024");
        assert_eq!(section.date.month(), 8, "Month should be 8");
        assert_eq!(section.date.day(), 20, "Day should be 20");

        // Should have at least one shot
        assert!(!section.shots.is_empty(), "Should have shots");

        let shot = &section.shots[0];

        // V4 should have LRUD data
        assert_eq!(shot.left, 1.0, "Left LRUD should be 1.0m");
        assert_eq!(shot.right, 1.5, "Right LRUD should be 1.5m");
        assert_eq!(shot.up, 2.0, "Up LRUD should be 2.0m");
        assert_eq!(shot.down, 0.8, "Down LRUD should be 0.8m");

        // V4 should have temperature
        assert!(
            (shot.temperature - 18.5).abs() < 0.1,
            "Temperature should be 18.5°C"
        );

        // V4 should have time
        assert_eq!(shot.hour, 14, "Hour should be 14");
        assert_eq!(shot.minute, 45, "Minute should be 45");
        assert_eq!(shot.second, 30, "Second should be 30");

        // Basic shot data
        assert_eq!(shot.heading_in, 90.0, "Heading in should be 90.0°");
        assert_eq!(shot.heading_out, 91.0, "Heading out should be 91.0°");
        assert_eq!(shot.length, 5.0, "Length should be 5.0m");
        assert_eq!(shot.depth_in, 1.0, "Depth in should be 1.0m");
        assert_eq!(shot.depth_out, 1.5, "Depth out should be 1.5m");
        // Pitch bytes decode to +50 / -50 tenths (+5.0° / -5.0°) in the device's
        // +up frame; the parser hands them back in the model's +down frame.
        assert_eq!(shot.pitch_in, -5.0, "Pitch in should be -5.0° (+down)");
        assert_eq!(shot.pitch_out, 5.0, "Pitch out should be +5.0° (+down)");
    }

    /// Test 4.3: Verify graceful error handling for files with invalid magic bytes or corrupted headers
    #[test]
    fn test_parse_dmp_corrupted_header() {
        // Test case 1: Wrong magic bytes
        let mut values: Vec<i32> = Vec::new();
        values.push(3); // Version 3 (expects magic)
        values.extend(&[99, 99, 99]); // Wrong magic (should be [68, 89, 101])
        values.extend(&[24, 6, 15, 10, 30]); // Valid date
        values.extend(&[84, 83, 84]); // Name "TST"
        values.push(0); // Direction
        let content = values
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(";");

        let (_file1, path1) = create_dmp_file(&content);
        let result1 = parse_dmp(&path1);
        std::fs::remove_file(&path1).ok();

        // Should fail with invalid magic
        assert!(result1.is_err(), "Should fail with wrong magic bytes");
        let err_msg = result1.unwrap_err().to_string();
        assert!(
            err_msg.contains("magic") || err_msg.contains("section"),
            "Error should mention magic or section issue: {err_msg}"
        );

        // Test case 2: Truncated magic (not enough bytes)
        let content2 = "3;68;89"; // Version + incomplete magic
        let (_file2, path2) = create_dmp_file(content2);
        let result2 = parse_dmp(&path2);
        std::fs::remove_file(&path2).ok();

        assert!(result2.is_err(), "Should fail with truncated magic");

        // Test case 3: Invalid version (unsupported version 1)
        let content3 = "1;0;0;0;0;0;0;0;0;0";
        let (_file3, path3) = create_dmp_file(content3);
        let result3 = parse_dmp(&path3);
        std::fs::remove_file(&path3).ok();

        assert!(result3.is_err(), "Should fail with unsupported version");
        let err_msg3 = result3.unwrap_err().to_string();
        assert!(
            err_msg3.contains("version") || err_msg3.contains("Unsupported"),
            "Error should mention version issue: {err_msg3}"
        );

        // Test case 4: Version 0 (completely invalid)
        let content4 = "0;0;0;0;0;0;0;0;0;0";
        let (_file4, path4) = create_dmp_file(content4);
        let result4 = parse_dmp(&path4);
        std::fs::remove_file(&path4).ok();

        assert!(result4.is_err(), "Should fail with version 0");
    }

    /// Test 4.4: Verify graceful handling when file ends unexpectedly mid-section or mid-shot
    #[test]
    fn test_parse_dmp_truncated_file() {
        // Test case 1: Mid-header truncation (version present but header incomplete)
        let content1 = "3;68;89;101;24;6"; // Version + magic + partial date
        let (_file1, path1) = create_dmp_file(content1);
        let result1 = parse_dmp(&path1);
        std::fs::remove_file(&path1).ok();

        assert!(result1.is_err(), "Should fail with truncated header");

        // Test case 2: Header complete but no shots (missing EOS marker too)
        let mut values2: Vec<i32> = Vec::new();
        values2.push(3); // Version 3
        values2.extend(&[68, 89, 101]); // Magic
        values2.extend(&[24, 6, 15, 10, 30]); // Date
        values2.extend(&[84, 83, 84]); // Name "TST"
        values2.push(0); // Direction
                         // No shots, no EOS marker
        let content2 = values2
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(";");

        let (_file2, path2) = create_dmp_file(&content2);
        let result2 = parse_dmp(&path2);
        std::fs::remove_file(&path2).ok();

        // Should fail because no sections found (no EOS marker to split on)
        assert!(result2.is_err(), "Should fail with no sections/shots");

        // Test case 3: Mid-shot truncation (partial shot data)
        let mut values3: Vec<i32> = Vec::new();
        values3.push(3); // Version 3
        values3.extend(&[68, 89, 101]); // Magic
        values3.extend(&[24, 6, 15, 10, 30]); // Date
        values3.extend(&[84, 83, 84]); // Name "TST"
        values3.push(0); // Direction
                         // Partial shot - only type and first heading bytes
        values3.push(2); // Shot type
        values3.extend(&[1, 194]); // heading_in
        values3.extend(&[1, 204]); // heading_out
                                   // Missing the rest of shot data and EOS marker
        let content3 = values3
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(";");

        let (_file3, path3) = create_dmp_file(&content3);
        let result3 = parse_dmp(&path3);
        std::fs::remove_file(&path3).ok();

        // Should fail because incomplete data won't find EOS marker
        assert!(result3.is_err(), "Should fail with truncated shot data");
    }

    /// Test 4.5: Verify no panic and graceful handling when date fields contain invalid values
    ///
    /// Note: The parser catches section-level errors and logs them as warnings, then continues.
    /// If all sections fail to parse (due to invalid dates), the overall parse fails with
    /// "No valid sections found". The key requirement is that invalid dates don't cause panics.
    #[test]
    fn test_parse_dmp_invalid_date_values() {
        // Helper to create section data with specific date values
        fn make_section_with_date(
            year_offset: i32,
            month: i32,
            day: i32,
            hour: i32,
            minute: i32,
        ) -> String {
            let mut values: Vec<i32> = Vec::new();
            values.push(3); // Version 3
            values.extend(&[68, 89, 101]); // Magic
            values.extend(&[year_offset, month, day, hour, minute]); // Date
            values.extend(&[84, 83, 84]); // Name "TST"
            values.push(0); // Direction
                            // Valid shot
            values.push(2); // type
            values.extend(&[1, 194, 1, 204, 3, 232, 0, 200, 1, 44, 0, 0, 0, 0]); // shot data
            values.push(1); // marker
                            // EOS marker
            values.push(3);
            values.extend(vec![0; 20]);
            values
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(";")
        }

        // Test case 1: Month 13 (invalid)
        let content1 = make_section_with_date(24, 13, 15, 10, 30);
        let (_file1, path1) = create_dmp_file(&content1);
        let result1 = parse_dmp(&path1);
        std::fs::remove_file(&path1).ok();

        // Should not panic, and should return error (either specific or "no valid sections")
        assert!(result1.is_err(), "Should fail with month 13");
        // Just verify we got an error without panicking - the actual message depends on implementation

        // Test case 2: Day 32 (invalid for any month)
        let content2 = make_section_with_date(24, 6, 32, 10, 30);
        let (_file2, path2) = create_dmp_file(&content2);
        let result2 = parse_dmp(&path2);
        std::fs::remove_file(&path2).ok();

        assert!(result2.is_err(), "Should fail with day 32");

        // Test case 3: February 30 (invalid - Feb never has 30 days)
        let content3 = make_section_with_date(24, 2, 30, 10, 30);
        let (_file3, path3) = create_dmp_file(&content3);
        let result3 = parse_dmp(&path3);
        std::fs::remove_file(&path3).ok();

        assert!(result3.is_err(), "Should fail with Feb 30");

        // Test case 4: Year offset 200 = year 2200, which is outside 2000-2100 range
        let content4 = make_section_with_date(200, 6, 15, 10, 30); // year 2200 - after 2099
        let (_file4, path4) = create_dmp_file(&content4);
        let result4 = parse_dmp(&path4);
        std::fs::remove_file(&path4).ok();

        assert!(
            result4.is_err(),
            "Should fail with year outside valid range"
        );

        // Test case 5: Hour 25 (invalid)
        let content5 = make_section_with_date(24, 6, 15, 25, 30);
        let (_file5, path5) = create_dmp_file(&content5);
        let result5 = parse_dmp(&path5);
        std::fs::remove_file(&path5).ok();

        assert!(result5.is_err(), "Should fail with hour 25");

        // Test case 6: Minute 60 (invalid)
        let content6 = make_section_with_date(24, 6, 15, 10, 60);
        let (_file6, path6) = create_dmp_file(&content6);
        let result6 = parse_dmp(&path6);
        std::fs::remove_file(&path6).ok();

        assert!(result6.is_err(), "Should fail with minute 60");

        // All tests completed without panicking - that's the key requirement
        println!("All invalid date tests completed without panicking");
    }

    // ========================================================================
    // Additional robustness tests
    // ========================================================================

    #[test]
    fn test_parse_dmp_wrong_extension() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(b"3;68;89;101").unwrap();
        let path = file.path().with_extension("txt");
        std::fs::copy(file.path(), &path).unwrap();

        let result = parse_dmp(&path);
        std::fs::remove_file(&path).ok();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not a DMP file"));
    }

    #[test]
    fn test_parse_dmp_non_integer_content() {
        let content = "3;68;89;hello;24;6;15;10;30";
        let (_file, path) = create_dmp_file(content);
        let result = parse_dmp(&path);
        std::fs::remove_file(&path).ok();
        assert!(result.is_err(), "non-integer content should fail");
    }

    #[test]
    fn test_parse_dmp_single_byte() {
        let content = "3"; // Just a version byte, nothing else
        let (_file, path) = create_dmp_file(content);
        let result = parse_dmp(&path);
        std::fs::remove_file(&path).ok();
        assert!(result.is_err(), "single byte should fail (no sections)");
    }

    #[test]
    fn test_parse_dmp_only_semicolons() {
        let content = ";;;;;;;";
        let (_file, path) = create_dmp_file(content);
        let result = parse_dmp(&path);
        std::fs::remove_file(&path).ok();
        assert!(result.is_err(), "only semicolons = empty data");
    }

    #[test]
    fn test_int_buffer_has_remaining() {
        let buffer = IntBuffer::new(vec![1, 2, 3]);
        assert!(buffer.has_remaining(3));
        assert!(!buffer.has_remaining(4));
        assert!(buffer.has_remaining(0));
    }

    #[test]
    fn test_int_buffer_read_n_beyond_end() {
        let mut buffer = IntBuffer::new(vec![1, 2]);
        assert!(buffer.read_n(3).is_err());
    }

    #[test]
    fn test_split_into_sections_empty_data() {
        let sections = split_into_sections(&[], 2).sections;
        assert!(sections.is_empty());
    }

    #[test]
    fn test_split_into_sections_data_shorter_than_pattern() {
        let sections = split_into_sections(&[2, 0, 0], 2).sections;
        assert!(
            sections.is_empty(),
            "data shorter than pattern = no sections"
        );
    }

    #[test]
    fn test_split_into_sections_unsupported_version() {
        let sections = split_into_sections(&[1, 0, 0, 0, 0], 99).sections;
        assert!(sections.is_empty(), "unsupported version = no patterns");
    }

    #[test]
    fn test_end_of_section_patterns_all_versions() {
        // V2 has one pattern of 16 bytes
        let v2 = end_of_section_patterns(2);
        assert_eq!(v2.len(), 1);
        assert_eq!(v2[0].len(), 16);

        // V3 has one pattern of 21 bytes
        let v3 = end_of_section_patterns(3);
        assert_eq!(v3.len(), 1);
        assert_eq!(v3[0].len(), 21);

        // V4 has one pattern of 29 bytes
        let v4 = end_of_section_patterns(4);
        assert_eq!(v4.len(), 1);
        assert_eq!(v4[0].len(), 29);

        // V5 has two patterns of 35 bytes
        let v5 = end_of_section_patterns(5);
        assert_eq!(v5.len(), 2);
        assert_eq!(v5[0].len(), 35);
        assert_eq!(v5[1].len(), 35);

        // Invalid version has no patterns
        let invalid = end_of_section_patterns(99);
        assert!(invalid.is_empty());
    }

    // ========================================================================
    // Content hash tests
    // ========================================================================

    #[test]
    fn test_content_hash_deterministic() {
        let shots = vec![DmpShot {
            shot_type: DmpShotType::Standard,
            heading_in: 90.0,
            heading_out: 270.0,
            length: 5.0,
            depth_in: 0.0,
            depth_out: 1.0,
            pitch_in: 0.0,
            pitch_out: 0.0,
            left: 1.0,
            right: 1.0,
            up: 2.0,
            down: 0.5,
            temperature: 18.0,
            hour: 10,
            minute: 30,
            second: 0,
            marker_index: 1,
            comment: None,
        }];
        let h1 = compute_section_hash("ABC", DmpDirection::In, &shots);
        let h2 = compute_section_hash("ABC", DmpDirection::In, &shots);
        assert_eq!(h1, h2, "same data must produce identical hash");
        assert_eq!(h1.len(), 64, "SHA256 hex = 64 chars");
    }

    #[test]
    fn test_content_hash_changes_with_shot_data() {
        let shots_a = vec![DmpShot {
            shot_type: DmpShotType::Standard,
            heading_in: 90.0,
            heading_out: 270.0,
            length: 5.0,
            depth_in: 0.0,
            depth_out: 1.0,
            pitch_in: 0.0,
            pitch_out: 0.0,
            left: 1.0,
            right: 1.0,
            up: 2.0,
            down: 0.5,
            temperature: 18.0,
            hour: 10,
            minute: 30,
            second: 0,
            marker_index: 1,
            comment: None,
        }];
        let mut shots_b = shots_a.clone();
        shots_b[0].length = 5.1; // slightly different length
        let h_a = compute_section_hash("ABC", DmpDirection::In, &shots_a);
        let h_b = compute_section_hash("ABC", DmpDirection::In, &shots_b);
        assert_ne!(h_a, h_b, "different shot data must produce different hash");
    }

    #[test]
    fn test_content_hash_excludes_date() {
        // Same shots, different dates - hash should be identical
        let shots = vec![DmpShot {
            shot_type: DmpShotType::Standard,
            heading_in: 90.0,
            heading_out: 270.0,
            length: 5.0,
            depth_in: 0.0,
            depth_out: 1.0,
            pitch_in: 0.0,
            pitch_out: 0.0,
            left: 1.0,
            right: 1.0,
            up: 2.0,
            down: 0.5,
            temperature: 18.0,
            hour: 10,
            minute: 30,
            second: 0,
            marker_index: 1,
            comment: None,
        }];
        // Date is not an input to compute_section_hash, so same data = same hash
        let h1 = compute_section_hash("ABC", DmpDirection::In, &shots);
        let h2 = compute_section_hash("ABC", DmpDirection::In, &shots);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_content_hash_ignores_non_standard_shots() {
        let shots_with_eos = vec![
            DmpShot {
                shot_type: DmpShotType::Standard,
                heading_in: 90.0,
                heading_out: 270.0,
                length: 5.0,
                depth_in: 0.0,
                depth_out: 1.0,
                pitch_in: 0.0,
                pitch_out: 0.0,
                left: 1.0,
                right: 1.0,
                up: 2.0,
                down: 0.5,
                temperature: 18.0,
                hour: 10,
                minute: 30,
                second: 0,
                marker_index: 1,
                comment: None,
            },
            DmpShot {
                shot_type: DmpShotType::EndOfSurvey,
                heading_in: 0.0,
                heading_out: 0.0,
                length: 0.0,
                depth_in: 0.0,
                depth_out: 0.0,
                pitch_in: 0.0,
                pitch_out: 0.0,
                left: 0.0,
                right: 0.0,
                up: 0.0,
                down: 0.0,
                temperature: 0.0,
                hour: 0,
                minute: 0,
                second: 0,
                marker_index: 0,
                comment: None,
            },
        ];
        let shots_without = vec![shots_with_eos[0].clone()];
        let h_with = compute_section_hash("ABC", DmpDirection::In, &shots_with_eos);
        let h_without = compute_section_hash("ABC", DmpDirection::In, &shots_without);
        assert_eq!(h_with, h_without, "non-Standard shots must not affect hash");
    }

    // ========================================================================
    // kb5.dmp regression tests. The fixture is committed (e2e/fixtures/kb5.dmp),
    // so these run in every `cargo test`.

    /// The `ShotData` these regression tests feed the matcher, built exactly as
    /// the deleted `DmpShot::to_shot_data` did (#576 B24), so their inputs are
    /// unchanged: headings wrapped into `[0, 360)` (the core import gate does
    /// that now), pitch as parsed, LRUD/temperature/marker as read.
    fn kb5_shot_data(shot: &DmpShot) -> crate::models::ShotData {
        crate::models::ShotData {
            uuid: None,
            azimuth_in: crate::models::normalize_azimuth(shot.heading_in),
            azimuth_out: crate::models::normalize_azimuth(shot.heading_out),
            length: shot.length,
            depth_in: shot.depth_in,
            depth_out: shot.depth_out,
            pitch_in: Some(shot.pitch_in),
            pitch_out: Some(shot.pitch_out),
            left: Some(shot.left),
            right: Some(shot.right),
            up: Some(shot.up),
            down: Some(shot.down),
            comment: shot.comment.clone(),
            kind: ShotKind::Normal,
            marker_index: Some(u32::from(shot.marker_index)),
            temperature: Some(shot.temperature),
        }
    }
    // ========================================================================

    /// Parse kb5.dmp and validate section structure, shot counts, and key shot data.
    /// Run with: cargo test --manifest-path src-tauri/Cargo.toml kb5_parse_regression -- --nocapture
    #[test]
    fn kb5_parse_regression() {
        let test_file = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../e2e/fixtures/kb5.dmp");
        assert!(
            test_file.exists(),
            "Fixture missing: {}",
            test_file.display()
        );

        let dmp = parse_dmp(&test_file).expect("should parse kb5.dmp");

        // Version
        assert_eq!(dmp.version, 5);

        // Section count
        assert_eq!(dmp.sections.len(), 5, "expected 5 sections");

        // Section names and directions
        let names: Vec<&str> = dmp.sections.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["001", "002", "003", "004", "005"]);
        for section in &dmp.sections {
            assert_eq!(
                section.direction,
                DmpDirection::In,
                "all sections should be direction In"
            );
        }

        // DMP shot counts (all shot types)
        let dmp_shot_counts: Vec<usize> = dmp.sections.iter().map(|s| s.shots.len()).collect();
        assert_eq!(dmp_shot_counts, vec![108, 365, 30, 5, 8]);

        // Standard shot counts (after filtering non-Standard types)
        let standard_counts: Vec<usize> = dmp
            .sections
            .iter()
            .map(|s| {
                s.shots
                    .iter()
                    .filter(|sh| sh.shot_type == DmpShotType::Standard)
                    .count()
            })
            .collect();
        assert_eq!(standard_counts, vec![107, 364, 29, 4, 7]);

        // Validate key shot data for each section (first and last standard shots)
        let assert_shot = |section_idx: usize,
                           shot_idx: usize,
                           az_in: f64,
                           az_out: f64,
                           len: f64,
                           d_in: f64,
                           d_out: f64| {
            let standard: Vec<&DmpShot> = dmp.sections[section_idx]
                .shots
                .iter()
                .filter(|s| s.shot_type == DmpShotType::Standard)
                .collect();
            let shot = standard[shot_idx];
            assert!(
                (shot.heading_in - az_in).abs() < 0.05,
                "section[{}] shot[{}] az_in: {} != {}",
                section_idx,
                shot_idx,
                shot.heading_in,
                az_in
            );
            assert!(
                (shot.heading_out - az_out).abs() < 0.05,
                "section[{}] shot[{}] az_out: {} != {}",
                section_idx,
                shot_idx,
                shot.heading_out,
                az_out
            );
            assert!(
                (shot.length - len).abs() < 0.005,
                "section[{}] shot[{}] length: {} != {}",
                section_idx,
                shot_idx,
                shot.length,
                len
            );
            assert!(
                (shot.depth_in - d_in).abs() < 0.005,
                "section[{}] shot[{}] depth_in: {} != {}",
                section_idx,
                shot_idx,
                shot.depth_in,
                d_in
            );
            assert!(
                (shot.depth_out - d_out).abs() < 0.005,
                "section[{}] shot[{}] depth_out: {} != {}",
                section_idx,
                shot_idx,
                shot.depth_out,
                d_out
            );
        };

        // Section 001: first, last standard shots
        assert_shot(0, 0, 209.7, 209.7, 11.360, 8.45, 8.55);
        assert_shot(0, 106, 343.7, 343.7, 12.040, 8.03, 7.89);

        // Section 002: first, last standard shots
        assert_shot(1, 0, 23.2, 23.2, 3.580, 9.22, 9.12);
        assert_shot(1, 363, 258.1, 258.1, 9.120, 7.42, 7.49);

        // Section 003: first, last standard shots
        assert_shot(2, 0, 229.9, 229.9, 5.210, 9.55, 9.44);
        assert_shot(2, 28, 59.0, 59.0, 5.480, 7.21, 7.11);

        // Section 004: all 4 standard shots
        assert_shot(3, 0, 45.6, 45.6, 5.090, 9.35, 9.40);
        assert_shot(3, 1, 352.0, 352.0, 3.760, 9.30, 9.35);
        assert_shot(3, 2, 33.7, 33.7, 5.080, 9.48, 9.49);
        assert_shot(3, 3, 9.5, 9.5, 5.710, 9.49, 9.39);

        // Section 005: first, last standard shots
        assert_shot(4, 0, 256.4, 256.4, 14.010, 8.30, 8.40);
        assert_shot(4, 6, 299.0, 299.0, 13.360, 9.36, 9.16);
    }

    /// Run cross-section suggestions on kb5.dmp sections (with 002 as existing)
    /// and validate the overlap detection results.
    ///
    /// Re-blessed for #576 NF-KB5-ROT (decision E13-D3): the 2026-06-11
    /// placement tuning removed the two-shot overlaps of 001's back and 004's
    /// back against existing 002 on purpose, and the test was skipped at the time,
    /// so nothing noticed; it now pins the stricter outcome. Measured by reverting
    /// each tuning step at the tip (#576 RV5-RL-2), more than one cutoff rejects
    /// each, so a suggestion reappearing there means several loosened at once:
    ///
    /// - 001 -> 002 (002 station 182, reverse) is refused by each of three steps on
    ///   its own: the depth cutoff 0.50 -> 0.30 m (5570de8d6; the second shot's
    ///   depth differs by 0.47 m), the length cutoff's floor 2.0 -> 0.4 m
    ///   (d93f93b2; the second shot's length differs by 1.71 m), and the
    ///   shallow-crossing veto (9a4889dfe / 14c412b8e; every merge offset fails
    ///   it). Only with all three reverted does it come back.
    /// - 004 -> 002 (002 station 257, forward) is refused by the depth cutoff and
    ///   by the length floor, each on its own; the crossing veto passes it.
    ///
    /// (The re-bless first credited d93f93b2 alone; restoring that cutoff by itself
    /// brings neither back.)
    /// Run with: cargo test --manifest-path src-tauri/Cargo.toml kb5_suggestions_regression -- --nocapture
    #[test]
    fn kb5_suggestions_regression() {
        use crate::models::placement_suggest::{
            suggest_all_placements, ExistingSection, ImportSectionForSuggestion, TargetType,
        };
        use crate::models::shot::{Shot, ShotKind};

        let test_file = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../e2e/fixtures/kb5.dmp");
        assert!(
            test_file.exists(),
            "Fixture missing: {}",
            test_file.display()
        );

        let dmp = parse_dmp(&test_file).expect("should parse kb5.dmp");

        // Build section 002 as the "existing" project section (simulates it being already imported)
        let section_002 = &dmp.sections[1]; // index 1 = "002"
        let existing_shots: Vec<Shot> = section_002
            .shots
            .iter()
            .filter(|s| s.shot_type == DmpShotType::Standard)
            .enumerate()
            .map(|(i, s)| Shot {
                id: i as u32,
                azimuth_in: s.heading_in,
                azimuth_out: s.heading_out,
                length: s.length,
                depth_in: s.depth_in,
                depth_out: s.depth_out,
                pitch_in: Some(s.pitch_in),
                pitch_out: Some(s.pitch_out),
                left: Some(s.left),
                right: Some(s.right),
                up: Some(s.up),
                down: Some(s.down),
                comment: s.comment.clone(),
                kind: ShotKind::Normal,
                marker_index: Some(u32::from(s.marker_index)),
                temperature: Some(s.temperature),
                uuid: None,
            })
            .collect();

        let existing_sections = vec![ExistingSection {
            id: 0,
            name: "002".to_string(),
            shots: existing_shots,
            azimuth_reference: crate::models::AzimuthReference::default(),
        }];

        // Build import sections (all 5 from DMP, including 002)
        let import_sections: Vec<ImportSectionForSuggestion> = dmp
            .sections
            .iter()
            .enumerate()
            .map(|(i, s)| ImportSectionForSuggestion {
                index: i as u32,
                name: s.name.clone(),
                shots: s
                    .shots
                    .iter()
                    .filter(|sh| sh.shot_type == DmpShotType::Standard)
                    .map(kb5_shot_data)
                    .collect(),
                // The mNemo measures magnetic and declares no declination.
                azimuth_reference: Some(stalagmight_models::AzimuthReference::magnetic(
                    0.0,
                    stalagmight_models::CorrectionSource::Unknown,
                )),
            })
            .collect();

        let suggestions = suggest_all_placements(
            &import_sections,
            crate::models::AzimuthReference::default(),
            &existing_sections,
            &crate::models::config::PlacementSuggestionConfig::default(),
        );

        // Print all suggestions for debugging
        for (i, s) in suggestions.iter().enumerate() {
            println!(
                "  suggestion[{}]: import={} '{}' impStn={} → target={} '{}' exStn={} matched={} dir={:?} score={:.3} conf={:?} partial={}",
                i, s.import_section_index, s.import_section_name,
                s.import_station_index,
                match s.target_type { TargetType::Existing => format!("existing:{}", s.existing_section_id.unwrap_or(0)), TargetType::Importing => format!("importing:{}", s.target_import_index.unwrap_or(0)) },
                s.existing_section_name,
                s.existing_station_index,
                s.matched_shots, s.direction, s.score, s.confidence, s.is_partial,
            );
        }

        assert!(!suggestions.is_empty(), "should find suggestions");

        // The key confirmed suggestions from the user's import flow:
        // 1. import=4(005) impStn=7 → target=importing:0(001) exStn=25 matched=2 dir=forward
        // 2. import=3(004) impStn=0 → target=importing:0(001) exStn=28 matched=2 dir=reverse
        // 3. import=2(003) impStn=0 → target=importing:0(001) exStn=27 matched=2 dir=forward
        // Before the 2026-06-11 tuning there was a fourth, import=0(001)
        // impStn=107 → target=existing:0(002) exStn=182 matched=2 dir=reverse;
        // the depth cutoff, the length floor and the crossing veto each refuse it
        // now (see the doc comment).
        let s001_to_002 = suggestions.iter().find(|s| {
            s.import_section_index == 0
                && s.target_type == TargetType::Existing
                && s.existing_section_id == Some(0)
        });
        assert!(
            s001_to_002.is_none(),
            "001 has no placement onto existing 002 under the tuned cutoffs, got {s001_to_002:?}"
        );

        // Verify import→import suggestions exist
        // 005 → 001
        let s005_to_001 = suggestions.iter().find(
            |s| {
                s.import_section_index == 4
                    && s.target_type == TargetType::Importing
                    && s.target_import_index == Some(0)
                    && s.import_station_index == 7
            }, // back of 005
        );
        assert!(
            s005_to_001.is_some(),
            "should find 005→001 importing suggestion"
        );
        let s = s005_to_001.unwrap();
        assert_eq!(s.matched_shots, 2);
        assert_eq!(
            s.existing_station_index, 25,
            "005 connects to 001 station 25"
        );

        // 004 → 001 (front of 004 connects to station 28 of 001)
        let s004_to_001 = suggestions.iter().find(
            |s| {
                s.import_section_index == 3
                    && s.target_type == TargetType::Importing
                    && s.target_import_index == Some(0)
                    && s.import_station_index == 0
            }, // front of 004
        );
        assert!(
            s004_to_001.is_some(),
            "should find 004→001 importing suggestion"
        );
        let s = s004_to_001.unwrap();
        assert_eq!(s.matched_shots, 2);
        assert_eq!(
            s.existing_station_index, 28,
            "004 connects to 001 station 28"
        );

        // 003 → 001 (front of 003 connects to station 27 of 001)
        let s003_to_001 = suggestions.iter().find(
            |s| {
                s.import_section_index == 2
                    && s.target_type == TargetType::Importing
                    && s.target_import_index == Some(0)
                    && s.import_station_index == 0
            }, // front of 003
        );
        assert!(
            s003_to_001.is_some(),
            "should find 003→001 importing suggestion"
        );
        let s = s003_to_001.unwrap();
        assert_eq!(s.matched_shots, 2);
        assert_eq!(
            s.existing_station_index, 27,
            "003 connects to 001 station 27"
        );

        // Section 004's back used to overlap existing 002 too (two shots at 002
        // station 257 - the source of the double-treatment bug); under the
        // tuned depth and length cutoffs it is not offered either.
        let s004_to_existing = suggestions.iter().find(
            |s| {
                s.import_section_index == 3
                    && s.target_type == TargetType::Existing
                    && s.import_station_index == 4
            }, // back of 004
        );
        assert!(
            s004_to_existing.is_none(),
            "004's back has no placement onto existing 002 under the tuned cutoffs, got \
             {s004_to_existing:?}"
        );
    }

    /// Simulate overlap treatment for kb5.dmp and validate post-import shot counts.
    ///
    /// Scenario: section 002 is already imported as existing (zero datum).
    /// Sections 001, 003, 004, 005 are imported via the wizard with all default
    /// suggestions and `interpolate` overlap mode.
    ///
    /// The treatment rules (matching the TypeScript `applyAllOverlapTreatments`):
    /// - Import-to-import (targetType=Importing): import overlap shots are DELETED
    /// - Import-to-existing (targetType=Existing): import overlap shots are INTERPOLATED (kept alive)
    /// - treatedShots guard: a shot already treated by a prior suggestion is skipped
    /// - Suggestions targeting importing:1 (section 002) are skipped (002 is existing, not imported)
    ///
    ///
    /// Re-blessed for #576 NF-KB5-ROT (decision E13-D3): since the 2026-06-11
    /// tuning there is no 001→002 suggestion (the depth cutoff, the length floor
    /// and the crossing veto each refuse it - see `kb5_suggestions_regression`),
    /// so nothing deletes 001's two overlap shots and 001 keeps all 107. Nor is
    /// there a 004→002 one, so the treated-shots guard is no longer driven
    /// through an import-to-import plus import-to-existing overlap on the same
    /// shots (the double-treatment case) by any kb5 test.
    /// Run with: cargo test --manifest-path src-tauri/Cargo.toml kb5_overlap_treatment_regression -- --nocapture
    #[test]
    fn kb5_overlap_treatment_regression() {
        use crate::models::placement_suggest::{
            suggest_all_placements, CrossSectionSuggestion, ExistingSection,
            ImportSectionForSuggestion, TargetType,
        };
        use crate::models::shot::{Shot, ShotKind};
        use std::collections::{HashMap, HashSet};

        let test_file = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../e2e/fixtures/kb5.dmp");
        let dmp = parse_dmp(&test_file).expect("should parse kb5.dmp");

        // Raw standard shot counts (pre-treatment)
        let raw_counts: Vec<usize> = dmp
            .sections
            .iter()
            .map(|s| {
                s.shots
                    .iter()
                    .filter(|sh| sh.shot_type == DmpShotType::Standard)
                    .count()
            })
            .collect();
        assert_eq!(raw_counts[2], 29, "003 raw standard shots");
        assert_eq!(raw_counts[3], 4, "004 raw standard shots");
        assert_eq!(raw_counts[4], 7, "005 raw standard shots");

        // Build 002 as existing, all 5 as imports (same setup as suggestions test)
        let section_002 = &dmp.sections[1];
        let existing_shots: Vec<Shot> = section_002
            .shots
            .iter()
            .filter(|s| s.shot_type == DmpShotType::Standard)
            .enumerate()
            .map(|(i, s)| Shot {
                id: i as u32,
                azimuth_in: s.heading_in,
                azimuth_out: s.heading_out,
                length: s.length,
                depth_in: s.depth_in,
                depth_out: s.depth_out,
                pitch_in: Some(s.pitch_in),
                pitch_out: Some(s.pitch_out),
                left: Some(s.left),
                right: Some(s.right),
                up: Some(s.up),
                down: Some(s.down),
                comment: s.comment.clone(),
                kind: ShotKind::Normal,
                marker_index: Some(u32::from(s.marker_index)),
                temperature: Some(s.temperature),
                uuid: None,
            })
            .collect();
        let existing_sections = vec![ExistingSection {
            id: 0,
            name: "002".to_string(),
            shots: existing_shots,
            azimuth_reference: crate::models::AzimuthReference::default(),
        }];
        let import_sections: Vec<ImportSectionForSuggestion> = dmp
            .sections
            .iter()
            .enumerate()
            .map(|(i, s)| ImportSectionForSuggestion {
                index: i as u32,
                name: s.name.clone(),
                shots: s
                    .shots
                    .iter()
                    .filter(|sh| sh.shot_type == DmpShotType::Standard)
                    .map(kb5_shot_data)
                    .collect(),
                // The mNemo measures magnetic and declares no declination.
                azimuth_reference: Some(stalagmight_models::AzimuthReference::magnetic(
                    0.0,
                    stalagmight_models::CorrectionSource::Unknown,
                )),
            })
            .collect();

        let suggestions = suggest_all_placements(
            &import_sections,
            crate::models::AzimuthReference::default(),
            &existing_sections,
            &crate::models::config::PlacementSuggestionConfig::default(),
        );

        // Simulate applyAllOverlapTreatments import-side logic.
        // deleted_shots: section_index → set of shot IDs marked deleted.
        // treated_shots: "section_index:shot_id" → already processed.
        //
        // The wizard confirms one suggestion per import section (the highest-scoring one).
        // Confirmed import-to-existing suggestions delete overlap shots (the existing
        // section already has the data). Unconfirmed import-to-existing suggestions
        // only interpolate (they may be false positives from the pool).
        let mut confirmed_sections: HashSet<u32> = HashSet::new();
        let mut confirmed_suggestions: HashSet<usize> = HashSet::new();
        for (i, s) in suggestions.iter().enumerate() {
            if s.import_section_index == 1 {
                continue;
            }
            if s.target_type == TargetType::Importing && s.target_import_index == Some(1) {
                continue;
            }
            if s.matched_shots == 0 {
                continue;
            }
            // First (highest-scoring) suggestion per section is the confirmed one
            if confirmed_sections.insert(s.import_section_index) {
                confirmed_suggestions.insert(i);
            }
        }

        let mut deleted_shots: HashMap<u32, HashSet<usize>> = HashMap::new();
        let mut treated_shots: HashSet<String> = HashSet::new();

        for (i, s) in suggestions.iter().enumerate() {
            if s.import_section_index == 1 {
                continue;
            }
            if s.target_type == TargetType::Importing && s.target_import_index == Some(1) {
                continue;
            }
            if s.matched_shots == 0 {
                continue;
            }

            let is_confirmed = confirmed_suggestions.contains(&i);

            // Compute overlap IDs (mirrors TypeScript computeOverlapIds)
            let overlap_ids: Vec<usize> = if s.import_station_index == 0 {
                (0..s.matched_shots).collect()
            } else {
                let start = s.import_station_index.saturating_sub(s.matched_shots);
                (start..s.import_station_index).collect()
            };

            for &shot_id in &overlap_ids {
                let key = format!("{}:{}", s.import_section_index, shot_id);
                if treated_shots.contains(&key) {
                    continue;
                }
                treated_shots.insert(key);

                // Import-to-import: always delete (target section keeps the overlap).
                // Confirmed import-to-existing: delete (target gets interpolated values).
                if s.target_type == TargetType::Importing || is_confirmed {
                    deleted_shots
                        .entry(s.import_section_index)
                        .or_default()
                        .insert(shot_id);
                }
            }
        }

        // Compute post-treatment alive counts
        let alive = |section_idx: u32| -> usize {
            let raw = raw_counts[section_idx as usize];
            let deleted = deleted_shots.get(&section_idx).map_or(0, |s| s.len());
            raw - deleted
        };

        let alive_001 = alive(0);
        let alive_003 = alive(2);
        let alive_004 = alive(3);
        let alive_005 = alive(4);

        println!(
            "Section 001: raw={}, deleted={:?}, alive={}",
            raw_counts[0],
            deleted_shots.get(&0),
            alive_001
        );
        println!(
            "Section 003: raw={}, deleted={:?}, alive={}",
            raw_counts[2],
            deleted_shots.get(&2),
            alive_003
        );
        println!(
            "Section 004: raw={}, deleted={:?}, alive={}",
            raw_counts[3],
            deleted_shots.get(&3),
            alive_004
        );
        println!(
            "Section 005: raw={}, deleted={:?}, alive={}",
            raw_counts[4],
            deleted_shots.get(&4),
            alive_005
        );

        assert_eq!(
            alive_001, 107,
            "001: 107 raw, nothing deleted - no import-to-existing placement with 002 is offered"
        );
        assert_eq!(
            alive_005, 5,
            "005: 7 raw - 2 deleted (import-to-import with 001) = 5"
        );
        assert_eq!(
            alive_004, 2,
            "004: 4 raw - 2 deleted (front overlap with 001) = 2"
        );
        assert_eq!(
            alive_003, 27,
            "003: 29 raw - 2 deleted (front overlap with 001) = 27"
        );

        // Verify attachment stations: all three sections connect to 001
        let find_primary = |import_idx: u32| -> &CrossSectionSuggestion {
            suggestions
                .iter()
                .find(
                    |s| {
                        s.import_section_index == import_idx
                            && s.target_type == TargetType::Importing
                            && s.target_import_index == Some(0)
                    }, // 001
                )
                .unwrap_or_else(|| {
                    panic!("no importing suggestion for section index {}", import_idx)
                })
        };

        let s005 = find_primary(4);
        assert_eq!(
            s005.existing_station_index, 25,
            "005 attaches to 001 station 25"
        );

        let s003 = find_primary(2);
        assert_eq!(
            s003.existing_station_index, 27,
            "003 attaches to 001 station 27"
        );

        let s004 = find_primary(3);
        assert_eq!(
            s004.existing_station_index, 28,
            "004 attaches to 001 station 28"
        );

        // 001 no longer gets a placement onto existing 002 (it used to attach at
        // its back, station 107, to 002 station 182).
        let s001 = suggestions.iter().find(|s| {
            s.import_section_index == 0
                && s.target_type == TargetType::Existing
                && s.existing_section_id == Some(0)
        });
        assert!(s001.is_none(), "no 001→002 suggestion, got {s001:?}");

        // Overlap shot raw data for the 001→002 connection a user places by hand:
        // import overlap shots 105, 106 of 001 and target shots 180, 181 of
        // existing section 002. The integration test
        // kb5_import_overlap_interpolation_integration (core/state/tests.rs)
        // imports that manual placement and checks the interpolated values.
        let std_001: Vec<_> = dmp.sections[0]
            .shots
            .iter()
            .filter(|s| s.shot_type == DmpShotType::Standard)
            .collect();
        let std_002: Vec<_> = dmp.sections[1]
            .shots
            .iter()
            .filter(|s| s.shot_type == DmpShotType::Standard)
            .collect();

        // Import side: 001 shots 105, 106 (deleted during import)
        assert_eq!(std_001[105].length, 4.81, "001 shot 105 length");
        assert_eq!(std_001[105].heading_in, 315.4, "001 shot 105 azimuth");
        assert_eq!(std_001[106].length, 12.04, "001 shot 106 length");
        assert_eq!(std_001[106].heading_in, 343.7, "001 shot 106 azimuth");

        // Target side: 002 shots 180, 181 (interpolated during import)
        assert_eq!(std_002[180].length, 6.52, "002 shot 180 raw length");
        assert_eq!(std_002[180].heading_in, 329.7, "002 shot 180 raw azimuth");
        assert_eq!(std_002[181].length, 12.71, "002 shot 181 raw length");
        assert_eq!(std_002[181].heading_in, 355.8, "002 shot 181 raw azimuth");
    }

    // ============================================================================
    // Signed int16 / negative pitch tests
    // ============================================================================

    /// Helper: build a v4 DMP section from field-level values (as signed i16).
    /// Encodes each i16 as two unsigned bytes [hi, lo] matching the on-disk format.
    fn build_v4_shot_section(
        name: [u8; 3],
        heading_in: i16,
        heading_out: i16,
        length: i16,
        depth_in: i16,
        depth_out: i16,
        pitch_in: i16,
        pitch_out: i16,
    ) -> String {
        fn push_i16(v: &mut Vec<i32>, val: i16) {
            let bytes = val.to_be_bytes(); // [hi, lo]
            v.push(i32::from(bytes[0]));
            v.push(i32::from(bytes[1]));
        }

        let mut values: Vec<i32> = Vec::new();

        // V4 header
        values.push(4);
        values.extend(&[68, 89, 101]); // magic
        values.extend(&[25, 1, 15, 9, 0]); // date: 2025-01-15 09:00
        values.extend(&[i32::from(name[0]), i32::from(name[1]), i32::from(name[2])]);
        values.push(0); // direction In

        // Shot (29 bytes for v4)
        values.push(2); // Standard
        push_i16(&mut values, heading_in);
        push_i16(&mut values, heading_out);
        push_i16(&mut values, length);
        push_i16(&mut values, depth_in);
        push_i16(&mut values, depth_out);
        push_i16(&mut values, pitch_in);
        push_i16(&mut values, pitch_out);
        // LRUD
        push_i16(&mut values, 100);
        push_i16(&mut values, 150);
        push_i16(&mut values, 200);
        push_i16(&mut values, 80);
        // Temperature
        push_i16(&mut values, 185);
        // Time
        values.extend(&[10, 30, 0]);
        // Marker
        values.push(0);

        // EOS
        values.push(3);
        values.extend(vec![0; 28]);

        values
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(";")
    }

    /// Full integration test: negative pitch values parsed through the entire DMP pipeline.
    #[test]
    fn test_parse_dmp_negative_pitch() {
        let content = build_v4_shot_section(
            *b"NEG", 900,  // heading_in 90.0°
            910,  // heading_out 91.0°
            500,  // length 5.00m
            100,  // depth_in 1.00m
            150,  // depth_out 1.50m
            -150, // pitch_in -15.0°
            -300, // pitch_out -30.0°
        );
        let (_file, path) = create_dmp_file(&content);
        let dmp = parse_dmp(&path).expect("should parse DMP with negative pitch");
        std::fs::remove_file(&path).ok();

        let shot = &dmp.sections[0].shots[0];
        assert_eq!(shot.heading_in, 90.0);
        assert_eq!(shot.heading_out, 91.0);
        assert_eq!(shot.length, 5.0);
        assert_eq!(shot.depth_in, 1.0);
        assert_eq!(shot.depth_out, 1.5);
        // The device wrote -150 / -300 tenths (-15.0° / -30.0°, +up); the parser
        // normalises the magnitudes into the model's +down frame.
        assert_eq!(
            shot.pitch_in, 15.0,
            "device -15.0° (+up) must arrive as +15.0° (+down)"
        );
        assert_eq!(
            shot.pitch_out, 30.0,
            "device -30.0° (+up) must arrive as +30.0° (+down)"
        );
    }

    /// Steep downward pitch near the device's -90° limit.
    #[test]
    fn test_parse_dmp_steep_negative_pitch() {
        let content = build_v4_shot_section(
            *b"STP", 1800, // heading_in 180.0°
            1810, // heading_out 181.0°
            200,  // length 2.00m
            500,  // depth_in 5.00m
            800,  // depth_out 8.00m
            -850, // pitch_in -85.0° (nearly vertical down)
            -900, // pitch_out -90.0° (straight down)
        );
        let (_file, path) = create_dmp_file(&content);
        let dmp = parse_dmp(&path).expect("should parse steep negative pitch");
        std::fs::remove_file(&path).ok();

        let shot = &dmp.sections[0].shots[0];
        // Device wrote -850 / -900 tenths (-85.0° / -90.0°, +up).
        assert_eq!(shot.pitch_in, 85.0, "Steep downward pitch_in (+down)");
        assert_eq!(shot.pitch_out, 90.0, "Vertical downward pitch_out (+down)");
    }

    /// Positive device pitch (upward-sloping passage), which the parser reports
    /// as negative in the model's +down frame.
    #[test]
    fn test_parse_dmp_positive_device_pitch() {
        let content = build_v4_shot_section(
            *b"UPP", 450,  // heading_in 45.0°
            460,  // heading_out 46.0°
            1000, // length 10.00m
            200,  // depth_in 2.00m
            100,  // depth_out 1.00m
            450,  // pitch_in +45.0°
            300,  // pitch_out +30.0°
        );
        let (_file, path) = create_dmp_file(&content);
        let dmp = parse_dmp(&path).expect("should parse positive pitch");
        std::fs::remove_file(&path).ok();

        let shot = &dmp.sections[0].shots[0];
        // Device wrote +450 / +300 tenths (+45.0° / +30.0°, +up = ascending);
        // depth decreases 2.00m -> 1.00m on the same shot, so +down is negative.
        assert_eq!(shot.pitch_in, -45.0, "Ascending pitch_in (+down)");
        assert_eq!(shot.pitch_out, -30.0, "Ascending pitch_out (+down)");
    }

    /// Zero pitch (horizontal passage).
    #[test]
    fn test_parse_dmp_zero_pitch() {
        let content = build_v4_shot_section(
            *b"ZRO", 2700, 2710, 300, 100, 100, 0, // pitch_in 0.0°
            0, // pitch_out 0.0°
        );
        let (_file, path) = create_dmp_file(&content);
        let dmp = parse_dmp(&path).expect("should parse zero pitch");
        std::fs::remove_file(&path).ok();

        let shot = &dmp.sections[0].shots[0];
        assert_eq!(shot.pitch_in, 0.0);
        assert_eq!(shot.pitch_out, 0.0);
    }

    /// Mixed positive/negative pitch within a single shot.
    #[test]
    fn test_parse_dmp_mixed_pitch() {
        let content = build_v4_shot_section(
            *b"MIX", 900, 910, 500, 200, 300, 50,  // pitch_in +5.0°
            -50, // pitch_out -5.0°
        );
        let (_file, path) = create_dmp_file(&content);
        let dmp = parse_dmp(&path).expect("should parse mixed pitch");
        std::fs::remove_file(&path).ok();

        let shot = &dmp.sections[0].shots[0];
        // Device wrote +50 / -50 tenths (+5.0° / -5.0°, +up); both signs flip.
        assert_eq!(shot.pitch_in, -5.0, "Ascending pitch_in (+down)");
        assert_eq!(shot.pitch_out, 5.0, "Descending pitch_out (+down)");
    }

    /// Small negative device pitch values (-0.1° to -0.9°) that sit near zero.
    #[test]
    fn test_parse_dmp_small_negative_pitch() {
        let content = build_v4_shot_section(
            *b"SML", 900, 910, 500, 100, 100, -1, // pitch_in -0.1°
            -9, // pitch_out -0.9°
        );
        let (_file, path) = create_dmp_file(&content);
        let dmp = parse_dmp(&path).expect("should parse small negative pitch");
        std::fs::remove_file(&path).ok();

        let shot = &dmp.sections[0].shots[0];
        // Device wrote -1 / -9 tenths (-0.1° / -0.9°, +up).
        assert!(
            (shot.pitch_in - 0.1).abs() < 1e-9,
            "pitch_in should be +0.1° (+down), got {}",
            shot.pitch_in
        );
        assert!(
            (shot.pitch_out - 0.9).abs() < 1e-9,
            "pitch_out should be +0.9° (+down), got {}",
            shot.pitch_out
        );
    }

    /// Negative heading values (headings are 0-3600 so this shouldn't normally
    /// occur, but the int16 reader must not break on high unsigned values that
    /// sit below i16::MAX -- e.g. heading 350.0° = 3500 = 0x0DAC, positive).
    #[test]
    fn test_parse_dmp_large_positive_heading() {
        let content = build_v4_shot_section(
            *b"HDG", 3500, // heading_in 350.0°
            3590, // heading_out 359.0°
            500, 100, 100, 0, 0,
        );
        let (_file, path) = create_dmp_file(&content);
        let dmp = parse_dmp(&path).expect("should parse large headings");
        std::fs::remove_file(&path).ok();

        let shot = &dmp.sections[0].shots[0];
        assert_eq!(shot.heading_in, 350.0);
        assert_eq!(shot.heading_out, 359.0);
    }

    /// End-to-end through the file reader: pitch magnitude survives, negated
    /// into the model's +down frame.  The device's negative (descending)
    /// readings come out positive, matching the depth increase on the same
    /// shot.  The import wizard copies this parsed pitch into `ShotData`
    /// verbatim (`editableShotToShotData`), so the flip happens exactly once.
    #[test]
    fn test_negative_device_pitch_leaves_the_parser_positive_down() {
        let content = build_v4_shot_section(
            *b"E2E", 900, 910, 500, 100, 150, -200, // pitch_in -20.0° (device, +up)
            -350, // pitch_out -35.0° (device, +up)
        );
        let (_file, path) = create_dmp_file(&content);
        let dmp = parse_dmp(&path).expect("should parse");
        std::fs::remove_file(&path).ok();

        // The parser is where the device frame is normalised.
        let shot = &dmp.sections[0].shots[0];
        assert_eq!(shot.pitch_in, 20.0, "DmpShot leaves the parser in +down");
        assert_eq!(shot.pitch_out, 35.0, "DmpShot leaves the parser in +down");
    }

    /// Cross-validate CaveChef's parser against known values from pytool_mnemo_lib's
    /// test_v5.dmp fixture. Both codebases must produce identical results.
    #[test]
    fn test_parse_v5_fixture_pitch_values() {
        let test_file =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../e2e/fixtures/test_v5.dmp");
        let test_file = require_file(test_file);

        let dmp = parse_dmp(&test_file).expect("should parse v5 fixture");

        // Section 0 ("BAS"), shot 0:
        // File bytes for pitch: -1;-17 and -1;-18
        // Python: (-1*256)+(-17&0xFF) = -17, (-1*256)+(-18&0xFF) = -18
        // Device wrote -1.7° / -1.8° (+up); parser normalises to +1.7° / +1.8°.
        let s0 = &dmp.sections[0];
        assert_eq!(s0.name, "BAS", "First section name");
        let shot0 = &s0.shots[0];
        assert!(
            (shot0.pitch_in - 1.7).abs() < 0.01,
            "v5 s0 shot0 pitch_in: expected +1.7 (device -1.7), got {}",
            shot0.pitch_in
        );
        assert!(
            (shot0.pitch_out - 1.8).abs() < 0.01,
            "v5 s0 shot0 pitch_out: expected +1.8 (device -1.8), got {}",
            shot0.pitch_out
        );
    }

    /// `compute_section_hash` is taken over the device's own bytes, *before*
    /// `parse_section` flips pitch into the +down frame.  That ordering is what
    /// lets a project imported before issue #296 still recognise a re-imported
    /// section as the same content, so the hash must not have moved.  The
    /// expected digest is pinned from a real fixture parse: reordering the flip
    /// ahead of the hash changes it and fails here.
    #[test]
    fn section_hash_is_unchanged_by_the_pitch_normalisation() {
        let test_file =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../e2e/fixtures/test_v5.dmp");
        let test_file = require_file(test_file);

        let dmp = parse_dmp(&test_file).expect("should parse v5 fixture");
        let s0 = &dmp.sections[0];
        assert_eq!(s0.name, "BAS", "First section name");
        assert_eq!(
            s0.content_hash, "b97987346709cd1089186517af06997c817fbaed0625259f5e12609b8bb66a89",
            "content_hash moved; is compute_section_hash still running before the pitch flip?"
        );

        // Re-hashing the shots as they are *now* (already +down) must NOT
        // reproduce the stored hash, which is the whole point: the stored one
        // was taken in the device frame.
        let rehashed = compute_section_hash(&s0.name, s0.direction, &s0.shots);
        assert_ne!(
            rehashed, s0.content_hash,
            "fixture has no non-zero pitch, so this test cannot detect the ordering"
        );
    }

    /// Cross-validate against known values from pytool_mnemo_lib's test_v2.dmp fixture.
    /// V2 format has many shots with negative pitch.
    #[test]
    fn test_parse_v2_fixture_pitch_values() {
        let test_file =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../e2e/fixtures/test_v2.dmp");
        let test_file = require_file(test_file);

        let dmp = parse_dmp(&test_file).expect("should parse v2 fixture");

        let s0 = &dmp.sections[0];
        assert_eq!(s0.name, "BAS", "First section name");

        // V2 section 0, shot 0: file bytes for pitch_in are 0;-123 and pitch_out is 0;14
        // Python: (0*256)+(-123&0xFF) = 133, (0*256)+(14&0xFF) = 14
        // Device wrote +13.3° / +1.4° (+up); parser normalises to -13.3° / -1.4°.
        let shot0 = &s0.shots[0];
        assert!(
            (shot0.pitch_in - (-13.3)).abs() < 0.01,
            "v2 s0 shot0 pitch_in: expected -13.3 (device +13.3), got {}",
            shot0.pitch_in
        );
        assert!(
            (shot0.pitch_out - (-1.4)).abs() < 0.01,
            "v2 s0 shot0 pitch_out: expected -1.4 (device +1.4), got {}",
            shot0.pitch_out
        );

        // Find a shot the device recorded with negative pitch in the v2 fixture.
        // Shot 2 has pitch bytes: 0;-125 and -1;-27
        // Python: (0*256)+(-125&0xFF) = 131, (-1*256)+(-27&0xFF) = -27
        // Device wrote +13.1° / -2.7° (+up); parser normalises to -13.1° / +2.7°.
        if s0.shots.len() > 2 {
            let shot2 = &s0.shots[2];
            assert!(
                (shot2.pitch_out - 2.7).abs() < 0.01,
                "v2 s0 shot2 pitch_out: expected +2.7 (device -2.7), got {}",
                shot2.pitch_out
            );
        }
    }

    /// Verify that pitch values from DMP files always fall within physical limits.
    /// Any value outside [-90, +90] indicates a parsing error.
    #[test]
    fn test_fixture_pitch_in_physical_range() {
        for fixture in &["test_v2.dmp", "test_v5.dmp"] {
            let test_file = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../e2e/fixtures")
                .join(fixture);
            let test_file = require_file(test_file);

            let dmp = parse_dmp(&test_file).expect("should parse fixture");
            for (si, section) in dmp.sections.iter().enumerate() {
                for (shi, shot) in section.shots.iter().enumerate() {
                    if shot.shot_type != DmpShotType::Standard {
                        continue;
                    }
                    assert!(
                        (-90.0..=90.0).contains(&shot.pitch_in),
                        "{fixture} section {si} shot {shi}: pitch_in={} out of [-90,90] range",
                        shot.pitch_in,
                    );
                    assert!(
                        (-90.0..=90.0).contains(&shot.pitch_out),
                        "{fixture} section {si} shot {shi}: pitch_out={} out of [-90,90] range",
                        shot.pitch_out,
                    );
                }
            }
        }
    }

    /// DMP files use signed decimal values on disk (e.g. "-1;-17;").
    /// Verify that the initial parse step `i32 -> u8` correctly converts
    /// negative file values to unsigned bytes for the buffer.
    #[test]
    fn test_signed_file_values_produce_correct_bytes() {
        // Simulate what parse_dmp does: parse semicolons, cast to u8, then read_int16_be.
        // File content for pitch=-17: signed bytes are -1 and -17.
        let file_content = "-1;-17";
        let data: Vec<u8> = file_content
            .split(';')
            .filter(|s| !s.is_empty())
            .map(|s| s.trim().parse::<i32>().unwrap() as u8)
            .collect();
        assert_eq!(data, vec![255, 239], "signed -1,-17 → unsigned 255,239");

        let mut buffer = IntBuffer::new(data);
        assert_eq!(
            buffer.read_int16_be().unwrap(),
            -17,
            "should reconstruct -17"
        );
    }

    // ========================================================================
    // #576 B13: what the parser could not read is reported, not dropped
    // ========================================================================

    /// A v5 section header: version, section magic, 2025-01-15 09:00, name,
    /// direction In.
    fn v5_header(name: &[u8; 3]) -> Vec<u8> {
        let mut bytes = vec![5, 68, 89, 101, 25, 1, 15, 9, 0];
        bytes.extend_from_slice(name);
        bytes.push(0);
        bytes
    }

    /// One 35-byte v5 record as the device lays it out: start magic, type,
    /// seven core i16s, LRUD, temperature, h/m/s, marker, end magic.
    fn v5_record(shot_type: u8, heading_tenths: i16, length_cm: i16) -> Vec<u8> {
        let mut bytes = SHOT_START_MAGIC.to_vec();
        bytes.push(shot_type);
        for value in [
            heading_tenths,
            heading_tenths,
            length_cm,
            500,
            520,
            -30,
            -30,
        ] {
            bytes.extend_from_slice(&value.to_be_bytes());
        }
        for value in [100i16, 120, 80, 60, 245] {
            bytes.extend_from_slice(&value.to_be_bytes());
        }
        bytes.extend_from_slice(&[10, 30, 0, 1]);
        bytes.extend_from_slice(&SHOT_END_MAGIC);
        assert_eq!(bytes.len(), shot_size(5));
        bytes
    }

    fn v5_shot(heading_tenths: i16) -> Vec<u8> {
        v5_record(2, heading_tenths, 500)
    }

    /// The normal v5 end-of-section record.
    fn v5_end_marker() -> Vec<u8> {
        let mut bytes = vec![57, 67, 77, 3];
        bytes.extend(vec![0u8; 28]);
        bytes.extend_from_slice(&[95, 25, 35]);
        bytes
    }

    /// Write `bytes` as a `.dmp` (semicolon-separated integers) and parse it.
    fn parse_bytes(bytes: &[u8]) -> Result<DmpFile> {
        let content = bytes
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(";");
        let (_file, path) = create_dmp_file(&content);
        let result = parse_dmp(&path);
        std::fs::remove_file(&path).ok();
        result
    }

    fn standard_counts(dmp: &DmpFile) -> Vec<(String, usize)> {
        dmp.sections
            .iter()
            .map(|s| {
                let standard = s
                    .shots
                    .iter()
                    .filter(|shot| shot.shot_type == DmpShotType::Standard)
                    .count();
                (s.name.clone(), standard)
            })
            .collect()
    }

    /// Section AAA is whole; section BBB has three shots and then the dump
    /// stops inside its fourth record - no end-of-section marker after it.
    fn cut_short_dump() -> Vec<u8> {
        let mut bytes = v5_header(b"AAA");
        bytes.extend(v5_shot(900));
        bytes.extend(v5_shot(910));
        bytes.extend(v5_end_marker());
        bytes.extend(v5_header(b"BBB"));
        bytes.extend(v5_shot(1800));
        bytes.extend(v5_shot(1810));
        bytes.extend(v5_shot(1820));
        bytes.extend(v5_shot(1830).iter().take(20));
        bytes
    }

    /// Section AAA: two shots, then a record whose shot type (9) does not
    /// exist, then one more shot, then the end marker.
    fn damaged_record_dump() -> Vec<u8> {
        let mut bytes = v5_header(b"AAA");
        bytes.extend(v5_shot(900));
        bytes.extend(v5_shot(910));
        bytes.extend(v5_record(9, 920, 500));
        bytes.extend(v5_shot(930));
        bytes.extend(v5_end_marker());
        bytes
    }

    /// Three sections; the second's header says month 13.
    fn unreadable_section_dump() -> Vec<u8> {
        let mut bytes = Vec::new();
        for (name, month) in [(b"AAA", 1u8), (b"BBB", 13), (b"CCC", 1)] {
            let mut header = v5_header(name);
            header[5] = month;
            bytes.extend(header);
            bytes.extend(v5_shot(900));
            bytes.extend(v5_end_marker());
        }
        bytes
    }

    /// A dump cut short inside its last section: the section is read up to its
    /// last whole record and imported, and the file says so. A v5 record
    /// carries start and end magic, so a record torn mid-way (or whole-length
    /// but with a broken end magic) is never read as a shot.
    #[test]
    fn a_last_section_without_an_end_marker_is_read_up_to_its_last_whole_record() {
        let dmp = parse_bytes(&cut_short_dump()).unwrap();
        assert_eq!(
            standard_counts(&dmp),
            vec![("AAA".to_string(), 2), ("BBB".to_string(), 3)],
            "the unterminated BBB is read up to its last whole record"
        );
        assert_eq!(dmp.warnings.len(), 1, "warnings: {:?}", dmp.warnings);
        let warning = &dmp.warnings[0];
        assert!(
            warning.contains("\"BBB\"")
                && warning.contains("no end-of-section marker")
                && warning.contains("3 shots"),
            "the warning names the section, the missing marker and what was read: {warning}"
        );

        // Same dump, but the fourth record is whole and its end magic is broken.
        let mut bytes = cut_short_dump();
        let torn_len = 20;
        bytes.truncate(bytes.len() - torn_len);
        let mut broken = v5_shot(1830);
        let last = broken.len() - 1;
        broken[last] = 0;
        bytes.extend(broken);
        let dmp = parse_bytes(&bytes).unwrap();
        assert_eq!(
            standard_counts(&dmp),
            vec![("AAA".to_string(), 2), ("BBB".to_string(), 3)],
            "a whole record with a broken end magic is not a shot"
        );
        assert_eq!(dmp.warnings.len(), 1, "warnings: {:?}", dmp.warnings);
    }

    /// A dump whose only section is unterminated still parses, with the warning.
    #[test]
    fn a_dump_that_is_one_unterminated_section_still_parses() {
        let mut bytes = v5_header(b"BBB");
        bytes.extend(v5_shot(1800));
        bytes.extend(v5_shot(1810));
        let dmp = parse_bytes(&bytes).unwrap();
        assert_eq!(standard_counts(&dmp), vec![("BBB".to_string(), 2)]);
        assert_eq!(dmp.warnings.len(), 1, "warnings: {:?}", dmp.warnings);
    }

    /// Reading stops at a damaged record - skipping it would hang every later
    /// shot off the wrong station - and the stop is reported.
    #[test]
    fn a_section_that_stops_at_a_damaged_record_is_reported() {
        let dmp = parse_bytes(&damaged_record_dump()).unwrap();
        assert_eq!(standard_counts(&dmp), vec![("AAA".to_string(), 2)]);
        assert_eq!(dmp.warnings.len(), 1, "warnings: {:?}", dmp.warnings);
        let warning = &dmp.warnings[0];
        assert!(
            warning.contains("\"AAA\"")
                && warning.contains("damaged record")
                && warning.contains("2 shots"),
            "the warning names the section and where it stopped: {warning}"
        );
    }

    /// A section that cannot be read at all is left out and named by position
    /// (its name may be part of what is unreadable).
    #[test]
    fn a_section_that_cannot_be_read_is_reported_by_position() {
        let dmp = parse_bytes(&unreadable_section_dump()).unwrap();
        assert_eq!(
            standard_counts(&dmp),
            vec![("AAA".to_string(), 1), ("CCC".to_string(), 1)]
        );
        assert_eq!(dmp.warnings.len(), 1, "warnings: {:?}", dmp.warnings);
        let warning = &dmp.warnings[0];
        assert!(
            warning.contains("#2") && warning.contains("month"),
            "the warning names the position and the reason: {warning}"
        );
    }

    /// When nothing can be read the parse still fails, and says why.
    #[test]
    fn a_dump_with_no_readable_section_fails_with_the_reasons() {
        let mut bytes = v5_header(b"AAA");
        bytes[5] = 13;
        bytes.extend(v5_shot(900));
        bytes.extend(v5_end_marker());
        let err = parse_bytes(&bytes).unwrap_err().to_string();
        assert!(
            err.contains("No valid sections found") && err.contains("month"),
            "{err}"
        );
    }

    /// The bytes a device writes after its last section - an ASCII `MN2Over`,
    /// or a single zero byte - are not a section and raise nothing.
    #[test]
    fn a_clean_dump_has_no_warnings_whatever_trails_its_last_section() {
        let mut clean = v5_header(b"AAA");
        clean.extend(v5_shot(900));
        clean.extend(v5_end_marker());
        for trailer in [&b""[..], &b"MN2Over"[..], &[0u8][..]] {
            let mut bytes = clean.clone();
            bytes.extend_from_slice(trailer);
            let dmp = parse_bytes(&bytes).unwrap();
            assert_eq!(standard_counts(&dmp), vec![("AAA".to_string(), 1)]);
            assert!(
                dmp.warnings.is_empty(),
                "trailer {trailer:?} raised {:?}",
                dmp.warnings
            );
        }
    }

    /// A file that breaks every section cannot flood the preview: the list is
    /// capped like the import receipt's.
    #[test]
    fn the_warning_list_is_capped() {
        let mut bytes = Vec::new();
        for _ in 0..(MAX_DMP_WARNINGS + 10) {
            bytes.extend(damaged_record_dump());
        }
        let dmp = parse_bytes(&bytes).unwrap();
        assert_eq!(dmp.warnings.len(), MAX_DMP_WARNINGS + 1);
        assert!(
            dmp.warnings[MAX_DMP_WARNINGS].contains("10 more"),
            "{:?}",
            dmp.warnings.last()
        );
    }

    /// #576 B27(g): a FIFO named like a `.dmp` passed the size check (a pipe
    /// reports length 0) and then blocked the read forever. It is refused at
    /// once, by the capped reader every importer shares.
    #[cfg(unix)]
    #[test]
    fn a_fifo_named_like_a_dmp_is_refused_at_once() {
        use crate::parsers::encoding::fifo_test_support::{make_fifo, within_five_seconds};
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("pipe.dmp");
        make_fifo(&fifo);
        let path = fifo.clone();
        let outcome = within_five_seconds(&fifo, move || {
            parse_dmp(&path)
                .map(|d| d.sections.len())
                .map_err(|e| e.to_string())
        });
        println!("parse_dmp(pipe.dmp): {outcome:?}");
        let err = outcome
            .expect("the read must not block on a FIFO")
            .expect_err("a FIFO is not a DMP file");
        assert!(err.contains("not a regular file"), "{err}");
    }
}
