//! The one-shot Ariane adoption (#214): take the project's write mutex, download its
//! `.tml`-only dump, import the `ariane.tml` through the TML importer into a fresh
//! cache `.stalx`, stamp the hand-off link, push it with its Ariane sidecar, release
//! the mutex, flip the server project type ariane->stalagmight, and close so the webview's
//! own load is a real load.
//!
//! Split out of `core/state/speleodb_pull.rs` (#157 file-size ratchet) into a
//! sibling `impl AppState` block. It shares the pull's entry classification
//! (`unopenable_to_error`) and dating (`pull_dating`, `usable_tip`), and differs
//! from every pull in importing rather than extracting - which is why it is the one
//! cache replace that does not run through `replace_cache_stalx`.
//!
//! [#576 A8] The steps' order is the types' (`adopt_order`).

use std::path::Path;

use super::session_commit::CommandsHeld;
use super::speleodb::{
    local_dto, lock_held_by_me, managed_type_patch, type_says_managed, validate_project_id, DumpAt,
};
use super::speleodb_pull::{unopenable_to_error, UnopenableDump};
use super::{AppState, VirtualShotMode};
use crate::core::command::{CommandResult, ImportReceipt};
use crate::models::{ImportClosure, ImportSectionConfig};
use crate::speleodb::contract::Endpoint;
use crate::speleodb::dump::{self, DumpClassification};
use crate::speleodb::models::AdoptUpload;
use crate::speleodb::{client, sync};
use crate::speleodb::{SpeleoAdoptResult, SpeleoDbError, SpeleoErrorDto};

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

#[cfg(test)]
pub(super) use adopt_order::AdoptionEditGate;

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
/// caps the list they land in, once. (Capping twice would now be harmless - since
/// #576 C17 `cap_warnings` carries the count it hid in `hidden_warnings` - but one cap
/// is all the list needs.)
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

impl AppState {
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
    ///
    /// [#576 A8] The steps' order is the types' (`adopt_order`): the verb's answer is
    /// made only by its last step, the close.
    pub fn speleodb_adopt_ariane(
        &self,
        project_id: String,
        project_name: String,
        latest_commit_sha: Option<String>,
        discard_local: bool,
        entry_path: Option<String>,
    ) -> Result<SpeleoAdoptResult, SpeleoErrorDto> {
        self.speleodb_adopt_ariane_chain(
            project_id,
            project_name,
            latest_commit_sha,
            discard_local,
            entry_path,
        )
        .map(adopt_order::AdoptClosed::into_result)
    }

