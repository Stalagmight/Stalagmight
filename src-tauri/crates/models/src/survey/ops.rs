//! Shared plumbing for the topology, closure, datum, move and Add commands
//! (stream M2): the typed refusals ([`OpError`]), what a command reports
//! ([`OpOutcome`]) and the one command pipeline ([`Project::run_op`]).
//!
//! Every command runs the M0 protocol: snapshot the anchors, mutate, then the
//! epilogue (RS 3): drop stale attachments (archived record lines included,
//! A2), give every station left with no native line the archived identity
//! naming its lines (`Project::restore_archived_identities`, A4 and RS 4.3),
//! settle the anchors (plan 4.1), run the orphan-closure sweep when the
//! command is a topology edit (plan 5.5), normalise the closure archives
//! (C1, C2, RS 5.4), settle the name collisions the epilogue made by rule L
//! (`Project::letter_collisions`), then [`Project::validate`]. A command that
//! carries the user's renames (merge, detach, closures) applies them right
//! after its mutation ([`Project::run_op_answered`]) and refuses a collision
//! of its own that they leave unanswered ([`OpError::NameTaken`]), judged
//! once the sweep ran: a collision a promotion merges away is no collision
//! (its preview, the op run with rule L, lists none), and one the sweep
//! letters is unanswered too. Collisions the epilogue itself makes (a
//! restore, a promotion, a dropped archive) are lettered by rule L.
//! A refusal or a violation restores the project as it was (the rollback is a
//! clone taken before the mutation), so a failed command never leaves a
//! trace.
//!
//! Commands never call [`Project::touch`]: the modification time is the
//! command layer's (C) business, which keeps every model operation a pure,
//! deterministic function of its input.

use std::collections::HashMap;
use std::fmt;

use crate::station_naming::user_station_name;
use crate::{LineId, StationId, StationRef};

use super::{
    AnchorReport, AnchorSnapshot, BuildError, ClosureId, EditContext, IdentityReport,
    LetteredStation, Project, RestoredIdentity, SweepReport, Violation,
};

/// Why a command was refused. Nothing was changed.
///
/// [`fmt::Display`] gives the user-facing text, word for word what today's
/// command answers where one exists.
#[derive(Debug, Clone, PartialEq)]
pub enum OpError {
    /// A station id that is not on any line.
    UnknownStation(StationId),
    /// A slot (`line`, `index`) that does not exist.
    UnknownSlot(StationRef),
    /// A line id that does not exist.
    UnknownLine(LineId),
    /// No loop closure has this id.
    UnknownClosure(ClosureId),
    /// No datum at this list index.
    UnknownDatum(usize),
    /// The station does not lie on the line.
    StationNotOnLine { station: StationId, line: LineId },
    /// Merge: the two stations are one station already.
    AlreadyMerged,
    /// Merge: the two stations are already connected (shots, junctions or
    /// aligned closures); a loop closure is what joins them.
    AlreadyConnected,
    /// Merge: a line would pass the merged station twice (invariant 3).
    StationTwiceOnLine { line: LineId, station: StationId },
    /// Detach: the two slots are not one station on two lines.
    NotDirectlyMerged,
    /// Merge or detach: the station to keep is not one the command can keep
    /// (a merge keeps one of its two stations; a detach's remaining lines
    /// take one of the records naming them).
    InvalidKeep(StationId),
    /// Merge or detach: the command would show two stations by one name on
    /// `line` (invariant N8) and its renames do not answer it.
    NameTaken { line: LineId, name: String },
    /// A dialog's rename gives `station` a name no station may take
    /// (`reason`: empty, or holding a control or invisible formatting
    /// character), the rule Set Station Name applies (RS-FX9).
    InvalidRename { station: StationId, reason: String },
    /// A dialog's rename addresses a station none of the command's own name
    /// collisions names (RS-FX9): a dialog answers only those.
    RenameNotAsked(StationId),
    /// Closure: both ends are one station.
    ClosureOnOneStation,
    /// Closure: the ends are in different pieces that are not both placed.
    NotConnected,
    /// Closure: a closure already joins the two stations.
    DuplicateClosure,
    /// Closure: it would tie the two ends of one shot together (#737).
    AcrossOneShot {
        line: LineId,
        index: usize,
        message: String,
    },
    /// Closure: the create-time distribution strategies are deferred.
    DeferredStrategy,
    /// Closure: `Tie` is a migration-only strategy.
    TieNotCreatable,
    /// Datum: the GPS position is invalid.
    InvalidGps(String),
    /// Datum: the accuracy is invalid.
    InvalidAccuracy(String),
    /// Move: an offset component is NaN or infinite.
    NonFiniteOffset,
    /// Move Segment: the piece (rigid group) holds a zero datum.
    SegmentHoldsDatum,
    /// Move Station: the station holds a zero datum.
    StationHoldsDatum,
    /// Move Station: a rewritten shot would have zero (or no finite) length.
    ZeroLengthShot,
    /// Add .stalx: the source survey breaks the model invariants.
    SourceInvalid(Vec<Violation>),
    /// An id space ran out, or a construction call refused.
    Build(BuildError),
    /// The edit would break a model invariant; it was rolled back.
    Invalid(Vec<Violation>),
}

