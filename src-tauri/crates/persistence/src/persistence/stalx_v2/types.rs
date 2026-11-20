//! The in-memory side of the format: what a file holds beside the survey
//! model ([`Extras`]), the serializer [`Mode`]s, decode options and limits,
//! and what a decode returns ([`Decoded`]).

use std::collections::BTreeMap;
use std::fmt;

use chrono::{DateTime, Utc};

use crate::models::survey::Project;
use crate::models::UndoMode;
use crate::persistence::undo_limits::MAX_PERSISTED_HISTORY_ROWS;

use super::canon::canonical;
use super::{DEFAULT_GENERATOR, PUSH_BYTE_CAP};

/// A JSON value in canonical text: no insignificant whitespace, strings
/// escaped the format's way, numbers and key order as written. Holds
/// preserved unknown content and opaque history entries.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RawJson(String);

impl RawJson {
    /// Canonicalise `text`, which must be one well-formed JSON value at most
    /// [`super::MAX_DEPTH`] deep.
    pub fn parse(text: &str) -> Result<Self, String> {
        let value: &serde_json::value::RawValue =
            serde_json::from_str(text).map_err(|e| e.to_string())?;
        canonical(value.get()).map(Self)
    }

    /// The canonical text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Wrap text already in canonical form (the canonicaliser's output).
    pub(super) fn from_canonical(text: String) -> Self {
        Self(text)
    }
}

impl fmt::Display for RawJson {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The shared SpeleoDB link identity (the `speleodb` member). It rides every
/// push; the per-machine sync state is in [`LocalSync`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeleoDbLink {
    pub endpoint: String,
    pub project_id: String,
    pub project_name: String,
    /// Whether undo history rides pushes (#303). `None` is the legacy
    /// "not yet initialised" value. A project property, so it is shared.
    pub undo_mode: Option<UndoMode>,
}

/// Per-machine SpeleoDB sync state (`local.speleodb`). Never pushed, never in
/// a detached copy.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalSync {
    /// The server revision this copy is based on; `None` is an empty repo.
    pub base_commit: Option<String>,
    /// Set by the first accepted edit; cleared only by a confirmed push.
    pub unpushed_edits: bool,
    pub last_edit_at: Option<DateTime<Utc>>,
    pub last_verify_at: Option<DateTime<Utc>>,
    /// The revision an Unwind walked back from (the chain pointer).
    pub unwound_from: Option<String>,
    /// The inherited-history watermark: undo entries at or before this instant
    /// came with a pulled revision.
    pub inherited_through: Option<DateTime<Utc>>,
}

impl LocalSync {
    /// Whether every field is unset (the member is then omitted).
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// The `local` member: per-machine state. Lenient on read (tier 3).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalState {
    /// Display settings (view toggles), key to stored text.
    pub display: BTreeMap<String, String>,
    pub speleodb: LocalSync,
}

impl LocalState {
    pub fn is_empty(&self) -> bool {
        self.display.is_empty() && self.speleodb.is_empty()
    }
}

/// One ignored warning, keyed by the shot's persistent uuid.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct IgnoredWarning {
    pub shot: String,
    pub code: String,
    pub at: DateTime<Utc>,
    pub comment: Option<String>,
}

/// The persisted undo history. Entries are opaque to the format (stream U
/// defines them) beyond two rules: each is one JSON value, and an undo entry
/// that carries a `"t"` member holds its RFC 3339 time there (the watermark
/// cut reads it). Each list is a stack: its last element is the next step.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct History {
    pub undo: Vec<RawJson>,
    pub redo: Vec<RawJson>,
}

impl History {
    pub fn is_empty(&self) -> bool {
        self.undo.is_empty() && self.redo.is_empty()
    }
}

/// Unknown content a newer writer added where this build keeps it verbatim
/// (tier 2): `(key, value)` pairs in file order, written after the known keys.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Preserved {
    /// Unknown top-level members.
    pub top: Vec<(String, RawJson)>,
    pub project: Vec<(String, RawJson)>,
    pub settings: Vec<(String, RawJson)>,
    /// Unknown keys of `settings.validation`.
    pub validation: Vec<(String, RawJson)>,
    pub speleodb: Vec<(String, RawJson)>,
    pub summary: Vec<(String, RawJson)>,
}

