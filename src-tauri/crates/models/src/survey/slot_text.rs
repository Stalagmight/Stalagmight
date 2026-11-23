//! A slot's station as plain text (RS 6, RS-FX5): the one shared wording
//! for every text that names a slot without the dot or the loop mark the
//! app draws beside a pill (warnings, refusals, history rows, notices,
//! receipts, closure texts).
//!
//! Since the user's decision "dotted slots don't collide" a line may show
//! one name twice: Branch shows Main's 12 at its start (a foreign slot, with
//! Main's dot) and its own 12 further on. Plain text names the dotted one
//! with the station's home line, and a loop end with the line it sits on:
//!
//! | slot | name | with its line |
//! |---|---|---|
//! | native | `12` | `Branch / 12` |
//! | foreign | `12 (Main)` | `Branch / 12 (Main)` |
//! | loop end | `12 (loop end on Branch)` | `Main / 12 (loop end on Branch)` |
//!
//! and a shot reads `Branch: 12 (Main) \u{2192} 13`. The frontend's
//! `stationLabelOf` and `shotLabelAt` (`src/utils/stationIdentity.ts`,
//! i18n `identity.foreignLabel` and `identity.loopEndLabel`) word it the
//! same, and the Warnings tab strips a warning's `Shot <label> ` prefix by
//! that equality.

use std::collections::HashMap;

use crate::{LineId, StationId, StationRef};

use super::{Project, RecordIndex, ShownIndex};

/// How a slot shows its node, for its plain text.
enum Shows {
    Native,
    /// The shown station's home line.
    Foreign(LineId),
    /// A loop end: the shown station's home line, when it has one.
    LoopEnd(Option<LineId>),
}

impl Project {
    fn shows(&self, slot: StationRef) -> Option<Shows> {
        let node = self.station_at(slot)?;
        if self.is_closure_archived(node) {
            return Some(Shows::LoopEnd(self.home_line(self.shown(node))));
        }
        if self.record_naming(node, slot.line_id).is_some() {
            if let Some(home) = self.home_line(node).filter(|&h| h != slot.line_id) {
                return Some(Shows::Foreign(home));
            }
        }
        Some(Shows::Native)
    }

    /// The name `slot` shows, as plain text: `12`, a foreign slot `12
    /// (Main)`, a loop end `12 (loop end on Branch)` (module docs). A slot
    /// that does not exist reads as its chain index.
    pub fn slot_name_text(&self, slot: StationRef) -> String {
        name_text(self, slot, self.shows(slot))
    }

    /// `slot` with its line, as plain text: `Branch / 12`, a foreign slot
    /// `Branch / 12 (Main)`, a loop end named on its shown station's home
    /// line, `Main / 12 (loop end on Branch)` (module docs). `None` for a
    /// slot that does not exist.
    pub fn slot_text(&self, slot: StationRef) -> Option<String> {
        full_text(self, slot, self.shows(slot)?)
    }

    /// The shot from chain index `index` of `line` as plain text: `Branch:
    /// 12 (Main) \u{2192} 13`, each end by [`Self::slot_name_text`] (a chain
    /// index stands in for a station the line does not reach).
    pub fn shot_text(&self, line: LineId, index: usize) -> String {
        shot_text_by(self, line, index, |slot| self.slot_name_text(slot))
    }
}

/// [`Project::slot_name_text`] of `slot`, which `shows` says how it shows.
fn name_text(project: &Project, slot: StationRef, shows: Option<Shows>) -> String {
    let Some(name) = project.slot_name(slot).map(str::to_string) else {
        return slot.station_index.to_string();
    };
    match shows {
        Some(Shows::Foreign(home)) => format!("{name} ({})", project.line_label_of(home)),
        Some(Shows::LoopEnd(_)) => {
            format!(
                "{name} (loop end on {})",
                project.line_label_of(slot.line_id)
            )
        }
        Some(Shows::Native) | None => name,
    }
}

