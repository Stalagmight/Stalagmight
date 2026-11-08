//! Everything that brings server bytes down into the cache `.stalx`: listing
//! projects, opening / re-opening one, the one-shot Ariane adoption, and the
//! fast-forward re-pull behind Start editing / Update / Take remote.
//!
//! Split out of `core/state/speleodb.rs` (which is frozen at the file-size ratchet
//! warn line) into a sibling `impl AppState` block.
//!
//! The fast-forward lives HERE rather than with the push because it is a pull: it
//! runs the same download -> classify -> replace pipeline as
//! [`AppState::speleodb_pull_project`] and differs only in the BASE stamp. Keeping
//! the two together is what lets one helper answer "which entry of this dump" for
//! every path that asks.
//!
//! The replace chokepoint itself is `speleodb_cache`, which owns the un-pushed-edits
//! guard. Note that the guard runs BEFORE the download on the pull path, so a
//! refusal costs no bytes - a source-order test pins that.

use std::path::Path;

use super::speleodb::{
    local_dto, lock_held_by_me, managed_type_patch, type_says_managed, validate_project_id, DumpAt,
};
use super::speleodb_cache::{
    preserved_undo_mode, restore_display_settings, InheritedUndo, LocalEditsPolicy,
};
use super::speleodb_revisions::validate_hexsha;
use super::{AppState, VirtualShotMode};
use crate::core::command::{Command, CommandResult, ImportReceipt};
use crate::models::{ImportClosure, ImportSectionConfig};
use crate::speleodb::contract::{dto_for_error, Endpoint};
use crate::speleodb::dump::{self, DumpClassification};
use crate::speleodb::models::{AdoptUpload, DoneResult, StartEditingResult};
use crate::speleodb::models::{SpeleoDumpCandidate, SpeleoProjectSummary};
use crate::speleodb::{client, sync};
use crate::speleodb::{SpeleoAdoptResult, SpeleoDbError, SpeleoErrorDto, SpeleoOpenResult};

/// The commit message adoption pushes its conversion under.
///
/// Fixed, and written from the SERVER's point of view: it is read in SpeleoDB's
/// revision list beside commits from Ariane and from other Stalagmight users, where
/// "what happened to this project" is the only useful thing a message can say.
const ADOPT_COMMIT_MESSAGE: &str = "Adopt into Stalagmight";

/// Prefix of the filename an adoption builds its converted project into, beside the
/// cache `.stalx` it will be renamed over. The full name appends this process's id -
/// see [`adopt_staging_name`].
///
/// A PREFIX rather than a fixed name, because the obvious justification for a fixed
/// one does not survive contact with this flow: "adoption holds the project's write
/// mutex, so two of them cannot race" is false here, since a lock it cannot take is
/// deliberately NOT a reason to stop - every lock-less route (read-only member,
/// offline, mutex held by a third party, unreadable probe) still converts. Two app
/// instances on one machine adopting the same project therefore share a cache
/// directory with no mutex between them, and a fixed name has each one's
/// remove-then-create unlink the other's in-flight database.
///
/// The prefix is what a crashed adoption's leftovers are still recognisable by, which
/// is the one thing the fixed name was actually buying. Nothing scans this directory
/// for `.stalx` files: `scan_cache` walks project-id DIRECTORIES and reads the one
/// canonical path inside each, so a leftover here is inert rather than a second
/// candidate.
const ADOPT_STAGING_PREFIX: &str = "adopt-staging-";

/// This process's staging filename. Per-process, so two instances adopting the same
/// project cannot delete each other's work in progress.
fn adopt_staging_name() -> String {
    format!("{ADOPT_STAGING_PREFIX}{}.stalx", std::process::id())
}

/// Remove this process's leftover staging files from `dir` - the ones a previous
/// adoption in THIS process died holding.
///
/// Scoped to our own pid on purpose. A sweep by bare prefix would delete a sibling
/// instance's live staging database, which is the collision the pid is here to avoid;
/// another process's leftovers are its own to clean up when it next adopts, and are
/// inert in the meantime.
fn sweep_own_staging(dir: &Path) {
    let ours = adopt_staging_name();
    let path = dir.join(&ours);
    if let Err(e) = std::fs::remove_file(&path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(
                error = %e,
                path = %path.display(),
                purpose = "clear this process's leftover adoption staging file",
                "speleodb_adopt_ariane: could not remove a stale staging .stalx"
            );
        }
    }
}

/// [#411] Run the remote `.tml` through the converter the way an adoption does, and
/// say what that cost: the converter's own warnings, led by the excluded shots this
/// path drops.
///
/// Adoption converts with `include_excluded = false` - the same default the import
/// dialog opens with - so a remote file's excluded shots are not imported. The import
/// dialog is honest about that by construction (the user set the switch), and its
/// receipt lists what the converter reported; adoption had neither, so the drop and
/// every parse anomaly with it were invisible.
///
/// The conversion and the counting are ONE function so the warnings cannot be
/// separated from the call that earns them: the previous shape took a warning list as
/// an argument, which a caller could satisfy with an empty vec while still dropping
/// the converter's own.
///
/// The excluded line goes FIRST, and that is load-bearing: `cap_warnings` truncates
/// from the tail, so a file carrying 50+ per-section placement warnings - a count the
/// file's author chooses - would otherwise fold this line into "... and N more".
///
/// Uncapped here: these go straight into an [`ImportReceipt`], and `decorate_receipt`
/// caps the list they land in. Capping twice folds the cap's own summary line into a
/// second summary.
///
/// CLOSURE shots are left out of the count deliberately: they are dropped from every
/// import whatever their `excluded` flag says, and the converter already reports the
/// closures it could not resolve. What this counts is measurements the file has and
/// the adopted project does not.
fn adopt_convert(
    tml: &crate::parsers::tml::TmlFile,
) -> Result<(Vec<ImportSectionConfig>, Vec<ImportClosure>, Vec<String>), SpeleoErrorDto> {
    let (sections, closures, converter_warnings) =
        crate::parsers::tml::convert_tml_to_import_configs(tml, false, true)
            .map_err(|e| local_dto(SpeleoDbError::Parse(e.to_string())))?;

    let excluded = tml
        .sections
        .iter()
        .flat_map(|section| &section.shots)
        .filter(|shot| shot.excluded && shot.shot_type != crate::parsers::tml::TmlShotType::Closure)
        .count();
    let mut warnings = Vec::with_capacity(converter_warnings.len() + 1);
    if excluded > 0 {
        warnings.push(format!(
            "{excluded} excluded shot{} in the remote file {} not imported",
            if excluded == 1 { "" } else { "s" },
            if excluded == 1 { "was" } else { "were" }
        ));
    }
    warnings.extend(converter_warnings);
    Ok((sections, closures, warnings))
}

/// [#411] The receipt an adoption reports, from the result of its import.
///
/// `execute_import_sections` (`command_import.rs`) attaches a receipt to every
/// success it returns, including the all-excluded case - an empty
/// `ImportReceipt::empty()` when it was handed no sections, a filled-in one
/// otherwise. `adopt_build_into` only reaches this call after checking
/// `imported.success`, so by the time this runs the receipt is always `Some`;
/// this just decorates it with the converter's own warnings the same way
/// `execute_tml_import` does.
fn adopt_receipt(
    imported: &mut CommandResult,
    entry_name: &str,
    sections_found: u32,
    unit_conversion: Option<String>,
    warnings: Vec<String>,
) -> Option<ImportReceipt> {
    // The same decoration `execute_tml_import` applies, through the same helper:
    // adoption differs from that import only in having no wizard in front of it, and
    // the receipt is what stands in for what the wizard would have shown.
    crate::core::command::decorate_receipt(
        imported,
        "Ariane TML",
        Path::new(entry_name),
        sections_found,
        unit_conversion,
        warnings,
    );
    imported.import_receipt.take()
}

/// Whether an adoption is holding the project's write mutex.
///
/// Not a `bool`: the interesting half is WHY a lock was not taken, and that answer
/// is the outcome the caller reports in place of a push. Carrying it here keeps the
/// two from being derived separately and drifting.
enum AdoptLock {
    /// This call took the mutex. It must reach a hand-over or a hand-back.
    Held,
    /// It did not, and this is what the webview is told instead.
    NotTaken(AdoptUpload),
}

/// What the single post-push GET established about the server project.
///
/// Both answers come off ONE request. The type flip needs that GET anyway, and the
/// same response carries `active_mutex` - so reading the holder here costs nothing
/// and replaces a guess the hand-over could not make for itself.
struct AdoptFinalize {
    /// Whether the project is typed `stalagmight` on the way out.
    type_flipped: bool,
    /// Who the server says holds the write mutex.
    ///
    /// The outer `Option` is UNKNOWN - the GET failed - and is deliberately kept
    /// apart from `Some(None)`, a project nobody holds. Collapsing the two would
    /// read "we could not ask" as "the lock is free", which is the one direction
    /// that turns a held mutex into a silent one.
    holder: Option<Option<String>>,
}

impl AdoptFinalize {
    /// The verdict for a push that never landed: nothing was asked, so nothing is
    /// known. `type_flipped: false` is the honest answer rather than a failure -
    /// the flip is deliberately not attempted until the bytes are up, because a
    /// project typed `stalagmight` whose only file is the Ariane one that type says to
    /// disregard is openable by nobody.
    const fn not_attempted() -> Self {
        Self {
            type_flipped: false,
            holder: None,
        }
    }
}

/// Hands an adoption's write lock back if the adoption never gets far enough to use
/// it.
///
/// Adoption takes the mutex FIRST - so no other writer can land a revision under the
/// conversion - and only then does a long, fallible stretch of work: resolve, download,
/// classify, extract, guard, import, save. Any step of it can return, and every one of
/// those exits is a way to strand the project locked for every other editor until the
/// server-side timeout. A hand-back written at each exit is a list, and a list goes
/// stale the first time an arm is added; this is the same duty expressed once, so an
/// arm added later inherits it for free.
///
/// [`Self::disarm`] passes the duty on to the hand-over once there is a converted
/// project for the lock to be about. From that point [`AppState::speleodb_done`] owns
/// it, and its rules are the ones that apply: released when the push lands, kept when
/// a retry could still recover the work.
struct AdoptLockGuard<'a> {
    state: &'a AppState,
    endpoint: &'a str,
    token: &'a str,
    insecure_http: bool,
    project_id: &'a str,
    /// False when there is nothing to hand back - either no lock was taken, or the
    /// hand-over has taken over responsibility for it.
    armed: bool,
}

impl<'a> AdoptLockGuard<'a> {
    fn new(
        state: &'a AppState,
        endpoint: &'a str,
        token: &'a str,
        insecure_http: bool,
        project_id: &'a str,
        armed: bool,
    ) -> Self {
        Self {
            state,
            endpoint,
            token,
            insecure_http,
            project_id,
            armed,
        }
    }

    /// Stop guarding: the lock now belongs to the hand-over.
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for AdoptLockGuard<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        tracing::info!(
            project_id = self.project_id,
            purpose = "hand back the lock an adoption took but never used",
            "speleodb_adopt_ariane: releasing the write lock after a failed conversion"
        );
        // Best-effort, and it drops the acquisition claim whether or not the POST
        // lands - the same sink every other abort arm releases through.
        self.state.speleodb_release_best_effort(
            self.endpoint,
            self.token,
            self.insecure_http,
            self.project_id,
        );
    }
}

/// Why a dump could not be opened without asking the user first. [#258]
///
/// Distinct from [`SpeleoDbError`] so the name-only classification stays a pure,
/// archive-free function: turning an ambiguity into the IPC error needs each
/// candidate's size and timestamp, which live in the archive the classifier
/// deliberately does not hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum UnopenableDump {
    /// `.tml`-only - route to the one-time adoption flow (#214).
    Adopt,
    /// Nothing openable at all.
    Empty,
    /// More than one `.stalx`.
    AmbiguousStalx(Vec<String>),
    /// No `.stalx` and more than one `ariane.tml`.
    AmbiguousAriane(Vec<String>),
}

/// [#258] What a caller is able to do with a dump that holds only Ariane data.
///
/// Adoption is a TIP-only operation: it imports a `.tml` by downloading the project's
/// current contents, with no import-at-hash counterpart. So the answer differs by
/// caller rather than by dump, and the caller has to say which it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ArianeRoute {
    /// Route to the one-shot adoption flow, which imports the `.tml`.
    Adopt,
    /// Refuse. For a caller with no import path, advertising an Ariane choice would
    /// offer a question whose every answer is an error.
    Refuse,
}

/// Refusal for [`ArianeRoute::Refuse`]. Says what is missing and where adoption does
/// work, so the user is not left to infer that a revision swap and adoption are
/// different operations.
const ARIANE_NOT_RESTORABLE: &str =
    "this revision holds only Ariane data and no Stalagmight file, so there is \
     nothing to view or restore; Ariane data can only be adopted from the project's \
     current revision";

/// [#258] Whether a caller can put a `.stalx` ambiguity to the user.
///
/// The same question as [`ArianeRoute`], asked about the other ambiguity, and for
/// the same reason: what to do with a dump the app cannot classify depends on the
/// CALLER's surface, not on the dump. Every frontend host that raises the picker is
/// bound to a specific verb (`dumpCandidates` is checked at each one), so the verbs
/// with no host must not answer with a candidate list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StalxAmbiguity {
    /// Report the candidates; this caller's frontend raises the picker and replays
    /// the verb with the answer. Open, open-discarding, adopt, restore and the
    /// lock-free probe behind the revert.
    Ask,
    /// Refuse with directions. The verbs behind [`AppState::speleodb_fast_forward`] -
    /// Start editing, Update, Take remote - accept no `entryPath` and have no picker
    /// bound to them, so a candidate list reaches a frontend that cannot use it.
    Refuse,
}

/// Refusal for [`StalxAmbiguity::Refuse`]. Names the flow that CAN ask, because
/// otherwise this is indistinguishable from the dead end it replaces: the verbs it
/// covers already surfaced the raw ambiguity as a bare error whenever the session had
/// no usable answer, which - before choices were dated - is exactly what our own push
/// left behind.
const AMBIGUOUS_NEEDS_REOPEN: &str =
    "this project holds more than one Stalagmight file on the server, and the file \
     you chose is not the one this revision was built from; re-open the project to \
     choose which file to use";

/// Refusal for an Ariane ambiguity that reaches a caller with no picker. Separate from
/// [`AMBIGUOUS_NEEDS_REOPEN`] only in what it names the tree as holding - the remedy is
/// the same, because re-opening is the flow that can ask either question.
const AMBIGUOUS_ARIANE_NEEDS_REOPEN: &str =
    "this project holds more than one Ariane file on the server and no Stalagmight \
     file, so there is nothing here to update; re-open the project to choose which \
     file to adopt";

/// The two shas a pull derives from one head resolution. [#258]
///
/// Deliberately not the same value, which is the whole reason this is a type and not
/// a `let`. A wrong BASE costs at most a redundant fast-forward on the next Start
/// editing, so it is allowed to fall back to the caller's advisory sha. A wrong DATE
/// costs the entire staleness guarantee, and the advisory sha is the worst imaginable
/// source for one: `latest_commit_sha` is `max(authored_date)`, so it can name the
/// root creation commit and STAY there across any number of real revisions. A choice
/// recorded under it would go on matching while the tree moved underneath - the exact
/// rollback the dating exists to prevent, wearing the mark of a guard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PullDating {
    /// May date a remembered choice. An authoritative topology tip, or nothing.
    pub(super) datable: Option<String>,
    /// Stamps BASE. Falls back to the caller's advisory sha.
    pub(super) base: Option<String>,
}

/// Split a resolved tip and the caller's advisory sha into [`PullDating`].
pub(super) fn pull_dating(tip: Option<String>, advisory: Option<String>) -> PullDating {
    PullDating {
        base: tip.clone().or(advisory),
        datable: tip,
    }
}

/// A server-reported topology tip, accepted only if it can safely be interpolated
/// into a by-hash download URL. [#258]
///
/// The tip arrives over the wire and `download_dump_at_hash` puts it straight into an
/// authenticated request path - the same boundary `speleodb_restore_revision` and the
/// lock-free probe both cross `validate_hexsha` before building that URL. Being
/// server-derived is no licence to skip it: the value is exactly as trustworthy as the
/// box we happen to be pointed at, and a malformed one reshapes the request rather
/// than failing it. Pinning the download to a sha is what made this a live path, so
/// the check has to come with it.
///
/// A rejected tip answers `None`, which every caller already handles: the dump is
/// fetched at plain HEAD, and `None` is also what says it is undatable - so nothing is
/// recalled against it and nothing is recorded. Fail closed, and never issue the
/// by-hash request at all.
///
/// The sha is deliberately kept out of the log line. It is the untrusted value, and a
/// warning is not worth handing it a way into whatever reads the logs.
pub(super) fn usable_tip(tip: Option<String>, project_id: &str) -> Option<String> {
    let sha = tip?;
    if validate_hexsha(&sha).is_ok() {
        return Some(sha);
    }
    tracing::warn!(
        project_id = %project_id,
        len = sha.len(),
        "server history reported a tip that is not a usable revision id; falling back \
         to an undated HEAD download"
    );
    None
}

/// Trust-boundary check on the lock-free probe's inputs. [#258]
///
/// Both ids reach an authenticated request path, so both are validated exactly as
/// the flows the probe predicts validate them - `speleodb_restore_revision` checks
/// the same pair before it builds its download-at-hash URL. Pure and separate from
/// the command so the check is unit-testable: inside the command every rejection
/// looks like the advisory `None` it shares with "not logged in", which would make a
/// missing guard invisible to a test.
pub(super) fn probe_inputs_valid(project_id: &str, hexsha: Option<&str>) -> bool {
    validate_project_id(project_id).is_ok() && hexsha.is_none_or(|sha| validate_hexsha(sha).is_ok())
}

/// Map a downloaded dump's entry list to the single openable `.stalx` path, or the
/// reason a human has to decide. Pure, so it is unit-testable without an archive.
pub(super) fn classify_dump_entry(names: &[String]) -> Result<String, UnopenableDump> {
    match dump::classify(names) {
        DumpClassification::Stalx(path) => Ok(path),
        DumpClassification::AdoptAriane(_) => Err(UnopenableDump::Adopt),
        DumpClassification::NothingToOpen => Err(UnopenableDump::Empty),
        DumpClassification::AmbiguousStalx(paths) => Err(UnopenableDump::AmbiguousStalx(paths)),
        DumpClassification::AmbiguousAriane(paths) => Err(UnopenableDump::AmbiguousAriane(paths)),
    }
}

/// Turn an [`UnopenableDump`] into the IPC error, reading each ambiguous
/// candidate's metadata out of the archive it came from.
pub(super) fn unopenable_to_error(
    reason: UnopenableDump,
    dump: &mut dump::Dump<'_>,
) -> SpeleoDbError {
    match reason {
        UnopenableDump::Adopt => SpeleoDbError::AdoptAriane,
        UnopenableDump::Empty => SpeleoDbError::EmptyDump,
        UnopenableDump::AmbiguousStalx(paths) => match dump.entry_details(&paths) {
            Ok(candidates) => SpeleoDbError::AmbiguousDump(candidates),
            Err(e) => e,
        },
        UnopenableDump::AmbiguousAriane(paths) => match dump.entry_details(&paths) {
            Ok(candidates) => SpeleoDbError::AmbiguousAriane(candidates),
            Err(e) => e,
        },
    }
}

/// Which entry of this dump to open, honouring an explicit [#258] choice.
///
/// `chosen` is the `entryPath` the picker sent back. It crosses IPC, so it is
/// accepted ONLY when the classifier itself offered it for THIS archive - which
/// refuses a non-openable entry and a stale pick (the dump moved on while the
/// picker was open) alike. A refused choice falls through to a fresh
/// classification, so the user is re-offered what the dump holds NOW rather than
/// told their pick was invalid.
pub(super) fn resolve_dump_entry(
    dump: &mut dump::Dump<'_>,
    chosen: Option<&str>,
) -> Result<String, SpeleoDbError> {
    let names = dump.entry_paths()?;
    if let Some(chosen) = chosen {
        match dump::chosen_candidate(&names, chosen) {
            Some(dump::ChosenKind::Stalx) => return Ok(chosen.to_owned()),
            // An `ariane.tml` is openable, but NOT by this path: every caller hands
            // the result to `replace_cache_stalx_with`, which would write TML bytes
            // into the cache `.stalx` - unopenable, and over whatever clean copy was
            // there. It routes to adoption instead, the one flow that imports rather
            // than extracts, and which carries the chosen entry so the pick is kept.
            Some(dump::ChosenKind::Ariane) => return Err(SpeleoDbError::AdoptAriane),
            None => {}
        }
    }
    classify_dump_entry(&names).map_err(|reason| unopenable_to_error(reason, dump))
}

