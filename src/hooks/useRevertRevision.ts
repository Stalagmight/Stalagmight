import { useCallback } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../store/uiStore';
import { useProjectStore } from '../store/projectStore';
import { asDoneKind, doneFinished, useSpeleoStore } from '../store/speleoStore';
import { invokeIpc } from '../utils/ipc';
import { useSpeleoActions } from './useSpeleoActions';
import { useRestoreRevision } from './useRestoreRevision';
import { humanizeSpeleoError, unpushedLocalEdits, dumpCandidates } from '../utils/speleoError';
import { projectChangedMessage } from '../utils/projectChangedNotice';
import type { SpeleoDumpCandidate } from '../types/ipc.generated';
import { TOAST_KEYS } from '../utils/toastKeys';

export interface RevertRevisionArgs {
  projectId: string;
  projectName: string;
  /** Full commit SHA to revert to. */
  hexsha: string;
  /** Short SHA for the copy. */
  shortSha: string;
  /** Commit message for the push that publishes the revert; `null` lets the server pick. */
  message: string | null;
  /**
   * Waive the backend's un-pushed-edits guard. Only ever `true` for the dialog's
   * armed second press, after the user has been told which local work it replaces.
   */
  discardLocal?: boolean;
  /**
   * [#258] The dump entry the user picked, when this whole composite is a retry from
   * the ambiguity picker. Only the restore step reads it; `resumeAtPush` returns
   * before that step, so it is inert there by construction.
   */
  entryPath?: string;
  /**
   * Resume a revert whose restore already landed: re-issue ONLY the upload +
   * release, against the working copy this flow already swapped in and the write
   * lock it is still holding.
   *
   * Valid in exactly the two outcomes that leave things standing that way -
   * `push_failed` and `push_unconfirmed` - which both keep the lock AND the dirty
   * bit precisely so a retry can recover (`DoneResult::PushUnconfirmed` says so in
   * as many words). Re-running the whole composite from either is wrong twice
   * over. It re-downloads the revision and swaps the tree in again under the
   * discard policy, resetting undo/redo and dropping anything edited since. And
   * where the upload actually LANDED but the local record of it did not (a 200
   * whose body did not parse), it re-enters Start editing's reconcile, which has
   * no authorship recovery: our own just-landed commit reads as a divergence, so
   * the lock is released and the user is asked to resolve a conflict against
   * themselves, with a discard as the loudest way out. The push path classifies
   * that same commit as ours and finishes cleanly, so the retry has to stay on it.
   *
   * Skips BOTH pre-flight guards by construction. The restore is what SET the
   * un-pushed bit, so that guard would refuse the retry over the revert's own
   * tree; and the restore left the working copy saved (`useRestoreRevision`
   * clears `isDirty` behind it), with this modal dialog the only thing on screen
   * since, so there is no unsaved edit for the other guard to find.
   */
  resumeAtPush?: boolean;
  /** Progress callback, so a three-round-trip operation is legible while it runs. */
  onStep?: (step: 'locking' | 'restoring' | 'pushing') => void;
}

/**
 * What the publish tail needs - a subset of {@link RevertRevisionArgs}. `onStep` is
 * spelled out rather than `Pick`ed because `exactOptionalPropertyTypes` stops a
 * picked optional accepting an EXPLICIT `undefined`, which is exactly what a caller
 * forwarding its own optional callback passes.
 */
type PublishArgs = Pick<RevertRevisionArgs, 'message' | 'shortSha'> & {
  onStep?: RevertRevisionArgs['onStep'];
};

/**
 * What the revert ended up doing, so the dialog can say it exactly. `lockHeld`
 * answers the question the user actually has after a failure - "am I now holding a
 * lock I did not have?" - which the generic error text cannot.
 */
