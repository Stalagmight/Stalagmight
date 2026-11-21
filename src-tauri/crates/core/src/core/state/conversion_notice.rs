//! The conversion notice: what opening a 0.6.0 (SQLite) file changed that a
//! line shows (plan 7.2.10, C4, UR2; review findings UX-1, UX-3), and the
//! banner's view of it (demo issue I-5).
//!
//! The banner is one short lead ("Converted from the old file format."),
//! then only what applies, each a short clause with its count
//! ([`ConversionNoticeView::changes`]), then ONE sentence on saving that is
//! true for the file's state ([`ConversionFile`]). The lead and that sentence
//! are the frontend's: it knows when a later save converts the file
//! (`CommandResult::legacy_converted`), so the banner never promises a save
//! that already happened. A long list of renamed stations goes behind a
//! Details disclosure ([`ConversionNoticeView::details`]).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::core::command::MAX_FILE_NAME_LEN;
use crate::models::sanitize_display_text;
use crate::persistence::migrate::{MigrationReport, NameChange, Verification};

/// What a converted 0.6.0 file's notice says about saving, and where the
/// original is (the banner's last sentence). Also what a save that converted
/// the open project reports (`CommandResult::legacy_converted`: `saved` or
/// `saved_as_new_file`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../bindings/")]
pub enum ConversionFile {
    /// Converted in memory only: the next save writes the new format and
    /// keeps the original (Recent Projects, Restore an earlier copy).
    NotYetSaved,
    /// The project file is in the new format, the original kept in the
    /// backups folder: this session's save, or a Recents rename or a SpeleoDB
    /// link strip of the closed file before this open.
    Saved,
    /// A Save As wrote the new format to another file; the 0.6.0 file is
    /// left as it was.
    SavedAsNewFile,
    /// A SpeleoDB revision converted on download: this computer's copy is
    /// already converted, and the original revision is unchanged on SpeleoDB
    /// (Revision history).
    Downloaded,
}

/// The conversion notice as the banner shows it (`take_conversion_notice`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../bindings/")]
pub struct ConversionNoticeView {
    /// What the conversion changed that a line shows, one short clause each,
    /// worded with its count ("1 station moved by 0.40 m", "3 stations moved
    /// by up to 2.50 m", "Side / 2 is now shown as 2a", "a merged
    /// station's two comments were joined"), in this order: moves, depth
    /// changes, stations not placed, names, joined comments. Never empty.
    pub changes: Vec<String>,
    /// Every renamed station ("Side / 2 is now shown as 2a") when
    /// there are more than `changes` spells out (it then says how many), for
    /// a Details disclosure; empty otherwise.
    pub details: Vec<String>,
    /// Whether the new format is on disk yet, and where the original is.
    pub file: ConversionFile,
}

/// The notice an upgrade shows when the migration changed what the user
/// sees (plan 7.2.10, G4, C4): stations moved in plan (a torn legacy merge
/// collapsed, or an unexplained raw difference), stations whose depth changed
/// (a snap's derived alignment changed a depth tie, or an unexplained one),
/// stations the conversion could not place, or a name a line showed (review
/// findings D-migration-upgrade-8, UX-3 and UX-1; since I-3 every line keeps
/// the names 0.6.0 showed on it, a merged station's included, so only a name
/// a line showed twice is lettered). Computed View differences never raise it
/// (C4).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ConversionNotice {
    pub(crate) slots_moved: usize,
    pub(crate) max_displacement_m: f64,
    /// Slots whose depth changed beyond the 1e-9 m tolerance, and by how much
    /// at most (review finding D-migration-upgrade-6).
    pub(crate) depth_changed: usize,
    pub(crate) max_depth_change_m: f64,
    /// Slots with no finite migrated position.
    pub(crate) unplaced: usize,
    pub(crate) unexplained: usize,
    /// The slots whose name changed, in slot order (survey content: shown,
    /// never logged).
    pub(crate) names_changed: Vec<NameChange>,
    /// Merged stations whose members' different comments were joined into
    /// one (each line now shows both).
    pub(crate) comments_joined: usize,
    /// Where the unconverted original is, which the notice tells the user.
    pub(crate) original: ConversionOriginal,
}

/// Where the original of a converted legacy file is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ConversionOriginal {
    /// A local file: converted in memory, written as version 2 by its first
    /// save, which moves the original into the backups folder (Recent
    /// Projects: Restore an earlier copy).
    #[default]
    LocalFile,
    /// A SpeleoDB revision downloaded into the cache: the cache copy is
    /// already version 2 when it opens, and no local backup is made; the
    /// original is the revision itself, unchanged on the server (Revision
    /// history).
    SpeleoDbRevision,
}

/// How many renamed stations the banner lists in its sentence; more go behind
/// Details, the sentence saying how many.
pub(crate) const NAMES_INLINE: usize = 3;