impl AppState {
    /// [#258] Which entry of this dump to use, consulting - and then updating - the
    /// choice this session has already made for `project_id`.
    ///
    /// Every path that classifies a dump goes through here, so that recalling and
    /// remembering cannot drift apart. They did once: restore recorded the user's
    /// answer but never consulted it, so someone who had already answered at open
    /// time - before any lock existed - was asked again by the revert, which by then
    /// had taken the write lock and had to hand it back to ask.
    ///
    /// An explicit `chosen` wins over the remembered entry, but only while it is an
    /// answer to the question we asked - see `speleodb_dump_offer_answers`. A pick and
    /// a recall are now dated the same way, which they were not: dating the REMEMBERED
    /// answer while letting a fresh `entryPath` through on membership alone left the
    /// hole open at the point the user is actually deciding. The picker lists a size
    /// and a timestamp per candidate so the choice can be informed; it is a question
    /// about one tree, and confirming it replays the verb, which resolves the tip
    /// again. So an answer about A was being applied to B - and membership is exactly
    /// what a surviving backup copy passes.
    ///
    /// Neither is trusted: both are then checked for membership against the archive
    /// being extracted, so a name that made sense for the tip's tree simply falls
    /// through to the ambiguity when a revision's tree does not contain it.
    ///
    /// Membership used to be the only guard on a recall and it is nowhere near enough
    /// on its own, because the stale case USUALLY still passes it: a push writes the
    /// canonical entry and leaves the others where they are, so a choice that named a
    /// backup is still a member of every later tree while pointing at bytes from
    /// before the push. `at` is what catches that, and it catches a collaborator's
    /// push as well as our own - which membership never could.
    ///
    /// `stalx` says whether this caller's frontend can put an ambiguity to the user.
    /// The verbs behind `speleodb_fast_forward` cannot: they carry no `entryPath` and
    /// have no picker host, so handing them a candidate list is handing them a
    /// question nobody will ever be asked. They get a refusal that names the way out
    /// instead - the same shape as `ARIANE_NOT_RESTORABLE`.
    ///
    /// Only an answer the USER gave is recorded. An unambiguous dump asks nothing, so
    /// there is nothing to remember, and recording its sole `.stalx` anyway would
    /// promote an automatic classification into a standing choice: once a second
    /// `.stalx` appears the recalled name still passes membership (the old file is
    /// still in the tree), so the picker never opens, and every later pull - plus the
    /// push that follows - silently keeps using the file nobody picked.
    pub(super) fn speleodb_resolve_entry_remembering(
        &self,
        project_id: &str,
        dump: &mut dump::Dump<'_>,
        chosen: Option<&str>,
        ariane: ArianeRoute,
        at: DumpAt<'_>,
        stalx: StalxAmbiguity,
    ) -> Result<String, SpeleoDbError> {
        // A pick that answers a DIFFERENT revision is discarded here rather than
        // membership-checked, because membership is what it would pass: a push writes
        // the canonical entry and leaves every other `.stalx` where it was, so the
        // backup the user chose at A is still in B's tree. Dropping it falls through to
        // a fresh classification, which re-offers what the dump holds now - and records
        // the new revision, so the next reply matches and this cannot loop.
        let answered_here = chosen.filter(|_| self.speleodb_dump_offer_answers(project_id, at));
        let recalled = self.speleodb_recalled_dump_entry(project_id, at);
        let candidate = answered_here.or(recalled.as_deref());
        match resolve_dump_entry(dump, candidate) {
            Ok(entry) => {
                // Honoured explicit pick only: `resolve_dump_entry` returns the
                // candidate verbatim when it accepts it, and a refused pick falls
                // through to a classification that cannot equal it (a refused name is
                // not an openable entry of this archive, a classified one always is). A
                // hit recalled from the session is already recorded, so re-recording it
                // would be a no-op.
                //
                // `answered_here`, NOT `chosen`: a pick discarded for answering another
                // revision must not be recorded on its way past. An unambiguous dump
                // classifies to its sole entry, which can equal the discarded name - so
                // testing `chosen` would promote a pick we just refused into a standing
                // session answer, over a question that was never asked.
                if answered_here.is_some_and(|c| c == entry.as_str()) {
                    self.speleodb_remember_dump_entry(project_id, &entry, at);
                }
                Ok(entry)
            }
            // [#258] An ambiguity this caller has no way to ask about. Every verb
            // behind the fast-forward lands here once the tree has moved under a
            // remembered choice - which is now the ordinary outcome of a collaborator
            // pushing, not a rare one - and a candidate list would reach a frontend
            // with no picker bound to it and surface as a bare "more than one
            // Stalagmight file". Name the way out instead: re-opening the project is
            // the flow that DOES host the picker, and answering it there re-arms this
            // one for the revision the answer was actually about.
            Err(SpeleoDbError::AmbiguousDump(_)) if stalx == StalxAmbiguity::Refuse => {
                Err(SpeleoDbError::Parse(AMBIGUOUS_NEEDS_REOPEN.to_owned()))
            }
            // [#258 review] The same refusal for the OTHER ambiguity, which slipped
            // between the two flags. `ArianeRoute` answers "can you import a `.tml`?"
            // and `StalxAmbiguity` answers "can you ASK the user?" - the fast-forward
            // says yes to the first and no to the second, so an ambiguous Ariane set
            // matched neither refusing arm and shipped a candidate list to Start
            // editing / Update / Take remote, none of which has a picker bound to it.
            // Deliberately narrower than the arm above it: a LONE `ariane.tml` is a
            // routing signal the frontend does act on, so only the ambiguous set is
            // refused here.
            Err(SpeleoDbError::AmbiguousAriane(_)) if stalx == StalxAmbiguity::Refuse => Err(
                SpeleoDbError::Parse(AMBIGUOUS_ARIANE_NEEDS_REOPEN.to_owned()),
            ),
            // [#258] Ariane data, and this caller cannot import it. Refuse with a
            // reason instead of handing back an adoption signal it has no way to act
            // on: a revision swap that routed here downloaded the TIP, answering a
            // question about an old revision with today's contents. Covers the lone
            // `.tml` and the ambiguous set alike - neither is restorable, so offering
            // a picker for the second would only move the dead end one click later.
            // Nothing is remembered: a pick this flow refuses is not an answer.
            Err(SpeleoDbError::AdoptAriane | SpeleoDbError::AmbiguousAriane(_))
                if ariane == ArianeRoute::Refuse =>
            {
                Err(SpeleoDbError::Parse(ARIANE_NOT_RESTORABLE.to_owned()))
            }
            // [#258] The pick was an `ariane.tml`, so this path refuses it and the
            // flow routes to adoption. Record it anyway: adoption is a SEPARATE
            // command reached through a confirm dialog, so without this the answer
            // would be lost between the two and the picker would re-ask forever.
            // Only when it really is this archive's Ariane pick - the same refusal
            // arises for an ordinary `.tml`-only dump, where there was no choice.
            Err(SpeleoDbError::AdoptAriane) => {
                if let Some(chosen) = candidate {
                    let names = dump.entry_paths()?;
                    if dump::chosen_candidate(&names, chosen) == Some(dump::ChosenKind::Ariane) {
                        self.speleodb_remember_dump_entry(project_id, chosen, at);
                    }
                }
                Err(SpeleoDbError::AdoptAriane)
            }
            // [#258] An ambiguity that is about to be REPORTED - the refusing arms are
            // above, so reaching here means this caller does host a picker and these
            // candidates are going to the user. Record which revision they describe,
            // because the reply arrives through a SEPARATE call that resolves the tip
            // again, and that is the only way to tell an answer to this question from
            // an answer to a different one.
            Err(e @ (SpeleoDbError::AmbiguousDump(_) | SpeleoDbError::AmbiguousAriane(_))) => {
                self.speleodb_note_dump_offer(project_id, at);
                Err(e)
            }
            Err(e) => Err(e),
        }
    }

    /// [#258] Ask what a dump holds WITHOUT taking a lock or touching the cache.
    ///
    /// Exists so the revert composite can put its question before the write lock
    /// rather than after it. Restoring a revision needs the lock, so discovering the
    /// ambiguity inside that call meant taking the lock, failing, handing it back to
    /// ask, and re-taking it - a visible round trip for a question that needed no
    /// lock at all.
    ///
    /// Advisory, and deliberately never fails the caller's flow: anything that is not
    /// a clear ambiguity answers `None`, leaving the real call to produce the
    /// authoritative error. A probe that could report its own failures would just be a
    /// second place the same problem is surfaced, out of order with the flow.
    ///
    /// It used to short-circuit on ANY remembered answer, which cost no network at
    /// all and answered the wrong question. The probe is asked about a specific
    /// `hexsha`, and a session's answer is about the tree it was given - normally the
    /// tip, since opening the project is what settles this. Those are different trees
    /// whenever the revert is doing its job, so the short-circuit was skipping the
    /// requested revision on the strength of an answer that never described it. It is
    /// gone; the recall is consulted where every other path consults it, inside the
    /// resolver, which will only honour it for the revision it belongs to. The cost is
    /// the download this used to skip - which the probe was already prepared to make
    /// on every miss, and still no lock.
    pub fn speleodb_dump_candidates(
        &self,
        project_id: String,
        hexsha: Option<String>,
    ) -> Result<Option<Vec<SpeleoDumpCandidate>>, SpeleoErrorDto> {
        // Trust boundary first, before any network or session work: both ids are
        // interpolated into an authenticated URL path.
        if !probe_inputs_valid(&project_id, hexsha.as_deref()) {
            return Ok(None);
        }
        let Ok((endpoint, token)) = self.speleodb_active_token() else {
            return Ok(None);
        };
        let insecure_http = self.speleodb_insecure_http();
        let downloaded = match &hexsha {
            // Validated at the entry by `probe_inputs_valid`, exactly like the restore
            // path validates before building its own download-at-hash URL.
            Some(sha) => self.speleodb_handle_auth(
                Endpoint::DownloadDump,
                client::download_dump_at_hash(&endpoint, &token, insecure_http, &project_id, sha),
            ),
            None => self.speleodb_handle_auth(
                Endpoint::DownloadDump,
                client::download_dump(&endpoint, &token, insecure_http, &project_id),
            ),
        };
        let Ok(zip) = downloaded else { return Ok(None) };
        let Ok(mut dump) = dump::Dump::open(&zip) else {
            return Ok(None);
        };
        // Through the shared resolver, not the bare one, so the probe classifies and
        // maps errors exactly as the flow it is predicting - including whether the
        // session's answer applies to the revision being asked about, which is the whole
        // reason the short-circuit above is gone.
        //
        // ONE of the resolver's two session side effects is inert here and the other is
        // load-bearing. Nothing is remembered, because a probe passes no `chosen` and
        // "would this ask?" is not itself an answer. But an ambiguity it reports DOES
        // record the offer, and must: the reply arrives at the restore through a
        // separate call that resolves the revision again, and the offer is the only
        // thing that tells an answer to this question from an answer to another.
        //
        // The Ariane route has to match the flow being predicted, or the probe would
        // advertise a picker the real call then refuses: at a `hexsha` this predicts
        // the revision swap, which cannot import.
        let ariane = if hexsha.is_some() {
            ArianeRoute::Refuse
        } else {
            ArianeRoute::Adopt
        };
        // Predicting a flow that ASKS, in both cases: the restore behind a `hexsha`
        // raises the picker, and so does the pull without one. `Refuse` here would
        // convert the very ambiguity this exists to report into a flat message.
        match self.speleodb_resolve_entry_remembering(
            &project_id,
            &mut dump,
            None,
            ariane,
            DumpAt::from_sha(hexsha.as_deref()),
            StalxAmbiguity::Ask,
        ) {
            Ok(_) => Ok(None),
            Err(
                SpeleoDbError::AmbiguousDump(candidates)
                | SpeleoDbError::AmbiguousAriane(candidates),
            ) => Ok(Some(candidates)),
            Err(_) => Ok(None),
        }
    }

    /// List the projects visible to the logged-in user, as display summaries.
    pub fn speleodb_list_projects(&self) -> Result<Vec<SpeleoProjectSummary>, SpeleoErrorDto> {
        let (endpoint, token) = self.speleodb_active_token().map_err(local_dto)?;
        let insecure_http = self.speleodb_insecure_http();
        // [Track C] (#188, C5): route through the 401 auto-drop handler - a
        // genuine 401 on this established-session call fully clears the token -
        // then map the surfaced SpeleoDbError to the typed DTO ([Track A] D-B2).
        let projects = self
            .speleodb_handle_auth(
                Endpoint::ListProjects,
                client::list_projects(&endpoint, &token, insecure_http),
            )
            .map_err(|e| dto_for_error(Endpoint::ListProjects, &e))?;
        Ok(projects
            .into_iter()
            .map(SpeleoProjectSummary::from_project)
            .collect())
    }

    /// Pull a project's dump, extract its `.stalx` into the app cache, and return
    /// the local path. The frontend then opens it via the normal `open_project`
    /// flow. Read-only against the server (nothing is locked or pushed).
    ///
    /// [Track B] (#189, D-B9) `project_name` + `latest_commit_sha` are threaded
    /// through the open path (extra args, SAME command name → EVENT_NAMES stays
    /// 418) so the extracted `.stalx` is stamped `is_remote == true` end-to-end.
    ///
    /// FRESHNESS-FOLD SEAM (D-B1): #185's get_project/list_projects return the
    /// authoritative Project on every call. The persistence fold (cache the
    /// freshness verdict + reset last_verify_at) is a DOCUMENTED NO-OP until the
    /// #202 piggyback path consumes it. base_commit_sha is captured ONCE here on
    /// pull and NEVER mutated by #189 (a later Sync/Release issue re-stamps it).
    pub fn speleodb_open_project(
        &self,
        project_id: String,
        project_name: String,
        latest_commit_sha: Option<String>,
        entry_path: Option<String>,
    ) -> Result<SpeleoOpenResult, SpeleoErrorDto> {
        self.speleodb_pull_project(
            project_id,
            project_name,
            latest_commit_sha,
            LocalEditsPolicy::Protect,
            entry_path,
        )
    }

    /// [`Self::speleodb_open_project`] with the un-pushed-edits guard WAIVED: pull
    /// the server copy over the local one, destroying edits that were never pushed.
    ///
    /// The "take remote and discard local" branch of the prompt the refusal raises
    /// (see [`crate::speleodb::UnpushedLocalEdits`]) - the frontend calls this with
    /// the same arguments the refused open was given, and only after the user has
    /// confirmed the loss. Named, not a flag, so there is no way to reach the
    /// destructive pull without spelling it out (mirrors `speleodb_take_remote`
    /// being its own verb rather than a parameter on `speleodb_update`).
    pub fn speleodb_open_project_discarding_local(
        &self,
        project_id: String,
        project_name: String,
        latest_commit_sha: Option<String>,
        entry_path: Option<String>,
    ) -> Result<SpeleoOpenResult, SpeleoErrorDto> {
        self.speleodb_pull_project(
            project_id,
            project_name,
            latest_commit_sha,
            LocalEditsPolicy::Discard,
            entry_path,
        )
    }

