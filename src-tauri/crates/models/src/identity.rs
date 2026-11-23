//! The wire shapes of kept and archived stations (plan 2026-11-23, RS 6 and
//! 7): what each slot shows, the records a node keeps, a closure's archived
//! end, and what the Merge, Detach, Create Loop Closure and Remove Closure
//! dialogs and the notices need. The station-graph model fills them
//! (`survey::wire`); their serde and TS shapes are the IPC contract.
//!
//! Every identity is named by its Station ID and its own name, so a dialog
//! can say "Side / 0" for a station that is not a node of the state it was
//! asked on (a record a detach restores, an end a closure hides).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{LineId, StationId, StationRef};

/// An identity as a dialog, a preview or a notice names it: its Station ID,
/// its own name and comment, and its home line (RS 2), which colours its
/// pill.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct StationIdentity {
    pub id: StationId,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub comment: Option<String>,
    /// The first native line of the node holding the identity, in the
    /// model's line order (`None` for an identity no node holds now, e.g. a
    /// record).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub home_line: Option<LineId>,
}

/// A station a merge archived inside a node (RS 3): its identity and the
/// lines whose own station it was. A record with no lines is history (A5):
/// nothing restores it. The station details list them; Detach Line from
/// Station on one of `lines` brings the record back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(rename = "ArchivedStation")]
pub struct ArchivedStationWire {
    pub id: StationId,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub comment: Option<String>,
    /// The lines whose own station it was, in the model's line order; empty
    /// for a history record.
    pub lines: Vec<LineId>,
}

/// How a slot shows its node when that is not simply the node's own
/// identity on one of its native lines (RS 6): the slot is foreign (its line
/// knew another station there, or the node is a loop end) or a loop closure
/// archives its node. `crate::Station::shown` is absent otherwise.
///
/// The slot's `name` and `comment` are already the shown identity's; this
/// adds the Station ID it shows, the dot and the loop mark.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ShownSlot {
    /// The Station ID the slot shows: the shown identity's (the Station ID
    /// row, the pill superscript, the Station ID column and IDs-mode labels
    /// read it). The slot's `id` stays the node's.
    pub id: StationId,
    /// The shown identity's home line: the colour of the dot a foreign slot
    /// carries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub home_line: Option<LineId>,
    /// The slot's line is not a native line of the node holding the shown
    /// identity: the name carries a dot in the home line's colour.
    pub foreign: bool,
    /// A loop closure archives the slot's node: the slot carries the loop
    /// mark, and its map point has no label.
    pub loop_end: bool,
    /// The closure archiving the node (loop ends only): "Remove the
    /// closure" brings the node's own station back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub loop_closure: Option<u64>,
}

/// Which end of a loop closure (`station_a` or `station_b`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(rename = "ClosureEnd")]
pub enum ClosureEndWire {
    A,
    B,
}

/// The end a loop closure archives (RS 5): it shows the other end's shown
/// identity on every line through it, until the closure is removed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ClosureArchive {
    /// The end archived.
    pub end: ClosureEndWire,
    /// Its own identity (hidden now): what Remove Closure brings back
    /// ("17 comes back").
    pub station: StationIdentity,
    /// The identity it is shown as (the kept end's shown identity).
    pub shown_as: StationIdentity,
}

/// One answer of a dialog to a name collision: rename the identity with
/// Station ID `station_id` to `name` in the same operation. Commands that
/// carry renames refuse a collision they leave unanswered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct StationRename {
    pub station_id: StationId,
    pub name: String,
}

/// A suggested rename: `station` (by its name before) takes `to`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct RenameSuggestion {
    pub station: StationIdentity,
    pub to: String,
}

/// A name an operation would show for two stations on one line (N8), with
/// the dialog's two answers (RS 4.1 step 6): `letter` is rule L's (the
/// default, e.g. "Rename Side / 12 to 12a"), `alternative` letters the other
/// station instead (e.g. renames the kept station).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(rename = "NameCollision")]
pub struct NameCollisionWire {
    pub line_id: LineId,
    pub name: String,
    pub letter: RenameSuggestion,
    pub alternative: RenameSuggestion,
}

/// One station a dialog may keep (the Merge dialog's "Keep" radios, the
/// Create Loop Closure dialog's "Both ends show" choices).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct KeepCandidate {
    /// The slot the user picked.
    pub station: StationRef,
    /// The station's own identity (its node's).
    pub identity: StationIdentity,
    /// The identity a loop closure shows it as, when one archives it (RS 4.1
    /// step 5: "shown as K through a loop closure").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub shown_as: Option<StationIdentity>,
    /// It holds a datum (the Merge dialog's default keeps it).
    pub holds_datum: bool,
}

/// What a merge keeping `keep` does to names (RS 4.1, 7), one per choice the
/// dialog offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct MergeKeepPreview {
    /// The station kept (a candidate's Station ID).
    pub keep: StationId,
    /// The lines whose shown name for the junction changes, in line order
    /// ("the lines that will show the kept name").
    pub renamed_lines: Vec<LineId>,
    /// The names the merge would show twice, each with its two answers.
    pub collisions: Vec<NameCollisionWire>,
    /// The station archived: hidden everywhere ("Side / 0 is archived", plus
    /// "with its comment" when it has one).
    pub hidden: StationIdentity,
}

