//! The station fold: one station replaced by another everywhere (plan 5.1,
//! "Effect"). A merge, a closure promotion (plan 5.5) and Combine Lines' join
//! (plan 5.4 step 4) all end in it; each decides the direction (which
//! station survives) and the anchors itself.
//!
//! What the fold does, and nothing else:
//! - `retired` is replaced by `survivor` in every chain (`line.start`,
//!   `shot.to`), every datum, every closure end and every hand-move key;
//! - a closure whose two ends become the survivor is removed (#738);
//! - a placement on `retired` is re-keyed to the survivor (plan 5.1; the
//!   caller's anchor settle then gives the merged piece its one placement);
//! - the survivor keeps its id and name; the retired id and name (and the
//!   retired station's own aliases) become the survivor's aliases; the
//!   comment follows the [`CommentRule`];
//! - a station whose name now repeats the survivor's on a line it shares with
//!   it is renamed by the letter-suffix rule, so names stay unique on every
//!   line (plan 5.3); the survivor keeps its name.
//!
//! Positions are not touched: the caller settles anchors afterwards.
//!
//! Its inverse, the detach ([`Project::detach_line_from`]), is plan 5.2's
//! mutation without the anchor settle: Combine Lines tears junctions with it
//! (plan 5.4 steps 2, 3 and 6). Stream M2 owns Unmerge/Detach as a command;
//! the integration keeps one copy of this mutation.

use crate::{CombinePick, LineId, StationId};

use super::super::{LoopClosure, Project, StationAlias};
use super::EditError;

/// How a fold settles the two stations' comments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentRule {
    /// Plan 5.1: both kept, the retired one appended after a blank line, when
    /// both are non-empty and different; otherwise whichever exists.
    Join,
    /// Combine Lines: the dialog's pick between the survivor's (kept) and the
    /// retired one's (other), or both joined by a newline (today's
    /// `pick_text`). Identical texts are kept once.
    Pick(CombinePick),
}

/// What a fold did.
#[derive(Debug, Clone, PartialEq)]
pub struct Fold {
    pub survivor: StationId,
    pub retired: StationId,
    /// Closures removed because both their ends became the survivor (#738),
    /// as they were before the fold, in list order.
    pub removed_closures: Vec<LoopClosure>,
    /// Stations renamed so names stay unique on every line through the
    /// survivor: `(station, old name, new name)`.
    pub renamed: Vec<(StationId, String, String)>,
}

/// The joined comment of `rule` (today's `pick_text` for a pick).
pub(super) fn fold_comment(
    survivor: Option<String>,
    retired: Option<String>,
    rule: CommentRule,
) -> Option<String> {
    let nonempty = |text: Option<String>| text.filter(|t| !t.is_empty());
    match (nonempty(survivor), nonempty(retired)) {
        (Some(kept), Some(other)) if kept == other => Some(kept),
        (Some(kept), Some(other)) => Some(match rule {
            CommentRule::Join => format!("{kept}\n\n{other}"),
            CommentRule::Pick(CombinePick::Kept) => kept,
            CommentRule::Pick(CombinePick::Other) => other,
            CommentRule::Pick(CombinePick::Both) => format!("{kept}\n{other}"),
        }),
        (kept, other) => kept.or(other),
    }
}