impl fmt::Display for OpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownStation(id) => write!(f, "Station {id} does not exist"),
            Self::UnknownSlot(slot) => write!(
                f,
                "Station {} of line {} does not exist",
                slot.station_index, slot.line_id
            ),
            Self::UnknownLine(_) => f.write_str("Line not found"),
            Self::UnknownClosure(_) => f.write_str("Loop closure not found"),
            Self::UnknownDatum(index) => write!(f, "Zero datum {index} does not exist"),
            Self::StationNotOnLine { station, line } => {
                write!(f, "Station {station} is not on line {line}")
            }
            Self::AlreadyMerged => f.write_str("These stations are already merged"),
            Self::AlreadyConnected => {
                f.write_str("Stations are already connected. Use Create Loop Closure instead.")
            }
            Self::StationTwiceOnLine { line, station } => write!(
                f,
                "Line {line} would pass station {station} twice; use Create Loop Closure instead."
            ),
            Self::NotDirectlyMerged => f.write_str("Stations are not directly merged"),
            Self::InvalidKeep(id) => write!(f, "Station {id} is not one this can keep"),
            Self::NameTaken { line, name } => write!(
                f,
                "Line {line} would show two stations named {name:?}; rename one of them"
            ),
            Self::InvalidRename { reason, .. } => f.write_str(reason),
            Self::RenameNotAsked(id) => write!(
                f,
                "Station {id} is in none of the name collisions this would make, so it cannot \
                 be renamed here"
            ),
            Self::ClosureOnOneStation => f.write_str(
                "These stations are already merged. Closures are for unmerged station pairs.",
            ),
            Self::NotConnected => f.write_str(
                "Stations are not connected. They must be in the same connected group to create \
                 a loop closure.",
            ),
            Self::DuplicateClosure => {
                f.write_str("A loop closure already exists between these stations.")
            }
            Self::AcrossOneShot { message, .. } => f.write_str(message),
            Self::DeferredStrategy => {
                f.write_str("Closure distribution strategies are deferred. Use Batch.")
            }
            Self::TieNotCreatable => {
                f.write_str("A rigid tie closure is only carried over from old files.")
            }
            Self::InvalidGps(why) | Self::InvalidAccuracy(why) => f.write_str(why),
            Self::NonFiniteOffset => f.write_str("Move offset must be finite (no NaN/inf)"),
            Self::SegmentHoldsDatum => {
                f.write_str("Cannot move a segment whose rigid group holds a zero datum")
            }
            Self::StationHoldsDatum => f.write_str("Cannot move a zero datum station"),
            Self::ZeroLengthShot => f.write_str("Move would create zero-length shot"),
            Self::SourceInvalid(violations) => write!(
                f,
                "Blocked: the file's survey has integrity problems that would corrupt this \
                 project ({}). Open it, repair it, and add it again. No changes were made.",
                first_violation(violations)
            ),
            Self::Build(error) => write!(f, "{error}"),
            Self::Invalid(violations) => write!(
                f,
                "The edit would break the survey ({}); nothing was changed.",
                first_violation(violations)
            ),
        }
    }
}

impl std::error::Error for OpError {}

impl From<BuildError> for OpError {
    fn from(error: BuildError) -> Self {
        Self::Build(error)
    }
}

fn first_violation(violations: &[Violation]) -> String {
    violations
        .first()
        .map_or_else(|| "no detail".to_string(), ToString::to_string)
}

/// What a successful command did: its own result, what the anchor algorithm
/// did, what the orphan-closure sweep did (empty when it did not run), and
/// what the epilogue did to identities.
#[derive(Debug, Clone, PartialEq)]
pub struct OpOutcome<T> {
    pub value: T,
    pub anchors: AnchorReport,
    pub sweep: SweepReport,
    /// Nodes under another id after the command, `(old, new)`: the command's
    /// own (a merge keeping its moving station) and the epilogue's restores.
    pub reidentified: Vec<(StationId, StationId)>,
    /// Restores, dropped closure archives and rule-L letters (RS 3).
    pub identities: IdentityReport,
}

impl<T> OpOutcome<T> {
    /// The outcome of a command that changed nothing.
    pub(super) fn unchanged(value: T) -> Self {
        Self {
            value,
            anchors: AnchorReport::default(),
            sweep: SweepReport::default(),
            reidentified: Vec::new(),
            identities: IdentityReport::default(),
        }
    }