    /// Shared body of the two pull verbs above; `policy` is what separates them.
    fn speleodb_pull_project(
        &self,
        project_id: String,
        project_name: String,
        latest_commit_sha: Option<String>,
        policy: LocalEditsPolicy,
        entry_path: Option<String>,
    ) -> Result<SpeleoOpenResult, SpeleoErrorDto> {
        // Validate the id at the trust boundary, before it reaches the download
        // URL or the cache-path join (closes the path/route-traversal vector).
        validate_project_id(&project_id).map_err(local_dto)?;
        let (endpoint, token) = self.speleodb_active_token().map_err(local_dto)?;
        let insecure_http = self.speleodb_insecure_http();

        let dest = self.speleodb_cache_path(&project_id).map_err(local_dto)?;
        // Run the un-pushed-edits guard BEFORE the download, not just inside the
        // replace. It is a purely LOCAL check (it reads the cached copy's link row),
        // so nothing about it needs the dump - and when it is going to refuse, every
        // byte of that dump is downloaded, held in memory and thrown away first. With
        // the dump ceiling at [`client::DUMP_MAX_BYTES`] that is a long wait and a
        // large allocation spent to arrive at a decision we could have made
        // immediately. The call inside `replace_cache_stalx` stays as the backstop
        // that no call site can forget (and it is what the OTHER two replace flows
        // rely on); this one is purely to fail fast.
        if policy == LocalEditsPolicy::Protect {
            self.guard_local_edits(&project_id, &dest)
                .map_err(local_dto)?;
        }

        // FRESHNESS-FOLD SEAM (D-B1, #189/#191): #185's get_project/list_projects
        // return the authoritative Project; the pull stamp (project_name +
        // latest_commit_sha → speleo_link) and the cache-the-verdict fold land
        // with #189/#191 - this call site is where they hook in.
        //
        // [Track C] (#188, C5): route through the 401 auto-drop handler before
        // mapping the surfaced SpeleoDbError to the typed DTO ([Track A] D-B2).
        // Resolve the server's TRUE HEAD (the revision-topology leaf) FIRST, and then
        // fetch that commit's dump BY HASH.
        //
        // The order is the point. Downloading HEAD and asking what HEAD is are two
        // requests, so a push landing between them hands back commit A's tree
        // labelled B - and both things that label it matter: BASE is stamped from it,
        // and [#258] so is the remembered entry choice. A copy of A stamped B looks
        // current to every later fence, so Start editing sees BASE == B, never
        // reconciles, and the next push puts A over B. Pinning the download to the
        // sha we resolved removes the window rather than narrowing it: the bytes, the
        // date on the choice and BASE are then one immutable object by construction.
        //
        // NOT the list/get_project `latest_commit` field, which is
        // `max(authored_date)` and ties to the root creation commit (see
        // speleodb_server_head). Fetch the history DIRECTLY here (NOT via
        // speleodb_handle_auth) so a transient/spurious 401 on this best-effort
        // resolve cannot auto-drop the whole session mid-open; Start editing / Done DO
        // route revisions through the auto-drop (a dead token there must force
        // re-login). (#197 review.)
        let tip = match client::get_revisions(&endpoint, &token, insecure_http, &project_id) {
            // Through `usable_tip`: what comes back is about to be interpolated into an
            // authenticated by-hash URL, so it crosses the same boundary the restore
            // and the probe cross before building one.
            Ok(history) => usable_tip(
                sync::tip_of(&history.commits).map(str::to_owned),
                &project_id,
            ),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    project_id = %project_id,
                    purpose = "resolve server HEAD on pull",
                    "speleodb_open_project: could not resolve server HEAD; \
                     falling back to an undated HEAD download"
                );
                None
            }
        };
        let zip = self
            .speleodb_handle_auth(
                Endpoint::DownloadDump,
                match tip.as_deref() {
                    Some(sha) => client::download_dump_at_hash(
                        &endpoint,
                        &token,
                        insecure_http,
                        &project_id,
                        sha,
                    ),
                    // No resolvable tip (an empty repo, or a history call that failed):
                    // take HEAD as before. What comes back is then genuinely undatable,
                    // which `tip` being `None` already says.
                    None => client::download_dump(&endpoint, &token, insecure_http, &project_id),
                },
            )
            .map_err(|e| dto_for_error(Endpoint::DownloadDump, &e))?;

        // [#258] The advisory sha may stamp BASE and may NOT date a choice - see
        // `PullDating` for why those are different questions.
        let PullDating {
            datable,
            base: base_sha,
        } = pull_dating(tip, latest_commit_sha);
        let dated_at = DumpAt::from_sha(datable.as_deref());

        // Open the dump once: list entries (to classify) and extract the chosen
        // `.stalx` from the same parsed archive.
        let mut dump = dump::Dump::open(&zip).map_err(local_dto)?;
        let entry = self
            .speleodb_resolve_entry_remembering(
                &project_id,
                &mut dump,
                entry_path.as_deref(),
                ArianeRoute::Adopt,
                dated_at,
                // The pull is where the picker lives: both its verbs carry an
                // `entryPath`, and all three frontend hosts raise the dialog on the
                // candidates this reports.
                StalxAmbiguity::Ask,
            )
            .map_err(local_dto)?;

        // Carry the user's local view toggles across the download-replace (a re-open
        // of a previously-customized remote project shouldn't reset their view -
        // display_settings are stripped from the server copy). Empty on first open.
        // Also drops the live handle when this very cache file is the open project
        // (a re-pull of what's already open); the FE re-opens it via `open_project`
        // once this returns.
        //
        // Under `Protect` this REFUSES when the cached copy still holds un-pushed
        // edits: re-opening a project from the SpeleoDB list is an ordinary,
        // non-destructive-looking action, so it must never be the thing that
        // destroys a week of unpushed survey. The refusal crosses IPC typed, and
        // the frontend re-enters through `speleodb_open_project_discarding_local`
        // if - and only if - the user chose to abandon the local copy.
        let preserved_view = self
            .replace_cache_stalx_with(
                &mut dump,
                &entry,
                &dest,
                &project_id,
                policy,
                // [#303] A pull adopts the server's tree; its undo history is the last
                // pusher's and is never inherited implicitly.
                InheritedUndo::Drop,
            )
            .map_err(local_dto)?;

        // Concurrent same-account session notice (#193 b8): if the server already
        // reports a lock held by our OWN account, a second session is editing this
        // project - the ancestry classifier's "author_email == me" proxy can't tell
        // the two sessions apart (a second session's commit reads as "mine" and
        // could wrongly clear the dirty bit), so surface a notice on open. Computed
        // here, before `endpoint`/`token` are moved into the stamped link.
        // Best-effort: a failed lookup (or no session email) never fails the open.
        let concurrent_session = {
            let holder = self.speleodb_lock_holder(&endpoint, &token, insecure_http, &project_id);
            self.speleodb_my_email()
                .is_some_and(|me| sync::concurrent_session_notice(holder.as_deref(), &me))
        };

        // [Track B] (#189, D-B9) Stamp the SpeleoDB hand-off link into the pulled
        // `.stalx`. Opening via `open_existing` runs `ensure_schema_version`,
        // which creates the `speleo_link` table on the freshly-extracted file;
        // then upsert the single row. After this, `load_metadata().is_remote` is
        // true and the dirty bit (`has_unpushed_edits`) starts false.
        match crate::persistence::Database::open_existing(&dest) {
            Ok(db) => {
                restore_display_settings(&db, &preserved_view);
                // [#303] Read BEFORE the upsert below overwrites the whole row.
                let pulled_undo_mode = preserved_undo_mode(&db);
                let link = crate::models::SpeleoLink {
                    endpoint,
                    project_id: project_id.clone(),
                    project_name,
                    base_commit_sha: base_sha,
                    has_unpushed_edits: false,
                    last_edit_at: None,   // freshly pulled: no local edits yet
                    last_verify_at: None, // no lock held on pull; stamped on acquire (#202)
                    // [#303] Carried from the file we just extracted. A legacy NULL is
                    // initialized once from the application default and stamped explicitly.
                    undo_mode: Some(self.effective_undo_mode(pulled_undo_mode)),
                };
                if let Err(e) = db.save_speleo_link(Some(&link)) {
                    // Non-fatal: the pull succeeded and the file is openable; the
                    // worst case is the project opens as local-only (editable)
                    // rather than read-only-remote. Surfacing a hard error here
                    // would strand a successfully-downloaded `.stalx`.
                    tracing::warn!(
                        error = %e,
                        project_id = %project_id,
                        purpose = "stamp SpeleoDB hand-off link on pull",
                        "speleodb_open_project: failed to stamp speleo_link; project will open local-only"
                    );
                }
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    project_id = %project_id,
                    purpose = "stamp SpeleoDB hand-off link on pull",
                    "speleodb_open_project: could not reopen extracted .stalx to stamp speleo_link"
                );
            }
        }

        Ok(SpeleoOpenResult {
            local_path: dest.to_string_lossy().into_owned(),
            concurrent_session,
        })
    }

    /// [#214] Adopt an Ariane (`.tml`-only) SpeleoDB project: the ONE sanctioned
    /// server-`.tml` read, a one-time bootstrap (never an ongoing reconcile).
    ///
    /// The whole hand-over, in order: take the project's write mutex, download the
    /// dump, import its `ariane.tml` through the existing TML importer, save a new
    /// `.stalx` at the same cache path a pull would use, stamp the hand-off link,
    /// push it with its Ariane sidecar, release the mutex, flip the server project
    /// `type` ariane→stalagmight, and close so the webview's own load is a real load.
    ///
    /// It DOES push - that is the point of taking the lock first, and of taking it
    /// before the download rather than after the import. Only the routes that cannot
    /// push leave the conversion local: a mutex held by anyone, an unreadable lock
    /// probe, no write access, or no network. Those are not failures - the project is
    /// adopted either way, `has_unpushed_edits` stays set, and [`AdoptUpload`] says
    /// which happened so the webview can too.
    ///
    /// The FE calls this only after `speleodb_open_project` surfaced
    /// `SpeleoDbAction::Adopt` and the user confirmed. Adoption replaces the open
    /// `AppState` project (the FE runs its usual unsaved-changes guard first, exactly
    /// as for any open); the webview then reloads `local_path`, which opens
    /// read-only-remote (`is_remote`, #191) once the lock has been handed back.
    ///
    /// `discard_local == true` waives the un-pushed-edits guard below, for the ONE
    /// caller that arrives having already had that loss confirmed - see the command
    /// wrapper.
    pub fn speleodb_adopt_ariane(
        &self,
        project_id: String,
        project_name: String,
        latest_commit_sha: Option<String>,
        discard_local: bool,
        entry_path: Option<String>,
    ) -> Result<SpeleoAdoptResult, SpeleoErrorDto> {
        validate_project_id(&project_id).map_err(local_dto)?;

        // THE LOCK-OPS SPAN OPENS FIRST, BEFORE THE ACCOUNT IS READ.
        //
        // It has to, and the sibling path says so in as many words ("The guard comes
        // FIRST, before the account is read", `speleodb_session`). Opening it around
        // the acquire alone leaves the token read and the holder probe - a whole
        // network round trip - outside, and a logout that completes in THAT window is
        // the same failure with more steps: our acquire lands on the server with a
        // token captured beforehand, and `speleodb_record_claim` then drops the claim
        // because there is no account left to stamp it with. Server mutex held, nothing
        // recording it, freed only by the server-side timeout.
        //
        // It closes before the head resolve. Start editing holds this across its whole
        // acquire → commit, which is seconds; adoption's download-and-convert is not,
        // and a span held across it would block logout - and every other lock operation
        // - for as long as an import takes.
        let (endpoint, token, insecure_http, my_email, concurrent_session, lock) = {
            let _lock_ops = self.speleodb_lock_ops_guard();
            let (endpoint, token) = self.speleodb_active_token().map_err(local_dto)?;
            let insecure_http = self.speleodb_insecure_http();

            // Concurrent same-account session notice (mirrors the pull, #193 b8), read
            // BEFORE the lock below so it describes the world adoption arrived in - after
            // it, the probe would be reporting on this very call.
            //
            // The same answer decides whether to take the lock at all, so it is read once
            // and passed down: a mutex anybody holds is one this flow declines to push
            // through.
            //
            // The CHECKED probe, which keeps "unheld" and "could not find out" apart. The
            // collapsing variant beside it is right for the display probes it was written
            // for - an unknown holder and no holder both render as "unknown" - and wrong
            // here for the same reason it is wrong for `speleodb_start_editing`: this flow
            // RELEASES on the way out, so reading a failed GET as "unheld" is what lets an
            // adoption acquire over a mutex our own account is already holding in another
            // window (that acquire is a server-side no-op, so it cannot report the
            // collision) and then hand it back out from under that session, which goes on
            // editing until its next push 403s. Another ACCOUNT's lock is safe either way -
            // the server answers that acquire with the 400 - so it is only our own that
            // needs the probe to be honest about not knowing.
            let holder =
                self.speleodb_lock_holder_checked(&endpoint, &token, insecure_http, &project_id);
            // The account whose token takes the mutex below, captured HERE rather than
            // re-read later. The release verdict at the end of this function compares
            // the server's holder against it, and a sign-out or account switch landing
            // in between changes what `speleodb_my_email()` answers without changing who
            // acquired - so a live re-read there would be comparing the mutex against a
            // stranger's identity, or against no identity at all.
            let my_email = self.speleodb_my_email();
            let concurrent_session = my_email.as_deref().is_some_and(|me| {
                sync::concurrent_session_notice(holder.as_ref().ok().and_then(Option::as_deref), me)
            });

            // THE LOCK COMES FIRST - before the head resolve, before the download.
            //
            // Adoption reads a tree and writes it straight back, so everything between
            // those two acts is a window in which somebody else's push makes the bytes we
            // converted stale. Resolving the head and fetching by hash closes the window
            // between the two REQUESTS, but not the one that spans the import, and the
            // import is the slow part (parse the `.tml`, build the project, save it). Take
            // the write mutex first and the window shuts completely: no other writer can
            // land a revision between the download and the upload, so the tree we converted
            // is still the tip when we push it, and the divergence arms below become
            // unreachable rather than merely unlikely.
            //
            // Best-effort, and that is the whole design of it: a lock we cannot take is a
            // reason to adopt WITHOUT pushing, never a reason to refuse the adoption. A
            // READ_ONLY member and an offline machine both converted their project before
            // this flow existed, and both still do - the copy simply stays local, stamped
            // with un-pushed edits, which is exactly what it is.
            let lock = self.adopt_take_lock(&endpoint, &token, insecure_http, &project_id, holder);
            (
                endpoint,
                token,
                insecure_http,
                my_email,
                concurrent_session,
                lock,
            )
        };

        // Arms the hand-back for every way the stretch below can return early. Disarmed
        // once there is a converted project for the lock to be about - see the guard.
        let mut lock_guard = AdoptLockGuard::new(
            self,
            &endpoint,
            &token,
            insecure_http,
            &project_id,
            matches!(lock, AdoptLock::Held),
        );

        // [#258] Resolve the head BEFORE the download and fetch that commit by hash,
        // for the reason the pull does: two requests mean a push can land between
        // them, and a tree fetched at A but dated B would let a choice recorded here
        // be recalled against a dump it never described. Best-effort - an unresolvable
        // head just means the bytes are undatable, which is what `None` says, and
        // adoption then asks again through the picker it already hosts.
        //
        // DIRECTLY, not via `speleodb_server_head`, which wraps the same call in
        // `speleodb_handle_auth`. That handler `forget()`s the token on a 401 - keychain
        // entry included - and because this failure is deliberately swallowed and
        // `token` was captured above, routing it there let a spurious 401 delete the
        // user's stored credential while the adopt went on to succeed: signed out with
        // no signal and nothing to sign back in with. The pull states the rule for this
        // exact shape of call, and this function restates it 150 lines down for the type
        // flip; only the resolve was routed through the drop.
        let adopt_at = match client::get_revisions(&endpoint, &token, insecure_http, &project_id) {
            Ok(history) => usable_tip(
                sync::tip_of(&history.commits).map(str::to_owned),
                &project_id,
            ),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    project_id = %project_id,
                    purpose = "date a remembered Ariane pick on adopt",
                    "speleodb_adopt_ariane: could not resolve server HEAD; re-asking instead \
                     of honouring an undatable choice"
                );
                None
            }
        };
        // Download the dump (routed through the auth handler like the pull, so a dead
        // token forces re-login).
        let zip = self
            .speleodb_handle_auth(
                Endpoint::DownloadDump,
                match adopt_at.as_deref() {
                    Some(sha) => client::download_dump_at_hash(
                        &endpoint,
                        &token,
                        insecure_http,
                        &project_id,
                        sha,
                    ),
                    None => client::download_dump(&endpoint, &token, insecure_http, &project_id),
                },
            )
            .map_err(|e| dto_for_error(Endpoint::DownloadDump, &e))?;

        // Classify authoritatively. Only proceed for a genuine Ariane-only dump; if a
        // `.stalx` has since appeared (a stale `type_tag`, a TOCTOU race with another
        // writer), refuse and tell the user to reopen normally rather than clobber it.
        let mut dump = dump::Dump::open(&zip).map_err(local_dto)?;
        let names = dump.entry_paths().map_err(local_dto)?;
        // [#258] A dump with several `ariane.tml` is ambiguous, and this is the flow
        // that answers it: the picker's choice arrives here, checked for membership
        // against THIS archive exactly like a `.stalx` pick. Without it the picker
        // would re-raise the same question on every retry, because nothing else on
        // this path can act on the answer.
        // The picker's answer reaches this command either directly, or - when the
        // ambiguity surfaced on the OPEN path, which refuses a `.tml` and routes
        // here - through the entry that path recorded for this session. That hand-off
        // is why `adopt_at` above is the TRUE HEAD and not the caller-supplied
        // `latest_commit_sha`: the open recorded the answer against the topology tip,
        // so reading it back against `max(authored_date)` would never match and the
        // picker would re-ask forever.
        //
        // This classifier is adoption's own - it wants an Ariane entry and refuses a
        // `.stalx` with its own message - so the resolver's dating does not reach it,
        // and BOTH halves of that dating have to be repeated here or the whole
        // guarantee stops at the `.stalx` picker. An `ariane.tml` behaves exactly like
        // a `.stalx` under a collaborator's push: the canonical file moves, every other
        // entry stays where it was, so a backup chosen at A sails through membership at
        // B - and adoption then imports it and stamps BASE at B.
        let adopt_dump_at = DumpAt::from_sha(adopt_at.as_deref());
        let remembered = self.speleodb_recalled_dump_entry(&project_id, adopt_dump_at);
        let picked = entry_path
            .as_deref()
            // The fresh reply is gated exactly as the resolver gates one: honoured only
            // while it answers the revision its candidates were offered for. The recall
            // beside it is already dated, so a discarded reply correctly falls back to
            // a remembered answer that IS about this tree, and otherwise to a re-offer.
            .filter(|_| self.speleodb_dump_offer_answers(&project_id, adopt_dump_at))
            .or(remembered.as_deref())
            .filter(|chosen| {
                dump::chosen_candidate(&names, chosen) == Some(dump::ChosenKind::Ariane)
            });
        let tml_entry = match (picked, dump::classify(&names)) {
            (Some(chosen), _) => chosen.to_owned(),
            (None, DumpClassification::AdoptAriane(tml)) => tml,
            (None, DumpClassification::Stalx(_)) => {
                return Err(local_dto(SpeleoDbError::Parse(
                    "this project now has a Stalagmight file on the server; reopen it normally"
                        .to_owned(),
                )));
            }
            (None, DumpClassification::NothingToOpen) => {
                return Err(local_dto(SpeleoDbError::EmptyDump))
            }
            // Both ambiguities record the offer, for the same reason the resolver does:
            // these candidates are going to a user, and the reply arrives through a
            // separate call that resolves the tip again. The `.stalx` set is no
            // exception - the frontend routes it by kind to the OPEN verb, which reads
            // this same record.
            (None, DumpClassification::AmbiguousStalx(paths)) => {
                self.speleodb_note_dump_offer(&project_id, adopt_dump_at);
                return Err(local_dto(unopenable_to_error(
                    UnopenableDump::AmbiguousStalx(paths),
                    &mut dump,
                )));
            }
            (None, DumpClassification::AmbiguousAriane(paths)) => {
                self.speleodb_note_dump_offer(&project_id, adopt_dump_at);
                return Err(local_dto(unopenable_to_error(
                    UnopenableDump::AmbiguousAriane(paths),
                    &mut dump,
                )));
            }
        };

        // Extract the `ariane.tml` to a scratch file beside the cache `.stalx`, import
        // it, then remove the scratch file. The importer needs a path on disk.
        let dest = self.speleodb_cache_path(&project_id).map_err(local_dto)?;
        // The one cache replace that does NOT run through `replace_cache_stalx`:
        // `adopt_import_and_save` deletes `dest` and rebuilds it from the imported
        // `.tml`, so it needs the same data-safety gate. Re-adoption is reachable
        // (an adopted project is stamped dirty, and the server type flip is
        // best-effort - a failed flip leaves the project still offering Adopt), and
        // it would otherwise delete the adopted copy's un-pushed edits.
        //
        // Waived only for the caller that has already had exactly this loss
        // confirmed: "take the server copy and discard my local changes" on a
        // refused open, which lands here when the project is still Ariane. Refusing
        // that user again asks the same question twice and answers it with a dead
        // end, since re-adopting IS the only way to open the project at all.
        //
        // Written as the plain condition it is: adoption rebuilds `dest` itself
        // instead of going through `replace_cache_stalx_with`, so it has no
        // [`LocalEditsPolicy`] to pass on - the value used to be constructed here and
        // then only compared against, purely so a source-string tripwire in
        // `speleodb_cache` would count it. See that module's test for what pins the
        // waiver now.
        if !discard_local {
            self.guard_local_edits(&project_id, &dest)
                .map_err(local_dto)?;
        }
        let tml_scratch = dest.with_file_name("adopt-source.tml");
        dump.extract_entry_to(&tml_entry, &tml_scratch)
            .map_err(local_dto)?;
        let adopt =
            self.adopt_import_and_save(&tml_scratch, &tml_entry, &dest, project_name.clone());
        // Best-effort scratch cleanup regardless of import outcome.
        let _ = std::fs::remove_file(&tml_scratch);
        // [#411] What the conversion actually did, carried to the return below rather
        // than dropped here: the call site was the last place it existed, so a pulled
        // Ariane project lost its excluded shots and every parse anomaly in silence.
        let import_receipt = adopt?;

        // BASE = the commit this project was actually built from, which is the one
        // already resolved above and downloaded by hash - NOT a second lookup here.
        //
        // A second lookup reintroduces the window the by-hash download closed, and on
        // the worst possible side of it. The import between them is slow (parse the
        // `.tml`, build the project, save it), so a revision landing during it is not
        // exotic; the copy would then be A-derived content stamped BASE = B, and
        // stamped `has_unpushed_edits` too. Start editing reads BASE == tip and calls
        // it UpToDate, so nothing ever fast-forwards, and the push then reads base ==
        // tip and plans a plain Upload - putting A over B with no divergence raised.
        // Silently overwriting the very revision that landed while we were importing.
        //
        // Keeping the original sha instead means BASE is A, so if B did land the copy
        // is honestly behind: the fence sees a commit that is not ours and raises the
        // divergence prompt, which is the outcome the user can act on.
        let PullDating { base: base_sha, .. } = pull_dating(adopt_at, latest_commit_sha);

        // Stamp the hand-off link onto the project this call just built, through the
        // LIVE handle rather than a second connection over the same file: the
        // hand-over below reads it straight back (`speleodb_done` decides whether to
        // push at all from `has_unpushed_edits`), so the two must be the same view.
        //
        // `has_unpushed_edits = true` because at this instant it is TRUE - the server
        // has never been given this `.stalx`. The hand-over clears it by pushing; every
        // path that does not push leaves it standing, which is what arms the quit guard
        // over a conversion that exists only in this cache.
        //
        // `last_edit_at` is stamped WITH it, and must be: a dirty bit with no edit time
        // is one a confirmed push can never clear. `edits_captured_by_push` reads a
        // missing timestamp as "unknown timing" and keeps the bit - the conservative
        // answer, since an edit it cannot date might postdate the upload - so a link
        // stamped dirty-but-undated pushes successfully, stays dirty, reports
        // `PushUnconfirmed`, and holds the lock for a retry that will do the same thing
        // again. That is the exact defect the v9→v10 migration had to backfill the
        // epoch for, and adoption was minting fresh rows with it.
        //
        // The conversion IS the edit and it happened just now, so `now` is both true and
        // the value that works: the push below starts after this line, so
        // `last_edit_at <= push_start` and a landed push clears the bit.
        //
        // [#303] Adoption builds the `.stalx` from an imported `.tml`, so there is
        // normally no prior link to carry an undo policy from - but the file can be a
        // re-adopt over an existing cache copy, and that copy's override is still the
        // project's answer. Read it off the SAME live handle before the upsert
        // overwrites the row; no link yet initializes from the global default once.
        let adopted_undo_mode = self.speleodb_open_link().ok().and_then(|l| l.undo_mode);
        let stamped = self.speleodb_set_open_link(&crate::models::SpeleoLink {
            endpoint: endpoint.clone(),
            project_id: project_id.clone(),
            project_name,
            base_commit_sha: base_sha,
            has_unpushed_edits: true,
            last_edit_at: Some(chrono::Utc::now().to_rfc3339()),
            last_verify_at: None,
            undo_mode: Some(self.effective_undo_mode(adopted_undo_mode)),
        });
        if let Err(e) = &stamped {
            tracing::warn!(
                error = %e,
                project_id = %project_id,
                purpose = "stamp SpeleoDB hand-off link on adopt",
                "speleodb_adopt_ariane: failed to stamp speleo_link; project will open local-only"
            );
        }

        // The conversion exists AND it is linked, so the lock has a hand-over to
        // belong to: the guard's "release it, nothing came of this" duty ends here,
        // and `speleodb_done`'s rules take over (release when the push lands, keep it
        // when a retry could still recover the work).
        //
        // Conditional on the stamp, because an unstamped project has no hand-over to
        // give the lock TO. `speleodb_start_editing` opens by reading the link
        // (`speleodb_session_snapshot` -> `speleodb_open_link`), so with no link row it
        // returns `NoRemoteProject` before touching the mutex - and a guard disarmed
        // ahead of that leaves the lock held by nobody's code, until the server-side
        // timeout. The documented "retry from the status button" is no recovery either:
        // that button reads the very link that failed to write. Only the next login's
        // held-lock prompt, which is server-derived, would ever find it.
        if stamped.is_ok() {
            lock_guard.disarm();
        }

        // Upload the converted `.stalx` with its Ariane sidecar and hand the lock
        // back. Best-effort by construction: every failure arm leaves the adopted
        // project on disk, stamped dirty, and reports why - adoption that could not
        // reach the server is still an adoption.
        let upload = match (stamped, lock) {
            (Ok(()), AdoptLock::Held) => self.adopt_hand_over(&project_id),
            (Ok(()), AdoptLock::NotTaken(why)) => why,
            // Nothing to hand over: the push reads the link to decide whether to run at
            // all. Say so rather than letting `start_editing` discover it and report the
            // same thing one indirection later. The guard above is still armed, so the
            // lock this call took goes back on the way out.
            (Err(e), _) => AdoptUpload::Failed {
                message: format!("the converted project could not be linked to SpeleoDB: {e}"),
            },
        };

        // ONE lock-ops span over the whole read → decide → clear below, for the reason
        // this module gives everywhere else it compares a claim and then acts on it: the
        // two must not be separable. The holder read and the claim clear are otherwise
        // minutes apart in machine terms - two HTTP calls - and a `speleodb_start_editing`
        // landing in that gap for THIS project and account records a REPLACEMENT claim
        // (`speleodb_record_claim` drops the same-project entry before pushing the new
        // one). The clear matches on project + account, so it would retire that new
        // claim instead of the stale one, leaving the fresh edit session holding a server
        // mutex with no janitor retry pointer and no authorship provenance.
        //
        // Opened HERE and not earlier: `adopt_hand_over` above runs `speleodb_start_editing`
        // and `speleodb_done`, which take this same guard, and it is not reentrant. Held
        // across a GET and a PATCH, which is the shape Start Editing already holds it
        // across - not across the download and import, which is why the acquire span
        // further up closes before them.
        let (finalized, upload) = {
            let _lock_ops = self.speleodb_lock_ops_guard();

            // Make sure the server project ends up typed `stalagmight`, and report whether it
            // did. Only after a landed push, which is the precondition either way:
            // flipping first leaves a project typed `stalagmight` whose only file is the Ariane
            // one that type says to disregard, openable by nobody.
            let finalized = if matches!(upload, AdoptUpload::Pushed | AdoptUpload::PushedLockHeld) {
                self.adopt_finalize(&endpoint, &token, insecure_http, &project_id)
            } else {
                AdoptFinalize::not_attempted()
            };

            // Correct a release verdict the hand-over could only guess at. It reports
            // `PushedLockHeld` whenever `speleodb_done` returned `Err` over a landed
            // push - but that `Err` covers a release the server REFUSED and one whose
            // success we failed to read alike, and the GET above has just asked the
            // server outright. A mutex that is not ours means the release did land,
            // whatever the transport said on the way back.
            //
            // Only ever demotes, and only on a POSITIVE answer. There are two ways not
            // to have one, and both keep the protective arm: a holder we could not read,
            // and an identity of our own we could not establish. `lock_held_by_me`
            // answers false for either, so testing `!lock_held_by_me(..)` alone would
            // read "we do not know who we are" as "the lock is somebody else's" and drop
            // the warning in exactly the case with least evidence for it - the same "we
            // could not ask" becoming "the lock is free" that the holder side is guarded
            // against. `my_email` is `None` on a poisoned session mutex as readily as on
            // a sign-out, so this needs no race to reach.
            let upload = match (upload, &finalized.holder) {
                (AdoptUpload::PushedLockHeld, Some(holder))
                    if my_email.is_some()
                        && !lock_held_by_me(holder.as_deref(), my_email.as_deref()) =>
                {
                    tracing::info!(
                        holder = ?holder,
                        project_id,
                        purpose = "correct a release that landed despite reporting an error",
                        "speleodb_adopt_ariane: the write lock is not ours; the release did land"
                    );
                    // The claim says we hold a mutex the server has just told us we do
                    // not. Left standing it outlives this call: the close teardown, the
                    // janitor and the held-lock prompt all read it, and each would try to
                    // hand back a lock that is either gone or, if another writer took it
                    // in between, somebody else's.
                    //
                    // Scoped to THIS project. `speleodb_note_lock_acquired(None)` says
                    // "the account holds nothing" and drops EVERY claim it owns -
                    // including a lock deliberately kept on another project, whose
                    // janitor retry pointer and authorship provenance would go with it,
                    // over a call that established nothing about that project at all.
                    //
                    // And scoped to the account that TOOK it, for the same reason the
                    // guard above compares against a captured identity: the plain
                    // `speleodb_forget_lock_acquired_for` resolves the account from the
                    // live session, which a sign-out or switch landing in here has
                    // already moved. The claim would then outlive the demotion that
                    // disproved it, and the reclaim check is account-blind - so it would
                    // go on standing a later release down over a mutex the server has
                    // told us is not ours.
                    if let Some(email) = my_email.as_deref() {
                        let _ =
                            self.speleodb_forget_lock_acquired_as(&project_id, &endpoint, email);
                    }
                    AdoptUpload::Pushed
                }
                (upload, _) => upload,
            };

            (finalized, upload)
        };
        let type_flipped = finalized.type_flipped;

        // Hand the project BACK to the frontend to open, instead of leaving it
        // committed to `AppState` for a `loadProject` that then short-circuits.
        //
        // `do_create_project_full` inside the import commits the adopted project
        // unconditionally, so on return the backend held it open at `dest` - and
        // `do_load_project_full` answers a load of the path it already has open with
        // `ALREADY_OPEN`. The frontend reads that as "it is on screen already" (the
        // only way a pull can produce it) and short-circuits to a view flip, so
        // adoption finished with the view switched, no project in any store, and a
        // success toast over an empty screen. Closing here makes the frontend's load
        // an ordinary full load, which is how the pull path has always behaved: it
        // runs `close_project_if_open_at` before it replaces a cache file, and the
        // frontend's `loadProject` is what opens the result.
        //
        // LAST, because everything above needs the project open: the link stamp writes
        // through the live handle, and the hand-over renders the sidecar from the open
        // survey. A held lock is deliberately NOT released here - `speleodb_done`'s
        // non-landing arms keep it so a retry can recover, and the frontend re-opens
        // this very project a moment later.
        self.close_project();

        Ok(SpeleoAdoptResult {
            local_path: dest.to_string_lossy().into_owned(),
            type_flipped,
            concurrent_session,
            upload,
            import_receipt,
        })
    }

    /// Take the write mutex for an adoption that is about to download and convert,
    /// so no other writer can land a revision inside that window.
    ///
    /// `holder` is the pre-read lock holder, with its three outcomes kept apart.
    /// Anybody at all stops the acquire: another account's mutex is not ours to take,
    /// and our own account's - a second window, the website - belongs to a session
    /// that is mid-edit, which [`AppState::speleodb_start_editing`] would read as
    /// inherited and [`AppState::speleodb_done`] would then hand back out from under
    /// them. A probe that FAILED stops it too, for the same reason in its unknowable
    /// form: an acquire over a lock our account already holds is a server-side no-op,
    /// so nothing later in the flow can discover the collision, and "cannot say" must
    /// never be read as "unheld" by a caller that will release on the way out.
    ///
    /// Never fails the adoption. Every arm that does not end in
    /// [`AdoptLock::Held`] carries the [`AdoptUpload`] the caller reports instead
    /// of pushing, so "we could not take the lock" travels as a described outcome
    /// rather than as an absence.
    ///
    /// The acquire goes through the auth handler, unlike the head resolve and the
    /// type flip beside it. Those two are deliberately swallowed, which is what makes
    /// a spurious 401 there invisible AND destructive (it drops the keychain entry
    /// mid-adopt). This one is not swallowed: a 401 here is a token that will fail the
    /// very next request too, so the drop is the correct response and the user is told
    /// to sign in again - the same reading `speleodb_start_editing` takes of its own
    /// acquire.
    fn adopt_take_lock(
        &self,
        endpoint: &str,
        token: &str,
        insecure_http: bool,
        project_id: &str,
        holder: Result<Option<String>, SpeleoDbError>,
    ) -> AdoptLock {
        let holder = match holder {
            Ok(holder) => holder,
            // Unreadable. In practice the download two lines down is about to fail the
            // same way, and the adoption then fails outright - so refusing to push here
            // usually costs nothing, and when it does cost something it costs the push,
            // never the conversion.
            Err(e) => {
                tracing::info!(
                    error = %e,
                    project_id,
                    purpose = "refuse to acquire over a lock whose holder we could not read",
                    "speleodb_adopt_ariane: lock state unknown; the conversion will stay local"
                );
                return AdoptLock::NotTaken(AdoptUpload::Failed {
                    message: format!("could not check who holds the write lock: {e}"),
                });
            }
        };
        if let Some(email) = holder {
            tracing::info!(
                holder = %email,
                project_id,
                purpose = "decline to convert-and-push through somebody else's write lock",
                "speleodb_adopt_ariane: lock held; the conversion will stay local"
            );
            return AdoptLock::NotTaken(AdoptUpload::LockHeld { email: Some(email) });
        }

        // NO lock-ops guard here: the caller holds it across this whole call, from
        // before the account read to after the claim below. Taking it again would
        // self-deadlock - `std::sync::Mutex` is not reentrant - so this function must
        // only ever be called from inside that span.
        let acquired = self.speleodb_handle_auth(
            Endpoint::Acquire,
            client::acquire_lock(endpoint, token, insecure_http, project_id),
        );

        match acquired {
            Ok(_project) => {
                // Recorded so the rest of the app can account for this mutex: the
                // janitor, the close teardown and the held-lock prompt all read the
                // claim, and a lock taken without one is a lock only the server-side
                // timeout can free. Inside the caller's span, so no teardown can land
                // between the mutex becoming ours and the record saying so.
                self.speleodb_note_lock_acquired(Some(project_id));
                AdoptLock::Held
            }
            // 400 is the server's "someone else holds it" (no first-class status row,
            // per #194) - the probe above lost a race with another writer.
            Err(SpeleoDbError::Status { status: 400, .. }) => {
                AdoptLock::NotTaken(AdoptUpload::LockHeld { email: None })
            }
            // A READ_ONLY member (403) and an unreachable server land here alike, and
            // both mean the same thing to the adoption: convert locally, push later.
            Err(e) => {
                tracing::info!(
                    error = %e,
                    project_id,
                    purpose = "adopt without the write lock when it cannot be taken",
                    "speleodb_adopt_ariane: could not take the lock; the conversion will stay local"
                );
                AdoptLock::NotTaken(AdoptUpload::Failed {
                    message: e.to_string(),
                })
            }
        }
    }

    /// Leave the server project typed `stalagmight`, and report both what it ended up
    /// typed and who the server says holds its write mutex.
    ///
    /// Called only after a landed push, and it is a READ first: the push's own type
    /// heal has usually flipped it already, so asking costs one GET and the common case
    /// issues no write at all.
    ///
    /// The holder rides along on that same GET because it is free there and it is the
    /// only evidence that separates a release which failed server-side from one whose
    /// 2xx we simply did not manage to read. `speleodb_done`'s release is a
    /// non-idempotent POST whose client also parses the response, so a mid-response
    /// transport failure returns `Err` over a mutex the server has already handed back;
    /// [`AdoptUpload::PushedLockHeld`] then told the user to go and release a lock that
    /// no longer existed. See the caller for what it does with the answer.
    ///
    /// Adoption used to PATCH here unconditionally, which meant every adoption sent the
    /// same PATCH twice - and `type_flipped` was decided by the SECOND one, so a
    /// transient failure on a redundant request told the user "its SpeleoDB type could
    /// not be changed" about a project whose type had just been changed. Reading first
    /// removes the redundant write AND makes the answer the server's rather than an
    /// inference from our own request's exit code.
    ///
    /// A GET we cannot make falls through to the PATCH rather than giving up. Delegating
    /// the whole job to the heal was the other way round - it declines to act on a type
    /// it could not read, which is right for a generic push (blindly re-typing someone
    /// else's Compass project would be worse than doing nothing) and wrong here, because
    /// adoption is the one caller that KNOWS the project was Ariane: it just converted
    /// it. Losing that knowledge turned a transient GET failure into a project left
    /// typed `ariane` with a `.stalx` on it, still offering Adopt over a copy that
    /// already exists.
    ///
    /// Called directly rather than through the auth handler, like the head resolve
    /// above: this runs after the conversion is safely local and its failure is
    /// swallowed, so a spurious 401 here would drop the session - keychain entry
    /// included - over work that already landed.
    fn adopt_finalize(
        &self,
        endpoint: &str,
        token: &str,
        insecure_http: bool,
        project_id: &str,
    ) -> AdoptFinalize {
        let (needs_flip, mut holder) =
            match client::get_project(endpoint, token, insecure_http, project_id) {
                // Already `stalagmight`: the push's heal got there first, which is the
                // ordinary path. (A legacy `other` is not managed - SpeleoDB never maps
                // it - so it falls through to the PATCH below.) Nothing to do, and nothing to report but success - but the holder
                // still comes back, because the release verdict needs it either way.
                Ok(fetched) if type_says_managed(Some(&fetched.kind)) => {
                    return AdoptFinalize {
                        type_flipped: true,
                        holder: Some(fetched.active_mutex.map(|m| m.user)),
                    }
                }
                // Anything else: still `ariane`, or re-typed under us to `compass` /
                // `therion` while the adoption ran. Both get the PATCH. This test used to
                // be `!type_still_says_ariane`, which treated every non-Ariane label as
                // "already managed" - so a project whose metadata changed mid-run was
                // reported flipped with the server left on the other type. We converted
                // this project; whatever the label says now, the artefact up there is a
                // `.stalx` and `stalagmight` is the truthful type for it.
                Ok(fetched) => (true, Some(fetched.active_mutex.map(|m| m.user))),
                Err(e) => {
                    tracing::info!(
                        error = %e,
                        project_id,
                        purpose = "flip the adopted project's type without a readable current type",
                        "speleodb_adopt_ariane: could not read the project type; flipping anyway"
                    );
                    (true, None)
                }
            };
        debug_assert!(needs_flip);

        let type_flipped = match client::patch_project(
            endpoint,
            token,
            insecure_http,
            project_id,
            &managed_type_patch(),
        ) {
            Ok(patched) => {
                // The PATCH echoes the project back, mutex included - so where the GET
                // above failed, this is the only holder reading the whole flow gets, and
                // throwing it away left the verdict `PushedLockHeld` (and the claim
                // standing) over a mutex this response could prove free.
                //
                // Fills an unknown; never OVERRIDES a reading. `active_mutex` is an
                // `Option` the API may simply omit, and an omitted field would read as
                // "nobody holds it" - which demotes. Confined to the case where the
                // alternative is no answer at all, that can only improve on unknown;
                // preferring it over a good GET could turn one into a wrong answer.
                if holder.is_none() {
                    holder = Some(patched.active_mutex.map(|m| m.user));
                }
                true
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    project_id,
                    purpose = "flip SpeleoDB project type ariane->stalagmight on adopt",
                    "speleodb_adopt_ariane: type flip failed (likely no write access); adopted locally"
                );
                false
            }
        };
        AdoptFinalize {
            type_flipped,
            holder,
        }
    }

    /// Hand the just-converted project to SpeleoDB over the lock this adoption is
    /// already holding: open the edit session, push the `.stalx` with the Ariane
    /// sidecar `speleodb_done` renders, release. The user's adoption is one click, so
    /// its result should be a project that LIVES on the server - not a converted copy
    /// sitting in a cache directory waiting for someone to remember to push it.
    ///
    /// Composed from the two ordinary verbs rather than a private upload, so the
    /// fence, the ancestry recovery and the release rules are the ones already
    /// audited for those paths. `speleodb_start_editing` reads our lock as INHERITED
    /// (the account already holds it), which is exactly right: it opens the edit gate
    /// without minting a second authorship claim over the one
    /// [`Self::adopt_take_lock`] recorded.
    ///
    /// NEVER returns an error: the converted project is already on disk by the time
    /// this runs, and no failure here should turn a finished local conversion into a
    /// failed command. Each arm reports what happened so the webview can seed its
    /// dirty bit honestly and say what is left to do.
    fn adopt_hand_over(&self, expected_id: &str) -> AdoptUpload {
        let outcome = self.adopt_hand_over_inner(expected_id);
        // Logged HERE rather than at each arm, so no arm can be added without one. The
        // outcomes that never reached the server all reach the user as the same "it
        // stayed local" toast, deliberately - there is nothing they can do differently
        // about a lock held versus a push that did not confirm - which makes this the
        // only record of WHICH it was. Diagnosing the first of these without it meant
        // reading the server's mutex table.
        match &outcome {
            AdoptUpload::Pushed => tracing::info!(
                purpose = "confirm the adoption reached the server",
                "speleodb_adopt_ariane: conversion pushed and the lock handed back"
            ),
            // Its OWN arm. Sharing the one above asserted the opposite of the server
            // state - it says the lock came back, and this outcome is defined by its
            // not having - and carried no `outcome` field to tell the two apart, so the
            // trail for a stuck adoption read as a clean finish. Since the holder
            // re-check this also covers "the release may have landed but we could not
            // establish whose the mutex is", which is why the message does not assert
            // that it is still ours.
            AdoptUpload::PushedLockHeld => tracing::warn!(
                outcome = ?outcome,
                purpose = "report an adoption whose upload landed but whose lock did not come back",
                "speleodb_adopt_ariane: conversion pushed; the write lock is still ours or \
                 could not be confirmed released"
            ),
            other => tracing::warn!(
                outcome = ?other,
                purpose = "explain an adoption that converted but did not upload",
                "speleodb_adopt_ariane: the conversion stayed local"
            ),
        }
        outcome
    }

    /// [`Self::adopt_hand_over`] without the logging, so the arms stay readable.
    ///
    /// `expected_id` is the project this adoption converted, and the landed-but-locked
    /// inference below is only sound about THAT project's link - see the arm.
    fn adopt_hand_over_inner(&self, expected_id: &str) -> AdoptUpload {
        match self.speleodb_start_editing() {
            Ok(StartEditingResult::Editing) => {}
            // Unreachable while the lock is ours - nobody can have advanced the tip
            // since the download - but kept as arms rather than an `unreachable!`,
            // because the thing making them unreachable is a mutex on another machine.
            //
            // A fast-forward is likewise impossible here (the fence reaches it only
            // when the copy is clean, and this one is dirty by construction); folded in
            // with the divergence because both mean the same thing to the caller: the
            // server moved, so nothing was pushed.
            Ok(StartEditingResult::FastForwarded { .. } | StartEditingResult::DivergenceNeeded) => {
                return AdoptUpload::Diverged
            }
            Ok(StartEditingResult::LockedBy { email }) => return AdoptUpload::LockHeld { email },
            Err(e) => return AdoptUpload::Failed { message: e.message },
        }

        match self.speleodb_done(Some(ADOPT_COMMIT_MESSAGE.to_owned())) {
            Ok(DoneResult::Released) => AdoptUpload::Pushed,
            Ok(DoneResult::DivergenceNeeded) => AdoptUpload::Diverged,
            // Both mean the same thing to a caller with nothing left to retry with:
            // the bytes did not land. The lock stays held, by `speleodb_done`'s own
            // contract, so the user's retry from the status button can recover.
            Ok(DoneResult::NotEditing | DoneResult::PushUnconfirmed) => AdoptUpload::Failed {
                message: "the upload did not complete; the converted project is still local"
                    .to_owned(),
            },
            // An error from Done does NOT mean the bytes did not land. Its last act on
            // a landed push is to hand the lock back, and it reports a failure there by
            // returning `Err` - AFTER the upload has been accepted and the dirty bit
            // cleared. Reporting that as `Failed` told a user whose survey was safely on
            // the server that it was "on this machine only", which is the most alarming
            // way to be wrong about where someone's only copy is.
            //
            // The link is what tells the two apart, and it is already on disk: a push
            // that landed clears `has_unpushed_edits`, and nothing else in this flow
            // clears it. Read it back rather than guessing from the error, which cannot
            // distinguish a release that failed from an upload that never happened.
            //
            // The link has to be the ADOPTED project's for any of that to follow.
            // `speleodb_open_link` answers for whatever is open in `AppState`, not for a
            // project id, so a load landing under an in-flight hand-over would have it
            // read a stranger's link - and an unrelated CLEAN project then satisfies the
            // predicate, reporting an upload that never happened and seeding the
            // conversion clean over a copy that exists only in the cache. Nothing in this
            // flow is supposed to permit that swap, which is exactly why a mismatch is
            // logged as an error rather than absorbed: it means an invariant this
            // inference rests on has gone, and the log is the only place that would say
            // so. `Failed` is the fail-closed direction either way - it keeps the dirty
            // seed and arms the quit guard.
            Err(e) => match self.speleodb_open_link() {
                Ok(link) if link.project_id != expected_id => {
                    tracing::error!(
                        expected = expected_id,
                        found = %link.project_id,
                        purpose = "prove the adoption's own push landed",
                        "speleodb_adopt_ariane: the open link names a DIFFERENT project; \
                         the open database changed under the hand-over"
                    );
                    AdoptUpload::Failed { message: e.message }
                }
                Ok(link) if !link.has_unpushed_edits => {
                    tracing::warn!(
                        error = %e.message,
                        purpose = "report an adoption whose upload landed but whose lock did not come back",
                        "speleodb_adopt_ariane: pushed, but the write lock is still held"
                    );
                    AdoptUpload::PushedLockHeld
                }
                // Still dirty, or unreadable: neither proves a landing.
                _ => AdoptUpload::Failed { message: e.message },
            },
        }
    }

    /// Import an extracted `ariane.tml` into a fresh project and save it as a new
    /// `.stalx` at `dest`. Split out of [`Self::speleodb_adopt_ariane`] so the pure
    /// create → import → save pipeline is separable from the network/link steps.
    ///
    /// Reuses the existing importer defaults (`include_excluded = false`,
    /// `comments_on_stations = true`) - the same settings the collab `.tml`
    /// round-trip and the FE import dialog use, so adoption yields the identical
    /// survey a manual TML import would. Overwrites any stale cache `.stalx`
    /// (re-adoption is a clean re-pull, like the pull path).
    ///
    /// Built into a STAGING file beside `dest` and renamed over it only once the
    /// survey is complete on disk, which is the same order the pull path's extract
    /// uses and for the same two reasons.
    ///
    /// It used to delete `dest` and rebuild in place, so the copy being replaced was
    /// destroyed before anything that could fail had run: a `.tml` that would not
    /// parse cost the user the cached project as well as the adoption, and a
    /// re-adopt over un-pushed work turned a refusal into a loss if any later step
    /// gave way. Renaming means every failure below leaves `dest` byte-for-byte as
    /// it was - and the only window where it is not is the rename itself, which the
    /// filesystem makes atomic.
    ///
    /// The half-built project is closed on the failure path as well. `do_create_project_full`
    /// commits into `AppState` before the parse, so an import that failed after it left
    /// the backend holding a project the shell was not rendering, and every later
    /// command - each edit, each auto-save - landed on that stub. The previously open
    /// project is reopened behind it where there was one, which is the same undo the
    /// cache replace performs when its extract fails.
    fn adopt_import_and_save(
        &self,
        tml_path: &Path,
        entry_name: &str,
        dest: &Path,
        project_name: String,
    ) -> Result<Option<ImportReceipt>, SpeleoErrorDto> {
        // Captured BEFORE the create commits over it, so the failure path can put it
        // back rather than leaving the backend with nothing open under a shell that is
        // still drawing it.
        let previous = self.open_project_path();
        // Beside `dest`, so the promote is a same-directory rename: cross-filesystem
        // renames are not atomic and would reintroduce the torn window this avoids.
        let staged = dest.with_file_name(adopt_staging_name());
        // A leftover from an adoption that died mid-flight IN THIS PROCESS. Create
        // reserves the exact path only when nothing is there (it auto-renames around an
        // existing file), and the pid in the name is what keeps this from clearing a
        // sibling instance's live staging database.
        if let Some(dir) = staged.parent() {
            sweep_own_staging(dir);
        }

        let receipt = match self.adopt_build_into(tml_path, entry_name, &staged, project_name) {
            Ok(receipt) => receipt,
            Err(e) => {
                self.adopt_abandon_staging(&staged, previous.as_deref());
                return Err(e);
            }
        };

        // Close before the rename: `Database::open_existing` holds a 256 MB mmap over
        // the staging file, and on Windows SQLite opens without `FILE_SHARE_DELETE`,
        // so renaming a live handle is the hazard `close_project_if_open_at` exists
        // for. This is the same close, at the same point in the same sequence.
        self.close_project();
        if let Err(e) = std::fs::rename(&staged, dest) {
            // `rename` is atomic, so a failure leaves `dest` exactly as it was.
            self.adopt_abandon_staging(&staged, previous.as_deref());
            return Err(local_dto(SpeleoDbError::Io(format!(
                "could not put the adopted project in place: {e}"
            ))));
        }

        // Reopen at the canonical path, because everything after this reads the OPEN
        // project: the link stamp writes through the live handle, and the hand-over
        // renders the sidecar from the in-memory survey. The legacy open is the right
        // one here for the reason its doc gives - this is a reopen of a canonical cache
        // path, the same case the failed-replace recovery uses it for.
        let reopened = self.open_project(dest.to_path_buf());
        if !reopened.success {
            // The rename already consumed the staging file, so there is nothing to
            // clean up - but the close above still has to be undone. Without this the
            // one arm that fails AFTER a successful promote is also the one arm that
            // leaves the backend with nothing open, under a shell still rendering the
            // old project and still sending it edits: exactly the hazard the abandon
            // path exists to prevent, reintroduced by the one exit that skipped it.
            if let Some(previous) = previous.as_deref() {
                let restored = self.open_project(previous.to_path_buf());
                if !restored.success {
                    tracing::warn!(
                        error = ?restored.error,
                        path = %previous.display(),
                        purpose = "undo the pre-promote close after the adopted project failed to open",
                        "speleodb_adopt_ariane: could not reopen the project the adoption replaced"
                    );
                }
            }
            return Err(local_dto(SpeleoDbError::Io(format!(
                "could not open the adopted project: {}",
                reopened.error.unwrap_or_else(|| "unknown error".to_owned())
            ))));
        }
        Ok(receipt)
    }

    /// Create → import → save the `ariane.tml` at `tml_path` into a project saved at
    /// `staged`. The pure pipeline half of [`Self::adopt_import_and_save`], split out
    /// so its caller has ONE failure path to clean up after rather than four.
    ///
    /// Returns the import receipt, which travels back to the frontend on
    /// [`SpeleoAdoptResult::import_receipt`] and is rendered by the same receipt
    /// dialog every other foreign-format import ends in. `entry_name` is the file's
    /// name INSIDE the dump, because that is the name the user recognises - the path
    /// this reads is a scratch copy beside the cache.
    fn adopt_build_into(
        &self,
        tml_path: &Path,
        entry_name: &str,
        staged: &Path,
        project_name: String,
    ) -> Result<Option<ImportReceipt>, SpeleoErrorDto> {
        self.do_create_project_full(project_name, None, None, Some(staged.to_path_buf()), true)
            .map_err(|e| local_dto(SpeleoDbError::Io(e.to_string())))?;

        let tml = crate::parsers::tml::parse_tml(tml_path)
            .map_err(|e| local_dto(SpeleoDbError::Parse(e.to_string())))?;
        let (sections, closures, warnings) = adopt_convert(&tml)?;
        let sections_found = tml.sections.len() as u32;
        let unit_conversion = crate::parsers::tml::tml_unit_conversion_note(tml.unit);
        // Auto: adoption is an ordinary Ariane import with no dialog to carry a
        // per-file override, so its virtual shots get the chain rule (#27).
        let auto = VirtualShotMode::Auto;
        // Entrance elevation (#431), same no-data convention as execute_tml_import.
        let surface_elevation =
            crate::models::ariane_elevation_or_absent(tml.first_start_absolute_elevation);
        let mut imported = self.execute_tml_import_sections(
            sections,
            closures,
            None,
            Vec::new(),
            Vec::new(),
            auto,
            surface_elevation,
        );

        if !imported.success {
            return Err(local_dto(SpeleoDbError::Parse(format!(
                "could not import the Ariane .tml: {}",
                imported.error.unwrap_or_else(|| "unknown error".to_owned())
            ))));
        }
        let receipt = adopt_receipt(
            &mut imported,
            entry_name,
            sections_found,
            unit_conversion,
            warnings,
        );

        let saved = self.execute(Command::SaveProject {
            path: Some(staged.to_path_buf()),
        });
        if !saved.success {
            return Err(local_dto(SpeleoDbError::Io(format!(
                "could not save the adopted project: {}",
                saved.error.unwrap_or_else(|| "unknown error".to_owned())
            ))));
        }
        Ok(receipt)
    }

    /// Throw away a staging file the adoption is not going to promote, and put the
    /// project that was open before it back.
    ///
    /// Both halves matter and neither is optional. The staging file is a whole survey
    /// on disk that nothing will ever open again; the reopen is what stops a failed
    /// adoption from leaving the backend empty - or, before the staging existed,
    /// holding a stub - under a shell that is still rendering the old project and
    /// still sending it edits.
    ///
    /// Best-effort throughout: this runs on a path that is ALREADY returning an error,
    /// and the caller's error is the one worth reporting.
    fn adopt_abandon_staging(&self, staged: &Path, previous: Option<&Path>) {
        // Closing is what releases the handle the remove needs on Windows. Harmless
        // when the create never got as far as committing anything.
        self.close_project();
        if let Err(e) = std::fs::remove_file(staged) {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(
                    error = %e,
                    path = %staged.display(),
                    purpose = "clean up an adoption that will not be promoted",
                    "speleodb_adopt_ariane: could not remove the staging .stalx"
                );
            }
        }
        let Some(previous) = previous else { return };
        let reopened = self.open_project(previous.to_path_buf());
        if reopened.success {
            tracing::info!(
                path = %previous.display(),
                purpose = "undo the create that a failed adoption committed",
                "speleodb_adopt_ariane: reopened the project the adoption replaced"
            );
        } else {
            tracing::warn!(
                error = ?reopened.error,
                path = %previous.display(),
                purpose = "undo the create that a failed adoption committed",
                "speleodb_adopt_ariane: could not reopen the project the adoption replaced"
            );
        }
    }

    /// Fast-forward the open remote project to the server tip: re-pull the dump,
    /// re-stamp the link (`base = tip`, dirty cleared, `last_edit_at` reset), then
    /// reload the project in place (resetting undo/redo). Returns the reloaded
    /// `.stalx` path.
    ///
    /// `policy` because the two callers mean opposite things by the same mechanics:
    ///
    /// - `speleodb_start_editing` / `speleodb_update` reach the FF arm only on
    ///   [`ReconcileVerdict::FastForward`], which `sync::fence` yields exclusively
    ///   in its `!dirty` branch - so there are no un-pushed edits to lose and
    ///   `Protect` is a no-op there. It is passed anyway, as the assertion: if that
    ///   invariant ever breaks, the FF refuses instead of overwriting.
    /// - `speleodb_take_remote` is only ever reached WITH un-pushed edits and exists
    ///   to discard them, behind the frontend's second destroy-confirmation, so it
    ///   passes `Discard`.
    pub(super) fn speleodb_fast_forward(
        &self,
        endpoint: &str,
        token: &str,
        insecure_http: bool,
        link: &crate::models::SpeleoLink,
        tip: Option<&str>,
        policy: LocalEditsPolicy,
    ) -> Result<String, SpeleoDbError> {
        let project_id = &link.project_id;
        // [#258 review] Pinned to the sha this call is fast-forwarding TO, because that
        // sha is what dates the remembered choice below AND what stamps BASE. It used to
        // download plain HEAD and assert in a comment that the two agreed; they are two
        // requests, `tip` was resolved by the caller's fence, and `speleodb_update`
        // holds no lock while `speleodb_take_remote` releases before getting here - so a
        // push landing in between handed back commit B's tree labelled A. This is the
        // window the pull and adopt paths were both changed to close; the fast-forward
        // is the one that kept the claim without the fix.
        let pinned = usable_tip(tip.map(str::to_owned), project_id);
        let zip = self.speleodb_handle_auth(
            Endpoint::DownloadDump,
            match pinned.as_deref() {
                Some(sha) => {
                    client::download_dump_at_hash(endpoint, token, insecure_http, project_id, sha)
                }
                // No usable tip: take HEAD as before, and say so by dating it `Unknown`
                // rather than with a sha these bytes are not known to be.
                None => client::download_dump(endpoint, token, insecure_http, project_id),
            },
        )?;
        let mut dump = dump::Dump::open(&zip)?;
        // [#258] No `entryPath` reaches this verb - it honours whatever the session
        // already settled, and a miss still fails closed rather than guessing.
        // Adoption is reachable from here: this verb pulls the TIP, which is exactly
        // what adoption would import, so routing to it answers the same question.
        //
        // `tip` is what this whole call is fast-forwarding TO, so it is also what the
        // dump it just downloaded is - which makes it the right yardstick for a
        // remembered answer. A choice made at an earlier revision no longer applies,
        // and that is precisely the case this verb must not paper over: the entry it
        // names is still in the tree (a push leaves every non-canonical `.stalx`
        // alone), so membership would wave it through and extract pre-push bytes under
        // a BASE stamped at the new tip.
        //
        // And `Refuse`, because there is nowhere to ask: no picker is bound to Start
        // editing, Update or Take remote.
        let entry = self.speleodb_resolve_entry_remembering(
            project_id,
            &mut dump,
            None,
            ArianeRoute::Adopt,
            DumpAt::from_sha(pinned.as_deref()),
            StalxAmbiguity::Refuse,
        )?;
        let dest = self.speleodb_cache_path(project_id)?;
        // Capture the user's local view toggles BEFORE the download replaces the
        // file - they're stripped from the server copy, so they'd otherwise reset
        // to defaults on every fast-forward - and close the live handle to this
        // file before the replace. The reload below re-opens it and already resets
        // undo/redo, so hoisting the close ahead of the extract is behaviour-
        // preserving.
        let preserved_view = self.replace_cache_stalx_with(
            &mut dump,
            &entry,
            &dest,
            project_id,
            policy,
            // [#303] A fast-forward re-pulls the server copy and then OPENS the edit
            // gate, so inheriting the pusher's stack here would put a stranger's
            // operations one Ctrl-Z away.
            InheritedUndo::Drop,
        )?;

        // Re-stamp the hand-off link onto the refreshed file: BASE advances to the
        // server tip, the dirty bit clears, last_edit_at resets (clean checkout).
        match crate::persistence::Database::open_existing(&dest) {
            Ok(db) => {
                restore_display_settings(&db, &preserved_view);
                // [#303] The fast-forwarded file is the server's, so its undo policy is
                // authoritative; read it before this re-stamp overwrites the row.
                let pulled_undo_mode = preserved_undo_mode(&db);
                let new_link = crate::models::SpeleoLink {
                    endpoint: endpoint.to_owned(),
                    project_id: project_id.clone(),
                    project_name: link.project_name.clone(),
                    // The same sha the dump was fetched at, so BASE cannot name a
                    // commit these bytes did not come from.
                    base_commit_sha: pinned.clone(),
                    has_unpushed_edits: false,
                    last_edit_at: None,
                    // A fresh pull confirms BASE == the server tip as of now, so this
                    // IS a lock re-verify point (#202): stamp it so the 20-min lazy
                    // re-verify starts its window from the pull, not from acquire-time.
                    last_verify_at: Some(chrono::Utc::now().to_rfc3339()),
                    undo_mode: Some(self.effective_undo_mode(pulled_undo_mode)),
                };
                if let Err(e) = db.save_speleo_link(Some(&new_link)) {
                    tracing::warn!(
                        error = %e,
                        project_id = %project_id,
                        purpose = "re-stamp speleo_link after fast-forward",
                        "speleodb_fast_forward: failed to re-stamp speleo_link"
                    );
                }
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    project_id = %project_id,
                    purpose = "re-stamp speleo_link after fast-forward",
                    "speleodb_fast_forward: could not reopen extracted .stalx to re-stamp"
                );
            }
        }

        // Reload from the refreshed cache file. close_project clears the in-memory
        // project so do_load_project_full's "already open" guard won't reject the
        // same path; do_load_project_full re-opens it.
        //
        // [#303] The pulled snapshot MAY carry undo history now - a Full-mode push ships
        // it deliberately - so the reload starting with empty stacks is no longer a
        // property of the push. It is a property of the replace above, which passed
        // `InheritedUndo::Drop` and emptied those tables in the downloaded file. That
        // matters most on this path: a fast-forward opens the edit gate straight
        // afterwards, so an inherited stack would be one Ctrl-Z from replaying another
        // editor's operation (#194 b3).
        let local_path = dest.to_string_lossy().into_owned();
        self.close_project();
        self.do_load_project_full(dest)
            .map_err(|e| SpeleoDbError::Io(format!("reload after fast-forward failed: {e}")))?;
        // [#303] The working copy is now the server tip, so any Unwind chain position
        // from before describes a copy that no longer exists - the next Unwind restarts
        // from the new BASE. The replaced file carries no chain row either (it is
        // stripped from every push snapshot), so this is belt-and-braces for the case
        // where the replace left the old one behind.
        self.speleodb_set_unwound_from(None);
        // ...and the same for the inherited-history watermark: the stack this copy carries
        // came from the server tip, not from a chain step, so nothing in it is inherited.
        self.speleodb_set_inherited_through(None);
        Ok(local_path)
    }
}