export type RevertOutcome =
  | { kind: 'reverted' }
  /** `stateUnknown` [#576 W55 NF-W53-1]: the restore was refused because the copy could
      not be read (`UnpushedLocalEdits.state_unknown`), not because it was read dirty -
      the dialog words the discard step for that. False for the pre-flight, which
      refuses on a dirty bit it did read. */
  | { kind: 'needs_discard'; lockHeld: boolean; stateUnknown: boolean }
  /** The open project holds edits that are not on DISK. Nothing was taken and
      nothing was changed - see the pre-flight for why this is its own answer and
      not a `needs_discard`. */
  | { kind: 'unsaved_local' }
  | { kind: 'diverged' }
  | { kind: 'locked_by'; email: string | null }
  | { kind: 'lock_failed' }
  | { kind: 'restore_failed'; message: string; lockHeld: boolean }
  /** [#258] The revision's tree holds more than one openable file, so the composite
      is SUSPENDED on a question rather than failed. The write lock has already been
      handed back (that is why there is no `lockHeld` here), nothing was written, and
      answering re-runs the whole composite from the top - which is what re-acquires
      the lock. Never produced while the copy is dirty: there the lock is kept on
      purpose and a composite retry would be refused by the un-pushed pre-flight. */
  | { kind: 'awaiting_dump_choice'; candidates: SpeleoDumpCandidate[] }
  /** [#258] The open project changed while the composite was waiting on the dump
      probe, so everything after that point would be about a different project than
      the one this revert was asked for. Nothing was taken and nothing was written -
      the probe is deliberately the one step that needs no lock, which is why this
      can only be produced from before the acquire. */
  | { kind: 'project_changed' }
  | { kind: 'push_failed' }
  /** [#576 RV4-FE-SP4] The read-only Plan or Export workspace is active, where a
      restore is refused (RV2-R-15). Nothing was taken and nothing was written: the
      refusal comes before the acquire, so no lock round trip is spent on an action
      that cannot succeed. */
  | { kind: 'workspace_read_only' }
  /** The edit gate dropped mid-flight (usually a 401 auto-drop): nothing uploaded,
      nothing released, and - unlike `push_failed` - no lock held to retry with. */
  | { kind: 'not_editing' }
  | { kind: 'push_unconfirmed' };

/**
 * Revert a SpeleoDB revision from a READ-ONLY project: take the write lock, check
 * the revision out, upload it as a new revision, and release the lock again -
 * ending read-only exactly as it started.
 *
 * There is no backend verb for this and there should not be: `speleodb_restore_revision`
 * with `restore: true` refuses unless the edit gate is open, which is what stops a
 * revert quietly skipping the acquire. So this composes the three existing verbs and
 * owns the unwinding when a middle step fails - the part a caller would otherwise get
 * wrong, because "release what we took" is only correct while the copy is clean.
 *
 * It also owns RESUMING (`resumeAtPush`), for the same reason: once the restore has
 * landed, the only correct retry is the upload on its own, and the caller that most
 * wants to retry is the one holding the least context about why.
 */