/// [`Project::slot_text`] of `slot`, which `shows` says how it shows.
fn full_text(project: &Project, slot: StationRef, shows: Shows) -> Option<String> {
    let name = project.slot_name(slot)?.to_string();
    let line = project.line_label_of(slot.line_id);
    Some(match shows {
        Shows::Native => format!("{line} / {name}"),
        Shows::Foreign(home) => format!("{line} / {name} ({})", project.line_label_of(home)),
        Shows::LoopEnd(home) => format!(
            "{} / {name} (loop end on {line})",
            project.line_label_of(home.unwrap_or(slot.line_id))
        ),
    })
}

/// [`Project::shot_text`], each end named by `end`.
fn shot_text_by(
    project: &Project,
    line: LineId,
    index: usize,
    mut end: impl FnMut(StationRef) -> String,
) -> String {
    let mut at = |i: usize| {
        u32::try_from(i).map_or_else(|_| i.to_string(), |i| end(StationRef::new(line, i)))
    };
    let (from, to) = (at(index), at(index + 1));
    format!("{}: {from} \u{2192} {to}", project.line_label_of(line))
}

/// The plain texts of [`Project`] for a pass naming many slots (every
/// warning of a project, the conversion notice): the closure archives, the
/// record naming each slot's line and each shown identity's home line are
/// worked out once for the pass, not per slot. Per slot they cost the
/// closures, the node's records and its lines each time, so a hub of many
/// lines and records named on each of its lines cost lines x records (pre-PR
/// review, RS-FX9). Same words as the per-slot forms.
pub struct SlotTexts<'p> {
    project: &'p Project,
    shown: ShownIndex,
    records: RecordIndex,
    homes: HashMap<StationId, Option<LineId>>,
}

impl<'p> SlotTexts<'p> {
    /// The texts of `project`.
    pub fn new(project: &'p Project) -> Self {
        Self {
            project,
            shown: project.shown_index(),
            records: RecordIndex::of(project),
            homes: HashMap::new(),
        }
    }

    fn home_line(&mut self, identity: StationId) -> Option<LineId> {
        let (project, records) = (self.project, &self.records);
        *self
            .homes
            .entry(identity)
            .or_insert_with(|| records.native_lines(project, identity).first().copied())
    }

    fn shows(&mut self, slot: StationRef) -> Option<Shows> {
        let node = self.project.station_at(slot)?;
        if self.shown.is_archived(node) {
            let shown = self.shown.shown(node);
            return Some(Shows::LoopEnd(self.home_line(shown)));
        }
        if self.records.names(node, slot.line_id) {
            if let Some(home) = self.home_line(node).filter(|&h| h != slot.line_id) {
                return Some(Shows::Foreign(home));
            }
        }
        Some(Shows::Native)
    }

    /// [`Project::slot_name_text`].
    pub fn slot_name_text(&mut self, slot: StationRef) -> String {
        let shows = self.shows(slot);
        name_text(self.project, slot, shows)
    }

    /// [`Project::slot_text`].
    pub fn slot_text(&mut self, slot: StationRef) -> Option<String> {
        let shows = self.shows(slot)?;
        full_text(self.project, slot, shows)
    }

    /// [`Project::shot_text`].
    pub fn shot_text(&mut self, line: LineId, index: usize) -> String {
        let project = self.project;
        shot_text_by(project, line, index, |slot| self.slot_name_text(slot))
    }

    /// [`Project::station_text`].
    pub fn station_text(&mut self, station: StationId, line: Option<LineId>) -> String {
        let project = self.project;
        let topology = project.topology();
        let slot = line
            .and_then(|l| topology.slot_on(station, l))
            .or_else(|| topology.first_slot(station));
        match (
            slot.and_then(|s| self.slot_text(s)),
            project.stations.get(&self.shown.shown(station)),
        ) {
            (Some(text), _) => text,
            (None, Some(s)) => s.name.clone(),
            (None, None) => "a station".to_string(),
        }
    }
}
