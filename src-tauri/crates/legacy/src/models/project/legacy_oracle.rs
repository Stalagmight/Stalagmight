//! NOT part of the frozen 0.6.0 code: the read-only accessors the legacy
//! crate's API (`crate::open_path`) needs that the app never exposed. They are
//! built only from the frozen rules, so they report exactly what the frozen
//! solver does.

use std::collections::HashSet;

use crate::models::{LineId, StationRef};

use super::Project;

/// What holds a connected component of lines in place in the 0.6.0 solver.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ComponentHold {
    /// The component's first placement zero datum (list order) pins it to its
    /// GPS position.
    PlacementDatum,
    /// No placement datum: an edit recorded a held gauge for the component
    /// (`Project::held_gauges`), and the solver holds that station.
    HeldGauge,
    /// No placement datum and no held gauge: the default gauge of the
    /// component's lowest-id line (`Project::gauge_index`).
    DefaultGauge,
}

/// One connected component (lines joined by merges and closures, as
/// `get_connected_line_ids` walks them: the 0.6.0 least-squares component)
/// and the station the solve held fixed for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentAnchor {
    /// Every line of the component, ascending.
    pub line_ids: Vec<LineId>,
    /// The held station: `Project::adjustment_anchor_of` for the component.
    pub station: StationRef,
    /// Why that station.
    pub hold: ComponentHold,
}

impl Project {
    /// The station the 0.6.0 rules hold for the set of lines `lines` and why
    /// (`adjustment_anchor_of`: its first placement datum, else a held gauge
    /// on one of its lines, else the default gauge of its lowest-id line).
    /// Meant for a connected component, but answers for any set, e.g. a
    /// merge-only piece. `None` for a set with no line.
    pub fn anchor_for_lines(&self, lines: &HashSet<LineId>) -> Option<(StationRef, ComponentHold)> {
        let station = self.adjustment_anchor_of(lines)?;
        let hold = if self
            .placement_datum_refs()
            .iter()
            .any(|sr| lines.contains(&sr.line_id))
        {
            ComponentHold::PlacementDatum
        } else if self
            .held_gauges
            .iter()
            .filter(|held| lines.contains(&held.line_id))
            .any(|held| self.resolve_held_gauge(held).is_some())
        {
            ComponentHold::HeldGauge
        } else {
            ComponentHold::DefaultGauge
        };
        Some((station, hold))
    }

    /// The held station of every connected component, in the order the
    /// components are first reached walking `lines` in list order (the order
    /// `plan_adjustment_anchors` visits them). Every component is listed,
    /// whether or not it has anything for the closure solve to adjust.
    pub fn component_anchors(&self) -> Vec<ComponentAnchor> {
        let mut visited: HashSet<LineId> = HashSet::new();
        let mut anchors = Vec::new();
        for line in &self.lines {
            if visited.contains(&line.id) {
                continue;
            }
            let component = self.get_connected_line_ids(line.id);
            visited.extend(component.iter().copied());
            let Some((station, hold)) = self.anchor_for_lines(&component) else {
                continue;
            };
            let mut line_ids: Vec<LineId> = component.into_iter().collect();
            line_ids.sort_unstable();
            anchors.push(ComponentAnchor {
                line_ids,
                station,
                hold,
            });
        }
        anchors
    }
}