#[cfg(test)]
#[expect(clippy::expect_used, reason = "unit test fixtures")]
mod tests {
    use super::{
        adopt_convert, classify_dump_entry, probe_inputs_valid, pull_dating, resolve_dump_entry,
        usable_tip, AdoptLockGuard, ArianeRoute, DumpAt, PullDating, StalxAmbiguity,
        UnopenableDump,
    };
    use crate::core::state::tests_support::speleodb_prod_matches;
    use crate::core::state::AppState;
    use crate::parsers::tml::test_helpers::{parse_synthetic_tmlu, wrap_xml, TripShot};
    use crate::parsers::tml::{TmlFile, TmlSection, TmlShot, TmlShotType, TmlUnit};
    use crate::speleodb::dump::Dump;
    use crate::speleodb::SpeleoDbError;

    /// The revision the resolve tests pretend their dumps came from. Most of them
    /// are not about dating at all, so they use one sha throughout and let the recall
    /// behave as it did before choices carried a revision.
    const TIP: &str = "1111111111111111111111111111111111111111";
    /// A later revision, for the tests that ARE about dating.
    const NEXT: &str = "2222222222222222222222222222222222222222";

    /// A dump ZIP holding `entries`, for the resolve tests below.
    fn make_zip(entries: &[&str]) -> Vec<u8> {
        use std::io::Write as _;
        let mut writer = zip::write::ZipWriter::new(std::io::Cursor::new(Vec::<u8>::new()));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for name in entries {
            writer.start_file(*name, opts).expect("start_file");
            writer.write_all(b"x").expect("write entry");
        }
        writer.finish().expect("finish zip").into_inner()
    }