    /// The same outcome around another value.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> OpOutcome<U> {
        OpOutcome {
            value: f(self.value),
            anchors: self.anchors,
            sweep: self.sweep,
            reidentified: self.reidentified,
            identities: self.identities,
        }
    }
}

/// What [`Project::epilogue`] did, for the op's outcome.
struct Epilogue {
    restored: Vec<RestoredIdentity>,
    /// The op's edit context, with the restores' re-identifications.
    ctx: EditContext,
    anchors: AnchorReport,
    sweep: SweepReport,
    unarchived: Vec<ClosureId>,
    lettered: Vec<LetteredStation>,
}

/// Whether a command runs the orphan-closure sweep after settling (plan 5.5:
/// merge, detach, datum removal and every delete do).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Sweep {
    Run,
    Skip,
}

impl Project {
    /// The command pipeline: snapshot, `op` (which mutates and says what kind
    /// of edit it was), the epilogue (module docs), validate. On a refusal
    /// from `op` or any violation the project is restored exactly.
    pub(super) fn run_op<T>(
        &mut self,
        sweep: Sweep,
        op: impl FnOnce(&mut Project) -> Result<(T, EditContext), OpError>,
    ) -> Result<OpOutcome<T>, OpError> {
        self.run_op_answered(sweep, super::ops_merge::Answers::RuleL, op)
    }

    /// [`Self::run_op`] for a command that carries a dialog's `answers`: the
    /// renames apply right after `op`'s mutation, and a collision `op` made
    /// that they leave unanswered refuses ([`OpError::NameTaken`]) when it
    /// still stands once the sweep and the normalisation ran, or when the
    /// sweep lettered it (rule-L answers letter it instead, as without a
    /// dialog). A collision a promotion merged away refuses nothing: the
    /// preview, which runs the op with rule L, lists none for it.
    pub(super) fn run_op_answered<T>(
        &mut self,
        sweep: Sweep,
        answers: super::ops_merge::Answers<'_>,
        op: impl FnOnce(&mut Project) -> Result<(T, EditContext), OpError>,
    ) -> Result<OpOutcome<T>, OpError> {
        let backup = self.clone();
        let before = self.anchor_snapshot();
        let result = op(self).and_then(|(value, ctx)| {
            // The stations a rename may address are those the op's own
            // collisions name, each with the letters they offer it, worked
            // out as its preview lists them: the epilogue run with rule L on
            // a copy of the state right after the mutation, and the other
            // answer of each collision (only when there are renames).
            let own_collisions = answers.apply(self, |p| {
                let mut probe = p.clone();
                probe
                    .epilogue(sweep, &before, ctx.clone(), &backup, &[])
                    .map(|e| probe.offered_letters(&e.lettered))
                    .unwrap_or_default()
            })?;
            let done = self.epilogue(sweep, &before, ctx, &backup, &own_collisions)?;
            let violations = self.validate();
            if violations.is_empty() {
                let identities = IdentityReport {
                    restored: done.restored,
                    unarchived: done.unarchived,
                    lettered: done.lettered,
                    hidden: Vec::new(),
                }
                .with_hidden(&backup, self);
                Ok(OpOutcome {
                    value,
                    anchors: done.anchors,
                    sweep: done.sweep,
                    reidentified: done.ctx.reidentified,
                    identities,
                })
            } else {
                Err(OpError::Invalid(violations))
            }
        });
        if result.is_err() {
            *self = backup;
        }
        result
    }

    /// The epilogue after an op's mutation (module docs): drop stale
    /// attachments, restore (A4), settle the anchors against `before`,
    /// sweep, normalise the closure archives, refuse a collision of `own`
    /// (the op's own collisions its renames left) still standing
    /// ([`Self::refuse_unanswered`]), then letter what is left by rule L
    /// against `backup` (the state before the op).
    fn epilogue(
        &mut self,
        sweep: Sweep,
        before: &AnchorSnapshot,
        ctx: EditContext,
        backup: &Project,
        own: &[(LineId, String)],
    ) -> Result<Epilogue, OpError> {
        self.drop_stale_attachments();
        let restored = self.restore_archived_identities();
        let pairs: Vec<(StationId, StationId)> = restored.iter().map(|r| (r.old, r.new)).collect();
        let ctx = ctx.with_reidentified(&pairs);
        let anchors = self.settle_anchors(before, &ctx);
        let sweep = match sweep {
            Sweep::Run => self.sweep_orphan_closures(),
            Sweep::Skip => SweepReport::default(),
        };
        let mut unarchived = sweep.unarchived.clone();
        unarchived.extend(self.normalise_closure_archives());
        self.refuse_unanswered(own, &sweep)?;
        // Every letter the command gave: the sweep's (it letters every
        // collision standing when its promotions end, the command's own
        // included) and the final pass's, so a preview lists them all and
        // the notice names each.
        let mut lettered = sweep.lettered.clone();
        lettered.extend(self.letter_collisions(backup));
        Ok(Epilogue {
            restored,
            ctx,
            anchors,
            sweep,
            unarchived,
            lettered,
        })
    }