/// What creating the loop closure with one choice of kept end does (RS 5.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ClosureKeepPreview {
    /// The end kept (its Station ID); absent: "Keep both names" (the closure
    /// archives nothing and no name changes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub keep: Option<StationId>,
    /// The end archived and its own identity, which becomes hidden.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub hidden: Option<StationIdentity>,
    /// The lines on which a station shows another identity afterwards.
    pub renamed_lines: Vec<LineId>,
    pub collisions: Vec<NameCollisionWire>,
}

/// Why a loop closure may not keep an end (RS 5.2), for the dialog's note
/// when rule K's end is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum KeepRefusalReason {
    /// C1: the end the keep would archive (the other end) is already a loop
    /// end of another closure, which shows it as another station.
    OtherEndArchived,
    /// C2: the end kept is already shown as the end it would archive,
    /// through another closure.
    KeptShowsOtherEnd,
}

/// The Create Loop Closure dialog's naming part (`preview_loop_closure`):
/// both ends, which may be kept (C1, C2), the default, and what each choice
/// does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct LoopClosureNaming {
    /// The two ends, A's then B's.
    pub ends: Vec<KeepCandidate>,
    /// The ends that may be kept, A first.
    pub allowed: Vec<StationId>,
    /// Rule K's end, allowed or not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub rule_k: Option<StationId>,
    /// Why rule K's end may not be kept; absent when it may (or there is
    /// none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub rule_k_refused: Option<KeepRefusalReason>,
    /// The dialog's default: rule K's end when allowed; absent: "Keep both
    /// names".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub default_keep: Option<StationId>,
    /// One entry per allowed end, then "Keep both names" (no `keep`).
    pub options: Vec<ClosureKeepPreview>,
}

/// What Remove Closure does to names (`preview_remove_loop_closure`, RS
/// 5.3): the identity that comes back, on which lines, and the names that
/// would then repeat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct RemoveClosureNaming {
    pub closure_id: u64,
    /// The archived end's slot ("the slot and what comes back").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub archived_slot: Option<StationRef>,
    /// The identity that comes back ("17 comes back"); absent when the
    /// closure archives nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub comes_back: Option<StationIdentity>,
    /// The identity the archived end showed until now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub shown_as: Option<StationIdentity>,
    pub renamed_lines: Vec<LineId>,
    pub collisions: Vec<NameCollisionWire>,
}

/// Whose station a slot shows (`preview_station_identity`, RS 5.4): what a
/// rename or a comment edit through the slot affects, and the two ways back
/// the prompt offers on a foreign slot or a loop end.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct StationOwner {
    pub station: StationRef,
    /// The slot's node (geometry, datums and selection key on it).
    pub node: StationId,
    /// The identity the slot shows: a rename or comment edit through the
    /// slot changes this one ("12 belongs to Main Line").
    pub shown: StationIdentity,
    /// Every line the shown identity is shown on, in line order: what the
    /// rename or comment edit changes.
    pub lines: Vec<LineId>,
    pub foreign: bool,
    /// The closure archiving the node, on a loop end ("Shown as Main / 20
    /// through a loop closure. Remove the closure").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub loop_closure: Option<u64>,
    /// The line's own station there, archived by a merge ("Detach Side to
    /// get back its own station 0").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub own_station: Option<StationIdentity>,
}

/// A station rule L gave a letter (RS 2), for the action's notice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(rename = "LetteredStation")]
pub struct LetteredStationWire {
    /// The identity renamed (its Station ID).
    pub station_id: StationId,
    /// The line it collided on.
    pub line_id: LineId,
    /// The identity that kept the name.
    pub other_id: StationId,
    pub from: String,
    pub to: String,
}

/// A node that took back an archived identity because no native line was
/// left on it (RS 4.3), for the action's notice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(rename = "RestoredIdentity")]
pub struct RestoredIdentityWire {
    /// The Station ID it showed before (now a history record's).
    pub old_id: StationId,
    /// The identity it shows now.
    pub station: StationIdentity,
    /// The old identity's comment moved with it (the record had none).
    pub comment_moved: bool,
    /// The old identity's comment, hidden now (the record had its own):
    /// undo brings it back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub comment_hidden: Option<String>,
}

/// What an action did to identities on its own (RS 7 "Notices"): the
/// letters rule L gave, the archived identities it restored and the loop
/// closures whose archive it dropped. The frontend raises one notice per
/// action from it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct IdentityNotice {
    pub lettered: Vec<LetteredStationWire>,
    pub restored: Vec<RestoredIdentityWire>,
    /// Closures that no longer archive an end (their hidden end shows its
    /// own name again), by id.
    pub unarchived: Vec<u64>,
}

impl IdentityNotice {
    /// Whether the action did nothing to identities on its own.
    pub fn is_empty(&self) -> bool {
        self.lettered.is_empty() && self.restored.is_empty() && self.unarchived.is_empty()
    }

    /// Append `other`'s entries (a batch folds its sub-commands' notices).
    pub fn extend(&mut self, other: Self) {
        self.lettered.extend(other.lettered);
        self.restored.extend(other.restored);
        self.unarchived.extend(other.unarchived);
    }
}
