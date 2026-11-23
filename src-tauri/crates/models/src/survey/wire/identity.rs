//! Kept and archived stations on the wire (plan 2026-11-23, RS 6 and 7):
//! the identities, collisions and choices the Merge, Detach, Create Loop
//! Closure and Remove Closure dialogs show, the "whose station is this"
//! answer of a rename or comment edit, a closure's archived end, and the
//! notice an action raises for what it did to identities on its own.
//!
//! Every mapping takes the state the preview ran on (before the edit), so
//! the slots it names exist in the state the user sees; an identity that is
//! no node of that state (a record a detach restores) is named by the
//! record.

use crate::{
    ArchivedStationWire, ClosureArchive, ClosureEndWire, ClosureKeepPreview, DetachLinePreview,
    DetachLineResult, IdentityNotice, KeepCandidate, LetteredStationWire, LineId,
    LoopClosureNaming, MergeKeepPreview, NameCollisionWire, RemoveClosureNaming, RenameSuggestion,
    RestoredIdentityWire, ShownElsewhereWire, StationId, StationIdentity, StationOwner, StationRef,
};

use super::super::{
    ClosureEnd, ClosureId, ClosureStrategy, HiddenComment, IdentityReport, LetteredStation,
    LoopClosure, MergeMove, OpError, Project,
};

impl Project {
    /// The identity with Station ID `id` (RS 2): a node's own, or a record's
    /// (its home line then its first own line, which colours its pill).
    /// `None` for an id that is neither.
    pub fn station_identity(&self, id: StationId) -> Option<StationIdentity> {
        if let Some(s) = self.stations.get(&id) {
            return Some(StationIdentity {
                id,
                name: s.name.clone(),
                comment: s.comment.clone(),
                home_line: self.home_line(id),
            });
        }
        let record = self
            .stations
            .values()
            .flat_map(|s| &s.archived)
            .find(|r| r.id == id)?;
        Some(StationIdentity {
            id,
            name: record.name.clone(),
            comment: record.comment.clone(),
            home_line: record
                .lines
                .iter()
                .copied()
                .min_by_key(|&l| self.line_position(l).unwrap_or(usize::MAX)),
        })
    }

    /// The identity node `node` shows (RS 5.1).
    pub fn shown_identity(&self, node: StationId) -> Option<StationIdentity> {
        self.station_identity(self.shown(node))
    }

    /// `id`'s identity under the name `name` (a collision names a station by
    /// the name it had when it collided).
    fn identity_named(&self, id: StationId, name: &str) -> StationIdentity {
        let mut identity = self.station_identity(id).unwrap_or(StationIdentity {
            id,
            name: String::new(),
            comment: None,
            home_line: None,
        });
        identity.name = name.to_string();
        identity
    }

    /// The wire collisions of `letters` (rule L's answers) and
    /// `alternatives` (the other answer to each, same order).
    pub fn collisions_wire(
        &self,
        letters: &[LetteredStation],
        alternatives: &[LetteredStation],
    ) -> Vec<NameCollisionWire> {
        letters
            .iter()
            .zip(alternatives)
            .map(|(letter, other)| NameCollisionWire {
                line_id: letter.line,
                name: letter.from.clone(),
                letter: RenameSuggestion {
                    station: self.identity_named(letter.station, &letter.from),
                    to: letter.to.clone(),
                },
                alternative: RenameSuggestion {
                    station: self.identity_named(other.station, &other.from),
                    to: other.to.clone(),
                },
            })
            .collect()
    }

    /// The notice of what an op did to identities on its own (RS 7), read on
    /// the state after it.
    pub fn identity_notice(&self, report: &IdentityReport) -> IdentityNotice {
        IdentityNotice {
            lettered: report.lettered.iter().map(lettered_wire).collect(),
            restored: report
                .restored
                .iter()
                .map(|r| RestoredIdentityWire {
                    old_id: r.old,
                    station: self.station_identity(r.new).unwrap_or(StationIdentity {
                        id: r.new,
                        name: String::new(),
                        comment: None,
                        home_line: None,
                    }),
                    comment_moved: r.comment_moved,
                    comment_hidden: r.comment_hidden.clone(),
                })
                .collect(),
            unarchived: report.unarchived.clone(),
            hidden: report.hidden.iter().map(hidden_wire).collect(),
        }
    }