    /// [#258] An automatic classification is NOT an answer, so it must not be
    /// recorded as one. Recording it made a sole `.stalx` a standing session choice:
    /// the moment a collaborator added a second one the recall still validated - the
    /// original file is still in the tree - so the picker never opened and every
    /// later pull, plus the push after it, silently kept using the file nobody chose.
    #[test]
    fn an_unambiguous_dump_records_no_choice_so_a_later_rival_still_asks() {
        let state = AppState::new().expect("state");
        let project = "11111111-1111-1111-1111-111111111111";

        // One `.stalx`: resolved without a question, so nothing is remembered.
        let sole = make_zip(&["backup/old.stalx"]);
        let mut dump = Dump::open(&sole).expect("open");
        assert_eq!(
            state
                .speleodb_resolve_entry_remembering(
                    project,
                    &mut dump,
                    None,
                    ArianeRoute::Adopt,
                    DumpAt::Revision(TIP),
                    StalxAmbiguity::Ask,
                )
                .expect("sole .stalx opens"),
            "backup/old.stalx"
        );
        assert_eq!(
            state.speleodb_recalled_dump_entry(project, DumpAt::Revision(TIP)),
            None,
            "an unasked question must leave no remembered answer"
        );

        // A rival appears. The old name is still a member, so a recorded choice would
        // have been silently reused; with none recorded the user is asked instead.
        let rivals = make_zip(&["backup/old.stalx", "project.stalx"]);
        let mut dump = Dump::open(&rivals).expect("open");
        assert!(
            matches!(
                state.speleodb_resolve_entry_remembering(
                    project,
                    &mut dump,
                    None,
                    ArianeRoute::Adopt,
                    DumpAt::Revision(TIP),
                    StalxAmbiguity::Ask,
                ),
                Err(SpeleoDbError::AmbiguousDump(_))
            ),
            "a second .stalx must raise the ambiguity, not resolve from a phantom choice"
        );
    }

    /// The other half of the same rule: an answer the user actually gave IS recorded,
    /// so the next flow in the session does not re-ask. A pick the archive refuses is
    /// not an answer and leaves nothing behind.
    #[test]
    fn an_explicit_pick_is_remembered_and_a_refused_one_is_not() {
        let state = AppState::new().expect("state");
        let project = "22222222-2222-2222-2222-222222222222";
        let zip = make_zip(&["a.stalx", "b.stalx"]);

        let mut dump = Dump::open(&zip).expect("open");
        let refused = state.speleodb_resolve_entry_remembering(
            project,
            &mut dump,
            Some("gone.stalx"),
            ArianeRoute::Adopt,
            DumpAt::Revision(TIP),
            StalxAmbiguity::Ask,
        );
        assert!(matches!(refused, Err(SpeleoDbError::AmbiguousDump(_))));
        assert_eq!(
            state.speleodb_recalled_dump_entry(project, DumpAt::Revision(TIP)),
            None,
            "a pick this archive does not offer is not an answer"
        );

        let mut dump = Dump::open(&zip).expect("open");
        assert_eq!(
            state
                .speleodb_resolve_entry_remembering(
                    project,
                    &mut dump,
                    Some("b.stalx"),
                    ArianeRoute::Adopt,
                    DumpAt::Revision(TIP),
                    StalxAmbiguity::Ask,
                )
                .expect("honoured"),
            "b.stalx"
        );
        assert_eq!(
            state
                .speleodb_recalled_dump_entry(project, DumpAt::Revision(TIP))
                .as_deref(),
            Some("b.stalx"),
            "the user's answer must survive to the next flow in this session"
        );
    }

    /// [#258] Adoption downloads the project TIP, so a revision swap that routed to
    /// it would answer a question about an old `hexsha` with today's contents. Both
    /// Ariane shapes therefore refuse under `Refuse` - offering a picker for the
    /// ambiguous one would only move the dead end one click later - and a refused
    /// pick leaves no remembered answer behind.
    #[test]
    fn a_revision_refuses_ariane_instead_of_routing_to_a_tip_only_adoption() {
        let state = AppState::new().expect("state");
        let project = "33333333-3333-3333-3333-333333333333";

        for entries in [vec!["ariane.tml"], vec!["one/ariane.tml", "two/ariane.tml"]] {
            let zip = make_zip(&entries);
            let mut dump = Dump::open(&zip).expect("open");
            let refusal = state.speleodb_resolve_entry_remembering(
                project,
                &mut dump,
                None,
                ArianeRoute::Refuse,
                DumpAt::Revision(TIP),
                StalxAmbiguity::Ask,
            );
            assert!(
                matches!(refusal, Err(SpeleoDbError::Parse(_))),
                "{entries:?} must refuse with a reason, not an adoption signal"
            );
        }

        // Even an explicit Ariane pick records nothing on a flow that refuses it.
        let zip = make_zip(&["one/ariane.tml", "two/ariane.tml"]);
        let mut dump = Dump::open(&zip).expect("open");
        let _ = state.speleodb_resolve_entry_remembering(
            project,
            &mut dump,
            Some("two/ariane.tml"),
            ArianeRoute::Refuse,
            DumpAt::Revision(TIP),
            StalxAmbiguity::Ask,
        );
        assert_eq!(
            state.speleodb_recalled_dump_entry(project, DumpAt::Revision(TIP)),
            None,
            "a pick the flow refuses must not be remembered as an answer"
        );
    }

    /// [#258] A push writes the canonical name and touches nothing else, so a
    /// remembered entry is pre-push data the instant the push lands. Left in place it
    /// was a silent rollback: the next same-session open or fast-forward extracted
    /// that stale database and stamped it with the new server revision, publishing a
    /// commit that claims to hold work it does not.
    #[test]
    fn a_landed_push_forgets_every_remembered_choice() {
        let state = AppState::new().expect("state");
        let project = "55555555-5555-5555-5555-555555555555";

        // A canonical BASENAME earns no exemption, and this is the case that proves
        // why: with two of them in the tree, the one the user picked may be the
        // backup, and the push updates the other. A name that only says "could be
        // the target" cannot license keeping it - the same conclusion
        // `Dump::entry_details` reaches about the same predicate.
        for entry in [
            "backup/old.stalx",
            "backup/project.stalx",
            "misc/project.stalx",
            "project.stalx",
        ] {
            state.speleodb_remember_dump_entry(project, entry, DumpAt::Revision(TIP));
            state.speleodb_forget_dump_entry(project);
            assert_eq!(
                state.speleodb_recalled_dump_entry(project, DumpAt::Revision(TIP)),
                None,
                "`{entry}` must not survive a push: nothing here proves it is the \
                 file the upload actually replaced"
            );
        }
    }

    /// [#258] The rollback the rule above prevents for OUR push has a second author:
    /// anyone else with write access. Their push updates the canonical entry and
    /// leaves the backup exactly where it is, so the remembered name still passes
    /// membership - the guard the recall used to rely on - while pointing at bytes
    /// from before their work. Extracting that and stamping BASE at the new tip
    /// leaves the copy looking current, and the next push replaces their revision
    /// with no divergence to stop it. Nothing local can observe their push, so the
    /// choice has to carry the revision it was an answer about.
    #[test]
    fn a_choice_is_an_answer_about_one_revision_and_not_the_next() {
        let state = AppState::new().expect("state");
        let project = "66666666-6666-6666-6666-666666666666";

        // The user picks the backup at TIP, and it is honoured there.
        let zip = make_zip(&["backup/old.stalx", "project.stalx"]);
        let mut dump = Dump::open(&zip).expect("open");
        assert_eq!(
            state
                .speleodb_resolve_entry_remembering(
                    project,
                    &mut dump,
                    Some("backup/old.stalx"),
                    ArianeRoute::Adopt,
                    DumpAt::Revision(TIP),
                    StalxAmbiguity::Ask,
                )
                .expect("an explicit pick is honoured"),
            "backup/old.stalx"
        );
        assert_eq!(
            state
                .speleodb_recalled_dump_entry(project, DumpAt::Revision(TIP))
                .as_deref(),
            Some("backup/old.stalx"),
            "the answer stands for the revision it was given about"
        );

        // A collaborator pushes. Same two entries in the tree, so membership is no
        // help at all - only the revision says the answer no longer describes it.
        assert_eq!(
            state.speleodb_recalled_dump_entry(project, DumpAt::Revision(NEXT)),
            None,
            "a choice made at an earlier revision must not be honoured at a later one"
        );
        let mut dump = Dump::open(&zip).expect("open");
        assert!(
            matches!(
                state.speleodb_resolve_entry_remembering(
                    project,
                    &mut dump,
                    None,
                    ArianeRoute::Adopt,
                    DumpAt::Revision(NEXT),
                    StalxAmbiguity::Ask,
                ),
                Err(SpeleoDbError::AmbiguousDump(_))
            ),
            "the tree moved under the choice, so the user has to be asked again \
             rather than handed the pre-push file"
        );
    }

    /// [#258] Adoption must resolve the server head ONCE and stamp BASE from that same
    /// sha - the commit it downloaded by hash - rather than looking it up again after
    /// the import.
    ///
    /// A second lookup reopens the window the by-hash download closed, on the worst
    /// side of it. The import between them is slow (parse, build, save), so a revision
    /// landing during it is ordinary; the copy is then A-derived content stamped
    /// BASE = B and `has_unpushed_edits`. Start editing reads BASE == tip and calls it
    /// UpToDate so nothing fast-forwards, and the push reads base == tip and plans a
    /// plain Upload - putting A over B with no divergence raised, which silently
    /// overwrites the very revision that landed during the import.
    ///
    /// Guarded by SOURCE, like the pull's guard-before-download ordering above and for
    /// the same reason: reaching the real thing needs a server that can advance its
    /// head mid-import. The live orchestration tests are where a behavioural version
    /// would go.
    #[test]
    fn adoption_stamps_base_from_the_commit_it_downloaded_not_a_later_lookup() {
        let src = prod_source();
        let start = src
            .find("fn speleodb_adopt_ariane")
            .expect("the adoption verb is present");
        let body = &src[start..];
        let body = &body[..body.find("\n    }\n").expect("fn body terminates")];

        assert_eq!(
            body.matches("client::get_revisions(").count()
                + body.matches("speleodb_server_head(").count(),
            1,
            "adoption must resolve the head exactly once; a second lookup after the \
             import is what lets an A-derived project be stamped BASE = B"
        );
        assert!(
            body.contains("pull_dating(adopt_at"),
            "BASE must come from the sha adoption downloaded by hash, through the same \
             split that keeps the advisory fallback off the choice dating"
        );
    }

    /// A project id for the lock-guard tests below. Any valid-looking id will do -
    /// nothing here reaches a server.
    const GUARDED: &str = "77777777-7777-7777-7777-777777777777";

    /// Nothing listens here, so the release POST fails immediately. Best-effort is the
    /// contract, and the claim is dropped either way - the same endpoint the unwind
    /// tests use for the same reason.
    const UNREACHABLE: &str = "http://127.0.0.1:1/";

