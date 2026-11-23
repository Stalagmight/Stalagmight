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

use crate::{LineId, StationRef};

use super::Project;

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
        let Some(name) = self.slot_name(slot).map(str::to_string) else {
            return slot.station_index.to_string();
        };
        match self.shows(slot) {
            Some(Shows::Foreign(home)) => format!("{name} ({})", self.line_label_of(home)),
            Some(Shows::LoopEnd(_)) => {
                format!("{name} (loop end on {})", self.line_label_of(slot.line_id))
            }
            Some(Shows::Native) | None => name,
        }
    }

    /// `slot` with its line, as plain text: `Branch / 12`, a foreign slot
    /// `Branch / 12 (Main)`, a loop end named on its shown station's home
    /// line, `Main / 12 (loop end on Branch)` (module docs). `None` for a
    /// slot that does not exist.
    pub fn slot_text(&self, slot: StationRef) -> Option<String> {
        let name = self.slot_name(slot)?.to_string();
        let line = self.line_label_of(slot.line_id);
        Some(match self.shows(slot)? {
            Shows::Native => format!("{line} / {name}"),
            Shows::Foreign(home) => format!("{line} / {name} ({})", self.line_label_of(home)),
            Shows::LoopEnd(home) => format!(
                "{} / {name} (loop end on {line})",
                self.line_label_of(home.unwrap_or(slot.line_id))
            ),
        })
    }

    /// The shot from chain index `index` of `line` as plain text: `Branch:
    /// 12 (Main) \u{2192} 13`, each end by [`Self::slot_name_text`] (a chain
    /// index stands in for a station the line does not reach).
    pub fn shot_text(&self, line: LineId, index: usize) -> String {
        let end = |i: usize| {
            u32::try_from(i).map_or_else(
                |_| i.to_string(),
                |i| self.slot_name_text(StationRef::new(line, i)),
            )
        };
        format!(
            "{}: {} \u{2192} {}",
            self.line_label_of(line),
            end(index),
            end(index + 1)
        )
    }
}
