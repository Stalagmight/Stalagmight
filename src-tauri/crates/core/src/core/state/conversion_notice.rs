//! The conversion notice: what opening an older file changed that a line
//! shows, and the banner's view of it (demo issue I-5). Two routes raise it:
//! a 0.6.0 (SQLite) file converted to version 2 (plan 7.2.10, C4, UR2;
//! review findings UX-1, UX-3), and a version 2 file an earlier build wrote,
//! upgraded to kept and archived stations (plan 2026-11-23 RS 8.2).
//!
//! The banner is one short lead, the frontend's, chosen by
//! [`ConversionNoticeView::source`] ("Converted from the old file format.",
//! "Upgraded from an earlier version 2 file."), then only what applies, each a
//! short clause with its count ([`ConversionNoticeView::changes`]), then, when
//! the file now shows a station under another station's name or hides a
//! comment, the frontend's sentence that Detach Line from Station or Remove
//! Closure brings each back ([`ConversionNoticeView::brings_back`]), then ONE
//! sentence on saving that is true for the file's state ([`ConversionFile`]).
//! The frontend knows when a later save converts the file
//! (`CommandResult::legacy_converted`), so the banner never promises a save
//! that already happened. A long list goes behind a Details disclosure
//! ([`ConversionNoticeView::details`]), grouped by line.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::core::command::MAX_FILE_NAME_LEN;
use crate::models::sanitize_display_text;
use crate::persistence::migrate::{IdentityChanges, MigrationReport, NameChange, Verification};
use crate::persistence::stalx_v2::UpgradeReport;

/// What a converted file's notice says about saving, and where the original
/// is (the banner's last sentence). Also what a save that converted the open
/// project reports (`CommandResult::legacy_converted`: `saved` or
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
    /// A Save As wrote the new format to another file; the older file is
    /// left as it was.
    SavedAsNewFile,
    /// A SpeleoDB revision converted on download: this computer's copy is
    /// already converted, and the original revision is unchanged on SpeleoDB
    /// (Revision history).
    Downloaded,
}

/// Which older file the notice is about (the banner's lead).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../bindings/")]
pub enum ConversionSource {
    /// A 0.6.0 (SQLite) file, converted to version 2 (plan 7.2).
    #[default]
    OldFormat,
    /// A version 2 file an earlier build wrote, upgraded to kept and archived
    /// stations (plan 2026-11-23 RS 8.2).
    EarlierVersion2,
}

/// The conversion notice as the banner shows it (`take_conversion_notice`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../bindings/")]
pub struct ConversionNoticeView {
    /// Which older file was converted: the banner's lead.
    pub source: ConversionSource,
    /// What the conversion changed that a line shows, one short clause each,
    /// worded with its count ("1 station moved by 0.40 m", "3 stations moved
    /// by up to 2.50 m", "Side / 0 is now shown as Main / 12", "Side / 2 is
    /// now shown as 2a", "the comment of Side / 0 is hidden"), in this order:
    /// moves, depth changes, stations not placed, then what lines show
    /// differently, by line. Never empty.
    pub changes: Vec<String>,
    /// Every station shown differently, every hidden comment and every loop
    /// closure that keeps both names, grouped by line, when there are more
    /// than `changes` spells out (it then counts them), for a Details
    /// disclosure; empty otherwise.
    pub details: Vec<String>,
    /// Whether the banner says that Detach Line from Station or Remove
    /// Closure brings back each station now shown under another station's
    /// name and each hidden comment.
    pub brings_back: bool,
    /// Whether the new format is on disk yet, and where the original is.
    pub file: ConversionFile,
}