    /// Sign in against [`UNREACHABLE`], because an acquisition claim is keyed to the
    /// account that made it and the accessors answer only for the account signed in
    /// NOW. Without this the claim below is invisible to the assertion in BOTH
    /// directions, and the tests pass vacuously.
    fn sign_in_unreachable(state: &AppState) {
        let mut session = state.speleodb_session_lock().expect("session lock");
        session.endpoint = Some(UNREACHABLE.to_owned());
        session.email = Some("diver@example.com".to_owned());
        session.token_store = crate::speleodb::auth::TokenStore::from_in_memory_token("tok");
    }

    /// `PushedLockHeld` is inferred from the link, and the inference is only sound
    /// because the dirty bit is provably TRUE on entry - adoption stamps it before the
    /// hand-over, and the only things that clear it are the push's own landed arms. A
    /// stamp that stopped setting it would make every failed release read as a landed
    /// push, telling a user their survey was on the server when it was not.
    ///
    /// Pinned by source because the behavioural version needs a server that accepts an
    /// upload and then refuses a release. What it holds is the ordering the inference
    /// rests on, and that the unreadable-link case falls to `Failed`.
    #[test]
    fn the_pushed_but_locked_inference_rests_on_a_dirty_bit_set_before_the_push() {
        let body = adopt_body();
        let stamp = body
            .find("has_unpushed_edits: true")
            .expect("adoption stamps the link dirty");
        let hand_over = body
            .find("self.adopt_hand_over(")
            .expect("the hand-over follows it");
        assert!(
            stamp < hand_over,
            "the dirty bit has to be set BEFORE the push, or a cleared bit afterwards \
             proves nothing about whether anything landed"
        );

        let src = prod_source();
        let start = src
            .find("fn adopt_hand_over_inner")
            .expect("the hand-over is present");
        let inner = &src[start..];
        let inner = &inner[..inner.find("\n    }\n").expect("fn body terminates")];
        assert!(
            inner.contains("Ok(link) if !link.has_unpushed_edits"),
            "the inference reads the link, and only a CLEAN one may claim a landing"
        );
        assert!(
            inner.contains("_ => AdoptUpload::Failed"),
            "and the catch-all is what makes an unreadable link fall to `Failed` rather \
             than claim a landing nobody can confirm"
        );
        assert!(
            inner.contains("AdoptUpload::PushedLockHeld"),
            "and reports the landed-but-locked outcome apart from a failure"
        );

        // The link is read from `AppState`, not by project id, so the clean bit only
        // proves OUR push landed if the open link is still the adopted project's. A
        // mismatch is an invariant failure, not an ordinary one: it is logged at error
        // level precisely because nothing else in the flow would report it, and it
        // falls to `Failed` so the conversion keeps its protective dirty seed.
        let mismatch = inner
            .find("link.project_id != expected_id")
            .expect("the inference checks the link belongs to this adoption");
        let clean = inner
            .find("Ok(link) if !link.has_unpushed_edits")
            .expect("checked above, so it is present");
        assert!(
            mismatch < clean,
            "the identity check has to come BEFORE the dirty-bit read, or a stranger \
             project's clean link satisfies the inference first"
        );
        assert!(
            inner.contains("tracing::error!"),
            "and a foreign link is surfaced as an error - it means the open database \
             changed under the hand-over, which nothing else would report"
        );
    }

    /// The guard's whole purpose: an adoption that took the mutex and then failed
    /// before it had anything to push must not walk away still holding it. The
    /// acquisition claim is the observable half - `speleodb_release_best_effort` drops
    /// it whether or not the POST lands, precisely so a lock this process can no longer
    /// account for is not left recorded as ours.
    #[test]
    fn the_lock_guard_hands_back_a_lock_the_conversion_never_used() {
        let state = AppState::new().expect("state");
        sign_in_unreachable(&state);
        state.speleodb_note_lock_acquired(Some(GUARDED));

        drop(AdoptLockGuard::new(
            &state,
            UNREACHABLE,
            "token",
            true,
            GUARDED,
            true,
        ));

        assert!(
            !state.speleodb_lock_acquired_here(GUARDED),
            "a lock taken for a conversion that never happened is handed straight back"
        );
    }

    /// The other half: once the conversion exists, the lock belongs to the hand-over,
    /// and `speleodb_done` is what decides its fate - released when the push lands,
    /// KEPT when a retry could still recover the work. A guard that released anyway
    /// would free the project while the only copy of that work is local.
    #[test]
    fn a_disarmed_lock_guard_leaves_the_lock_to_the_hand_over() {
        let state = AppState::new().expect("state");
        sign_in_unreachable(&state);
        state.speleodb_note_lock_acquired(Some(GUARDED));

        let mut guard = AdoptLockGuard::new(&state, UNREACHABLE, "token", true, GUARDED, true);
        guard.disarm();
        drop(guard);

        assert!(
            state.speleodb_lock_acquired_here(GUARDED),
            "after the disarm the hand-over owns the lock; the guard must not touch it"
        );
    }

    /// An adoption that never took the mutex has nothing to hand back, and a release
    /// it issued anyway would be a release for a lock somebody ELSE is holding - the
    /// exact contention this flow declines to push through.
    #[test]
    fn an_unarmed_lock_guard_releases_nothing() {
        let state = AppState::new().expect("state");
        sign_in_unreachable(&state);
        state.speleodb_note_lock_acquired(Some(GUARDED));

        drop(AdoptLockGuard::new(
            &state,
            UNREACHABLE,
            "token",
            true,
            GUARDED,
            false,
        ));

        assert!(
            state.speleodb_lock_acquired_here(GUARDED),
            "no lock taken means no lock to release, and no claim to end"
        );
    }