    /// A closure's archived end on the wire (RS 5): its own identity and the
    /// one it is shown as; `None` when the closure archives nothing.
    pub fn closure_archive_wire(&self, closure: &LoopClosure) -> Option<ClosureArchive> {
        let (archived, kept) = closure.archive()?;
        Some(ClosureArchive {
            end: match closure.archived? {
                ClosureEnd::A => ClosureEndWire::A,
                ClosureEnd::B => ClosureEndWire::B,
            },
            station: self.station_identity(archived)?,
            shown_as: self.shown_identity(kept)?,
        })
    }

    /// One station a dialog may keep, picked at `slot`.
    pub fn keep_candidate(&self, slot: StationRef, station: StationId) -> Option<KeepCandidate> {
        let shown = self.shown(station);
        Some(KeepCandidate {
            station: slot,
            identity: self.station_identity(station)?,
            shown_as: if shown == station {
                None
            } else {
                self.station_identity(shown)
            },
            holds_datum: self.datums.iter().any(|d| d.station == station),
        })
    }

    /// What a merge does to names (RS 4.1), from its preview `m` (run on
    /// this state): the merge preview's naming half for `m.kept`.
    pub fn merge_keep_wire(&self, m: &MergeMove) -> MergeKeepPreview {
        let letters: Vec<LetteredStation> =
            m.collisions.iter().map(|c| c.suggestion.clone()).collect();
        let (id, name, comment) = m.hidden.clone();
        MergeKeepPreview {
            keep: m.kept,
            renamed_lines: m.renamed_lines.clone(),
            collisions: self.collisions_wire(&letters, &m.alternatives),
            hidden: StationIdentity {
                id,
                name,
                comment,
                home_line: self.home_line(id),
            },
            unarchived: m.unarchived,
            comments_hidden: m.comments_hidden.iter().map(hidden_wire).collect(),
        }
    }