impl Project {
    /// Fold station `retired` into `survivor` (module docs). Refused, with
    /// nothing changed, when either station is missing, they are the same
    /// station, or a line passes through both (it would pass the survivor
    /// twice, invariant 3).
    pub(in crate::survey) fn fold_station(
        &mut self,
        retired: StationId,
        survivor: StationId,
        comment: CommentRule,
    ) -> Result<Fold, EditError> {
        if retired == survivor {
            return Err(EditError::refused("A station cannot be merged with itself"));
        }
        let (Some(old), Some(kept)) = (
            self.stations.get(&retired).cloned(),
            self.stations.get(&survivor).cloned(),
        ) else {
            return Err(EditError::refused(
                "That station no longer exists.".to_string(),
            ));
        };
        let topology = self.topology_arc();
        let shared = topology
            .lines_of(retired)
            .into_iter()
            .find(|line| topology.slot_on(survivor, *line).is_some());
        if let Some(line) = shared {
            return Err(EditError::refused(format!(
                "Stations {} and {} are both on {}: merging them would make the line pass one \
                 station twice",
                old.name,
                kept.name,
                self.line_label_of(line)
            )));
        }

        let swap = |s: &mut StationId| {
            if *s == retired {
                *s = survivor;
            }
        };
        for line in self.lines_mut() {
            swap(&mut line.start);
            for shot in &mut line.shots {
                swap(&mut shot.to);
            }
            if let Some(hand) = line.hand_moves.remove(&retired) {
                line.hand_moves.insert(survivor, hand);
            }
        }
        for datum in &mut self.datums {
            swap(&mut datum.station);
        }
        let mut removed_closures = Vec::new();
        let mut kept_closures = Vec::with_capacity(self.closures.len());
        for closure in std::mem::take(&mut self.closures) {
            let mut moved = closure;
            swap(&mut moved.a);
            swap(&mut moved.b);
            if moved.a == moved.b {
                removed_closures.push(closure);
            } else {
                kept_closures.push(moved);
            }
        }
        self.closures = kept_closures;
        for placement in &mut self.placements {
            swap(&mut placement.station);
        }

        self.stations.remove(&retired);
        if let Some(station) = self.stations.get_mut(&survivor) {
            station.aliases.push(StationAlias {
                id: retired,
                name: Some(old.name.clone()),
            });
            station.aliases.extend(old.aliases.iter().cloned());
            station.comment = fold_comment(kept.comment, old.comment.clone(), comment);
        }

        let renamed = self.rename_clashes_with(survivor);
        Ok(Fold {
            survivor,
            retired,
            removed_closures,
            renamed,
        })
    }

    /// Rename every station whose name repeats `station`'s on a line they
    /// share, by the letter-suffix rule (free on every line it is on);
    /// `station` keeps its name. Chain order of `station`'s lines (list
    /// order), so the result is deterministic.
    pub(in crate::survey) fn rename_clashes_with(
        &mut self,
        station: StationId,
    ) -> Vec<(StationId, String, String)> {
        let Some(name) = self.stations.get(&station).map(|s| s.name.clone()) else {
            return Vec::new();
        };
        let mut renamed = Vec::new();
        for clash in self.conflicts_with_name(station, &name) {
            let other = clash.other;
            if renamed.iter().any(|(s, _, _)| *s == other) {
                continue;
            }
            let before = self
                .stations
                .get(&other)
                .map(|s| s.name.clone())
                .unwrap_or_default();
            if let Some(after) = self.uniquify_station_name(other) {
                renamed.push((other, before, after));
            }
        }
        renamed
    }

    /// Detach line `line` from station `station` (plan 5.2's mutation, no
    /// anchor settle): the line's slot gets a new station, named as `station`
    /// (then letter-suffixed if that name is taken on the line; no comment,
    /// no aliases); the datums and closure ends whose `via` is `line` move to
    /// it, and so does the line's hand move there. Returns the new station.
    /// Refused, with nothing changed, when the station is not on the line or
    /// no other line passes it (there is nothing to detach from).
    pub(in crate::survey) fn detach_line_from(
        &mut self,
        line: LineId,
        station: StationId,
    ) -> Result<StationId, EditError> {
        let topology = self.topology_arc();
        let lines = topology.lines_of(station);
        if !lines.contains(&line) || lines.len() < 2 {
            return Err(EditError::refused(format!(
                "{} does not share that station with another line",
                self.line_label_of(line)
            )));
        }
        let name = self
            .stations
            .get(&station)
            .map(|s| s.name.clone())
            .unwrap_or_default();
        let fresh = self.mint_station(name)?;
        if let Some(l) = self.line_mut(line) {
            if l.start == station {
                l.start = fresh;
            }
            for shot in &mut l.shots {
                if shot.to == station {
                    shot.to = fresh;
                }
            }
            if let Some(hand) = l.hand_moves.remove(&station) {
                l.hand_moves.insert(fresh, hand);
            }
        }
        for datum in &mut self.datums {
            if datum.station == station && datum.via == Some(line) {
                datum.station = fresh;
            }
        }
        for closure in &mut self.closures {
            if closure.a == station && closure.a_via == Some(line) {
                closure.a = fresh;
            }
            if closure.b == station && closure.b_via == Some(line) {
                closure.b = fresh;
            }
        }
        self.uniquify_station_name(fresh);
        Ok(fresh)
    }
}