impl Preserved {
    pub fn is_empty(&self) -> bool {
        self.top.is_empty()
            && self.project.is_empty()
            && self.settings.is_empty()
            && self.validation.is_empty()
            && self.speleodb.is_empty()
            && self.summary.is_empty()
    }
}

/// Why a file opens read-only (plan 6.4). Saving the body is refused; a
/// local-only rewrite ([`super::rewrite_local`]) still works.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReadOnly {
    /// `requires` names this build does not know.
    pub unknown_features: Vec<String>,
    /// Where unknown structural content was found (the first few paths).
    pub unknown_content: Vec<String>,
    /// How many unknown keys or values were found in all.
    pub unknown_content_total: usize,
}

impl fmt::Display for ReadOnly {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("made with a newer Stalagmight")?;
        if !self.unknown_features.is_empty() {
            write!(
                f,
                "; it requires features this version does not know: {}",
                self.unknown_features.join(", ")
            )?;
        }
        if self.unknown_content_total > 0 {
            write!(
                f,
                "; it holds {} unknown survey field(s), e.g. {}",
                self.unknown_content_total,
                self.unknown_content.join(", ")
            )?;
        }
        Ok(())
    }
}

/// Everything a file holds that is not the survey model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extras {
    /// Who wrote the body, e.g. `Stalagmight 0.6.0`. The app sets its own
    /// version before every save; empty writes [`DEFAULT_GENERATOR`].
    pub generator: String,
    /// The file's `requires` list. A save refuses names this build does not
    /// know (the body would lose what they mean).
    pub requires: Vec<String>,
    pub speleodb: Option<SpeleoDbLink>,
    pub local: LocalState,
    /// Survey content: rides every save and push.
    pub ignored_warnings: Vec<IgnoredWarning>,
    pub history: History,
    pub preserved: Preserved,
    /// Set by a decode that met unknown features or structural content. Every
    /// encode mode refuses while it is set.
    pub read_only: Option<ReadOnly>,
}

impl Default for Extras {
    fn default() -> Self {
        Self {
            generator: DEFAULT_GENERATOR.to_string(),
            requires: Vec::new(),
            speleodb: None,
            local: LocalState::default(),
            ignored_warnings: Vec::new(),
            history: History::default(),
            preserved: Preserved::default(),
            read_only: None,
        }
    }
}

/// Options of a push serialization (plan 6.5 mode 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PushOptions {
    /// Keep the undo history (the project's undo mode is Full).
    pub keep_undo: bool,
    /// Undo entries at or before this instant are inherited and cut.
    pub watermark: Option<DateTime<Utc>>,
    /// The whole file must fit this many bytes; the oldest undo entries go
    /// first, then the push is refused ([`super::EncodeError::TooLarge`]).
    pub byte_cap: usize,
}

impl Default for PushOptions {
    fn default() -> Self {
        Self {
            keep_undo: false,
            watermark: None,
            byte_cap: PUSH_BYTE_CAP,
        }
    }
}

/// The serializer modes (plan 6.5). The local-only rewrite is its own
/// function, [`super::rewrite_local`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Everything: survey, link, local state, undo and redo.
    #[default]
    Normal,
    /// For a SpeleoDB push: no `local`, no redo, undo only when kept, cut at
    /// the watermark and then by bytes.
    ForPush(PushOptions),
    /// Save a Copy of a managed project: no `speleodb`, no SpeleoDB keys in
    /// `local`, no undo inherited from the link.
    DetachedCopy,
}

/// Caps on what a file may contain, refused with
/// [`super::DecodeError::TooComplex`]. Checked while parsing, so they also
/// bound the memory a hostile file can make the reader allocate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CountLimits {
    pub stations: usize,
    /// Aliases over all stations.
    pub aliases: usize,
    pub lines: usize,
    pub shots: usize,
    /// Hand moves over all lines.
    pub hand_moves: usize,
    pub datums: usize,
    pub placements: usize,
    pub closures: usize,
    pub sections: usize,
    pub ignored_warnings: usize,
    /// File and line import hashes together.
    pub import_hashes: usize,
    /// People over every credit object.
    pub credit_people: usize,
    /// Undo and redo entries together. A history over the cap is dropped
    /// (tier 3), not refused.
    pub history_entries: usize,
    /// Unknown members and keys preserved verbatim.
    pub preserved: usize,
    /// Display settings in `local` (over the cap: dropped, tier 3).
    pub display_settings: usize,
}