    /// [`Self::speleodb_adopt_ariane`]'s body, as the chain of `adopt_order` proofs.
    #[expect(
        clippy::result_large_err,
        reason = "returns the SpeleoErrorDto every sibling SpeleoDB step returns; boxing it is a \
                  repo-wide change outside #576 A8"
    )]
    fn speleodb_adopt_ariane_chain(
        &self,
        project_id: String,
        project_name: String,
        latest_commit_sha: Option<String>,
        discard_local: bool,
        entry_path: Option<String>,
    ) -> Result<adopt_order::AdoptClosed, SpeleoErrorDto> {
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
        //
        // [#576 A8] Both halves by type. Everything the lock decision rests on is read
        // INSIDE the span (`InSpan`, made only by the span's own reads), and the decision
        // ends the span (`LockTaken::end_span` takes the guard by value): the account
        // read and the probe cannot drift above it, and the download - a method of the
        // guard armed from that decision - cannot drift into it.
        let (endpoint, token, insecure_http, my_email, concurrent_session, decision) = {
            let lock_ops = self.speleodb_lock_ops_guard();
            let account = lock_ops.read_account().transpose().map_err(local_dto)?;
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
            let holder = lock_ops.read_holder(&account, insecure_http, &project_id);
            // The account whose token takes the mutex below, captured HERE rather than
            // re-read later. The release verdict at the end of this function compares
            // the server's holder against it, and a sign-out or account switch landing
            // in between changes what `speleodb_my_email()` answers without changing who
            // acquired - so a live re-read there would be comparing the mutex against a
            // stranger's identity, or against no identity at all.
            let my_email = self.speleodb_my_email();
            let concurrent_session = my_email.as_deref().is_some_and(|me| {
                sync::concurrent_session_notice(
                    holder.get().as_ref().ok().and_then(Option::as_deref),
                    me,
                )
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
            let taken =
                self.adopt_take_lock(&lock_ops, &account, insecure_http, &project_id, holder);
            let (endpoint, token) = account.into_inner();
            let decision = taken.end_span(lock_ops);
            (
                endpoint,
                token,
                insecure_http,
                my_email,
                concurrent_session,
                decision,
            )
        };

        // Arms the hand-back for every way the stretch below can return early. Disarmed
        // once there is a converted project for the lock to be about - see the guard.
        let lock_guard = adopt_order::AdoptLockGuard::arm(
            self,
            &endpoint,
            &token,
            insecure_http,
            &project_id,
            decision,
        );

        // [#258] Resolve the head BEFORE the download and fetch that commit by hash -
        // the guard's own method, so it runs after the lock decision by construction
        // (see `AdoptLockGuard::fetch`).
        let fetched = lock_guard.fetch()?;

        // Classify authoritatively. Only proceed for a genuine Ariane-only dump; if a
        // `.stalx` has since appeared (a stale `type_tag`, a TOCTOU race with another
        // writer), refuse and tell the user to reopen normally rather than clobber it.
        let mut dump = dump::Dump::open(fetched.zip()).map_err(local_dto)?;
        let tml_entry = self.adopt_pick_entry(
            &project_id,
            &mut dump,
            entry_path.as_deref(),
            fetched.dump_at(),
        )?;

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
        //
        // [#576 NF-W5L-3] Checked INSIDE the adoption's `command_lock` span (see
        // `adopt_import_and_save`), not here: checked before the span, an edit to the
        // open cache copy landing between the check and the span was overwritten.
        let guard_local_edits_of = (!discard_local).then_some(project_id.as_str());
        let tml_scratch = dest.with_file_name("adopt-source.tml");
        dump.extract_entry_to(&tml_entry, &tml_scratch)
            .map_err(local_dto)?;

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
        //
        // [#576 A8] By type: the link is built from the `AdoptFetched` the bytes came
        // in (`AdoptedLink::new`), so no later lookup can reach BASE.
        let link = adopt_order::AdoptedLink::new(
            &fetched,
            endpoint.clone(),
            project_id.clone(),
            project_name.clone(),
            latest_commit_sha,
        );

        // Stamp the hand-off link onto the project this call builds, through the
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
        // the value that works: the link is built at the stamp and the push below starts
        // after it, so `last_edit_at <= push_start` and a landed push clears the bit.
        //
        // [#576 NF-W4C-3] Built here, STAMPED inside `adopt_import_and_save`, as the
        // last step of the one `command_lock` span that builds, saves, promotes and
        // reopens the copy - see that function for what a separate stamp let in.
        // [#576 NF-W4C-3-EDIT] The local span hands back the guard that lifts the edit
        // gate it shuts, when this adoption returns - on every path, early or not.
        // [#576 A8] By type: the close consumes it, so it is bound to a name for the
        // whole call and cannot be dropped early; [RV9a-3] and only the local span
        // makes one (`AdoptionEditGate`), so none can be built inline and dropped early.
        let adopt = self.adopt_local_span(
            guard_local_edits_of,
            &tml_scratch,
            &tml_entry,
            &dest,
            project_name,
            link,
        );
        // Best-effort scratch cleanup regardless of import outcome.
        let _ = std::fs::remove_file(&tml_scratch);
        // [#411] What the conversion actually did, carried to the return below rather
        // than dropped here: the call site was the last place it existed, so a pulled
        // Ariane project lost its excluded shots and every parse anomaly in silence.
        let (edit_gate, import_receipt, stamped) = adopt?;
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
        //
        // [#576 A8] By type: `disarm` takes the stamp's proof, and only a disarmed guard
        // hands a held lock to the hand-over (`HeldForHandOver`).
        //
        // Upload the converted `.stalx` with its Ariane sidecar and hand the lock
        // back. Best-effort by construction: every failure arm leaves the adopted
        // project on disk, stamped dirty, and reports why - adoption that could not
        // reach the server is still an adoption.
        let (landed, upload) = match stamped {
            Ok(stamped) => match lock_guard.disarm(&stamped) {
                adopt_order::Disarmed::Held(held) => {
                    let handed = self.adopt_hand_over(held, &project_id);
                    (handed.landed(), handed.into_upload())
                }
                adopt_order::Disarmed::NotTaken(why) => (None, why),
            },
            // Nothing to hand over: the push reads the link to decide whether to run at
            // all. Say so rather than letting `start_editing` discover it and report the
            // same thing one indirection later. The guard above is still armed, so the
            // lock this call took goes back on the way out.
            Err(e) => (
                None,
                AdoptUpload::Failed {
                    message: format!("the converted project could not be linked to SpeleoDB: {e}"),
                },
            ),
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
        let (verdict, upload) = {
            let lock_ops = self.speleodb_lock_ops_guard();
            // Make sure the server project ends up typed `stalagmight`, and report whether it
            // did. Only after a landed push, which is the precondition either way:
            // flipping first leaves a project typed `stalagmight` whose only file is the Ariane
            // one that type says to disregard, openable by nobody. [#576 A8] By type: the
            // settle takes the hand-over's `AdoptLanded` and this span's proof.
            //
            // [#576 NF-A8R-GAPS] Then the release-verdict correction, through the settle's
            // own span: `AdoptSettled` borrows `lock_ops`, and only its
            // `correct_release_verdict` makes the verdict the close takes.
            self.adopt_settle(
                &lock_ops,
                landed.as_ref(),
                &endpoint,
                &token,
                insecure_http,
                &project_id,
            )
            .correct_release_verdict(
                self,
                upload,
                my_email.as_deref(),
                &project_id,
                &endpoint,
            )
        };
        // Hand the project back to the frontend to open, LAST - see `adopt_close`,
        // whose answer is the verb's.
        Ok(self.adopt_close(
            edit_gate,
            verdict,
            adopt_order::AdoptReport {
                dest: &dest,
                concurrent_session,
                upload,
                import_receipt,
            },
        ))
    }

    /// [#258] Which `ariane.tml` of the dump adoption imports - the picker's answer,
    /// dated, or the dump's own one - recording the offer when it has to ask.
    ///
    /// [#576 A8] Its own step so the dating rules are driven by tests rather than read
    /// off the verb's source: `adopt_dump_at` is the revision the dump was fetched at
    /// (`AdoptFetched::dump_at`).
    #[expect(
        clippy::result_large_err,
        reason = "returns the SpeleoErrorDto every sibling SpeleoDB step returns; boxing it is a \
                  repo-wide change outside #576 A8"
    )]
    pub(super) fn adopt_pick_entry(
        &self,
        project_id: &str,
        dump: &mut dump::Dump<'_>,
        entry_path: Option<&str>,
        adopt_dump_at: DumpAt<'_>,
    ) -> Result<String, SpeleoErrorDto> {
        let names = dump.entry_paths().map_err(local_dto)?;
        // [#258] A dump with several `ariane.tml` is ambiguous, and this is the flow
        // that answers it: the picker's choice arrives here, checked for membership
        // against THIS archive exactly like a `.stalx` pick. Without it the picker
        // would re-raise the same question on every retry, because nothing else on
        // this path can act on the answer.
        // The picker's answer reaches this command either directly, or - when the
        // ambiguity surfaced on the OPEN path, which refuses a `.tml` and routes
        // here - through the entry that path recorded for this session. That hand-off
        // is why the dating below is the TRUE HEAD and not the caller-supplied
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
        let remembered = self.speleodb_recalled_dump_entry(project_id, adopt_dump_at);
        let picked = entry_path
            // The fresh reply is gated exactly as the resolver gates one: honoured only
            // while it answers the revision its candidates were offered for. The recall
            // beside it is already dated, so a discarded reply correctly falls back to
            // a remembered answer that IS about this tree, and otherwise to a re-offer.
            .filter(|_| self.speleodb_dump_offer_answers(project_id, adopt_dump_at))
            .or(remembered.as_deref())
            .filter(|chosen| {
                dump::chosen_candidate(&names, chosen) == Some(dump::ChosenKind::Ariane)
            });
        Ok(match (picked, dump::classify(&names)) {
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
                self.speleodb_note_dump_offer(project_id, adopt_dump_at);
                return Err(local_dto(unopenable_to_error(
                    UnopenableDump::AmbiguousStalx(paths),
                    dump,
                )));
            }
            (None, DumpClassification::AmbiguousAriane(paths)) => {
                self.speleodb_note_dump_offer(project_id, adopt_dump_at);
                return Err(local_dto(unopenable_to_error(
                    UnopenableDump::AmbiguousAriane(paths),
                    dump,
                )));
            }
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
    ///
    /// [#576 A8] The span-held body of `adopt_order`'s `adopt_take_lock`, which reads
    /// its inputs inside the span; `LockTaken::end_span` then ends the span.
    fn adopt_acquire_in_span(
        &self,
        _lock_ops: &super::speleodb_session::LockOpsHeld<'_>,
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
    ///
    /// [#576 A8] Takes the verdict span's proof (the holder it reads and the claim
    /// clear it decides are one act) and the hand-over's [`adopt_order::AdoptLanded`]
    /// (the flip is only ever earned by a landed push).
    fn adopt_finalize(
        &self,
        _lock_ops: &super::speleodb_session::LockOpsHeld<'_>,
        landed: &adopt_order::AdoptLanded,
        endpoint: &str,
        token: &str,
        insecure_http: bool,
        project_id: &str,
    ) -> AdoptFinalize {
        let (needs_flip, mut holder) =
            match client::get_project(endpoint, token, insecure_http, project_id) {
                // Already `stalagmight`: the push's heal got there first, which is the
                // ordinary path. (A legacy `other` is not managed - SpeleoDB never maps
                // it - so it falls through to the PATCH below.) Nothing to do, and nothing
                // to report but success - but the holder still comes back, because the
                // release verdict needs it either way.
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
            landed,
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

    /// The adoption's whole LOCAL half: build the `ariane.tml` at `tml_path` into a
    /// fresh project, save and promote it over `dest`, reopen it there
    /// ([`Self::adopt_build_and_promote`]), and stamp the hand-off link `make_link`
    /// builds onto it. `make_link` gets the undo mode the new link materializes.
    ///
    /// [#576 NF-W4C-3] ALL of it in ONE `command_lock` span, failure paths included
    /// (the abandon's close and reopen of the project it replaced). Each step used to
    /// take the lock on its own - the create's commit, the import, the save, the
    /// close, the reopen - and the stamp took only `database`, so a command (a
    /// frontend edit, an autosave) could run between two of them against the
    /// half-built project: an edit whose small section/shot ids resolved landed in the
    /// adopted copy and was saved into it, and an autosave between the reopen and the
    /// stamp copied the absent link and renamed a fresh file over the live one, so the
    /// stamp went into the replaced file. The steps run through their `_held`
    /// variants, which take the caller's proof instead of the lock. Nothing in the
    /// span makes a network call - the dump was downloaded before it, the hand-over
    /// runs after it - and nothing in it takes `speleodb_lock_ops`, which sits above
    /// `command_lock` (docs/undo-redo.md, Lock Ordering).
    ///
    /// Returns the import receipt and the stamp's own result: a stamp that fails
    /// still leaves a finished conversion, which the caller reports rather than
    /// refuses.
    #[expect(
        clippy::result_large_err,
        reason = "the SpeleoErrorDto every adoption step returns (its siblings carry the same \
                  warning); boxing the error type is a repo-wide change outside NF-W4C-3"
    )]
    pub(super) fn adopt_import_and_save(
        &self,
        guard_local_edits_of: Option<&str>,
        tml_path: &Path,
        entry_name: &str,
        dest: &Path,
        project_name: String,
        make_link: impl FnOnce(crate::models::UndoMode) -> crate::models::SpeleoLink,
    ) -> Result<(Option<ImportReceipt>, Result<(), SpeleoDbError>), SpeleoErrorDto> {
        let commands = self.hold_commands();
        // [#576 NF-W5L-3] The local-edits guard, first thing inside the span: an edit
        // to the open cache copy either landed before it (and is seen) or waits for
        // the span to end. `None` when the user has confirmed discarding them.
        if let Some(project_id) = guard_local_edits_of {
            self.guard_local_edits(project_id, dest)
                .map_err(local_dto)?;
        }
        let receipt =
            self.adopt_build_and_promote(&commands, tml_path, entry_name, dest, project_name)?;
        // [#303] Adoption builds the `.stalx` from an imported `.tml`, so there is
        // normally no prior link to carry an undo policy from - but the file can be a
        // re-adopt over an existing cache copy, and that copy's override is still the
        // project's answer. Read it off the SAME live handle before the upsert
        // overwrites the row; no link yet initializes from the global default once.
        let adopted_undo_mode = self.speleodb_open_link().ok().and_then(|l| l.undo_mode);
        let stamped =
            self.speleodb_set_open_link(&make_link(self.effective_undo_mode(adopted_undo_mode)));
        // [#576 NF-W4C-3-EDIT] Still inside the span: the adopted project stays shut
        // to edits until the hand-over push lands. `speleodb_adopt_ariane` lifts it
        // when it returns (`AdoptionEditGate`).
        self.speleodb_shut_gate_for_adoption();
        drop(commands);
        Ok((receipt, stamped))
    }

    /// Import an extracted `ariane.tml` into a fresh project and save it as a new
    /// `.stalx` at `dest`, reopened there. Split out of [`Self::speleodb_adopt_ariane`]
    /// so the pure create → import → save pipeline is separable from the network/link
    /// steps.
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
    fn adopt_build_and_promote(
        &self,
        commands: &CommandsHeld<'_>,
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

        let receipt =
            match self.adopt_build_into(commands, tml_path, entry_name, &staged, project_name) {
                Ok(receipt) => receipt,
                Err(e) => {
                    self.adopt_abandon_staging(commands, &staged, previous.as_deref());
                    return Err(e);
                }
            };

        // Close before the rename: `Database::open_existing` holds a 256 MB mmap over
        // the staging file, and on Windows SQLite opens without `FILE_SHARE_DELETE`,
        // so renaming a live handle is the hazard `close_project_if_open_at_held` exists
        // for. This is the same close, at the same point in the same sequence.
        self.close_project_held(commands);
        // [#576 RV20-2] The cache replace's install, sidecar handling included: a
        // `-journal` the old copy left beside `dest` would otherwise stay beside the
        // adopted file, and the reopen below would play the old pages into it.
        if let Err(e) = super::speleodb_cache::install_over(&staged, dest) {
            // `rename` is atomic, so a failure leaves `dest` exactly as it was.
            self.adopt_abandon_staging(commands, &staged, previous.as_deref());
            return Err(local_dto(SpeleoDbError::Io(format!(
                "could not put the adopted project in place: {e}"
            ))));
        }

        // Reopen at the canonical path, because everything after this reads the OPEN
        // project: the link stamp writes through the live handle, and the hand-over
        // renders the sidecar from the in-memory survey. The legacy open is the right
        // one here for the reason its doc gives - this is a reopen of a canonical cache
        // path, the same case the failed-replace recovery uses it for.
        let reopened = self.open_project_held(commands, dest.to_path_buf());
        if !reopened.success {
            // The rename already consumed the staging file, so there is nothing to
            // clean up - but the close above still has to be undone. Without this the
            // one arm that fails AFTER a successful promote is also the one arm that
            // leaves the backend with nothing open, under a shell still rendering the
            // old project and still sending it edits: exactly the hazard the abandon
            // path exists to prevent, reintroduced by the one exit that skipped it.
            if let Some(previous) = previous.as_deref() {
                let restored = self.open_project_held(commands, previous.to_path_buf());
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
    /// `staged`. The pure pipeline half of [`Self::adopt_build_and_promote`], split out
    /// so its caller has ONE failure path to clean up after rather than four.
    ///
    /// Returns the import receipt, which travels back to the frontend on
    /// [`SpeleoAdoptResult::import_receipt`] and is rendered by the same receipt
    /// dialog every other foreign-format import ends in. `entry_name` is the file's
    /// name INSIDE the dump, because that is the name the user recognises - the path
    /// this reads is a scratch copy beside the cache.
    fn adopt_build_into(
        &self,
        commands: &CommandsHeld<'_>,
        tml_path: &Path,
        entry_name: &str,
        staged: &Path,
        project_name: String,
    ) -> Result<Option<ImportReceipt>, SpeleoErrorDto> {
        self.do_create_project_full_held(
            commands,
            project_name,
            None,
            None,
            Some(staged.to_path_buf()),
            true,
        )
        // [#576 C4] The create's own code and plain message. Its `Display` used to
        // be JSON, and wrapped in `Io` that reached the user as
        // "filesystem error: {\"code\":...}".
        .map_err(|e| local_dto(SpeleoDbError::refused_with(e.code, e.message)))?;

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
        let mut imported = self.execute_tml_import_sections_held(
            commands,
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

        // [#576 NF-W4C-3] What `execute(SaveProject)` runs under its lock, run here
        // under the adoption's span (`execute` would take the lock again and
        // self-deadlock). Of `execute`'s guards a save trips none: it is a lifecycle
        // pass-through, so the read-only gate and the lock re-verify skip it; it is
        // never a no-op reverse or comment move; it pushes no undo entry, takes no
        // integrity snapshot, and its epilogue has no step (no orphan conversion, no
        // reframe or reconcile, no dirty flip). The one step with an effect -
        // dropping the closure-adjusted cache - is kept.
        self.invalidate_adjusted_cache();
        let saved = self.save_project(Some(staged.to_path_buf()));
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
    fn adopt_abandon_staging(
        &self,
        commands: &CommandsHeld<'_>,
        staged: &Path,
        previous: Option<&Path>,
    ) {
        // Closing is what releases the handle the remove needs on Windows. Harmless
        // when the create never got as far as committing anything.
        self.close_project_held(commands);
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
        let reopened = self.open_project_held(commands, previous.to_path_buf());
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
}

/// [#576 A8] The Ariane adoption's chain as types. Each later step takes a value only
/// the earlier one returns, and every proof is made only here:
/// - the lock decision ([`LockDecision`]) is made from values read INSIDE the lock-ops
///   span and ENDS it, so the account read and the probe cannot move above the span
///   and the download cannot move into it;
/// - the hand-back guard is armed from that decision, and the head resolve and the
///   download are ITS method ([`AdoptLockGuard::fetch`]): nothing is read before the
///   lock is decided, and every early return after it hands the lock back;
/// - BASE is the commit the bytes were fetched at ([`AdoptedLink::new`] takes the
///   [`AdoptFetched`]), and the stamped link is dirty and dated by construction;
/// - the stamp's proof ([`DirtyStamped`]) disarms the guard, and only a disarmed guard
///   hands a held lock over ([`HeldForHandOver`]);
/// - only a hand-over that landed may settle the server type ([`AdoptLanded`]);
/// - the type settle borrows the verdict span, and the release-verdict correction
///   that clears a disproved claim is its method ([`AdoptSettled`]), so the two
///   cannot be split across spans (#576 NF-A8R-GAPS);
/// - the close is the last step, and the verb's answer is made by it ([`AdoptClosed`]).
///
/// [#576 NF-A8R-GAPS] What the types do NOT stop: an EXTRA `close_project()` written
/// earlier in the verb. The close is made last by [`AdoptClosed`], but the verb runs
/// on `&AppState`, whose `close_project` is the general teardown every command and
/// flow uses, and no proof can take a method away from one function body. Forbidding
/// it would mean running the whole chain on a narrowed handle that exposes only the
/// steps above - a rewrite of the verb, not a proof type - so it is left to review.
/// It is not behaviour-neutral (a close before the stamp leaves the stamp no live
/// handle to write through), and a close before the hand-over's Done is caught end to
/// end (decision NF-A8R-GAPS-D1): `sdb_adopt_e2e_tests` drives the verb against a stub
/// for the whole acquire / download / push / release / type-flip exchange, and fails
/// when the conversion stays local, dirty and locked - what an early close leaves.
mod adopt_order {
    use std::path::Path;

    use super::super::speleodb::DumpAt;
    use super::super::speleodb_pull::{pull_dating, usable_tip, PullDating};
    use super::super::speleodb_session::{InSpan, LockOpsHeld};
    use super::{lock_held_by_me, AdoptFinalize, AdoptLock, AppState, ADOPT_COMMIT_MESSAGE};
    use crate::core::command::ImportReceipt;
    use crate::models::{SpeleoLink, UndoMode};
    use crate::speleodb::contract::{dto_for_error, Endpoint};
    use crate::speleodb::models::{AdoptUpload, DoneResult, StartEditingResult};
    use crate::speleodb::{client, sync, SpeleoAdoptResult, SpeleoDbError, SpeleoErrorDto};

    /// Proof the adoption decided whether it holds the project's write mutex, inside
    /// the lock-ops span - which the decision ended.
    pub(super) struct LockDecision {
        lock: AdoptLock,
    }

    /// The decision, taken inside the span and not usable until the span has ended
    /// ([`LockTaken::end_span`]). [#576 RV10a-3] Two steps because the values the
    /// decision rests on borrow the span (`InSpan<'s, _>`): the guard can be given up
    /// only once they are done with, so the decision cannot also take it by value.
    pub(super) struct LockTaken {
        lock: AdoptLock,
    }

    impl AppState {
        /// Decide the adoption's write lock (see `adopt_acquire_in_span`) from the
        /// account and the holder read inside `lock_ops`.
        pub(super) fn adopt_take_lock(
            &self,
            lock_ops: &LockOpsHeld<'_>,
            account: &InSpan<'_, (String, String)>,
            insecure_http: bool,
            project_id: &str,
            holder: InSpan<'_, Result<Option<String>, SpeleoDbError>>,
        ) -> LockTaken {
            let (endpoint, token) = account.get();
            let lock = self.adopt_acquire_in_span(
                lock_ops,
                endpoint,
                token,
                insecure_http,
                project_id,
                holder.into_inner(),
            );
            LockTaken { lock }
        }
    }

    impl LockTaken {
        /// END the span the decision was taken in: the download that follows must
        /// not run inside it (an import-long span would block logout and every other
        /// lock operation for as long as it takes).
        pub(super) fn end_span(self, lock_ops: LockOpsHeld<'_>) -> LockDecision {
            drop(lock_ops);
            LockDecision { lock: self.lock }
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
    /// [`AdoptLockGuard::disarm`] passes the duty on to the hand-over once there is a converted
    /// project for the lock to be about. From that point [`AppState::speleodb_done`] owns
    /// it, and its rules are the ones that apply: released when the push lands, kept when
    /// a retry could still recover the work.
    pub(super) struct AdoptLockGuard<'a> {
        release: HandBack<'a>,
        lock: AdoptLock,
    }

    /// The hand-back itself: released on drop while `armed`. Apart from the lock it
    /// guards, so [`AdoptLockGuard::disarm`] can take the lock out by value.
    struct HandBack<'a> {
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
        /// Arm the hand-back from the lock decision: armed exactly when the lock was
        /// taken.
        pub(super) fn arm(
            state: &'a AppState,
            endpoint: &'a str,
            token: &'a str,
            insecure_http: bool,
            project_id: &'a str,
            decision: LockDecision,
        ) -> Self {
            Self {
                release: HandBack {
                    state,
                    endpoint,
                    token,
                    insecure_http,
                    project_id,
                    armed: matches!(decision.lock, AdoptLock::Held),
                },
                lock: decision.lock,
            }
        }

        /// A guard over a lock that was (`armed`) or was not taken, for the guard's own
        /// tests: production arms one only from a [`LockDecision`].
        #[cfg(test)]
        pub(super) fn for_tests(
            state: &'a AppState,
            endpoint: &'a str,
            token: &'a str,
            insecure_http: bool,
            project_id: &'a str,
            armed: bool,
        ) -> Self {
            let lock = if armed {
                AdoptLock::Held
            } else {
                AdoptLock::NotTaken(AdoptUpload::LockHeld { email: None })
            };
            Self::arm(
                state,
                endpoint,
                token,
                insecure_http,
                project_id,
                LockDecision { lock },
            )
        }

        /// [#258] Resolve the head and download the dump AT it - the guard's own step,
        /// so it can only run once the lock is decided and the hand-back armed.
        ///
        /// Fetched by hash for the reason the pull does: two requests mean a push can
        /// land between them, and a tree fetched at A but dated B would let a choice
        /// recorded here be recalled against a dump it never described. Best-effort -
        /// an unresolvable head just means the bytes are undatable, which is what `None`
        /// says, and adoption then asks again through the picker it already hosts.
        ///
        /// The resolve calls the client DIRECTLY, not via `speleodb_server_head`, which
        /// wraps the same call in `speleodb_handle_auth`. That handler `forget()`s the
        /// token on a 401 - keychain entry included - and because this failure is
        /// deliberately swallowed and the token was captured before, routing it there
        /// let a spurious 401 delete the user's stored credential while the adopt went
        /// on to succeed: signed out with no signal and nothing to sign back in with.
        /// The download, by contrast, IS routed through the auth handler like the pull's,
        /// so a dead token forces re-login.
        #[expect(
            clippy::result_large_err,
            reason = "returns the SpeleoErrorDto every sibling SpeleoDB step returns; boxing it is a \
                      repo-wide change outside #576 A8"
        )]
        pub(super) fn fetch(&self) -> Result<AdoptFetched, SpeleoErrorDto> {
            let HandBack {
                state,
                endpoint,
                token,
                insecure_http,
                project_id,
                ..
            } = self.release;
            let at = match client::get_revisions(endpoint, token, insecure_http, project_id) {
                Ok(history) => usable_tip(
                    sync::tip_of(&history.commits).map(str::to_owned),
                    project_id,
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
            let zip = state
                .speleodb_handle_auth(
                    Endpoint::DownloadDump,
                    match at.as_deref() {
                        Some(sha) => client::download_dump_at_hash(
                            self,
                            endpoint,
                            token,
                            insecure_http,
                            project_id,
                            sha,
                        ),
                        None => {
                            client::download_dump(self, endpoint, token, insecure_http, project_id)
                        }
                    },
                )
                .map_err(|e| dto_for_error(Endpoint::DownloadDump, &e))?;
            Ok(AdoptFetched { zip, at })
        }

        /// Stop guarding: the lock now belongs to the hand-over. Takes the stamp's
        /// proof - an unstamped project has no hand-over to give the lock TO - and
        /// hands a held lock on only as [`HeldForHandOver`].
        pub(super) fn disarm(self, _stamped: &DirtyStamped) -> Disarmed {
            let Self { mut release, lock } = self;
            release.armed = false;
            match lock {
                AdoptLock::Held => Disarmed::Held(HeldForHandOver { _held: () }),
                AdoptLock::NotTaken(why) => Disarmed::NotTaken(why),
            }
        }
    }

    /// [#576 A8 NF-A8-BYPASS] The adoption's download licence: the armed (or
    /// deliberately unarmed) guard, which exists only once the lock is decided.
    impl client::DumpLicence for AdoptLockGuard<'_> {}

    impl Drop for HandBack<'_> {
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

    /// What disarming the guard hands on.
    pub(super) enum Disarmed {
        /// The adoption holds the lock: the hand-over may use it.
        Held(HeldForHandOver),
        /// It did not take it, and this is what the webview is told instead.
        NotTaken(AdoptUpload),
    }

    /// Proof the guard was disarmed over a held lock - the hand-over's precondition.
    pub(super) struct HeldForHandOver {
        _held: (),
    }

    /// The dump adoption converts, with the commit it was fetched at (`None`: HEAD,
    /// undatable).
    pub(super) struct AdoptFetched {
        zip: Vec<u8>,
        at: Option<String>,
    }

    impl AdoptFetched {
        pub(super) fn zip(&self) -> &[u8] {
            &self.zip
        }

        /// The revision these bytes are - what dates the picker's answers.
        pub(super) fn dump_at(&self) -> DumpAt<'_> {
            DumpAt::from_sha(self.at.as_deref())
        }

        /// A fetch at `at`, for the tests of the steps that take one.
        #[cfg(test)]
        pub(super) fn for_tests(at: Option<&str>) -> Self {
            Self {
                zip: Vec::new(),
                at: at.map(str::to_owned),
            }
        }
    }

    /// The hand-off link an adoption stamps, before the stamp.
    pub(super) struct AdoptedLink {
        endpoint: String,
        project_id: String,
        project_name: String,
        /// BASE, decided at [`AdoptedLink::new`].
        base: Option<String>,
    }

    impl AdoptedLink {
        /// BASE = the commit this project was actually built from, which is the one
        /// `fetched` was downloaded at - NOT a second lookup.
        ///
        /// A second lookup reintroduces the window the by-hash download closed, and on
        /// the worst possible side of it. The import between them is slow (parse the
        /// `.tml`, build the project, save it), so a revision landing during it is not
        /// exotic; the copy would then be A-derived content stamped BASE = B, and
        /// stamped `has_unpushed_edits` too. Start editing reads BASE == tip and calls
        /// it UpToDate, so nothing ever fast-forwards, and the push then reads base ==
        /// tip and plans a plain Upload - putting A over B with no divergence raised.
        ///
        /// Keeping the original sha instead means BASE is A, so if B did land the copy
        /// is honestly behind: the fence sees a commit that is not ours and raises the
        /// divergence prompt, which is the outcome the user can act on. `advisory` (the
        /// caller's sha) may stamp BASE when no tip resolved, and nothing else
        /// ([`PullDating`]).
        pub(super) fn new(
            fetched: &AdoptFetched,
            endpoint: String,
            project_id: String,
            project_name: String,
            advisory: Option<String>,
        ) -> Self {
            let PullDating { base, .. } = pull_dating(fetched.at.clone(), advisory);
            Self {
                endpoint,
                project_id,
                project_name,
                base,
            }
        }

        /// The link as the stamp writes it, built AT the stamp.
        ///
        /// `has_unpushed_edits = true` because at this instant it is TRUE - the server
        /// has never been given this `.stalx`. The hand-over clears it by pushing; every
        /// path that does not push leaves it standing, which is what arms the quit guard
        /// over a conversion that exists only in this cache.
        ///
        /// `last_edit_at` is stamped WITH it, and must be: a dirty bit with no edit time
        /// is one a confirmed push can never clear. `edits_captured_by_push` reads a
        /// missing timestamp as "unknown timing" and keeps the bit, so a link stamped
        /// dirty-but-undated pushes successfully, stays dirty, reports
        /// `PushUnconfirmed`, and holds the lock for a retry that will do the same thing
        /// again. The conversion IS the edit and it happened just now, so `now` is both
        /// true and the value that works: the push starts after the stamp, so
        /// `last_edit_at <= push_start` and a landed push clears the bit.
        pub(super) fn stamped_at_now(self, undo_mode: UndoMode) -> SpeleoLink {
            SpeleoLink {
                endpoint: self.endpoint,
                project_id: self.project_id,
                project_name: self.project_name,
                base_commit_sha: self.base,
                has_unpushed_edits: true,
                last_edit_at: Some(chrono::Utc::now().to_rfc3339()),
                last_verify_at: None,
                undo_mode: Some(undo_mode),
            }
        }
    }

    /// Proof the adopted copy is stamped with its dirty, dated link - the hand-over's
    /// landed-but-locked inference rests on that bit being TRUE on entry.
    pub(super) struct DirtyStamped {
        _stamped: (),
    }

    #[cfg(test)]
    impl DirtyStamped {
        /// A stamp, for the tests of the guard's disarm.
        pub(super) fn for_tests() -> Self {
            Self { _stamped: () }
        }
    }

    /// [#576 NF-W4C-3-EDIT] Lifts an adoption's hold on the edit gate (see
    /// `AppState::speleodb_shut_gate_for_adoption`) when the adoption returns.
    ///
    /// [#576 RV9a-3] Made only by [`AppState::adopt_local_span`], the step whose span
    /// shuts the gate (the field is private to this module): a guard built inline
    /// elsewhere and dropped would lift the hold mid-hand-over.
    pub(in crate::core::state) struct AdoptionEditGate<'a> {
        state: &'a AppState,
    }

    impl Drop for AdoptionEditGate<'_> {
        fn drop(&mut self) {
            self.state.speleodb_open_gate_after_adoption();
        }
    }

    #[cfg(test)]
    impl<'a> AdoptionEditGate<'a> {
        /// The guard on its own, for the tests of its drop.
        pub(in crate::core::state) fn for_tests(state: &'a AppState) -> Self {
            Self { state }
        }
    }

    impl AppState {
        /// The adoption's local half as the verb runs it
        /// ([`AppState::adopt_import_and_save`]): the span shuts the edit gate, and this
        /// returns the guard that lifts it ([`AdoptionEditGate`], made first so every
        /// exit from here lifts it too), with the link [`AdoptedLink`] builds stamped.
        /// A stamp that lands returns the [`DirtyStamped`] the hand-over needs.
        #[expect(
            clippy::result_large_err,
            reason = "returns the SpeleoErrorDto every sibling SpeleoDB step returns; boxing it is a \
                      repo-wide change outside #576 A8"
        )]
        pub(super) fn adopt_local_span(
            &self,
            guard_local_edits_of: Option<&str>,
            tml_path: &Path,
            entry_name: &str,
            dest: &Path,
            project_name: String,
            link: AdoptedLink,
        ) -> Result<
            (
                AdoptionEditGate<'_>,
                Option<ImportReceipt>,
                Result<DirtyStamped, SpeleoDbError>,
            ),
            SpeleoErrorDto,
        > {
            let edit_gate = AdoptionEditGate { state: self };
            let (receipt, stamped) = self.adopt_import_and_save(
                guard_local_edits_of,
                tml_path,
                entry_name,
                dest,
                project_name,
                |undo_mode| link.stamped_at_now(undo_mode),
            )?;
            Ok((
                edit_gate,
                receipt,
                stamped.map(|()| DirtyStamped { _stamped: () }),
            ))
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
        ///
        /// [#576 A8] Takes the disarmed guard's [`HeldForHandOver`], which only a guard
        /// disarmed by the stamp's proof hands out, and returns the sealed [`HandedOver`]
        /// whose [`HandedOver::landed`] is the type flip's precondition.
        pub(super) fn adopt_hand_over(
            &self,
            _held: HeldForHandOver,
            expected_id: &str,
        ) -> HandedOver {
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
            HandedOver { upload: outcome }
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
                Ok(
                    StartEditingResult::FastForwarded { .. } | StartEditingResult::DivergenceNeeded,
                ) => return AdoptUpload::Diverged,
                Ok(StartEditingResult::LockedBy { email }) => {
                    return AdoptUpload::LockHeld { email }
                }
                Err(e) => return AdoptUpload::Failed { message: e.message },
            }

            match self.speleodb_done(Some(ADOPT_COMMIT_MESSAGE.to_owned())) {
                Ok(DoneResult::Released) => AdoptUpload::Pushed,
                // [#576 SDB-2MACHINE-D1] Not reached in practice: the adoption's Done
                // uploads (the copy is dirty by construction), and an upload is an
                // edit made under the lock, so even an adopted lock is released. Kept
                // as the arm it was before #576 W72: landed, the lock left held.
                Ok(DoneResult::LockHeldElsewhere) => AdoptUpload::PushedLockHeld,
                Ok(DoneResult::DivergenceNeeded) => AdoptUpload::Diverged,
                // Both mean the same thing to a caller with nothing left to retry with:
                // the bytes did not land. The lock stays held, by `speleodb_done`'s own
                // contract, so the user's retry from the status button can recover.
                Ok(DoneResult::NotEditing | DoneResult::PushUnconfirmed) => AdoptUpload::Failed {
                    message: "the upload did not complete; the converted project is still local"
                        .to_owned(),
                },
                // An error from Done does NOT mean the bytes did not land - see
                // `landed_despite_release_error` for how the link tells the two apart.
                Err(e) => {
                    landed_despite_release_error(self.speleodb_open_link(), expected_id, e.message)
                }
            }
        }
    }

    /// An error from Done does NOT mean the bytes did not land. Its last act on
    /// a landed push is to hand the lock back, and it reports a failure there by
    /// returning `Err` - AFTER the upload has been accepted and the dirty bit
    /// cleared. Reporting that as `Failed` told a user whose survey was safely on
    /// the server that it was "on this machine only", which is the most alarming
    /// way to be wrong about where someone's only copy is.
    ///
    /// The link is what tells the two apart, and it is already on disk: a push
    /// that landed clears `has_unpushed_edits`, and nothing else in this flow
    /// clears it. Read it back rather than guessing from the error, which cannot
    /// distinguish a release that failed from an upload that never happened.
    ///
    /// The link has to be the ADOPTED project's for any of that to follow.
    /// `speleodb_open_link` answers for whatever is open in `AppState`, not for a
    /// project id, so a load landing under an in-flight hand-over would have it
    /// read a stranger's link - and an unrelated CLEAN project then satisfies the
    /// predicate, reporting an upload that never happened and seeding the
    /// conversion clean over a copy that exists only in the cache. Nothing in this
    /// flow is supposed to permit that swap, which is exactly why a mismatch is
    /// logged as an error rather than absorbed: it means an invariant this
    /// inference rests on has gone, and the log is the only place that would say
    /// so. `Failed` is the fail-closed direction either way - it keeps the dirty
    /// seed and arms the quit guard.
    pub(super) fn landed_despite_release_error(
        open_link: Result<SpeleoLink, SpeleoDbError>,
        expected_id: &str,
        message: String,
    ) -> AdoptUpload {
        match open_link {
            Ok(link) if link.project_id != expected_id => {
                tracing::error!(
                    expected = expected_id,
                    found = %link.project_id,
                    purpose = "prove the adoption's own push landed",
                    "speleodb_adopt_ariane: the open link names a DIFFERENT project; \
                     the open database changed under the hand-over"
                );
                AdoptUpload::Failed { message }
            }
            Ok(link) if !link.has_unpushed_edits => {
                tracing::warn!(
                    error = %message,
                    purpose = "report an adoption whose upload landed but whose lock did not come back",
                    "speleodb_adopt_ariane: pushed, but the write lock is still held"
                );
                AdoptUpload::PushedLockHeld
            }
            // Still dirty, or unreadable: neither proves a landing.
            _ => AdoptUpload::Failed { message },
        }
    }

    /// What the hand-over did, sealed: only [`AppState::adopt_hand_over`] makes one.
    pub(super) struct HandedOver {
        upload: AdoptUpload,
    }

    /// Proof the hand-over's push landed: the server type may be settled.
    pub(super) struct AdoptLanded {
        _landed: (),
    }

    /// [#576 NF-A8R-4] The type flip's PATCH is only ever earned by a landed push.
    impl client::PatchLicence for AdoptLanded {}

    impl HandedOver {
        /// Landed exactly for the outcomes that mean the bytes are up. Claiming a flip
        /// on a lock we could not take describes a project that does not exist.
        pub(super) fn landed(&self) -> Option<AdoptLanded> {
            matches!(
                self.upload,
                AdoptUpload::Pushed | AdoptUpload::PushedLockHeld
            )
            .then_some(AdoptLanded { _landed: () })
        }

        pub(super) fn into_upload(self) -> AdoptUpload {
            self.upload
        }

        /// A hand-over that answered `upload`, for the tests of the landing gate.
        #[cfg(test)]
        pub(super) fn for_tests(upload: AdoptUpload) -> Self {
            Self { upload }
        }
    }

    #[cfg(test)]
    impl AdoptLanded {
        /// A landing, for the tests of the steps it gates.
        pub(super) fn for_tests() -> Self {
            Self { _landed: () }
        }
    }

    /// [#576 NF-A8R-GAPS] The type settle's verdict, still inside the lock-ops span it
    /// was reached in: it BORROWS that span. The release-verdict correction is its
    /// method and clears a disproved claim through that same span, and the close takes
    /// only what the correction returns ([`AdoptVerdict`]) - so a fresh span opened
    /// between the settle and the clear is a borrow error (E0505), not a reorder a
    /// reviewer has to spot. The RV8a-1 shape (`GatesDropped`, `Relocated`).
    pub(super) struct AdoptSettled<'s, 'g> {
        span: &'s LockOpsHeld<'g>,
        verdict: AdoptFinalize,
    }

    /// The settled verdict after the correction: what the close reports.
    pub(super) struct AdoptVerdict {
        finalized: AdoptFinalize,
    }

    impl AppState {
        /// Settle the server type inside `lock_ops`: flip it after a landed push
        /// (`adopt_finalize`), or nothing when the push never landed.
        pub(super) fn adopt_settle<'s, 'g>(
            &self,
            lock_ops: &'s LockOpsHeld<'g>,
            landed: Option<&AdoptLanded>,
            endpoint: &str,
            token: &str,
            insecure_http: bool,
            project_id: &str,
        ) -> AdoptSettled<'s, 'g> {
            let verdict = match landed {
                Some(landed) => self.adopt_finalize(
                    lock_ops,
                    landed,
                    endpoint,
                    token,
                    insecure_http,
                    project_id,
                ),
                None => AdoptFinalize::not_attempted(),
            };
            AdoptSettled {
                span: lock_ops,
                verdict,
            }
        }
    }

    impl AdoptSettled<'_, '_> {
        /// The hand-over's release verdict, corrected by the holder the settle read,
        /// with the disproved claim cleared in the settle's span. `my_email` is the
        /// account that took the lock (captured inside the acquire span).
        pub(super) fn correct_release_verdict(
            self,
            state: &AppState,
            upload: AdoptUpload,
            my_email: Option<&str>,
            project_id: &str,
            endpoint: &str,
        ) -> (AdoptVerdict, AdoptUpload) {
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
            let upload = match (upload, &self.verdict.holder) {
                (AdoptUpload::PushedLockHeld, Some(holder))
                    if my_email.is_some() && !lock_held_by_me(holder.as_deref(), my_email) =>
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
                    if let Some(email) = my_email {
                        let _ = state.speleodb_forget_lock_acquired_as(
                            self.span, project_id, endpoint, email,
                        );
                    }
                    AdoptUpload::Pushed
                }
                (upload, _) => upload,
            };

            (
                AdoptVerdict {
                    finalized: self.verdict,
                },
                upload,
            )
        }
    }

    /// The parts of the verb's answer the close passes through untouched.
    pub(super) struct AdoptReport<'a> {
        pub(super) dest: &'a Path,
        pub(super) concurrent_session: bool,
        pub(super) upload: AdoptUpload,
        pub(super) import_receipt: Option<ImportReceipt>,
    }

    /// The verb's answer, made only by the close - so nothing can follow it.
    pub(super) struct AdoptClosed {
        result: SpeleoAdoptResult,
    }

    impl AdoptClosed {
        pub(super) fn into_result(self) -> SpeleoAdoptResult {
            self.result
        }
    }

    impl AppState {
        /// Hand the project BACK to the frontend to open, instead of leaving it
        /// committed to `AppState` for a `loadProject` that then short-circuits.
        ///
        /// `do_create_project_full` inside the import commits the adopted project
        /// unconditionally, so on return the backend held it open at `dest` - and
        /// `do_load_project_full` answers a load of the path it already has open with
        /// `ALREADY_OPEN`. The frontend reads that as "it is on screen already" (the
        /// only way a pull can produce it) and short-circuits to a view flip, so
        /// adoption finished with the view switched, no project in any store, and a
        /// success toast over an empty screen. Closing here makes the frontend's load
        /// an ordinary full load, which is how the pull path has always behaved: it
        /// runs `close_project_if_open_at_held` before it replaces a cache file, and the
        /// frontend's `loadProject` is what opens the result.
        ///
        /// LAST, because everything above needs the project open: the link stamp writes
        /// through the live handle, and the hand-over renders the sidecar from the open
        /// survey. A held lock is deliberately NOT released here - `speleodb_done`'s
        /// non-landing arms keep it so a retry can recover, and the frontend re-opens
        /// this very project a moment later.
        ///
        /// [#576 A8] Takes the verdict, so it cannot run before the hand-over, the
        /// type settle and the release-verdict correction ([`AdoptVerdict`], made only
        /// by [`AdoptSettled::correct_release_verdict`]), and the edit gate by value,
        /// which it lifts after the close - the same order the end of the verb used to
        /// drop them in.
        pub(super) fn adopt_close(
            &self,
            edit_gate: AdoptionEditGate<'_>,
            verdict: AdoptVerdict,
            report: AdoptReport<'_>,
        ) -> AdoptClosed {
            self.close_project();
            let result = SpeleoAdoptResult {
                local_path: report.dest.to_string_lossy().into_owned(),
                type_flipped: verdict.finalized.type_flipped,
                concurrent_session: report.concurrent_session,
                upload: report.upload,
                import_receipt: report.import_receipt,
            };
            drop(edit_gate);
            AdoptClosed { result }
        }
    }
}

#[cfg(test)]
#[expect(clippy::expect_used, reason = "unit test fixtures")]
#[expect(
    clippy::print_stdout,
    reason = "the driven A8 tests print the values they observed, so a failure shows them"
)]
mod tests {
    use super::adopt_order::{
        landed_despite_release_error, AdoptFetched, AdoptLanded, AdoptLockGuard, AdoptedLink,
        DirtyStamped, HandedOver,
    };
    use super::{adopt_convert, DumpAt};
    use crate::core::state::speleodb::{authed_session_at, logged_events, routing_stub};
    use crate::core::state::speleodb_pull::{make_zip, GUARDED, NEXT, TIP};
    use crate::core::state::AppState;
    use crate::parsers::tml::test_helpers::{parse_synthetic_tmlu, wrap_xml, TripShot};
    use crate::parsers::tml::{TmlFile, TmlSection, TmlShot, TmlShotType, TmlUnit};
    use crate::speleodb::dump::Dump;
    use crate::speleodb::models::AdoptUpload;
    use crate::speleodb::SpeleoDbError;