export function useRevertRevision() {
  const { t } = useTranslation();
  const showToast = useUIStore((s) => s.showToast);
  const { startEditing, done } = useSpeleoActions();
  const restoreRevision = useRestoreRevision();

  // The tail of the revert - upload the restored tree, release the lock, and say
  // what came back. Split out so the `resumeAtPush` retry enters HERE instead of at
  // the top, and so the done-result -> RevertOutcome mapping stays in ONE place: a
  // second copy of it (in the dialog, say) would be free to drift from this one.
  const publish = useCallback(
    async ({ message, shortSha, onStep }: PublishArgs): Promise<RevertOutcome> => {
      onStep?.('pushing');
      const result = await done(message);
      // A thrown call returns null; `useSpeleoActions.done` has already toasted it and
      // raised the #260 push-failure banner, whose Retry re-issues the upload. Never
      // auto-release from here: that would leave the reverted tree sitting as un-pushed
      // edits with no lock over them.
      if (result === null) return { kind: 'push_failed' };
      const kind = asDoneKind(result.kind);
      if (doneFinished(kind)) {
        showToast(t('speleodb.revert.done', { sha: shortSha }), TOAST_KEYS.SPELEODB_REVERT, 'success');
        return { kind: 'reverted' };
      }
      if (kind === 'push_unconfirmed') return { kind: 'push_unconfirmed' };
      if (kind === 'divergence_needed') return { kind: 'diverged' };
      // `not_editing`: the edit gate dropped under us mid-flight (a 401 auto-drop is
      // the usual way in), so nothing was uploaded and nothing released. It reaches
      // the same dead end as a failed push - the revert exists only in the working
      // copy - but it is NOT the same situation and must not borrow that message:
      // `push_failed` tells the user they are holding the write lock and can Retry
      // from the banner, and here they are holding nothing. Sending them to a Retry
      // that cannot work is worse than saying plainly that they were signed out.
      return { kind: 'not_editing' };
    },
    [t, showToast, done],
  );

  return useCallback(
    async ({
      projectId,
      projectName,
      hexsha,
      shortSha,
      message,
      discardLocal = false,
      entryPath,
      resumeAtPush = false,
      onStep,
    }: RevertRevisionArgs): Promise<RevertOutcome> => {
      // Which project this composite is FOR, against what is open right now. The
      // dialog captured `projectId` from its open data, so it is a snapshot; nothing
      // downstream re-checks it, and the two steps that would act on a mismatch act on
      // different projects - `startEditing()` takes no argument and locks whatever is
      // open, while the restore targets the captured id.
      //
      // Checked at ENTRY as well as before the acquire below, because this callback is
      // re-entered from the picker with `entryPath` set, which skips the probe branch
      // and every check inside it. The picker is the longest wait in the whole flow.
      if (useSpeleoStore.getState().remoteProjectId !== projectId) {
        return { kind: 'project_changed' };
      }

      // The restore already landed and the lock is still ours, so the upload is the
      // only step outstanding - see `resumeAtPush`. Deliberately AHEAD of both
      // pre-flights below: the restore is what set the bit the un-pushed guard
      // tests, so running it here would refuse the retry over the revert's own tree.
      //
      // But BEHIND the identity check, which it used to precede. `publish` calls
      // `done(message)` - argument-free, acting on whatever is open, and `message` is a
      // real commit message, so it PUSHES. A retry after a project switch therefore
      // uploaded the replacement project and released its lock, under a comment two
      // hunks above claiming this check ran "at ENTRY". The pre-flights are what must
      // stay behind the resume; the identity is not.
      if (resumeAtPush) return publish({ message, shortSha, onStep });

      // [#576 RV4-FE-SP4] The restore below is refused in the read-only Plan and
      // Export workspaces (RV2-R-15), and the acquire before it is not: so refuse
      // here, before any IPC, rather than take the server lock (and possibly
      // fast-forward) only to hand it back. Behind the resume: an upload of a restore
      // that already landed replaces nothing.
      if (useUIStore.getState().appMode !== 'survey') return { kind: 'workspace_read_only' };

      // Un-SAVED is a different loss from un-PUSHED, and this flow resolves neither:
      // the restore closes and reopens the project at the checked-out tree, so an edit
      // that exists only in memory is gone, and `useRestoreRevision` then force-clears
      // `isDirty` - leaving nothing behind that says it ever existed. The persisted
      // SpeleoDB bit read below normally covers this, but only normally:
      // `mark_remote_dirty` logs and swallows a failure to write `has_unpushed_edits`,
      // and once the lock is lost or released the link can look clean while `isDirty`
      // is the ONLY evidence there is anything to lose. Same guard the other
      // project-replacement entry points already carry (`handBackRefusal` in
      // `useOpenRemoteProject`, `guardQuit`, `useProject`'s close guard).
      //
      // FIRST, and NOT waived by `discardLocal` - the opposite order to
      // `handBackRefusal`, deliberately. There, naming SpeleoDB first settles both
      // senses in one hop because the hand-back it sends you to carries a "save
      // changes locally" checkbox. Here the un-pushed refusal leads to this dialog's
      // OWN armed discard, which saves nothing: reaching it while dirty on disk would
      // destroy the in-memory work too, under a confirmation whose copy only ever
      // mentions SpeleoDB. Save first and those edits simply become part of the
      // un-pushed set that copy then describes accurately.
      if (useProjectStore.getState().isDirty) return { kind: 'unsaved_local' };

      // Pre-flight, deliberately BEFORE any IPC: a copy with un-pushed edits will be
      // refused by the restore anyway, so taking the lock first would block another
      // user for the duration of a call we already know fails. Read fresh from the
      // store rather than from a render snapshot - an autosave or a poll can have
      // flipped the bit since the dialog last rendered.
      if (!discardLocal && (useSpeleoStore.getState().link?.hasUnpushedEdits ?? false)) {
        return { kind: 'needs_discard', lockHeld: false, stateUnknown: false };
      }

      // [#258] Ask which file BEFORE acquiring, for the same reason the un-pushed
      // check above runs before any IPC: a question that needs no lock should not
      // cost one. Discovering the ambiguity inside the restore meant taking the
      // lock, failing, handing it back to ask, and re-taking it - all visible.
      //
      // Skipped once `entryPath` is set, because that IS the answer. The probe takes no
      // lock, touches no cache and never reports its own failures - but it DOES always
      // download. It used to skip that when the session held an answer; that answer was
      // about the tip while this asks about `hexsha`, so the short-circuit was answering
      // a different question than the one put to it.
      if (!entryPath) {
        const candidates = await invokeIpc(
          'speleodb_dump_candidates',
          { projectId, hexsha },
        ).catch(() => null);
        // The probe can involve a dump download, so it is the longest step before the
        // acquire. Candidates painted after a swap would describe a server tree nobody
        // is looking at any more.
        if (useSpeleoStore.getState().remoteProjectId !== projectId) {
          return { kind: 'project_changed' };
        }
        if (candidates && candidates.length > 0) {
          return { kind: 'awaiting_dump_choice', candidates };
        }
      }

      // Last call before anything becomes irreversible: the acquire below takes a
      // server mutex and the restore after it replaces the whole working tree. Both
      // pre-flights are re-run here rather than trusted from the top, because every
      // route into this point has a wait behind it - the probe's download, or the
      // picker, whose answer re-enters this callback from the very beginning with
      // `entryPath` set. The identity check answers "still the same project?"; the
      // dirty check answers "did they edit while we waited?", which the entry check
      // cannot, since it ran before the wait. Skipping the second let a restore
      // destroy in-memory work that arrived during the probe, under a confirmation
      // whose copy only ever mentions SpeleoDB.
      if (useSpeleoStore.getState().remoteProjectId !== projectId) {
        return { kind: 'project_changed' };
      }
      if (useProjectStore.getState().isDirty) return { kind: 'unsaved_local' };
      // ...and "still in Survey?", for the same reason: a switch to Plan or Export
      // during the wait would only have the restore refused after the acquire.
      if (useUIStore.getState().appMode !== 'survey') return { kind: 'workspace_read_only' };

      onStep?.('locking');
      if (!(await startEditing())) {
        // The shared start-editing router now rejects a reply before applying it when
        // the open project changed during the acquire. The backend may still have taken
        // this process's lock, so unwind that provenance-scoped claim before classifying
        // the false result as ordinary contention or failure.
        if (useSpeleoStore.getState().remoteProjectId !== projectId) {
          await invokeIpc('speleodb_release_lock_acquired_here').catch((e: unknown) => {
            console.warn(
              'revert: could not release the lock taken before the project changed',
              e,
            );
          });
          return { kind: 'project_changed' };
        }
        // `startEditing` resolves false for three different reasons, and only one of
        // them leaves anything for us to say.
        if (useSpeleoStore.getState().divergence !== null) {
          // The backend released the lock it took, and App.tsx has already swapped in
          // the divergence dialog over ours. Nothing to unwind, nothing to report.
          return { kind: 'diverged' };
        }
        const link = useSpeleoStore.getState().link;
        // `useSpeleoActions` has already toasted the holder; report it so the dialog
        // stays open and the user can retry once they release.
        if (link?.lockedByOther) return { kind: 'locked_by', email: link.lockHolder };
        return { kind: 'lock_failed' };
      }

      // The acquire is itself a round trip, and the restore on the other side of it is
      // the destructive step - it replaces the whole working tree. So the identity is
      // checked once more here, where a mismatch still costs nothing but the lock we
      // just took.
      //
      // Handing that lock back is the point: every other failure arm below unwinds
      // what it acquired, and walking away from a server mutex because the project
      // changed would strand it for the holder's whole session.
      //
      // The unwind names NEITHER a project nor `done`, and both exclusions are load
      // bearing.
      //
      // Not `done`: it is argument-free and acts on whatever is open - the very thing
      // this branch has just proved changed - and `message: null` picks the commit
      // message rather than suppressing the upload, so a replacement that is dirty and
      // editable would have been PUSHED by an error path.
      //
      // Not `speleodb_release_lock(projectId)` either, which was the first correction
      // and only fixed half of it. That verb targets an ARBITRARY project by design
      // (#201's held-lock prompt), so it unwinds whatever holds the id it is handed -
      // and `startEditing()` locks whatever is OPEN, so the id captured here is not
      // necessarily the id that got locked. Two ways to be wrong, and one destructive:
      // an acquire that met a lock this ACCOUNT already held inherited it from a
      // sibling session guarding its own un-pushed work, and `speleodb_start_editing`
      // is explicit that no unwind may hand that back.
      //
      // Only the backend knows which of those happened, so it decides:
      // `speleodb_release_lock_acquired_here` releases exactly what this process took
      // and no-ops on an inherited lock or none at all.
      //
      // Best-effort: a failed release warns and nothing more. The outcome is already
      // decided, and reporting a release failure instead of the project change would
      // name the smaller problem; a stranded lock stays visible in the held-lock
      // resume prompt.
      if (useSpeleoStore.getState().remoteProjectId !== projectId) {
        await invokeIpc('speleodb_release_lock_acquired_here').catch((e: unknown) => {
          console.warn('revert: could not release the lock taken before the project changed', e);
        });
        return { kind: 'project_changed' };
      }

      onStep?.('restoring');
      try {
        // Quiet: this restore is a middle step, and its toast tells the user to push -
        // which is precisely what the next line does for them.
        const applied = await restoreRevision({
          projectId,
          projectName,
          hexsha,
          shortSha,
          restore: true,
          discardLocal,
          quiet: true,
          ...(entryPath ? { entryPath } : {}),
        });
        if (!applied) {
          await invokeIpc('speleodb_release_lock_acquired_here').catch((e: unknown) => {
            console.warn(
              'revert: could not release the lock after the project changed during restore',
              e,
            );
          });
          return { kind: 'project_changed' };
        }
      } catch (e) {
        const refusedForLocalEdits = unpushedLocalEdits(e);
        // Ask the BACKEND whether the copy is dirty, because that is the bit
        // `speleodb_done` decides on - `message: None` selects the commit message,
        // it does not suppress the upload. Reading only the frontend mirror made
        // this release able to publish the very edits it was about to ask the user
        // to confirm discarding: the pre-flight above has already forced the local
        // bit to false on the `discardLocal: false` run, so `dirty` was false by
        // construction, while the backend row could be dirty from an autosave that
        // landed between the pre-flight and this call - which is one of the ways
        // the restore throws `unpushedLocalEdits` in the first place.
        //
        // That error IS the backend saying the bit is set, so it settles the
        // question on its own; the refresh covers the other failure kinds.
        if (!refusedForLocalEdits) await useSpeleoStore.getState().refreshLinkStatus();
        const dirty =
          refusedForLocalEdits || (useSpeleoStore.getState().link?.hasUnpushedEdits ?? false);
        // Hand the lock back ONLY while the copy is clean. `done(null)` is the one
        // release verb that both skips the push (the dirty bit is clear, so there is
        // nothing to upload) and drops the edit gate. While the copy IS dirty we keep
        // the lock on purpose: releasing would strand the user's own un-pushed work
        // with nothing protecting it, and `done(message)` would publish edits they
        // never asked to publish.
        //
        // `lockHeld` answers "am I now holding a lock I did not have?", so it must
        // key on whether a lock IS held, not on whether this release succeeded.
        // Folding every non-`released` answer into `true` made `not_editing` - the
        // edit gate having already dropped under us, usually a 401 auto-drop - report
        // "You still hold the write lock" for a gate that was demonstrably gone. A
        // thrown release (`null`) genuinely leaves it unknown, and staying on the
        // pessimistic answer there is right: being told to check a lock we no longer
        // hold is a wasted trip, being told nothing about one we DO hold is a
        // stranded server mutex.
        let lockHeld = true;
        if (!dirty) {
          const resolved = await done(null);
          const kind = resolved ? asDoneKind(resolved.kind) : null;
          // A lock left with another session of the account (#576 RV2-R-13) is
          // not held by THIS one either.
          lockHeld = !(doneFinished(kind) || kind === 'not_editing');
        }
        if (refusedForLocalEdits) {
          return {
            kind: 'needs_discard',
            lockHeld,
            stateUnknown: refusedForLocalEdits.state_unknown === true,
          };
        }
        // [#258] After the unwind, not before it: the release must have HAPPENED, and
        // `lockHeld` being false is the evidence that a composite retry can re-acquire.
        // While the copy is dirty the lock is deliberately kept and the retry's own
        // un-pushed pre-flight would refuse it, so that case falls through to the
        // message below - which is accurate - rather than to a picker that cannot work.
        const candidates = dumpCandidates(e);
        if (candidates && !lockHeld) return { kind: 'awaiting_dump_choice', candidates };
        return {
          kind: 'restore_failed',
          // [#576 Q-W44-7-D1] Refused because the project changed: the short line instead.
          message:
            projectChangedMessage(e) ??
            humanizeSpeleoError(e, t('speleodb.revert.errors.restorePrefix', { sha: shortSha })),
          lockHeld,
        };
      }

      return publish({ message, shortSha, onStep });
    },
    [t, startEditing, done, restoreRevision, publish],
  );
}