impl Default for CountLimits {
    fn default() -> Self {
        Self {
            stations: 2_100_000,
            aliases: 2_100_000,
            lines: 100_000,
            shots: 2_000_000,
            hand_moves: 2_100_000,
            datums: 2_100_000,
            placements: 100_000,
            closures: 50_000,
            sections: 100_000,
            ignored_warnings: 2_000_000,
            import_hashes: 2_000_000,
            credit_people: 2_000_000,
            history_entries: MAX_PERSISTED_HISTORY_ROWS,
            preserved: 10_000,
            display_settings: 10_000,
        }
    }
}

/// Which [`CountLimits`] cap a file exceeded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit {
    Stations,
    Aliases,
    Lines,
    Shots,
    HandMoves,
    Datums,
    Placements,
    Closures,
    Sections,
    IgnoredWarnings,
    ImportHashes,
    CreditPeople,
    Preserved,
    /// Shots on one line: fixed by the model ([`super::MAX_SHOTS_PER_LINE`]).
    ShotsPerLine,
}

impl Limit {
    pub(super) fn cap(self, limits: &CountLimits) -> usize {
        match self {
            Self::Stations => limits.stations,
            Self::Aliases => limits.aliases,
            Self::Lines => limits.lines,
            Self::Shots => limits.shots,
            Self::HandMoves => limits.hand_moves,
            Self::Datums => limits.datums,
            Self::Placements => limits.placements,
            Self::Closures => limits.closures,
            Self::Sections => limits.sections,
            Self::IgnoredWarnings => limits.ignored_warnings,
            Self::ImportHashes => limits.import_hashes,
            Self::CreditPeople => limits.credit_people,
            Self::Preserved => limits.preserved,
            Self::ShotsPerLine => super::MAX_SHOTS_PER_LINE,
        }
    }

    pub(super) const ALL: [Limit; 14] = [
        Self::Stations,
        Self::Aliases,
        Self::Lines,
        Self::Shots,
        Self::HandMoves,
        Self::Datums,
        Self::Placements,
        Self::Closures,
        Self::Sections,
        Self::IgnoredWarnings,
        Self::ImportHashes,
        Self::CreditPeople,
        Self::Preserved,
        Self::ShotsPerLine,
    ];
}

impl fmt::Display for Limit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Stations => "stations",
            Self::Aliases => "station aliases",
            Self::Lines => "lines",
            Self::Shots => "shots",
            Self::HandMoves => "hand moves",
            Self::Datums => "datums",
            Self::Placements => "placements",
            Self::Closures => "loop closures",
            Self::Sections => "Sections",
            Self::IgnoredWarnings => "ignored warnings",
            Self::ImportHashes => "import hashes",
            Self::CreditPeople => "credited people",
            Self::Preserved => "preserved unknown members",
            Self::ShotsPerLine => "shots on one line",
        })
    }
}

/// Decode options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodeOptions {
    /// Inputs larger than this are refused before parsing.
    pub max_file_bytes: u64,
    pub limits: CountLimits,
}

impl Default for DecodeOptions {
    fn default() -> Self {
        Self {
            max_file_bytes: 256 * 1024 * 1024,
            limits: CountLimits::default(),
        }
    }
}

/// Something a decode noticed and tolerated (tiers 2 and 3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeNote {
    /// The file was written by another solver version; positions are derived
    /// with this build's solver.
    OtherSolver { found: u64 },
    /// Part of `local` was unreadable or unknown and was dropped.
    LocalDropped { what: String },
    /// The history was unreadable and was dropped whole.
    HistoryDropped { reason: String },
    /// Ignored warnings that name no shot of the project, or repeat a
    /// (shot, code) pair, were dropped.
    IgnoresDropped { count: usize },
}

/// A decoded file.
#[derive(Debug, Clone, PartialEq)]
pub struct Decoded {
    /// The survey, exactly as the file holds it (load never mutates).
    pub project: Project,
    pub extras: Extras,
    pub notes: Vec<DecodeNote>,
}

impl Decoded {
    /// Why the project must open read-only, if it must.
    pub fn read_only(&self) -> Option<&ReadOnly> {
        self.extras.read_only.as_ref()
    }
}