    /// The station at a slot, or [`OpError::UnknownSlot`].
    pub fn station_of_slot(&self, slot: StationRef) -> Result<StationId, OpError> {
        self.station_at(slot).ok_or(OpError::UnknownSlot(slot))
    }

    /// Apply the user's `renames` (each renames the identity whose Station ID
    /// it names) and return the collisions still standing, by line and name:
    /// what a command that carries renames runs right after its mutation,
    /// before the epilogue ([`Self::refuse_unanswered`] judges them once the
    /// sweep ran). Each rename must address a station the op's own
    /// collisions name (`asked`, called only when there are renames: the
    /// stations of its preview's collisions, both answers of each, with the
    /// letters they offer each station). A name that is exactly a letter
    /// offered for its station is taken verbatim: it is the backend's own
    /// answer, free on the station's lines, and its base may be an imported
    /// name with padding (`" 5"` letters to `" 5a"`, which trimmed could
    /// clash with a `5a` of the line; RS-FX10). Any other name is a user's
    /// ([`user_station_name`]: trimmed, refused when empty or holding a
    /// control or invisible formatting character, as Set Station Name
    /// refuses it). Anything else refuses ([`OpError::InvalidRename`], then
    /// [`OpError::RenameNotAsked`]).
    pub(super) fn apply_renames(
        &mut self,
        renames: &[(StationId, String)],
        asked: impl FnOnce(&Project) -> HashMap<StationId, Vec<String>>,
    ) -> Result<Vec<(LineId, String)>, OpError> {
        let asked = if renames.is_empty() {
            HashMap::new()
        } else {
            asked(self)
        };
        let mut checked: Vec<(StationId, &str)> = Vec::with_capacity(renames.len());
        for (id, name) in renames {
            let offered = asked
                .get(id)
                .is_some_and(|letters| letters.iter().any(|letter| letter == name));
            let name = if offered {
                name.as_str()
            } else {
                user_station_name(name).map_err(|reason| OpError::InvalidRename {
                    station: *id,
                    reason,
                })?
            };
            checked.push((*id, name));
        }
        if let Some(&(id, _)) = checked.iter().find(|(id, _)| !asked.contains_key(id)) {
            return Err(OpError::RenameNotAsked(id));
        }
        for (id, name) in checked {
            let station = self
                .stations
                .get_mut(&id)
                .ok_or(OpError::UnknownStation(id))?;
            name.clone_into(&mut station.name);
        }
        Ok(self
            .name_collisions()
            .into_iter()
            .map(|c| (c.line, c.name))
            .collect())
    }

    /// Refuse with [`OpError::NameTaken`] when one of `own` (the collisions
    /// an op with a dialog's renames left right after its mutation) still
    /// stands, or the sweep lettered it ([`Self::run_op_answered`]).
    pub(super) fn refuse_unanswered(
        &self,
        own: &[(LineId, String)],
        sweep: &SweepReport,
    ) -> Result<(), OpError> {
        if own.is_empty() {
            return Ok(());
        }
        let lettered = sweep
            .lettered
            .iter()
            .map(|l| (l.line, l.from.clone()))
            .find(|pair| own.contains(pair));
        let standing = || {
            self.name_collisions()
                .into_iter()
                .map(|c| (c.line, c.name))
                .find(|pair| own.contains(pair))
        };
        match lettered.or_else(standing) {
            Some((line, name)) => Err(OpError::NameTaken { line, name }),
            None => Ok(()),
        }
    }

    /// `station` when it lies on some line, else [`OpError::UnknownStation`].
    pub(super) fn existing_station(&self, station: StationId) -> Result<StationId, OpError> {
        if self.topology().contains(station) && self.stations.contains_key(&station) {
            Ok(station)
        } else {
            Err(OpError::UnknownStation(station))
        }
    }

    /// A station as the user reads it on `line`: `Main Line / 4`, by the
    /// name the slot shows, with the qualifier a dotted slot or a loop end
    /// takes in plain text (`Side / 12 (Main)`, [`Self::slot_text`],
    /// RS-FX5). Falls back to the station's first line, then to `a station`.
    pub fn station_text(&self, station: StationId, line: Option<LineId>) -> String {
        let topology = self.topology();
        let slot = line
            .and_then(|l| topology.slot_on(station, l))
            .or_else(|| topology.first_slot(station));
        match (
            slot.and_then(|s| self.slot_text(s)),
            self.shown_station(station),
        ) {
            (Some(text), _) => text,
            (None, Some(s)) => s.name.clone(),
            (None, None) => "a station".to_string(),
        }
    }
}
