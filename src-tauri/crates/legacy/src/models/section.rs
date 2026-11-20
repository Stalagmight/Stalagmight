//! Sections: named, coloured groupings of lines (#327).
//!
//! A Section is an arbitrary region of a cave the user (or an importer) groups
//! lines into - "Upper level", "Downstream sump". Each line belongs to at most
//! one Section ([`crate::models::Line::section_id`]); a line in none is "ungrouped".
//! A Section holds no survey data of its own, so nothing geometric reads it.
//!
//! Before the rename to Line, "section" was the name of the survey line
//! itself, and the files keep that vocabulary (`sections` is the lines table,
//! `section_id` a line id, see `disk_names`). The Section concept therefore
//! never uses those names on disk: its serde keys are `cave_sections`,
//! `next_cave_section_id` and `cave_section_id`, and its tables
//! `cave_sections` / `cave_section_lines`.

use serde::{Deserialize, Serialize};

/// Identifier of a [`Section`], unique within a project. Allocated from
/// `Project::next_section_id`; a reload derives that counter from the
/// highest saved id, like `next_line_id`.
pub type SectionId = u32;

/// Every valid Section id is below this (2^31), as Station IDs are below
/// [`crate::models::MAX_STATION_ID`]. A stored id at or above it (a crafted file, a
/// corrupt undo record) is re-minted by `Project::normalize_sections`, and a
/// counter past it is brought back; the headroom keeps `next_section_id`
/// arithmetic far from `u32::MAX`, where a saturating add hands one id out
/// twice.
pub const MAX_SECTION_ID: SectionId = 1 << 31;

/// The longest Section name a user may type, in characters. Automatic paths
/// (an importer naming a Section after a file's survey) truncate instead.
pub const MAX_SECTION_NAME_CHARS: usize = 200;

/// A named grouping of lines. The project keeps its Sections in panel order
/// (`Project::sections`); membership lives on the lines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Section {
    pub id: SectionId,
    /// Project-unique, trimmed, never empty.
    pub name: String,
    /// Display colour, `#rgb` or `#rrggbb`.
    pub color: String,
}

/// Is `color` a hex colour (`#rgb` or `#rrggbb`)?
pub fn is_section_color(color: &str) -> bool {
    let Some(hex) = color.strip_prefix('#') else {
        return false;
    };
    matches!(hex.len(), 3 | 6) && hex.chars().all(|c| c.is_ascii_hexdigit())
}

/// The palette automatic Sections take their colours from, in order (the
/// default line palette, so an imported Section looks like the app's own).
pub const SECTION_PALETTE: [&str; 8] = [
    "#4ecdc4", "#ff6b6b", "#95e1d3", "#f38181", "#aa96da", "#fcbad3", "#a8d8ea", "#ffcb77",
];

/// `name` made fit for a Section on automatic paths (an import, a file, an
/// undo record): the characters a typed name is refused for
/// ([`crate::models::project::station_naming::is_forbidden_name_char`]) go - a control
/// character becomes a space, an invisible formatting one is dropped - then
/// it is trimmed and cut to [`MAX_SECTION_NAME_CHARS`].
pub fn clamp_section_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter_map(|c| {
            if c.is_control() {
                Some(' ')
            } else if crate::models::project::station_naming::is_forbidden_name_char(c) {
                None
            } else {
                Some(c)
            }
        })
        .collect();
    cleaned
        .trim()
        .chars()
        .take(MAX_SECTION_NAME_CHARS)
        .collect::<String>()
        .trim_end()
        .to_string()
}