/// The notice an upgrade shows when it changed what the user sees (plan
/// 7.2.10, G4, C4; RS 8.2 and 8.3): stations moved in plan (a torn legacy
/// merge collapsed, or an unexplained raw difference), stations whose depth
/// changed (a snap's derived alignment changed a depth tie, or an
/// unexplained one), stations the conversion could not place, or what a line
/// shows (a junction's kept station, a loop closure's kept end, a letter, a
/// hidden comment, a closure left showing both names). Computed View
/// differences never raise it (C4).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ConversionNotice {
    pub(crate) source: ConversionSource,
    pub(crate) slots_moved: usize,
    pub(crate) max_displacement_m: f64,
    /// Slots whose depth changed beyond the 1e-9 m tolerance, and by how much
    /// at most (review finding D-migration-upgrade-6).
    pub(crate) depth_changed: usize,
    pub(crate) max_depth_change_m: f64,
    /// Slots with no finite migrated position.
    pub(crate) unplaced: usize,
    pub(crate) unexplained: usize,
    /// What lines show differently (survey content: shown, never logged).
    pub(crate) changes: IdentityChanges,
    /// Where the unconverted original is, which the notice tells the user.
    pub(crate) original: ConversionOriginal,
}

/// Where the original of a converted file is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ConversionOriginal {
    /// A local file: converted in memory, written by its first save, which
    /// keeps the original in the backups folder (Recent Projects: Restore an
    /// earlier copy).
    #[default]
    LocalFile,
    /// A SpeleoDB revision downloaded into the cache: the cache copy is
    /// already converted when it opens, and no local backup is made; the
    /// original is the revision itself, unchanged on the server (Revision
    /// history).
    SpeleoDbRevision,
}

/// How many entries of what lines show differently the banner lists in its
/// sentence; more are counted there and listed behind Details.
pub(crate) const NAMES_INLINE: usize = 3;

/// The clause of a notice whose only difference is an unexplained one.
const SOMETHING_CHANGED: &str = "some stations are placed differently than in the old version";

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

/// File text, bounded and stripped of bidi controls like every receipt
/// string.
fn text(s: &str) -> String {
    sanitize_display_text(s, MAX_FILE_NAME_LEN)
}

/// "Side / 0 is now shown as Main / 12" (the slot shows another station, the
/// line it belongs to named first), "Side / 2 is now shown as 2a" (a letter):
/// the line as the converted project names it, the name the file showed
/// there, the name it shows now.
fn rename_clause(change: &NameChange) -> String {
    let to = match (&change.kept_line, change.lettered) {
        (Some(line), false) => format!("{} / {}", text(line), text(&change.to)),
        _ => text(&change.to),
    };
    format!(
        "{} / {} is now shown as {to}",
        text(&change.line),
        text(&change.from)
    )
}

/// Every entry of `changes` worded, grouped by line (its names, then its
/// hidden comments), then the closures that keep both names.
fn identity_entries(changes: &IdentityChanges) -> Vec<String> {
    let mut out = Vec::new();
    for line in &changes.lines {
        out.extend(
            changes
                .names
                .iter()
                .filter(|c| &c.line == line)
                .map(rename_clause),
        );
        out.extend(
            changes
                .comments_hidden
                .iter()
                .filter(|c| &c.line == line)
                .map(|c| {
                    format!(
                        "the comment of {} / {} is hidden",
                        text(&c.line),
                        text(&c.station)
                    )
                }),
        );
    }
    out.extend(changes.closures_keep_both.iter().map(|c| {
        format!(
            "the loop closure between {} and {} keeps both names",
            text(&c.a),
            text(&c.b)
        )
    }));
    out
}

impl ConversionNotice {
    /// [`Self::of`] plus what a 0.6.0 file's lines show differently (RS 8.3).
    /// A notice also when nothing moved but something visible changed
    /// (review finding UX-1, U-P1-2).
    pub(crate) fn of_migration(
        report: &MigrationReport,
        changes: &IdentityChanges,
    ) -> Option<Self> {
        match Self::of(&report.verification) {
            Some(notice) => Some(Self {
                changes: changes.clone(),
                ..notice
            }),
            None => (!changes.is_empty()).then(|| Self {
                changes: changes.clone(),
                ..Self::nothing(ConversionSource::OldFormat)
            }),
        }
    }

    /// The notice of an earlier version 2 file's upgrade (RS 8.2): what its
    /// lines show differently, if anything. Nothing moves.
    pub(crate) fn of_upgrade(report: &UpgradeReport) -> Option<Self> {
        (!report.changes.is_empty()).then(|| Self {
            changes: report.changes.clone(),
            ..Self::nothing(ConversionSource::EarlierVersion2)
        })
    }