    /// The body of `speleodb_adopt_ariane`, for the ordering tripwires below.
    ///
    /// Sliced the same way the dating tripwire above slices it: to the first line that
    /// is a closing brace at method indentation, which is the method's own.
    fn adopt_body() -> &'static str {
        let src = prod_source();
        let start = src
            .find("fn speleodb_adopt_ariane")
            .expect("the adoption verb is present");
        let body = &src[start..];
        let body = &body[..body.find("\n    }\n").expect("fn body terminates")];
        // The terminator is "a closing brace at method indentation", which is the
        // method's own only as long as nothing inside the body is written at that
        // indent. A refactor that introduces one truncates this slice - and the
        // tripwires built on it include NEGATIVE assertions (`!body.contains(..)`),
        // which then pass over an empty tail without anyone noticing. Anchoring on the
        // verb's own return is what makes a short slice loud instead of silent.
        assert!(
            body.contains("Ok(SpeleoAdoptResult {"),
            "the sliced body must reach the verb's return, or every tripwire built on \
             it is measuring a fragment"
        );
        body
    }

    /// Adoption reads a tree and writes it straight back, so the whole span between
    /// those two acts is a window in which another writer's push makes the converted
    /// bytes stale. Fetching by hash closes the window between the two REQUESTS; it
    /// does nothing about the one that spans the import, which is the slow part
    /// (parse the `.tml`, build the project, save it). Taking the write mutex before
    /// the first byte is what closes that one, and the ORDER is the entire mechanism -
    /// a lock taken after the download protects nothing that was read before it.
    ///
    /// By source, because the real thing needs a server that can advance its head
    /// mid-import. The live orchestration tests are where a behavioural version goes.
    #[test]
    fn adoption_takes_the_lock_before_it_reads_anything() {
        let body = adopt_body();
        let lock = body
            .find("self.adopt_take_lock(")
            .expect("adoption takes the write mutex");
        let resolve = body
            .find("client::get_revisions(")
            .expect("adoption resolves the head");
        let download = body
            .find("download_dump")
            .expect("adoption downloads the dump");

        assert!(
            lock < resolve && lock < download,
            "the mutex must be taken BEFORE the head resolve and the download, or the \
             tree adoption converts can be superseded while it is converting it"
        );
    }

    /// The lock is taken before a long fallible stretch - resolve, download, classify,
    /// extract, guard, import, save - and every `?` in it is a way to walk away holding
    /// a mutex that blocks every other writer until the server-side timeout. One guard
    /// covers them all, which is what stops the next arm added from having to remember.
    ///
    /// Pinned as an ordering because that is what makes it a guard rather than a
    /// release: armed before the stretch, disarmed only once there is a converted
    /// project for the lock to be about.
    #[test]
    fn the_lock_guard_spans_every_early_return_between_the_lock_and_the_conversion() {
        let body = adopt_body();
        let armed = body
            .find("AdoptLockGuard::new(")
            .expect("adoption arms the hand-back guard");
        let disarmed = body
            .find("lock_guard.disarm()")
            .expect("adoption disarms it once the conversion exists");
        let import = body
            .find("self.adopt_import_and_save(")
            .expect("adoption imports the .tml");

        assert!(
            armed < import && import < disarmed,
            "the guard must be armed before the import and disarmed after it, so every \
             early return in between hands the mutex back"
        );
        assert!(
            body.find("self.adopt_hand_over(")
                .is_some_and(|h| disarmed < h),
            "the hand-over owns the lock from the disarm onwards - `speleodb_done`'s \
             rules decide whether it is released or kept for a retry"
        );
    }

    /// A project typed `stalagmight` whose only file is the Ariane one that type says to
    /// disregard is openable by nobody: the dump classifies as adopt-Ariane, and the
    /// flipped type is what tells the app not to trust it. Flipping only after the
    /// `.stalx` is actually on the server means the failure mode is the honest one -
    /// an un-flipped project that still offers Adopt.
    #[test]
    fn the_server_type_flips_only_after_the_stalx_lands() {
        let body = adopt_body();

        // The verb itself issues no PATCH: the flip lives in `adopt_finalize`,
        // which reads before it writes. Two PATCHes was not merely wasteful - the push's
        // own heal had already flipped it, and `type_flipped` was decided by the second
        // request, so a transient failure there reported "the type could not be changed"
        // about a project whose type had just been changed.
        assert!(
            !body.contains("client::patch_project("),
            "the verb must not flip the type inline; it goes through the read-first helper"
        );

        let hand_over = body
            .find("self.adopt_hand_over(")
            .expect("adoption hands the conversion over");
        let ensure = body
            .find("self.adopt_finalize(")
            .expect("adoption settles the project type");
        assert!(
            hand_over < ensure,
            "the type is settled only after the push that earns it"
        );
        assert!(
            body.contains("matches!(upload, AdoptUpload::Pushed | AdoptUpload::PushedLockHeld)"),
            "and gated on the outcomes that mean the bytes landed - claiming a flip on a \
             lock we could not take describes a project that does not exist"
        );
    }

    /// The half the ordering tripwire cannot see: what the helper DOES.
    ///
    /// Adoption is the one caller that knows the project was Ariane - it just converted
    /// it - so a type it cannot read is still a type it may set. Delegating that decision
    /// to the generic push heal was a regression: the heal declines to act on an unknown
    /// type, which is right when it might be someone's Compass project and wrong here,
    /// and it turned one transient GET failure into a project left typed `ariane` with a
    /// `.stalx` on it, still offering Adopt over a copy that already exists.
    #[test]
    fn an_unreadable_project_type_still_gets_flipped() {
        let src = prod_source();
        let start = src
            .find("fn adopt_finalize")
            .expect("the helper is present");
        let body = &src[start..];
        let body = &body[..body.find("\n    }\n").expect("fn body terminates")];

        let get = body
            .find("client::get_project(")
            .expect("it reads the current type first");
        let patch = body
            .find("client::patch_project(")
            .expect("it flips when it has to");
        assert!(
            get < patch,
            "READ first: the common case is a type the push already flipped, and asking \
             costs a GET where assuming costs a redundant PATCH"
        );

        // The `Err` arm must fall THROUGH to the flip, not return.
        let err_arm = body.find("Err(e) => {").expect("the unreadable-type arm");
        assert!(
            err_arm < patch,
            "a type we could not read must fall through to the flip, not skip it"
        );
        assert!(
            body[err_arm..patch].contains("flipping anyway"),
            "and it must say so, because silently not flipping is what the old shape did"
        );
    }

    /// Adoption used to delete the cache `.stalx` and rebuild in place, so the copy
    /// being replaced was destroyed before anything that could fail had run. A `.tml`
    /// that would not parse then cost the user the cached project as well as the
    /// adoption - and re-adoption is reachable (an adopted copy is stamped dirty, and
    /// the type flip is best-effort, so a project can keep offering Adopt), which is
    /// exactly when there is un-pushed work behind that file.
    ///
    /// Building into a staging file and renaming means the only window in which `dest`
    /// is not the old copy is the rename, which the filesystem makes atomic.
    ///
    /// What this reaches offline is the DELETE, which the old code ran before anything
    /// else: `AppState::new()` has no app handle, so the create refuses immediately and
    /// the run stops one step in. That step is the one that used to destroy the file,
    /// so the test fails against the old ordering and passes against this one. The
    /// deeper half - a create that COMMITS and a parse that then refuses - needs a
    /// handle, and is covered by the dev-box runbook.
    #[test]
    fn a_failed_conversion_leaves_the_cached_copy_untouched() {
        let temp = tempfile::TempDir::new().expect("temp");
        let dest = temp.path().join("project.stalx");
        let sentinel = b"the cached copy, which this adoption must not cost the user";
        std::fs::write(&dest, sentinel).expect("seed the cache copy");
        // Not a `.tml`, so the conversion fails after the create has already committed.
        let not_a_tml = temp.path().join("adopt-source.tml");
        std::fs::write(&not_a_tml, b"<nope/>").expect("seed the source");

        let state = AppState::new().expect("state");
        let failed =
            state.adopt_import_and_save(&not_a_tml, "ariane.tml", &dest, "Adopted".to_owned());

        assert!(
            failed.is_err(),
            "the conversion cannot succeed on that input"
        );
        assert_eq!(
            std::fs::read(&dest).expect("the cache copy still exists"),
            sentinel,
            "a failed adoption must leave the copy it was replacing byte-for-byte"
        );
        // By PREFIX, not by our own name: the point is that no staging file survives,
        // and a check for `adopt_staging_name()` alone would pass over a leftover
        // written under any other pid.
        let staging_left: Vec<_> = std::fs::read_dir(temp.path())
            .expect("read the cache dir")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(super::ADOPT_STAGING_PREFIX))
            .collect();
        assert!(
            staging_left.is_empty(),
            "a failed adoption must not leave a whole survey behind in a staging file \
             nothing will ever open, found: {staging_left:?}"
        );
    }

    /// The other half of that failure. `do_create_project_full` commits into
    /// `AppState` before the parse can refuse, so an adoption that failed afterwards
    /// left the backend holding a stub the shell was not rendering - and every later
    /// command, each edit and each auto-save, landed on it.
    ///
    /// The same undo the cache replace performs when its extract fails: put back what
    /// was open before.
    ///
    /// Offline this reaches the ABANDON path but not the stub that motivates it (the
    /// create needs an app handle to get as far as committing one), so read it as
    /// pinning what that path must never do: the close it performs has to be followed
    /// by the reopen. Drop the reopen and this fails with nothing open at all, which is
    /// the failure mode that would replace the old one rather than fix it.
    #[test]
    fn a_failed_conversion_puts_the_project_it_replaced_back() {
        let temp = tempfile::TempDir::new().expect("temp");
        let previous = temp.path().join("previous.stalx");
        {
            let mut db = crate::persistence::Database::open(&previous).expect("open db");
            db.save_project(&crate::models::Project::new("Previous".to_owned(), None))
                .expect("seed the project that was open");
        }
        let not_a_tml = temp.path().join("adopt-source.tml");
        std::fs::write(&not_a_tml, b"<nope/>").expect("seed the source");

        let state = AppState::new().expect("state");
        assert!(
            state.open_project(previous.clone()).success,
            "the project the adoption is about to replace"
        );

        let dest = temp.path().join("project.stalx");
        let failed =
            state.adopt_import_and_save(&not_a_tml, "ariane.tml", &dest, "Adopted".to_owned());

        assert!(
            failed.is_err(),
            "the conversion cannot succeed on that input"
        );
        assert_eq!(
            state.open_project_path().as_deref(),
            Some(previous.as_path()),
            "the backend must be back on the project the shell is still drawing, not \
             empty and not holding the stub the create committed"
        );
    }

    /// A synthetic remote file, PARSED - not hand-built like `adopt_tml`: the
    /// per-trip cut (#427) happens during the parse, so a `TmlFile` literal
    /// cannot exercise it and a test built on one would pass whatever the cut
    /// did. Production adoption parses the downloaded bytes, so this is the
    /// shape that path actually hands `adopt_convert`.
    fn parsed_remote(shots: &[TripShot<'_>]) -> TmlFile {
        let xml: Vec<String> = shots.iter().map(TripShot::xml).collect();
        parse_synthetic_tmlu(&wrap_xml(&xml.join("\n")))
    }

    /// The adopted `.tml` gets the same per-trip cut the wizard import does -
    /// adoption runs the same parse and converter, and a remote file surveyed
    /// by two teams must not land under one of them.
    #[test]
    fn an_adopted_file_is_cut_per_trip_like_any_other() {
        let tml = parsed_remote(&[
            TripShot {
                id: 1,
                from_id: -1,
                section: "S",
                explorer: "Team One",
                ..TripShot::default()
            },
            TripShot {
                id: 2,
                from_id: 1,
                section: "S",
                explorer: "Team Two",
                ..TripShot::default()
            },
            TripShot {
                id: 3,
                from_id: 2,
                section: "S",
                explorer: "Team Two",
                ..TripShot::default()
            },
        ]);

        let (configs, _, warnings) = adopt_convert(&tml).expect("the conversion succeeds");
        assert_eq!(
            configs
                .iter()
                .map(|c| (c.name.clone().unwrap_or_default(), c.surveyor.clone()))
                .collect::<Vec<_>>(),
            vec![
                ("S".to_owned(), Some("Team One".to_owned())),
                ("S (trip 2)".to_owned(), Some("Team Two".to_owned())),
            ],
            "each trip is adopted as its own section, with its own surveyor"
        );
        assert!(
            !warnings.iter().any(|w| w.contains("recorded a different")),
            "and nothing was collapsed, so nothing is reported: {warnings:?}"
        );
    }

    /// A trip change with no station seam to cut at is reported instead - and
    /// the excluded line still comes FIRST, because `cap_warnings` truncates
    /// from the tail (see this module's doc comment).
    #[test]
    fn the_collapsed_trip_line_follows_the_excluded_line_it_must_not_displace() {
        let tml = parsed_remote(&[
            TripShot {
                id: 1,
                from_id: -1,
                section: "S",
                explorer: "Team One",
                ..TripShot::default()
            },
            // Excluded, so neither seam around it is a station boundary.
            TripShot {
                id: 2,
                from_id: 1,
                section: "S",
                excluded: true,
                explorer: "Team One",
                ..TripShot::default()
            },
            TripShot {
                id: 3,
                from_id: 2,
                section: "S",
                explorer: "Team Two",
                ..TripShot::default()
            },
        ]);

        let (configs, _, warnings) = adopt_convert(&tml).expect("the conversion succeeds");
        assert_eq!(configs.len(), 1, "there was no seam to cut at");
        assert_eq!(
            warnings.first().map(String::as_str),
            Some("1 excluded shot in the remote file was not imported"),
            "the excluded line keeps its place at the front: {warnings:?}"
        );
        assert!(
            warnings[1..]
                .iter()
                .any(|w| w.starts_with("1 shot at 1 trip change in 1 section recorded a different")),
            "and the collapsed trip is reported behind it: {warnings:?}"
        );
    }

    /// One shot of a synthetic remote `.tml`, chained onto its predecessor.
    fn adopt_shot(id: i64, excluded: bool, shot_type: TmlShotType) -> TmlShot {
        TmlShot {
            id,
            // No `<UUID>` in this fixture: the adoption tests here are about the
            // pull/adopt plumbing, and identity carry has its own coverage
            // (`core::state::shot_uuid_tests`).
            uuid: None,
            from_id: id - 1,
            shot_type,
            azimuth: 90.0,
            length: 10.0,
            depth: 0.0,
            depth_in: -1.0,
            inclination: 0.0,
            left: 0.0,
            right: 0.0,
            up: 0.0,
            down: 0.0,
            comment: None,
            explorer: None,
            date: None,
            color: "#000000".to_owned(),
            excluded,
            latitude: 0.0,
            longitude: 0.0,
            closure_to_id: -1,
            section: "S".to_owned(),
        }
    }

    /// One section of a synthetic remote `.tml`.
    fn adopt_section(name: &str, start_ariane_id: i64, shots: Vec<TmlShot>) -> TmlSection {
        let end_ariane_id = shots.last().map_or(start_ariane_id, |s| s.id);
        // The colour comes from the head shot, as `linearize_section` builds it.
        // A section that disagrees with its own shots is a shape the parser
        // cannot emit since #427, and the converter reports it as a collapsed
        // trip - which would be this fixture lying, not a finding.
        let color = shots
            .first()
            .map_or_else(|| "#ff0000".to_owned(), |shot| shot.color.clone());
        TmlSection {
            name: name.to_owned(),
            shots,
            surveyor: None,
            date: None,
            color,
            is_branch_split: false,
            start_ariane_id,
            end_ariane_id,
            start_depth: 0.0,
        }
    }

    /// A synthetic remote `.tml` holding `sections`, with no geo-reference.
    fn adopt_tml(sections: Vec<TmlSection>) -> TmlFile {
        TmlFile {
            cave_name: "Remote".to_owned(),
            unit: TmlUnit::Metric,
            use_magnetic_azimuth: false,
            first_start_absolute_elevation: 0.0,
            unrecognized_unit: None,
            geo_coding: None,
            sections,
            closures: Vec::new(),
            parse_anomalies: None,
            encoding_warning: None,
        }
    }

    /// [#411] The pull -> adopt conversion bound its converter warnings to
    /// `_converter_warnings` and dropped them on the floor. This flow has no import
    /// dialog and no receipt anywhere in it, so that call site was the last place a
    /// remote file's parse anomalies existed - a pulled project lost them in silence.
    ///
    /// Driven through the REAL converter, so what is asserted is what an adoption
    /// would actually carry rather than a hand-made string: a section whose every
    /// shot is excluded is dropped whole, and the converter says so.
    #[test]
    fn the_adoption_carries_the_converter_warnings_and_names_the_excluded_shots() {
        let mut closure = adopt_shot(6, true, TmlShotType::Closure);
        closure.from_id = 1;
        closure.closure_to_id = 3;
        let tml = adopt_tml(vec![
            adopt_section(
                "Kept",
                0,
                vec![
                    adopt_shot(1, false, TmlShotType::Real),
                    adopt_shot(2, true, TmlShotType::Real),
                    adopt_shot(3, false, TmlShotType::Real),
                    closure,
                ],
            ),
            adopt_section(
                "AllExcluded",
                3,
                vec![
                    adopt_shot(4, true, TmlShotType::Real),
                    adopt_shot(5, true, TmlShotType::Real),
                ],
            ),
        ]);

        // The one call the adoption makes: the converter, plus its own counting.
        let (configs, closures, warnings) = adopt_convert(&tml).expect("the conversion succeeds");
        // The losses the warnings are ABOUT, proven rather than assumed.
        assert_eq!(
            configs.len(),
            1,
            "the all-excluded section is dropped whole"
        );
        assert_eq!(
            configs[0].shots.len(),
            2,
            "and the excluded shot inside the surviving section goes with it"
        );
        assert_eq!(closures.len(), 1, "the CLOSURE shot resolves as a closure");

        // What the converter itself reported, read back from a second run so the
        // comparison is against the real thing rather than a copy of the assertion.
        let (_, _, converter_warnings) =
            crate::parsers::tml::convert_tml_to_import_configs(&tml, false, true)
                .expect("the conversion succeeds");
        assert!(
            !converter_warnings.is_empty(),
            "the fixture must make the converter report something, or this test cannot \
             see it being dropped"
        );
        for reported in &converter_warnings {
            assert!(
                warnings.contains(reported),
                "every converter warning must survive the adoption, missing: {reported}"
            );
        }
        assert_eq!(
            warnings.first().map(String::as_str),
            Some("3 excluded shots in the remote file were not imported"),
            "the excluded shots this path drops must be named with their count, and \
             named FIRST so the cap cannot fold them away - the CLOSURE shot is not \
             one of them; got {warnings:?}"
        );
    }

    /// The count is a count, and a file with nothing excluded gets no line about it -
    /// a warning that is always there is one nobody reads.
    #[test]
    fn the_excluded_warning_counts_and_is_absent_when_nothing_was_excluded() {
        let one = adopt_tml(vec![adopt_section(
            "S",
            0,
            vec![
                adopt_shot(1, false, TmlShotType::Real),
                adopt_shot(2, true, TmlShotType::Real),
            ],
        )]);
        let (_, _, warnings) = adopt_convert(&one).expect("the conversion succeeds");
        assert_eq!(
            warnings,
            vec!["1 excluded shot in the remote file was not imported".to_owned()],
            "one excluded shot reads in the singular"
        );

        let none = adopt_tml(vec![adopt_section(
            "S",
            0,
            vec![adopt_shot(1, false, TmlShotType::Real)],
        )]);
        let (_, _, warnings) = adopt_convert(&none).expect("the conversion succeeds");
        assert!(
            warnings.is_empty(),
            "nothing excluded means nothing said; got {warnings:?}"
        );
    }

    /// The source-level half: the call site must not go back to discarding what the
    /// conversion reported, and the receipt it now returns must be decorated by the
    /// SAME helper the wizard-driven Ariane import uses - a hand-rolled one would be
    /// free to drift from it, which is how the two imports of one format stop
    /// agreeing about the file.
    ///
    /// Reads the production text because the behavioural path needs an app handle -
    /// `do_create_project_full` refuses without one - so `adopt_build_into` cannot be
    /// driven offline at all.
    #[test]
    fn the_adopt_call_site_does_not_discard_the_conversions_warnings() {
        let src = prod_source();
        let start = src
            .find("fn adopt_build_into")
            .expect("the conversion half is present");
        let body = &src[start..];
        let body = &body[..body.find("\n    }\n").expect("fn body terminates")];

        assert!(
            !body.contains("_converter_warnings"),
            "binding the converter's warnings to an underscore is how they were lost \
             (#411); pass them on instead"
        );
        // The LITERAL call: `adopt_convert` both converts and counts, so there is no
        // warning list for a caller to satisfy with an empty vec - the previous shape
        // took one as an argument and this assertion passed over `(&tml, Vec::new())`.
        assert!(
            body.contains("let (sections, closures, warnings) = adopt_convert(&tml)?;"),
            "the conversion and its warnings must come from ONE call, or the call site \
             is free to keep the sections and drop the account of what they cost"
        );
        assert!(
            !body.contains("convert_tml_to_import_configs("),
            "converting directly here re-opens the seam `adopt_convert` closed: the \
             excluded-shot count is not the converter's job and would be skipped"
        );
        let receipt = body
            .find("adopt_receipt(")
            .expect("the adoption builds its receipt through the helper that guarantees one");
        assert!(
            body[receipt..].contains("Ok(receipt)"),
            "the receipt is what leaves this function - building one that is then \
             dropped is the same silence with more steps"
        );
    }

    /// The acquire and the claim that records it have to be ONE act under the shared
    /// lock-operations guard, the same span every other lock operation takes.
    ///
    /// Without it a logout can start while the acquire is in flight, see no acquisition
    /// to hand back, finish clearing the session, and only then have our acquire succeed
    /// with the token it captured beforehand - a server mutex held by a session that no
    /// longer exists, with no local claim naming it, freed only by the server timeout.
    ///
    /// Structural, because reproducing it needs a logout to interleave with an in-flight
    /// HTTP request. What it pins is that the span exists and covers BOTH steps - and
    /// that it does not swallow the conversion, which would block logout for as long as
    /// an import takes.
    #[test]
    fn the_adopt_acquire_and_its_claim_share_the_lock_ops_span() {
        let body = adopt_body();

        let span = body
            .find("self.speleodb_lock_ops_guard()")
            .expect("the verb opens the shared lock-ops span");
        let token = body
            .find("self.speleodb_active_token()")
            .expect("the account is read");
        let probe = body
            .find("self.speleodb_lock_holder_checked(")
            .expect("the holder is probed");
        let acquire = body
            .find("self.adopt_take_lock(")
            .expect("the mutex is taken");

        // The ACCOUNT READ is inside the span, not just the acquire. A span that opens
        // later leaves the token read and a whole network round trip outside it, and a
        // logout completing in THAT window is the same failure with more steps: the
        // acquire lands with a token captured beforehand and the claim is dropped for
        // want of an account to stamp it with. The sibling path states the rule - the
        // guard comes first, before the account is read.
        assert!(
            span < token && span < probe && span < acquire,
            "the span must cover the account read and the probe, not only the acquire"
        );

        // SCOPE, not just order: the block has to still be open at the acquire. Closing
        // it early would satisfy every inequality above while re-opening the exact
        // window the span exists to shut.
        let closes = body[acquire..]
            .find("\n        };")
            .map(|at| acquire + at)
            .expect("the span's block closes");
        let resolve = body
            .find("client::get_revisions(")
            .expect("the head resolve follows it");
        assert!(
            acquire < closes && closes < resolve,
            "the span must still be open at the acquire and shut before the download - \
             holding it across the import would block logout for the length of one"
        );
        // Held, not merely declared. An explicit `drop` inside the block satisfies every
        // position check above while re-opening the window between the mutex becoming
        // ours and the claim recording it.
        assert!(
            !body[span..closes].contains("drop(_lock_ops"),
            "the span must not be released early: the acquire and the claim it mints are \
             one act, and a drop between them is the teardown window reopened"
        );
    }

    /// The release verdict's own span, and the same argument one stage later: reading
    /// who holds the mutex and retiring the claim that says WE do are one act.
    ///
    /// Separated, a `speleodb_start_editing` for this project and account lands in
    /// between, and `speleodb_record_claim` REPLACES the same-project entry rather than
    /// appending - so by the time the clear runs there is one claim for that pair and it
    /// is the new session's. The clear matches on project + account, retires that, and
    /// leaves a live edit session holding a server mutex with no janitor retry pointer
    /// and no authorship provenance.
    ///
    /// Structural for the same reason as the acquire span: reproducing it needs a second
    /// edit session to interleave with an in-flight HTTP request.
    #[test]
    fn the_release_verdict_and_its_claim_clear_share_a_lock_ops_span() {
        let body = adopt_body();

        // Located from the hand-over forward, not by `find` from the top - the acquire
        // span above is the first occurrence and this assertion is not about it. That
        // ordering is also what keeps this span legal: `adopt_hand_over` runs
        // `speleodb_start_editing` and `speleodb_done`, which take this same
        // non-reentrant guard, so opening any earlier is a self-deadlock.
        let hand_over = body
            .find("self.adopt_hand_over(")
            .expect("the hand-over precedes the verdict");
        let span = body[hand_over..]
            .find("self.speleodb_lock_ops_guard()")
            .map(|at| hand_over + at)
            .expect("the verdict opens its own lock-ops span, after the hand-over");
        let finalize = body
            .find("self.adopt_finalize(")
            .expect("the holder and type are read");
        let clear = body
            .find("self.speleodb_forget_lock_acquired_as(")
            .expect("the disproved claim is retired");

        assert!(
            span < finalize && finalize < clear,
            "the span must open before the holder read and still cover the clear"
        );

        // SCOPE, not just order - the same distinction the acquire span's tripwire draws.
        let closes = body[clear..]
            .find("\n        };")
            .map(|at| clear + at)
            .expect("the span's block closes");
        assert!(
            clear < closes,
            "the clear has to be INSIDE the block, or the span shuts between the read \
             that disproved the claim and the act of retiring it"
        );
        assert!(
            !body[span..closes].contains("drop(_lock_ops"),
            "and it must not be released early: a drop between the holder read and the \
             clear reopens the very window the span exists to shut"
        );
    }

    /// The counterpart to the span above: the helper it calls must NOT take the guard
    /// itself. `std::sync::Mutex` is not reentrant, so a second acquisition inside the
    /// caller's span is not a redundancy - it is a self-deadlock on the first adoption
    /// anyone runs.
    #[test]
    fn the_acquire_helper_does_not_retake_the_span_its_caller_holds() {
        let src = prod_source();
        let start = src
            .find("fn adopt_take_lock")
            .expect("the acquire helper is present");
        let body = &src[start..];
        let body = &body[..body.find("\n    }\n").expect("fn body terminates")];

        assert!(
            !body.contains("speleodb_lock_ops_guard()"),
            "the caller holds the span across this call; retaking it self-deadlocks"
        );
        assert!(
            body.contains("self.speleodb_note_lock_acquired("),
            "and the claim is still minted here, inside the caller's span"
        );
        assert!(
            !body.contains("adopt_import_and_save"),
            "the helper must not perform the conversion; the span its caller holds is \
             not one to keep across it"
        );
    }

    /// Adoption is a caller that RELEASES on the way out, which is what makes the
    /// difference between the two lock probes matter here.
    ///
    /// `speleodb_lock_holder` collapses "unheld" and "could not find out" into `None`
    /// - right for the display probes it was written for, where both render as
    /// unknown. Adoption reading a failed GET as unheld goes on to acquire, and an
    /// acquire over a mutex OUR OWN ACCOUNT already holds is a server-side no-op: it
    /// returns 200 and reports no collision, so nothing later in the flow can discover
    /// one. The push then lands and `speleodb_done` hands the lock back out from under
    /// the window that was actually using it, which goes on editing until its next
    /// push 403s. Another account's lock is safe either way - the server answers that
    /// acquire with the 400 this flow already handles - so it is only our own that
    /// needs the probe to admit it does not know.
    #[test]
    fn adoption_reads_the_lock_probe_honestly_rather_than_as_unheld() {
        let body = adopt_body();
        assert!(
            body.contains("self.speleodb_lock_holder_checked("),
            "adoption must use the probe that keeps `unheld` and `unreadable` apart"
        );
        assert!(
            !body.contains("self.speleodb_lock_holder("),
            "and must not use the collapsing one: it reads a failed GET as unheld, \
             which is how an adoption acquires - and later releases - a lock our own \
             account is holding in another window"
        );
    }

    /// A dirty bit with no edit time is one a confirmed push can NEVER clear:
    /// `edits_captured_by_push` reads a missing timestamp as unknown timing and keeps
    /// the bit, because an edit it cannot date might postdate the upload. Adoption
    /// stamps `has_unpushed_edits: true` on a link it mints from nothing, so it is the
    /// one place that can create that pairing - and it did, which made every adoption
    /// push successfully, stay dirty, report `PushUnconfirmed` and hold the lock for a
    /// retry that would do the same thing again.
    ///
    /// The same defect the v9→v10 migration had to backfill the epoch for. Pinned here
    /// against the predicate itself, so the rule travels with its reason.
    #[test]
    fn the_adopted_link_is_dated_so_a_landed_push_can_clear_it() {
        assert!(
            !crate::speleodb::sync::edits_captured_by_push(None, Some("2026-07-31T00:00:00+00:00")),
            "an undated dirty bit is never captured by a push - this is the trap"
        );

        let body = adopt_body();
        assert!(
            body.contains("has_unpushed_edits: true"),
            "the adopted copy is un-pushed at the moment it is stamped"
        );
        assert!(
            body.contains("last_edit_at: Some(chrono::Utc::now().to_rfc3339())"),
            "and it must be DATED, or the hand-over's push lands and the bit survives \
             it: the conversion is the edit, and it happened now"
        );
    }

    /// `do_create_project_full` inside the import commits the adopted project into
    /// `AppState`, so without this the backend held it open at the very path the
    /// frontend was about to load - and `do_load_project_full` answers a load of the
    /// path it already has open with `ALREADY_OPEN`. The frontend reads that as "it is
    /// on screen already", which is true after a re-open and false after an adopt, so
    /// adoption ended with the view flipped, nothing in any store, and a success toast
    /// over an empty screen.
    ///
    /// LAST is load-bearing, not tidiness: the link stamp writes through the live
    /// handle and the hand-over renders the sidecar from the open survey, so a close
    /// anywhere earlier takes the project out from under both.
    #[test]
    fn adoption_hands_the_project_back_to_the_frontend_to_open() {
        let body = adopt_body();
        assert_eq!(
            body.matches("self.close_project();").count(),
            1,
            "adoption must close the project it built exactly once, so the frontend's \
             loadProject is a real load rather than an ALREADY_OPEN short-circuit"
        );
        let close = body
            .find("self.close_project();")
            .expect("counted above, so it is present");
        let ensure = body
            .find("self.adopt_finalize(")
            .expect("adoption settles the project type");
        assert!(
            ensure < close,
            "everything that needs the project open - the link stamp, the sidecar \
             render, the hand-over and its verdict - has to run before the close"
        );
    }

    /// [#258] The tip that pins the by-hash download comes off the wire, and it lands
    /// in an authenticated request path - so it crosses the same boundary the restore
    /// and the probe cross before building one of those URLs. Server-derived is not
    /// trusted: it is only ever as trustworthy as the box we are pointed at.
    ///
    /// A rejection is a fallback, not a failure. `None` sends the caller to the plain
    /// HEAD download AND marks the result undatable, so no by-hash request is built
    /// and no choice can be recalled against it.
    #[test]
    fn a_tip_that_could_reshape_the_download_url_is_not_used_to_build_one() {
        let project = "99999999-9999-9999-9999-999999999999";

        assert_eq!(
            usable_tip(Some(TIP.to_owned()), project),
            Some(TIP.to_owned())
        );
        assert_eq!(
            usable_tip(Some("abc1234".to_owned()), project),
            Some("abc1234".to_owned()),
            "a short prefix is a legitimate git id and must still pin the download"
        );
        assert_eq!(usable_tip(None, project), None);

        for hostile in [
            "../../user",
            "abc1234?x=y",
            "abc1234#frag",
            "abc/1234",
            "abc 1234",
            "abc1234\n",
            "abc1234\u{0}",
            "abc123",  // 6 chars - under the floor, so not an id at all
            "abc123g", // non-hex
            "",
        ] {
            assert_eq!(
                usable_tip(Some(hostile.to_owned()), project),
                None,
                "`{hostile}` must never reach the by-hash URL; the caller falls back \
                 to plain HEAD and treats the dump as undatable"
            );
        }
    }

    /// [#258] BASE may fall back to the caller's advisory sha; a CHOICE DATE may not.
    /// Folding the two together is how the guarantee quietly dies: `latest_commit_sha`
    /// is `max(authored_date)`, so it can name the root commit and stay there while
    /// revision after revision lands - and a choice recorded under a sha that never
    /// changes goes on matching for as long as the session lasts, which is exactly the
    /// unscoped recall this replaced.
    #[test]
    fn only_an_authoritative_tip_may_date_a_choice() {
        let tip = || Some(TIP.to_owned());
        let advisory = || Some(NEXT.to_owned());

        // With a tip, both answers are that tip - the advisory sha is not consulted.
        assert_eq!(
            pull_dating(tip(), advisory()),
            PullDating {
                datable: tip(),
                base: tip(),
            }
        );

        // Without one, BASE takes the fallback and the dating does NOT.
        assert_eq!(
            pull_dating(None, advisory()),
            PullDating {
                datable: None,
                base: advisory(),
            },
            "an unresolvable head must leave the dump undatable even though BASE is \
             still worth a best-effort stamp"
        );

        assert_eq!(
            pull_dating(None, None),
            PullDating {
                datable: None,
                base: None,
            }
        );
    }

    /// [#258] The picker's reply is an answer about ONE tree. Confirming it replays the
    /// verb, which resolves the tip again - so if a revision landed while the dialog was
    /// open, the answer to A arrives against B. Membership is no defence at all here:
    /// the backup the user chose survives the push untouched, which is exactly why it
    /// still passes. The pick is discarded and the current candidates re-offered.
    #[test]
    fn a_pick_that_answers_an_earlier_revision_is_refused_and_re_offered() {
        let state = AppState::new().expect("state");
        let project = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
        let zip = make_zip(&["backup/old.stalx", "project.stalx"]);

        // Candidates offered for A. Recording that is the whole mechanism: it is what
        // later distinguishes an answer to this question from an answer to another.
        let mut dump = Dump::open(&zip).expect("open");
        assert!(matches!(
            state.speleodb_resolve_entry_remembering(
                project,
                &mut dump,
                None,
                ArianeRoute::Adopt,
                DumpAt::Revision(TIP),
                StalxAmbiguity::Ask,
            ),
            Err(SpeleoDbError::AmbiguousDump(_))
        ));

        // A collaborator pushes while the picker is open, then the user confirms. The
        // chosen entry is still in B's tree - untouched by their push - so nothing about
        // the archive objects.
        let mut dump = Dump::open(&zip).expect("open");
        let replayed = state.speleodb_resolve_entry_remembering(
            project,
            &mut dump,
            Some("backup/old.stalx"),
            ArianeRoute::Adopt,
            DumpAt::Revision(NEXT),
            StalxAmbiguity::Ask,
        );
        assert!(
            matches!(replayed, Err(SpeleoDbError::AmbiguousDump(_))),
            "an answer about an earlier revision must be refused, not extracted"
        );
        assert_eq!(
            state.speleodb_recalled_dump_entry(project, DumpAt::Revision(NEXT)),
            None,
            "and it must not be recorded on its way past either"
        );

        // The refusal re-offered B's candidates, so answering THAT question works and
        // the flow cannot loop.
        let mut dump = Dump::open(&zip).expect("open");
        assert_eq!(
            state
                .speleodb_resolve_entry_remembering(
                    project,
                    &mut dump,
                    Some("backup/old.stalx"),
                    ArianeRoute::Adopt,
                    DumpAt::Revision(NEXT),
                    StalxAmbiguity::Ask,
                )
                .expect("the re-offer is answerable"),
            "backup/old.stalx"
        );
    }

    /// [#258] The same question for Ariane. An `ariane.tml` behaves exactly like a
    /// `.stalx` under a collaborator's push - the canonical file moves, every other
    /// entry stays where it was - so a backup chosen at A still passes membership at B,
    /// and adoption would import it and stamp BASE at B. The gate is shared session
    /// state, so the offer an OPEN records is the one adoption's reply is measured
    /// against; this pins that hand-off, which is the half a structural scan cannot see.
    #[test]
    fn an_ariane_offer_is_dated_the_same_way_and_read_back_by_adoption() {
        let state = AppState::new().expect("state");
        let project = "dddddddd-dddd-dddd-dddd-dddddddddddd";
        let zip = make_zip(&["one/ariane.tml", "two/ariane.tml"]);

        // The OPEN path meets the ambiguity and offers the candidates for A.
        let mut dump = Dump::open(&zip).expect("open");
        assert!(matches!(
            state.speleodb_resolve_entry_remembering(
                project,
                &mut dump,
                None,
                ArianeRoute::Adopt,
                DumpAt::Revision(TIP),
                StalxAmbiguity::Ask,
            ),
            Err(SpeleoDbError::AmbiguousAriane(_))
        ));

        // Adoption reads that record back. An answer about A does not answer B...
        assert!(!state.speleodb_dump_offer_answers(project, DumpAt::Revision(NEXT)));
        // ...and does answer A, so the ordinary confirmation is untouched.
        assert!(state.speleodb_dump_offer_answers(project, DumpAt::Revision(TIP)));
    }

    /// [#258] Adoption classifies with its OWN matcher - it wants an Ariane entry and
    /// refuses a `.stalx` with its own message - so the resolver's dating never reaches
    /// it and both halves have to be repeated there. Missing either restored the
    /// overwrite for Ariane alone, which is exactly how it was missed the first time:
    /// the `.stalx` picker was bound and this one silently was not.
    ///
    /// Structural because the behavioural version needs a server - the gate itself is
    /// covered above, on the session state the two paths share.
    #[test]
    fn adoption_dates_its_own_picker_replies_too() {
        let src = prod_source();
        let start = src
            .find("fn speleodb_adopt_ariane")
            .expect("the adoption verb is present");
        let body = &src[start..];
        let body = &body[..body.find("\n    }\n").expect("fn body terminates")];

        assert_eq!(
            body.matches("speleodb_dump_offer_answers(").count(),
            1,
            "adoption must gate a FRESH `entry_path` on the revision its candidates \
             were offered for, exactly as the resolver gates one"
        );
        assert_eq!(
            body.matches("speleodb_note_dump_offer(").count(),
            2,
            "both ambiguities adoption reports - the Ariane set, and the `.stalx` set \
             the frontend routes by kind to the OPEN verb - must record the revision \
             they describe, or the reply has nothing to be measured against"
        );
    }

    /// [#258 review] A View's answer must not evict the tip's. `speleodb_revisions`
    /// promises a View is "a non-destructive peek... a later Update fast-forwards
    /// straight back to the latest", and with one slot per project that was false the
    /// moment the viewed revision was ambiguous too: answering its picker overwrote the
    /// tip's answer, the next Update fast-forwarded, found the recall dated elsewhere,
    /// and - having no picker of its own - dead-ended on "re-open the project".
    #[test]
    fn answering_a_viewed_revisions_picker_leaves_the_tip_answer_standing() {
        let state = AppState::new().expect("state");
        let project = "eeeeeeee-eeee-eeee-eeee-eeeeeeeeeeee";

        state.speleodb_remember_dump_entry(project, "project.stalx", DumpAt::Revision(TIP));
        // The View is a different tree, so it is a different question and gets its own
        // answer - which used to land in the same slot.
        state.speleodb_remember_dump_entry(project, "backup/old.stalx", DumpAt::Revision(NEXT));

        assert_eq!(
            state
                .speleodb_recalled_dump_entry(project, DumpAt::Revision(TIP))
                .as_deref(),
            Some("project.stalx"),
            "the tip's answer must survive a View, or Update cannot fast-forward back"
        );
        assert_eq!(
            state
                .speleodb_recalled_dump_entry(project, DumpAt::Revision(NEXT))
                .as_deref(),
            Some("backup/old.stalx"),
            "and the View keeps its own"
        );

        // A landing retires every revision's answer for the project, not one slot: a
        // non-canonical pick is stale whichever tree it was about.
        state.speleodb_forget_dump_entry(project);
        assert_eq!(
            state.speleodb_recalled_dump_entry(project, DumpAt::Revision(TIP)),
            None
        );
        assert_eq!(
            state.speleodb_recalled_dump_entry(project, DumpAt::Revision(NEXT)),
            None
        );
    }

    /// A pick is refused only where the offer positively disagrees - and the four
    /// combinations are not symmetric.
    ///
    /// "Nothing was asked" and "a question was asked about a tree we could not name"
    /// are different states, and collapsing them (as `is_none_or` did) let a reply to
    /// the second be accepted against ANY revision: a collaborator pushing while the
    /// picker was open could have the pick pass membership against a tree whose size
    /// and timestamp the user never saw. Only the both-undatable corner stays
    /// permissive, because refusing there would re-offer an equally undatable question
    /// forever.
    #[test]
    fn a_pick_is_only_refused_when_the_offer_positively_disagrees() {
        let state = AppState::new().expect("state");
        let zip = make_zip(&["a.stalx", "b.stalx"]);
        let pick = |project: &str, at: DumpAt<'_>| {
            let mut dump = Dump::open(&zip).expect("open");
            state.speleodb_resolve_entry_remembering(
                project,
                &mut dump,
                Some("b.stalx"),
                ArianeRoute::Adopt,
                at,
                StalxAmbiguity::Ask,
            )
        };

        // No offer at all: nothing to contradict, membership carries it.
        let never_asked = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb";
        assert_eq!(
            pick(never_asked, DumpAt::Revision(TIP)).expect("no offer contradicts nothing"),
            "b.stalx"
        );

        // An UNDATED offer answered by a DATED dump: positive evidence they are not
        // about the same thing, so refuse - and the re-offer is datable, so it ends.
        let undated = "cccccccc-cccc-cccc-cccc-cccccccccccc";
        state.speleodb_note_dump_offer(undated, DumpAt::Unknown);
        assert!(
            matches!(
                pick(undated, DumpAt::Revision(TIP)),
                Err(SpeleoDbError::AmbiguousDump(_))
            ),
            "a reply to a question about an unnameable tree must not be honoured \
             against a nameable one"
        );
        // ...and neither datable: the one corner that has to stay permissive. On a
        // FRESH project, because the refusal above re-offered - with a revision this
        // time - which is exactly the convergence that stops it looping, and therefore
        // also changes what the next reply is measured against.
        let both_undated = "cccccccc-2222-cccc-2222-cccccccccccc";
        state.speleodb_note_dump_offer(both_undated, DumpAt::Unknown);
        assert_eq!(
            pick(both_undated, DumpAt::Unknown).expect("nothing distinguishes them"),
            "b.stalx"
        );

        // A DATED offer answered by an undatable dump fails the same way round.
        let dated = "dddddddd-1111-dddd-1111-dddddddddddd";
        state.speleodb_note_dump_offer(dated, DumpAt::Revision(TIP));
        assert!(
            matches!(
                pick(dated, DumpAt::Unknown),
                Err(SpeleoDbError::AmbiguousDump(_))
            ),
            "an undatable dump cannot be shown to be the tree the offer described"
        );
        // The matching case, again on a fresh project: every refusal above re-offers,
        // so the recorded state after one is not the state it was measured against.
        // That is the mechanism, not an artefact - it is what makes the loop converge.
        let matching = "dddddddd-3333-dddd-3333-dddddddddddd";
        state.speleodb_note_dump_offer(matching, DumpAt::Revision(TIP));
        assert_eq!(
            pick(matching, DumpAt::Revision(TIP)).expect("the offer's own revision"),
            "b.stalx"
        );
    }

    /// [#258] A dump nobody can date cannot be shown to match any choice, so it
    /// neither honours one nor records one. Failing closed costs a question; the
    /// alternative is trusting a name on the strength of nothing, which is the whole
    /// bug.
    #[test]
    fn an_undatable_dump_neither_recalls_nor_records() {
        let state = AppState::new().expect("state");
        let project = "77777777-7777-7777-7777-777777777777";

        state.speleodb_remember_dump_entry(project, "a.stalx", DumpAt::Unknown);
        assert_eq!(
            state.speleodb_recalled_dump_entry(project, DumpAt::Revision(TIP)),
            None,
            "a choice with no revision on it must never have been stored"
        );

        state.speleodb_remember_dump_entry(project, "a.stalx", DumpAt::Revision(TIP));
        assert_eq!(
            state.speleodb_recalled_dump_entry(project, DumpAt::Unknown),
            None,
            "a dated choice is still no answer for a dump we cannot date"
        );
    }

    /// [#258] The fast-forward verbs accept no `entryPath` and no picker is bound to
    /// them, so reporting candidates to Start editing / Update / Take remote offers a
    /// question that reaches a frontend with no way to ask it - it surfaced as a bare
    /// "more than one Stalagmight file". They get the way out named instead. The
    /// asking callers are unaffected: the candidate list IS their answer.
    #[test]
    fn only_a_caller_that_can_ask_is_handed_the_candidates() {
        let state = AppState::new().expect("state");
        let project = "88888888-8888-8888-8888-888888888888";
        let zip = make_zip(&["a.stalx", "b.stalx"]);

        let mut dump = Dump::open(&zip).expect("open");
        let refused = state.speleodb_resolve_entry_remembering(
            project,
            &mut dump,
            None,
            ArianeRoute::Adopt,
            DumpAt::Revision(TIP),
            StalxAmbiguity::Refuse,
        );
        // Folded to one assertion rather than a match arm that panics: any other
        // outcome debug-prints into `got`, which cannot contain the phrase, so the
        // wrong-variant case fails here too and says what it got.
        let got = match refused {
            Err(SpeleoDbError::Parse(msg)) => msg,
            other => format!("{other:?}"),
        };
        assert!(
            got.contains("re-open"),
            "a caller with no picker must be refused with a message naming the flow \
             that CAN ask, or this is just the dead end it replaces; got: {got}"
        );

        let mut dump = Dump::open(&zip).expect("open");
        assert!(
            matches!(
                state.speleodb_resolve_entry_remembering(
                    project,
                    &mut dump,
                    None,
                    ArianeRoute::Adopt,
                    DumpAt::Revision(TIP),
                    StalxAmbiguity::Ask,
                ),
                Err(SpeleoDbError::AmbiguousDump(_))
            ),
            "a caller that hosts the picker must still get the candidates"
        );

        // [#258 review] And the OTHER ambiguity, which slipped between the two flags:
        // the fast-forward passes `ArianeRoute::Adopt` (it CAN route a lone `.tml` to
        // adoption) with `StalxAmbiguity::Refuse` (it CANNOT ask), so an ambiguous
        // Ariane set matched neither refusing arm and shipped candidates to a verb with
        // no picker bound to it.
        let ariane = make_zip(&["one/ariane.tml", "two/ariane.tml"]);
        let mut dump = Dump::open(&ariane).expect("open");
        let refused_ariane = state.speleodb_resolve_entry_remembering(
            project,
            &mut dump,
            None,
            ArianeRoute::Adopt,
            DumpAt::Revision(TIP),
            StalxAmbiguity::Refuse,
        );
        let got = match refused_ariane {
            Err(SpeleoDbError::Parse(msg)) => msg,
            other => format!("{other:?}"),
        };
        assert!(
            got.contains("re-open"),
            "an Ariane ambiguity must be refused with directions for a caller that \
             cannot ask, not answered with a candidate list; got: {got}"
        );

        // A LONE `ariane.tml` stays a routing signal even here - the frontend does act
        // on it - so the refusal above must be the ambiguous set only.
        let sole = make_zip(&["ariane.tml"]);
        let mut dump = Dump::open(&sole).expect("open");
        assert!(
            matches!(
                state.speleodb_resolve_entry_remembering(
                    project,
                    &mut dump,
                    None,
                    ArianeRoute::Adopt,
                    DumpAt::Revision(TIP),
                    StalxAmbiguity::Refuse,
                ),
                Err(SpeleoDbError::AdoptAriane)
            ),
            "a single .tml is an adoption route, not an unanswerable question"
        );
    }

    /// [#258 review] The fast-forward must fetch the commit it dates and stamps. It
    /// downloaded plain HEAD while dating with a `tip` its caller resolved in an earlier
    /// request, and asserted in a comment that the two agreed - which is the same
    /// two-request window the pull and adopt paths were changed to close. `Update` holds
    /// no lock and `Take remote` releases before getting here, so nothing prevents a
    /// push landing inside it.
    ///
    /// Source-order, because reaching the real thing needs a server that can advance its
    /// head between two requests; the live orchestration tests are where a behavioural
    /// version belongs.
    #[test]
    fn the_fast_forward_downloads_the_commit_it_dates_and_stamps() {
        let src = prod_source();
        let start = src
            .find("fn speleodb_fast_forward")
            .expect("the fast-forward verb is present");
        let body = &src[start..];
        let body = &body[..body.find("\n    }\n").expect("fn body terminates")];

        assert!(
            body.contains("usable_tip(tip.map(str::to_owned)"),
            "the caller's tip must cross the by-hash validation boundary before it can \
             pin a download"
        );
        assert_eq!(
            body.matches("download_dump_at_hash(").count(),
            1,
            "the fast-forward must fetch BY HASH when it has a usable tip"
        );
        assert!(
            body.contains("DumpAt::from_sha(pinned.as_deref())"),
            "the choice must be dated with the sha the bytes were fetched at, not one \
             resolved by a separate request"
        );
        assert!(
            body.contains("base_commit_sha: pinned.clone()"),
            "and BASE must name that same sha, so it cannot claim a commit these bytes \
             did not come from"
        );
    }

    /// [#258 review] Adoption's best-effort head resolve must not be able to delete the
    /// user's credential. `speleodb_server_head` wraps the call in `speleodb_handle_auth`,
    /// which `forget()`s the token on a 401 - keychain entry included - and this
    /// failure is deliberately swallowed, so a spurious 401 signed the user out with no
    /// signal while the adopt went on to succeed. The pull states the rule for exactly
    /// this shape of call, and adoption itself restates it for the type flip.
    #[test]
    fn adoptions_best_effort_head_resolve_stays_off_the_auth_drop() {
        let src = prod_source();
        let start = src
            .find("fn speleodb_adopt_ariane")
            .expect("the adoption verb is present");
        let body = &src[start..];
        let body = &body[..body.find("\n    }\n").expect("fn body terminates")];

        assert_eq!(
            body.matches("speleodb_server_head(").count(),
            0,
            "the head resolve must not route through the 401 auto-drop: its failure is \
             swallowed, so the drop is invisible and takes the keychain entry with it"
        );
        assert_eq!(
            body.matches("client::get_revisions(").count(),
            1,
            "it calls the client directly instead, as the pull path does"
        );
    }

    /// The rule above is only worth anything if every landing path applies it, and
    /// "landed" is not the same set as "returned Confirmed". A source scan because
    /// reaching the real thing needs a server; the failure being guarded is someone
    /// adding - or quietly un-routing - a push arm, which is structural.
    #[test]
    fn every_landing_push_arm_invalidates_the_remembered_entry() {
        // Definition plus the three arms that conclude a revision exists AND return an
        // outcome: the 200, the 304 (our content is already there - the upload that
        // put it there is what made the recall stale, whichever attempt returns), and
        // the late-recognised `AdoptConfirmed`. A count below this means an arm stopped
        // invalidating, which silently restores the stale-recall rollback.
        assert_eq!(
            speleodb_prod_matches("speleodb_revision_landed(", &[]),
            4,
            "expected one definition plus the three landing arms that return an \
             outcome (200, 304, AdoptConfirmed)"
        );
        // BOTH adoption plans conclude a prior push landed, and counting only the ones
        // that return an outcome missed the reachable one: `AdoptThenUpload` adopts the
        // tip and CONTINUES to push on top, so it has no outcome to hand back - while
        // `AdoptConfirmed` is documented as forward-compatible only. Enumerating them
        // separately is what stops a "landing" arm being overlooked because of the
        // shape of its return.
        assert_eq!(
            speleodb_prod_matches("speleodb_note_prior_push_landed(", &[]),
            3,
            "expected one definition, the `speleodb_revision_landed` wrapper, and \
             `AdoptThenUpload` - the adoption plan that concludes a landing without \
             returning one"
        );
        // And that the invalidation itself has exactly one caller - the primitive
        // above - so concluding a landing and dropping the recall cannot drift apart.
        assert_eq!(
            speleodb_prod_matches("speleodb_forget_dump_entry(", &[]),
            2,
            "the recall must be dropped through `speleodb_note_prior_push_landed` \
             alone; a second caller means some path invalidates without concluding a \
             landing (or vice versa)"
        );
    }

    /// Invalidating is not enough on its own - it has to happen BEFORE the fallible
    /// link write each arm performs, or a disk/db failure returns while a choice we
    /// already know is stale stays standing (and the next discarding open extracts
    /// that pre-push entry and re-stamps it). Inlining the call into the arm's return
    /// expression is exactly what puts it last, so require every arm to bind it to a
    /// local instead: the binding cannot sit after the `?` and still be returned.
    #[test]
    fn no_landing_arm_defers_the_invalidation_behind_its_link_write() {
        assert_eq!(
            speleodb_prod_matches("Ok(self.speleodb_revision_landed(", &[]),
            0,
            "a landing arm is returning `speleodb_revision_landed` inline, which \
             places the invalidation AFTER its `speleodb_mutate_link(..)?`. Bind it \
             to a local before the write and return that local instead."
        );
    }

    /// [#258] The probe's answer is advisory, but the request that produces it is
    /// authenticated: `hexsha` lands in the URL path, so anything that could reshape
    /// that path is rejected before the request is built - the same guard the restore
    /// path applies. Tested on the pure helper because inside the command every
    /// rejection is indistinguishable from the "not logged in" `None`.
    #[test]
    fn the_probe_validates_both_ids_before_building_an_authenticated_url() {
        let project = "44444444-4444-4444-4444-444444444444";
        assert!(probe_inputs_valid(project, None));
        assert!(probe_inputs_valid(project, Some("abc1234")));

        for bad in [
            "../../admin",
            "abc/../../x",
            "abc1234/../../projects",
            "not-hex",
            "abc",
            "",
        ] {
            assert!(
                !probe_inputs_valid(project, Some(bad)),
                "`{bad}` must never reach the download-at-hash URL"
            );
        }
        assert!(
            !probe_inputs_valid("../../etc", Some("abc1234")),
            "a bad project id is refused on the same boundary"
        );
    }

    #[test]
    fn resolve_honours_a_pick_the_archive_actually_offers() {
        // [#258] The picker's whole contract: having chosen, the user's next attempt
        // opens THAT file rather than hitting the same refusal again.
        let zip = make_zip(&["a.stalx", "b.stalx"]);
        let mut dump = Dump::open(&zip).expect("open");
        assert_eq!(
            resolve_dump_entry(&mut dump, Some("b.stalx")).expect("honoured"),
            "b.stalx"
        );
    }

    #[test]
    fn resolve_refuses_a_pick_this_archive_does_not_offer_and_re_offers_the_real_ones() {
        // A stale pick (the dump moved on while the picker sat open) and a hostile
        // one are the same case here, and neither may reach the extract. Crucially
        // the user is re-offered what the dump holds NOW rather than being told
        // their choice was invalid - the second attempt is the useful one.
        let zip = make_zip(&["a.stalx", "b.stalx", "notes.txt"]);
        let mut dump = Dump::open(&zip).expect("open");
        for hostile in ["notes.txt", "gone.stalx", "../../etc/passwd"] {
            let offered: Vec<String> = match resolve_dump_entry(&mut dump, Some(hostile)) {
                Err(SpeleoDbError::AmbiguousDump(candidates)) => {
                    candidates.iter().map(|c| c.entry_path.clone()).collect()
                }
                _ => Vec::new(),
            };
            assert_eq!(
                offered,
                vec!["a.stalx".to_owned(), "b.stalx".to_owned()],
                "`{hostile}` must be refused and the archive's real candidates re-offered"
            );
        }
    }

    #[test]
    fn every_classify_site_goes_through_the_remembering_resolver() {
        // [#258] Structural, like the guard-order tripwire above, and for the same
        // reason: the behaviour needs an `AppState`, whose construction latches the
        // process-wide app dirs into every other test in this binary.
        //
        // What it protects: the ambiguity is normally settled when the project is
        // OPENED, which takes no lock. Restore must CONSULT that answer, not merely
        // record it - it did only the latter once, so the revert composite asked
        // again while holding the write lock and had to hand it back to do so. A new
        // classify site that reaches for the bare resolver would silently reintroduce
        // that, since the bare one still works and just forgets.
        // Family-wide, not this file: the restore site lives in `speleodb_revisions`,
        // and a scan that cannot see it is a tripwire that cannot guard it.
        assert_eq!(
            speleodb_prod_matches("resolve_dump_entry(", &[]),
            2,
            "resolve_dump_entry must have exactly one definition and one caller - the \
             remembering wrapper. A direct call skips both the recall and the record."
        );
        assert_eq!(
            speleodb_prod_matches("speleodb_resolve_entry_remembering(", &[]),
            6,
            "expected one definition plus the five classify sites (pull, fast-forward, \
             restore, rewind-target resolution, and the lock-free probe); a change here \
             means a site was added or one stopped remembering"
        );
    }

    #[test]
    fn a_chosen_ariane_entry_is_never_handed_to_the_cache_replace() {
        // [#258] Every caller passes this result to `replace_cache_stalx_with`, so
        // returning a `.tml` path here wrote TML bytes into the cache `.stalx` - a
        // file that cannot be opened, written over whatever clean copy was there.
        // The choice is honoured, but by the adoption flow, which imports.
        let zip = make_zip(&["ariane.tml", "misc/ariane.tml"]);
        let mut dump = Dump::open(&zip).expect("open");
        assert!(matches!(
            resolve_dump_entry(&mut dump, Some("misc/ariane.tml")),
            Err(SpeleoDbError::AdoptAriane)
        ));
    }

    #[test]
    fn resolve_ignores_a_pick_when_the_dump_is_not_ambiguous() {
        // A chosen path must never be able to steer an open that had no question to
        // answer - otherwise the picker's arg becomes a way to extract an arbitrary
        // entry of any dump.
        let zip = make_zip(&["misc/project.stalx", "notes.txt"]);
        let mut dump = Dump::open(&zip).expect("open");
        assert_eq!(
            resolve_dump_entry(&mut dump, Some("notes.txt")).expect("resolves normally"),
            "misc/project.stalx"
        );
    }

    /// The production region of this file, split out of the test module so the
    /// source-level tripwire below cannot match its own assertions.
    fn prod_source() -> &'static str {
        include_str!("speleodb_pull.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("production region precedes the test module")
    }

    #[test]
    fn the_pull_runs_the_local_guard_before_it_downloads_the_dump() {
        // The guard is purely LOCAL (it reads the cached copy's link row), but it ran
        // only inside the replace - i.e. after `download_dump` had pulled the entire
        // working tree over the wire and into memory, to be thrown away. That is a long
        // wait and a large buffered allocation spent to reach a decision that was
        // available before the first byte, and it costs more the larger the dump.
        //
        // Guarded by ORDER in the source: a behavioural version would have to latch the
        // process-wide app dirs to resolve a cache path (`app_dirs::init_for_test`),
        // which leaks into every other test in the binary. The in-replace call is
        // deliberately left in place as the backstop no call site can forget, so an
        // assertion on the outcome alone cannot distinguish the two orderings anyway.
        let src = prod_source();
        let start = src
            .find("fn speleodb_pull_project")
            .expect("the shared pull body is present");
        let body = &src[start..];
        let body = &body[..body.find("\n    }\n").expect("fn body terminates")];
        let guard = body
            .find("self.guard_local_edits(")
            .expect("the pull must run the local guard itself, not only via the replace");
        let download = body
            .find("client::download_dump(")
            .expect("the pull downloads the dump");
        assert!(
            guard < download,
            "the local un-pushed-edits guard must run BEFORE the download, not after it"
        );
    }

    #[test]
    fn classify_dump_entry_maps_each_arm() {
        assert_eq!(
            classify_dump_entry(&["misc/project.stalx".to_owned()]).expect("single stalx"),
            "misc/project.stalx"
        );
        assert!(matches!(
            classify_dump_entry(&["ariane.tml".to_owned()]),
            Err(UnopenableDump::Adopt)
        ));
        assert!(matches!(
            classify_dump_entry(&["notes.txt".to_owned()]),
            Err(UnopenableDump::Empty)
        ));
        assert_eq!(
            classify_dump_entry(&["a.stalx".to_owned(), "b.stalx".to_owned()]),
            Err(UnopenableDump::AmbiguousStalx(vec![
                "a.stalx".to_owned(),
                "b.stalx".to_owned()
            ])),
            "the ambiguity must carry BOTH paths, in order - the picker renders them"
        );
        // [#258] Two ariane.tml with no .stalx used to take the first silently.
        assert_eq!(
            classify_dump_entry(&["ariane.tml".to_owned(), "misc/ariane.tml".to_owned()]),
            Err(UnopenableDump::AmbiguousAriane(vec![
                "ariane.tml".to_owned(),
                "misc/ariane.tml".to_owned()
            ]))
        );
        // A .stalx alongside any number of .tml is NOT ambiguous: the .tml is a
        // derived render sidecar and is disregarded whenever a .stalx exists.
        assert_eq!(
            classify_dump_entry(&[
                "misc/project.stalx".to_owned(),
                "ariane.tml".to_owned(),
                "misc/ariane.tml".to_owned()
            ])
            .expect("stalx wins"),
            "misc/project.stalx"
        );
    }
}