/// `1 {one}` or `{n} {many}`.
fn count(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

/// "by 0.40 m" for one station, "by up to 2.50 m" for several.
fn by(n: usize, metres: f64) -> String {
    if n == 1 {
        format!("by {metres:.2} m")
    } else {
        format!("by up to {metres:.2} m")
    }
}

/// "Side / 2 is now shown as 2a": the line as the converted project
/// names it, the name 0.6.0 showed there, the name it shows now. File text,
/// bounded and stripped of bidi controls like every receipt string.
fn rename_clause(change: &NameChange) -> String {
    let text = |s: &str| sanitize_display_text(s, MAX_FILE_NAME_LEN);
    format!(
        "{} / {} is now shown as {}",
        text(&change.line),
        text(&change.from),
        text(&change.to)
    )
}

impl ConversionNotice {
    /// [`Self::of`] plus what else a line shows differently: the names the
    /// conversion changed and the comments it joined. A notice also when
    /// nothing moved but something visible changed (review finding UX-1).
    pub(crate) fn of_migration(
        report: &MigrationReport,
        names_changed: &[NameChange],
    ) -> Option<Self> {
        let names_changed = names_changed.to_vec();
        let comments_joined = report.comments_joined;
        match Self::of(&report.verification) {
            Some(notice) => Some(Self {
                names_changed,
                comments_joined,
                ..notice
            }),
            None => (!names_changed.is_empty() || comments_joined > 0).then_some(Self {
                slots_moved: 0,
                max_displacement_m: 0.0,
                depth_changed: 0,
                max_depth_change_m: 0.0,
                unplaced: 0,
                unexplained: 0,
                names_changed,
                comments_joined,
                original: ConversionOriginal::LocalFile,
            }),
        }
    }

    /// The same notice for a SpeleoDB revision the cache got converted.
    pub(crate) fn downloaded(self) -> Self {
        Self {
            original: ConversionOriginal::SpeleoDbRevision,
            ..self
        }
    }

    pub(crate) fn of(verification: &Verification) -> Option<Self> {
        let moved = verification.moved();
        let depth = verification.depth_changed();
        let notice = Self {
            slots_moved: moved.slots,
            max_displacement_m: moved.max_displacement_m,
            depth_changed: depth.slots,
            max_depth_change_m: depth.max_displacement_m,
            unplaced: verification.unplaced(),
            unexplained: verification.mismatches.len(),
            names_changed: Vec::new(),
            comments_joined: 0,
            original: ConversionOriginal::LocalFile,
        };
        (notice.slots_moved > 0
            || notice.depth_changed > 0
            || notice.unplaced > 0
            || notice.unexplained > 0)
            .then_some(notice)
    }

    /// The banner's view. `saved`: the project file is already in the new
    /// format (a closed file's Recents rename or link strip upgraded it
    /// before this open); ignored for a downloaded revision.
    pub(crate) fn view(&self, saved: bool) -> ConversionNoticeView {
        let mut changes = Vec::new();
        if self.slots_moved > 0 {
            changes.push(format!(
                "{} moved {}",
                count(self.slots_moved, "station", "stations"),
                by(self.slots_moved, self.max_displacement_m)
            ));
        }
        if self.depth_changed > 0 {
            changes.push(format!(
                "{} changed depth {}",
                count(self.depth_changed, "station", "stations"),
                by(self.depth_changed, self.max_depth_change_m)
            ));
        }
        if self.unplaced > 0 {
            changes.push(format!(
                "{} could not be placed",
                count(self.unplaced, "station", "stations")
            ));
        }
        let renamed: Vec<String> = self.names_changed.iter().map(rename_clause).collect();
        let details = if renamed.len() > NAMES_INLINE {
            changes.push(format!(
                "{} now shown under another name",
                count(renamed.len(), "station is", "stations are")
            ));
            renamed
        } else {
            changes.extend(renamed);
            Vec::new()
        };
        match self.comments_joined {
            0 => {}
            1 => changes.push("a merged station's two comments were joined".to_owned()),
            n => changes.push(format!("the comments of {n} merged stations were joined")),
        }
        if changes.is_empty() {
            // `of` and `of_migration` raise no notice without one of the
            // counts above; never "0 stations moved" all the same.
            changes.push("some stations are placed differently than in the old version".to_owned());
        }
        let file = match (self.original, saved) {
            (ConversionOriginal::SpeleoDbRevision, _) => ConversionFile::Downloaded,
            (ConversionOriginal::LocalFile, false) => ConversionFile::NotYetSaved,
            (ConversionOriginal::LocalFile, true) => ConversionFile::Saved,
        };
        ConversionNoticeView {
            changes,
            details,
            file,
        }
    }
}