    /// A notice with nothing to say yet.
    fn nothing(source: ConversionSource) -> Self {
        Self {
            source,
            slots_moved: 0,
            max_displacement_m: 0.0,
            depth_changed: 0,
            max_depth_change_m: 0.0,
            unplaced: 0,
            unexplained: 0,
            changes: IdentityChanges::default(),
            original: ConversionOriginal::LocalFile,
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
            ..Self::nothing(ConversionSource::OldFormat)
        };
        (notice.slots_moved > 0
            || notice.depth_changed > 0
            || notice.unplaced > 0
            || notice.unexplained > 0)
            .then_some(notice)
    }

    /// What a source an Add .stalx or a Merge Projects read was converted
    /// with, counted and named nowhere (RS 8.2, 8.3): the copy renumbers its
    /// lines and Station IDs, so the banner's "Side / 0" would not match
    /// what the user then sees. The banner's order; never empty.
    pub(crate) fn counts(&self) -> Vec<String> {
        let mut out = self.geometry_clauses();
        out.extend(self.identity_counts());
        if out.is_empty() {
            out.push(SOMETHING_CHANGED.to_owned());
        }
        out
    }

    /// The moves, depth changes and stations not placed, one clause each.
    fn geometry_clauses(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.slots_moved > 0 {
            out.push(format!(
                "{} moved {}",
                count(self.slots_moved, "station", "stations"),
                by(self.slots_moved, self.max_displacement_m)
            ));
        }
        if self.depth_changed > 0 {
            out.push(format!(
                "{} changed depth {}",
                count(self.depth_changed, "station", "stations"),
                by(self.depth_changed, self.max_depth_change_m)
            ));
        }
        if self.unplaced > 0 {
            out.push(format!(
                "{} could not be placed",
                count(self.unplaced, "station", "stations")
            ));
        }
        out
    }

    /// What lines show differently, counted by kind.
    fn identity_counts(&self) -> Vec<String> {
        let identity = &self.changes;
        [
            (
                identity.shown_as_other(),
                "station is now shown under another station's name",
                "stations are now shown under another station's name",
            ),
            (
                identity.lettered(),
                "station now has a letter suffix",
                "stations now have a letter suffix",
            ),
            (
                identity.comments_hidden.len(),
                "station's comment is now hidden",
                "stations' comments are now hidden",
            ),
            (
                identity.closures_keep_both.len(),
                "loop closure keeps both names",
                "loop closures keep both names",
            ),
        ]
        .into_iter()
        .filter(|(n, _, _)| *n > 0)
        .map(|(n, one, many)| count(n, one, many))
        .collect()
    }

    /// The banner's view. `saved`: the project file is already in the new
    /// format (a closed file's Recents rename or link strip upgraded it
    /// before this open); ignored for a downloaded revision.
    pub(crate) fn view(&self, saved: bool) -> ConversionNoticeView {
        let mut changes = self.geometry_clauses();
        let entries = identity_entries(&self.changes);
        let details = if entries.len() > NAMES_INLINE {
            changes.extend(self.identity_counts());
            entries
        } else {
            changes.extend(entries);
            Vec::new()
        };
        if changes.is_empty() {
            // `of`, `of_migration` and `of_upgrade` raise no notice without
            // one of the counts above; never "0 stations moved" all the same.
            changes.push(SOMETHING_CHANGED.to_owned());
        }
        let identity = &self.changes;
        let file = match (self.original, saved) {
            (ConversionOriginal::SpeleoDbRevision, _) => ConversionFile::Downloaded,
            (ConversionOriginal::LocalFile, false) => ConversionFile::NotYetSaved,
            (ConversionOriginal::LocalFile, true) => ConversionFile::Saved,
        };
        ConversionNoticeView {
            source: self.source,
            changes,
            details,
            brings_back: identity.shown_as_other() > 0 || !identity.comments_hidden.is_empty(),
            file,
        }
    }
}