    /// Nothing listens here, so the release POST fails immediately. Best-effort is the
    /// contract, and the claim is dropped either way - the same endpoint the unwind
    /// tests use for the same reason.
    const UNREACHABLE: &str = "http://127.0.0.1:1/";

    /// Sign in against [`UNREACHABLE`], because an acquisition claim is keyed to the
    /// account that made it and the accessors answer only for the account signed in
    /// NOW. Without this the claim below is invisible to the assertion in BOTH
    /// directions, and the tests pass vacuously.
    fn sign_in_unreachable(state: &AppState) {
        let mut session = state.speleodb.lock();
        session.endpoint = Some(UNREACHABLE.to_owned());
        session.email = Some("diver@example.com".to_owned());
        session.token_store = crate::speleodb::auth::TokenStore::from_in_memory_token("tok");
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

        drop(AdoptLockGuard::for_tests(
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

        let guard = AdoptLockGuard::for_tests(&state, UNREACHABLE, "token", true, GUARDED, true);
        drop(guard.disarm(&DirtyStamped::for_tests()));

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

        drop(AdoptLockGuard::for_tests(
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

    /// The link an adoption stamps, for the conversion tests below. They fail before
    /// the stamp, so what it says does not matter.
    fn adopted_link(undo_mode: crate::models::UndoMode) -> crate::models::SpeleoLink {
        crate::models::SpeleoLink {
            endpoint: "http://speleodb.invalid/".to_owned(),
            project_id: GUARDED.to_owned(),
            project_name: "Adopted".to_owned(),
            base_commit_sha: None,
            has_unpushed_edits: true,
            last_edit_at: None,
            last_verify_at: None,
            undo_mode: Some(undo_mode),
        }
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
        let failed = state.adopt_import_and_save(
            None,
            &not_a_tml,
            "ariane.tml",
            &dest,
            "Adopted".to_owned(),
            adopted_link,
        );

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
        let failed = state.adopt_import_and_save(
            None,
            &not_a_tml,
            "ariane.tml",
            &dest,
            "Adopted".to_owned(),
            adopted_link,
        );

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
            scaled_depth_in_marker: None,
            warnings: None,
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

    /// [#411] What the conversion reported reaches the adoption's receipt, decorated
    /// the way the wizard-driven Ariane import decorates its own (`decorate_receipt`).
    ///
    /// [#576 A8] Driven through the adoption's local half (`adopt_import_and_save`)
    /// with a null event sink. It used to read `adopt_build_into`'s source on the
    /// grounds that `do_create_project_full` needs an app handle; it needs a sink.
    #[test]
    fn the_adoption_receipt_carries_what_the_conversion_reported() {
        let _cfg = crate::test_helpers::isolated_config();
        let temp = tempfile::TempDir::new().expect("temp dir");
        let shots: Vec<String> = (1..=3)
            .map(|id| {
                TripShot {
                    id,
                    from_id: if id == 1 { -1 } else { id - 1 },
                    section: "SectionA",
                    // One measurement the adopted project will not have.
                    excluded: id == 3,
                    ..TripShot::default()
                }
                .xml()
            })
            .collect();
        let remote = temp.path().join("adopt-source.tmlu");
        std::fs::write(&remote, wrap_xml(&shots.join("\n"))).expect("write remote file");
        let dest = temp.path().join("project.stalx");

        let state = crate::test_fixtures::state_with_null_sink();
        let (receipt, stamped) = state
            .adopt_import_and_save(
                None,
                &remote,
                "ariane.tml",
                &dest,
                "CaveA".to_owned(),
                |undo_mode| crate::models::SpeleoLink {
                    endpoint: "http://speleodb.invalid/".to_owned(),
                    project_id: "55555555-5555-5555-5555-555555555555".to_owned(),
                    project_name: "CaveA".to_owned(),
                    base_commit_sha: None,
                    has_unpushed_edits: true,
                    last_edit_at: Some("2026-07-27T10:00:00Z".to_owned()),
                    last_verify_at: None,
                    undo_mode: Some(undo_mode),
                },
            )
            .expect("the local half adopts");
        assert!(stamped.is_ok(), "and stamps the link");

        let receipt = receipt.expect("an adoption returns a receipt for the webview");
        assert_eq!(
            receipt.source_format, "Ariane TML",
            "decorated as the Ariane import decorates its own"
        );
        assert_eq!(
            receipt.file_name, "ariane.tml",
            "named after the entry inside the dump - the name the user recognises"
        );
        assert_eq!(receipt.sections_found, 1);
        assert!(
            receipt
                .warnings
                .iter()
                .any(|w| w == "1 excluded shot in the remote file was not imported"),
            "the conversion's account of what it dropped must reach the user, got {:?}",
            receipt.warnings
        );
    }

    /// The adoption's lock decision runs INSIDE the lock-ops span (`adopt_take_lock`
    /// takes the span's proof, the acquire under it takes it too), so neither half
    /// may take the guard itself. `std::sync::Mutex` is not reentrant, so a second
    /// acquisition inside the span is not a redundancy - it is a self-deadlock on the
    /// first adoption anyone runs.
    ///
    /// A sanctioned source scan (#576 A8-D2, docs/testing-recipes.md "Source scans"):
    /// "this function takes no guard of its own" is a site rule the type cannot state -
    /// the helper is HANDED the span's proof, which proves the span is held, not that
    /// nothing inside reaches for it again. (What else this used to read - that the
    /// claim is minted under the span and that the conversion is not run inside it - is
    /// the types' now: the conversion needs the `AdoptFetched` only the guard armed
    /// from the decision can fetch.)
    #[test]
    fn the_lock_decision_does_not_retake_the_span_it_runs_in() {
        use crate::models::source_scan::fn_body;
        for signature in [
            "pub(super) fn adopt_take_lock(",
            "fn adopt_acquire_in_span(",
        ] {
            assert!(
                !fn_body(prod_source(), signature).contains("speleodb_lock_ops_guard()"),
                "RULE (lock-ops is not reentrant): `{signature}` runs inside the span; \
                 retaking it self-deadlocks"
            );
        }
    }

    /// The production region of this file, split out of the test module so the
    /// source-level tripwire below cannot match its own assertions.
    fn prod_source() -> &'static str {
        crate::models::source_scan::prod_region(
            include_str!("speleodb_adopt.rs"),
            "pub fn speleodb_adopt_ariane(",
        )
    }

    // ---- [#576 A8] the adoption chain, driven -----------------------------------

    /// A link for the hand-over inference below.
    fn hand_over_link(project_id: &str, dirty: bool) -> crate::models::SpeleoLink {
        crate::models::SpeleoLink {
            endpoint: UNREACHABLE.to_owned(),
            project_id: project_id.to_owned(),
            project_name: "Adopted".to_owned(),
            base_commit_sha: None,
            has_unpushed_edits: dirty,
            last_edit_at: None,
            last_verify_at: None,
            undo_mode: None,
        }
    }

    /// `PushedLockHeld` is inferred from the link when `speleodb_done` returned `Err`
    /// over a push, and only the ADOPTED project's link, read back CLEAN, proves the
    /// bytes landed. A stamp that stopped setting the dirty bit would make every failed
    /// release read as a landed push, telling a user their survey was on the server
    /// when it was not; a stranger's clean link (a load landing under the hand-over)
    /// would do the same.
    ///
    /// [#576 A8] Was a source scan of `adopt_hand_over_inner`'s arms. The arms are
    /// driven here; that the bit is TRUE on entry is the types' job (the hand-over
    /// takes a lock only a guard disarmed by the dirty stamp's proof hands out).
    #[test]
    fn a_release_error_reads_the_adopted_link_to_tell_a_landing_from_a_failure() {
        let verdict = |open: Result<crate::models::SpeleoLink, SpeleoDbError>| {
            landed_despite_release_error(open, GUARDED, "release refused".to_owned())
        };
        let clean = verdict(Ok(hand_over_link(GUARDED, false)));
        let dirty = verdict(Ok(hand_over_link(GUARDED, true)));
        let unreadable = verdict(Err(SpeleoDbError::NoRemoteProject));
        let mut foreign = None;
        let logs = logged_events(|| {
            foreign = Some(verdict(Ok(hand_over_link(
                "88888888-8888-8888-8888-888888888888",
                false,
            ))));
        });
        println!(
            "own clean link -> {clean:?}; own dirty -> {dirty:?}; unreadable -> \
             {unreadable:?}; a stranger's clean link -> {foreign:?}; logged {logs:?}"
        );
        assert!(
            matches!(clean, AdoptUpload::PushedLockHeld),
            "a clean link for the adopted project proves the push landed"
        );
        assert!(
            matches!(dirty, AdoptUpload::Failed { ref message } if message == "release refused"),
            "a still-dirty link proves nothing landed"
        );
        assert!(
            matches!(unreadable, AdoptUpload::Failed { .. }),
            "an unreadable link falls to Failed rather than claim a landing"
        );
        assert!(
            matches!(foreign, Some(AdoptUpload::Failed { .. })),
            "a stranger's CLEAN link proves nothing about our push"
        );
        assert!(
            logs.iter()
                .any(|line| line.contains("ERROR") && line.contains("DIFFERENT project")),
            "and the swap is logged as an error - nothing else would report it"
        );
    }

    /// A project typed `stalagmight` whose only file is the Ariane one that type says to
    /// disregard is openable by nobody, so the type is settled only after a push that
    /// LANDED. [#576 A8] Was a source scan of the verb's gate; the gate is the only
    /// maker of `AdoptLanded`, which the settle takes.
    #[test]
    fn only_a_hand_over_whose_push_landed_may_settle_the_server_type() {
        for (upload, lands) in [
            (AdoptUpload::Pushed, true),
            (AdoptUpload::PushedLockHeld, true),
            (AdoptUpload::Diverged, false),
            (AdoptUpload::LockHeld { email: None }, false),
            (
                AdoptUpload::Failed {
                    message: "offline".to_owned(),
                },
                false,
            ),
        ] {
            let landed = HandedOver::for_tests(upload.clone()).landed().is_some();
            println!("{upload:?} -> may settle the type: {landed}");
            assert_eq!(landed, lands, "{upload:?}");
        }
    }

    /// BASE is the commit the converted bytes were fetched at, never a later lookup or
    /// the caller's advisory sha while a tip resolved - and the link is DIRTY (the
    /// server has never seen this `.stalx`) and DATED at the stamp: a dirty bit with no
    /// edit time is one a confirmed push can never clear (`edits_captured_by_push`
    /// keeps an undated one), which made every adoption push, stay dirty, report
    /// `PushUnconfirmed` and hold the lock.
    ///
    /// [#576 A8] Was two source scans of the verb's link literal and its dating call.
    #[test]
    fn the_adopted_link_is_based_on_the_fetched_commit_and_stamped_dirty_and_dated() {
        let link = AdoptedLink::new(
            &AdoptFetched::for_tests(Some(TIP)),
            UNREACHABLE.to_owned(),
            GUARDED.to_owned(),
            "Adopted".to_owned(),
            Some(NEXT.to_owned()),
        )
        .stamped_at_now(crate::models::UndoMode::Full);
        let push_start = chrono::Utc::now().to_rfc3339();
        let undatable = AdoptedLink::new(
            &AdoptFetched::for_tests(None),
            UNREACHABLE.to_owned(),
            GUARDED.to_owned(),
            "Adopted".to_owned(),
            Some(NEXT.to_owned()),
        )
        .stamped_at_now(crate::models::UndoMode::Full);
        println!(
            "fetched at TIP, advisory NEXT -> base {:?}, dirty {}, edited {:?}; fetched at \
             HEAD -> base {:?}",
            link.base_commit_sha,
            link.has_unpushed_edits,
            link.last_edit_at,
            undatable.base_commit_sha
        );
        assert_eq!(
            link.base_commit_sha.as_deref(),
            Some(TIP),
            "BASE names the commit the bytes came from"
        );
        assert!(link.has_unpushed_edits, "the conversion is un-pushed");
        assert!(
            crate::speleodb::sync::edits_captured_by_push(
                link.last_edit_at.as_deref(),
                Some(&push_start)
            ),
            "and dated, so a push that starts after the stamp clears it"
        );
        assert_eq!(
            undatable.base_commit_sha.as_deref(),
            Some(NEXT),
            "with no tip, BASE falls back to the advisory sha"
        );
    }

    /// The project as the server describes it to the settle: typed `stalagmight`, unlocked.
    const ADOPTED_AS_STALAGMIGHT: &str =
        r#"{"id":"77777777-7777-7777-7777-777777777777","name":"Adopted","type":"stalagmight"}"#;

    /// The settle's GET fails on every attempt; its PATCH lands.
    fn the_type_cannot_be_read(line: &str) -> (u16, &'static str) {
        if line.starts_with("GET ") {
            (503, "")
        } else {
            (200, ADOPTED_AS_STALAGMIGHT)
        }
    }

    /// Adoption is the one caller that knows the project was Ariane - it just converted
    /// it - so a type it cannot read is still a type it may set. Delegating that to
    /// the push's heal (which declines an unknown type) turned one transient GET
    /// failure into a project left typed `ariane` with a `.stalx` on it, still offering
    /// Adopt. And the PATCH's echo is the only holder reading left, so it fills the
    /// verdict's unknown.
    ///
    /// [#576 A8] Was a source scan of `adopt_finalize`'s arms.
    #[test]
    fn a_project_type_the_settle_cannot_read_is_flipped_anyway() {
        let (base, _listener, seen) = routing_stub(the_type_cannot_be_read);
        let state = AppState::new().expect("state");
        let lock_ops = state.speleodb_lock_ops_guard();
        let mut finalized = None;
        let logs = logged_events(|| {
            finalized = Some(state.adopt_finalize(
                &lock_ops,
                &AdoptLanded::for_tests(),
                &base,
                "tok",
                true,
                GUARDED,
            ));
        });
        let finalized = finalized.expect("the settle ran");
        let seen = seen.lock().expect("request log").clone();
        println!(
            "unreadable type -> flipped {}, holder {:?}; requests {seen:?}",
            finalized.type_flipped, finalized.holder
        );
        assert!(
            seen.iter().any(|line| line.starts_with("PATCH ")),
            "a type the settle could not read is flipped, not skipped"
        );
        assert!(finalized.type_flipped, "and the flip is reported");
        assert_eq!(
            finalized.holder,
            Some(None),
            "the PATCH echo answers who holds the mutex when the GET could not"
        );
        assert!(
            logs.iter().any(|line| line.contains("flipping anyway")),
            "and it says so: {logs:?}"
        );
    }

    /// The common case: the push's heal already flipped the type, so the settle READS
    /// first and issues no write at all. Two PATCHes was not merely wasteful -
    /// `type_flipped` was decided by the second, so a transient failure there reported
    /// "the type could not be changed" about a project whose type had just changed.
    #[test]
    fn a_type_the_push_already_flipped_costs_one_get_and_no_patch() {
        fn already_stalagmight(_line: &str) -> (u16, &'static str) {
            (200, ADOPTED_AS_STALAGMIGHT)
        }
        let (base, _listener, seen) = routing_stub(already_stalagmight);
        let state = AppState::new().expect("state");
        let lock_ops = state.speleodb_lock_ops_guard();
        let finalized = state.adopt_finalize(
            &lock_ops,
            &AdoptLanded::for_tests(),
            &base,
            "tok",
            true,
            GUARDED,
        );
        let seen = seen.lock().expect("request log").clone();
        println!(
            "already stalagmight -> flipped {}, holder {:?}; requests {seen:?}",
            finalized.type_flipped, finalized.holder
        );
        assert!(finalized.type_flipped);
        assert_eq!(finalized.holder, Some(None));
        assert!(
            !seen.iter().any(|line| line.starts_with("PATCH ")),
            "nothing to write"
        );
        assert!(
            seen.first().is_some_and(|line| line.starts_with("GET ")),
            "READ first"
        );
    }

    /// [#576 NF-A8R-GAPS] The release-verdict correction, driven through the settle's
    /// span (`AdoptSettled::correct_release_verdict`). A `PushedLockHeld` over a mutex
    /// the settle's GET shows is somebody else's is demoted to `Pushed`, and the claim
    /// this account took on the project is cleared; a holder that is us, or an
    /// identity of our own we could not establish, keeps the arm and the claim.
    #[test]
    fn the_release_verdict_correction_demotes_only_on_a_positive_answer() {
        fn held_by_another(_line: &str) -> (u16, &'static str) {
            (
                200,
                r#"{"id":"77777777-7777-7777-7777-777777777777","name":"Adopted","type":"stalagmight",
                    "active_mutex":{"user":"other@x"}}"#,
            )
        }
        fn held_by_us(_line: &str) -> (u16, &'static str) {
            (
                200,
                r#"{"id":"77777777-7777-7777-7777-777777777777","name":"Adopted","type":"stalagmight",
                    "active_mutex":{"user":"u@x"}}"#,
            )
        }
        let cases: [(&str, fn(&str) -> (u16, &'static str), Option<&str>, bool); 3] = [
            (
                "held by another, we are u@x",
                held_by_another,
                Some("u@x"),
                true,
            ),
            ("held by us", held_by_us, Some("u@x"), false),
            (
                "held by another, our identity unknown",
                held_by_another,
                None,
                false,
            ),
        ];
        for (case, route, my_email, demotes) in cases {
            let (base, _listener, _seen) = routing_stub(route);
            let state = AppState::new().expect("state");
            *state.speleodb.lock() = authed_session_at(&base);
            state.speleodb_note_lock_acquired(Some(GUARDED));
            let lock_ops = state.speleodb_lock_ops_guard();
            let (_verdict, upload) = state
                .adopt_settle(
                    &lock_ops,
                    Some(&AdoptLanded::for_tests()),
                    &base,
                    "tok",
                    true,
                    GUARDED,
                )
                .correct_release_verdict(
                    &state,
                    AdoptUpload::PushedLockHeld,
                    my_email,
                    GUARDED,
                    &base,
                );
            drop(lock_ops);
            let claim_kept = state
                .speleodb
                .lock()
                .locks_acquired_here
                .iter()
                .any(|claim| claim.project_id == GUARDED);
            println!("{case}: upload {upload:?}, claim kept {claim_kept}");
            assert_eq!(
                matches!(upload, AdoptUpload::Pushed),
                demotes,
                "{case}: demoted only on a positive answer"
            );
            assert_eq!(
                claim_kept, !demotes,
                "{case}: the claim goes with the demotion"
            );
        }
    }

    /// [#258] Adoption classifies with its OWN matcher, so the resolver's dating never
    /// reaches it and both halves are repeated there: a fresh picker reply is honoured
    /// only while it answers the revision its candidates were offered for, and every
    /// ambiguity it reports records the revision it describes.
    ///
    /// [#576 A8] Was a source scan counting the verb's two dating calls; the picker is
    /// its own step now (`adopt_pick_entry`) and is driven here.
    #[test]
    fn adoption_honours_a_picker_reply_only_for_the_revision_it_was_offered_for() {
        let state = AppState::new().expect("state");
        let ariane = make_zip(&["one/ariane.tml", "two/ariane.tml"]);
        let open = || Dump::open(&ariane).expect("open");

        let offered = state.adopt_pick_entry(GUARDED, &mut open(), None, DumpAt::Revision(TIP));
        let offer_recorded = state.speleodb_dump_offer_answers(GUARDED, DumpAt::Revision(TIP));
        let answered = state.adopt_pick_entry(
            GUARDED,
            &mut open(),
            Some("two/ariane.tml"),
            DumpAt::Revision(TIP),
        );
        let stale = state.adopt_pick_entry(
            GUARDED,
            &mut open(),
            Some("two/ariane.tml"),
            DumpAt::Revision(NEXT),
        );
        let reoffer_recorded = state.speleodb_dump_offer_answers(GUARDED, DumpAt::Revision(NEXT));

        let stalx_state = AppState::new().expect("state");
        let stalx = make_zip(&["a/project.stalx", "b/project.stalx"]);
        let stalx_offered = stalx_state.adopt_pick_entry(
            GUARDED,
            &mut Dump::open(&stalx).expect("open"),
            None,
            DumpAt::Revision(TIP),
        );
        let stalx_recorded =
            stalx_state.speleodb_dump_offer_answers(GUARDED, DumpAt::Revision(TIP));
        println!(
            "ariane set at TIP -> {:?} (offer recorded: {offer_recorded}); reply at TIP -> \
             {answered:?}; same reply at NEXT -> {:?} (re-offer recorded: \
             {reoffer_recorded}); .stalx set -> {:?} (offer recorded: {stalx_recorded})",
            offered.as_ref().err().map(|e| &e.message),
            stale.as_ref().err().map(|e| &e.message),
            stalx_offered.as_ref().err().map(|e| &e.message),
        );
        assert!(
            offered.is_err() && offer_recorded,
            "the Ariane set is offered for TIP"
        );
        assert_eq!(
            answered.ok().as_deref(),
            Some("two/ariane.tml"),
            "a reply to the TIP offer is honoured at TIP"
        );
        assert!(
            stale.is_err() && reoffer_recorded,
            "the same reply does not answer NEXT: it is re-offered, for NEXT"
        );
        assert!(
            stalx_offered.is_err() && stalx_recorded,
            "the .stalx ambiguity records its revision too (the frontend routes it to OPEN)"
        );
    }

    /// The adoption's flow against a server that 401s the history read and nothing
    /// else: the probe sees no holder, the acquire is refused (someone else took it),
    /// the head resolve gets the 401 and the dump is not found.
    fn a_401_on_the_head_resolve(line: &str) -> (u16, &'static str) {
        let path = line.split_whitespace().nth(1).unwrap_or_default();
        if path.ends_with("/revisions/") {
            (401, r#"{"detail":"Invalid token."}"#)
        } else if path.ends_with("/acquire/") {
            (400, r#"{"detail":"locked"}"#)
        } else if path.contains("/download/dump/") {
            (404, r#"{"detail":"Not found."}"#)
        } else {
            (
                200,
                r#"{"id":"77777777-7777-7777-7777-777777777777","name":"Adopted","type":"ariane"}"#,
            )
        }
    }

    /// [#258 review] Adoption's best-effort head resolve must not be able to delete the
    /// user's credential. `speleodb_server_head` wraps the call in `speleodb_handle_auth`,
    /// which `forget()`s the token on a 401 - keychain entry included - and this failure
    /// is deliberately swallowed, so a spurious 401 signed the user out with no signal
    /// while the adopt went on.
    ///
    /// [#576 A8] Was a source scan that the verb named no `speleodb_server_head`; the
    /// verb now runs against a stub that 401s exactly that request.
    #[test]
    fn a_401_on_the_adoptions_head_resolve_keeps_the_credential() {
        let _cfg = crate::test_helpers::isolated_config();
        let (base, _listener, seen) = routing_stub(a_401_on_the_head_resolve);
        let state = AppState::new().expect("state");
        *state.speleodb.lock() = authed_session_at(&base);

        let adopted = state.speleodb_adopt_ariane(
            GUARDED.to_owned(),
            "Adopted".to_owned(),
            None,
            false,
            None,
        );
        let seen = seen.lock().expect("request log").clone();
        let still_signed_in = state.speleodb_active_token().is_ok();
        println!(
            "adopt -> {:?}; still signed in: {still_signed_in}; requests {seen:?}",
            adopted.as_ref().err().map(|e| &e.message)
        );
        assert!(
            seen.iter().any(|line| line.contains("/revisions/")),
            "precondition: the head resolve ran and got its 401"
        );
        assert!(
            adopted.is_err(),
            "precondition: the missing dump ends the adoption"
        );
        assert!(
            still_signed_in,
            "a swallowed 401 on a best-effort read must not sign the user out"
        );
    }
}