    /// The Create Loop Closure dialog's naming part for slots `a` and `b`
    /// (RS 5.2, 7): both ends, the allowed choices, the default, and what
    /// each allowed end and "Keep both names" do. Refused as the creation
    /// is.
    pub fn loop_closure_naming_wire(
        &self,
        a: StationRef,
        b: StationRef,
        strategy: ClosureStrategy,
    ) -> Result<LoopClosureNaming, OpError> {
        let (sa, sb) = self.resolve_slot_pair(a, b)?;
        let ends = [(a, sa.station), (b, sb.station)]
            .into_iter()
            .map(|(slot, station)| {
                self.keep_candidate(slot, station)
                    .ok_or(OpError::UnknownStation(station))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let choices = self.closure_keep_choices(sa.station, sb.station);
        let mut options = Vec::with_capacity(choices.allowed.len() + 1);
        for &keep in &choices.allowed {
            let naming = self.closure_naming(
                (sa.station, Some(sa.line)),
                (sb.station, Some(sb.line)),
                strategy,
                Some(keep),
            )?;
            // Never a collision (RS-FX5): the archive makes loop ends only.
            debug_assert!(naming.collisions.is_empty(), "{:?}", naming.collisions);
            options.push(ClosureKeepPreview {
                keep: Some(keep),
                hidden: naming.archived.and_then(|id| self.station_identity(id)),
                renamed_lines: naming.renamed_lines,
            });
        }
        // "Keep both names" archives nothing, so no name changes; the
        // creation's own refusals were checked above, or are checked here
        // when no end may be kept.
        if choices.allowed.is_empty() {
            self.closure_refusal(sa.station, sb.station, strategy)?;
        }
        options.push(ClosureKeepPreview {
            keep: None,
            hidden: None,
            renamed_lines: Vec::new(),
        });
        Ok(LoopClosureNaming {
            ends,
            allowed: choices.allowed,
            rule_k: choices.rule_k,
            rule_k_refused: choices.rule_k_refused,
            default_keep: choices.default,
            options,
        })
    }

    /// The Remove Closure dialog's naming part (RS 5.3, 7): the identity
    /// that comes back, the lines that show it again, and the names that
    /// would repeat with their two answers.
    pub fn remove_closure_naming_wire(
        &self,
        id: ClosureId,
    ) -> Result<RemoveClosureNaming, OpError> {
        let closure = self
            .closures
            .iter()
            .find(|c| c.id == id)
            .copied()
            .ok_or(OpError::UnknownClosure(id))?;
        let naming = self.closure_removal(id)?;
        let archived_slot = closure.archive().and_then(|(archived, _)| {
            let via = if archived == closure.a {
                closure.a_via
            } else {
                closure.b_via
            };
            self.closure_end_slot(archived, via)
        });
        Ok(RemoveClosureNaming {
            closure_id: id,
            archived_slot,
            comes_back: naming.archived.and_then(|a| self.station_identity(a)),
            shown_as: naming.archived.and_then(|a| self.shown_identity(a)),
            renamed_lines: naming.renamed_lines,
            collisions: self.collisions_wire(&naming.collisions, &naming.alternatives),
        })
    }

    /// Whose station `slot` shows (RS 5.4): what a rename or a comment edit
    /// through it affects. `None` for a slot that names no station.
    pub fn station_owner_wire(&self, slot: StationRef) -> Option<StationOwner> {
        let owner = self.slot_owner(slot)?;
        Some(StationOwner {
            station: slot,
            node: owner.node,
            shown: self.station_identity(owner.shown)?,
            lines: self.lines_shown_on(owner.shown),
            foreign: owner.foreign,
            loop_closure: owner.loop_closure,
            own_station: owner.own_station.and_then(|id| self.station_identity(id)),
        })
    }

    /// `preview_detach_line`'s answer (RS 4.2, 7): what detaching `line`
    /// from the station at `slot` gives each line, what comes back (name,
    /// comment, Station ID), the lines that come off together, the
    /// remaining-junction choices when several records could show it, the
    /// identity a loop closure showed before, and each collision with its
    /// two answers. Refused as the detach is.
    pub fn detach_preview_wire(
        &self,
        slot: StationRef,
        line: LineId,
        rest_keep: Option<StationId>,
    ) -> Result<DetachLinePreview, OpError> {
        let station = self.station_of_slot(slot)?;
        let naming = self.detach_naming(station, line, rest_keep)?;
        let rest_choices = naming
            .rest_choices
            .iter()
            .filter_map(|&id| self.station_identity(id))
            .collect();
        Ok(DetachLinePreview {
            name: naming.name,
            station_id: naming.id,
            restored: naming.restored,
            kept: naming.kept,
            lines: naming
                .lines
                .into_iter()
                .map(|o| DetachLineResult {
                    line_id: o.line,
                    name: o.name,
                    station_id: o.station,
                    comes_off: o.comes_off,
                    joined_with: o.joined_with,
                })
                .collect(),
            comment: naming.comment,
            together: naming.together,
            rest_choices,
            rest_default: naming.rest_default,
            shown_before: naming.shown_before.and_then(|id| self.station_identity(id)),
            collisions: self.collisions_wire(&naming.collisions, &naming.alternatives),
            shown_elsewhere: naming
                .shown_elsewhere
                .iter()
                .map(|e| ShownElsewhereWire {
                    station: e.slot,
                    before: self.identity_named(e.before.0, &e.before.1),
                    after: self.identity_named(e.after.0, &e.after.1),
                })
                .collect(),
            comments_hidden: naming.comments_hidden.iter().map(hidden_wire).collect(),
        })
    }

    /// The records of `node` on the wire (RS 3).
    pub fn records_wire(&self, node: StationId) -> Vec<ArchivedStationWire> {
        self.stations.get(&node).map_or_else(Vec::new, |s| {
            s.archived
                .iter()
                .map(|r| super::stations::record_wire(self, r))
                .collect()
        })
    }
}

/// A hidden comment on the wire: the identity as it showed the comment
/// before the op, with the comment.
pub(super) fn hidden_wire(hidden: &HiddenComment) -> StationIdentity {
    StationIdentity {
        id: hidden.station,
        name: hidden.name.clone(),
        comment: Some(hidden.comment.clone()),
        home_line: hidden.home_line,
    }
}

/// A rule-L letter on the wire.
pub fn lettered_wire(letter: &LetteredStation) -> LetteredStationWire {
    LetteredStationWire {
        station_id: letter.station,
        line_id: letter.line,
        other_id: letter.other,
        from: letter.from.clone(),
        to: letter.to.clone(),
    }
}
