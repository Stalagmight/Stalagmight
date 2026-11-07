import { useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore, type HeldLock } from '../../store/speleoStore';
import { useSpeleoActions } from '../../hooks/useSpeleoActions';
import { useSpeleoLocalStates } from '../../hooks/useSpeleoLocalStates';
import { hasUnpushedEdits, localStateUnknown, UNPUSHED_ACCENT } from '../../utils/speleoLocalState';
import { humanizeSpeleoError } from '../../utils/speleoError';
import { needsSpeleoHandBackPrompt } from '../../utils/quit';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import { TOAST_KEYS } from '../../utils/toastKeys';
import { useScreenOccupant } from '../../hooks/useScreenOccupant';

/**
 * Held-lock resume/release prompt (#201, B1). SpeleoDB has no server-side lock
 * TTL, so a crashed/abandoned session leaves its write locks held. On login /
 * app-open a single `list_projects` scan finds the locks THIS user holds; if any,
 * this prompt offers to resolve them.
 *
 * That scan is server-only, so on its own it cannot tell a lock over an abandoned
 * empty session from a lock over hours of work that never reached the server.
 * #270 joins in the LOCAL half (`speleodb_local_project_states`, one call for every
 * listed id) and lets it pick the row's action:
 * - **no un-pushed edits** -> **Release** (`POST /projects/{id}/release/`), as before;
 *   releasing the last one auto-closes the prompt.
 * - **un-pushed edits** -> **Continue editing** INSTEAD: open that project and keep
 *   the lock (`speleodb_start_editing`, the app's open-and-hold verb). Release is not
 *   offered at all for such a row - it is the one action that would strand the work
 *   behind a lock the user no longer holds.
 * - **local copy present but UNREADABLE** (`state_unknown`) -> **Continue editing**
 *   too. The listing fails open per row, so such a row arrives with
 *   `has_unpushed_edits: false` - identical to clean. The likely cause is a hot
 *   journal from the kill/power-loss that stranded this very lock, which a read-only
 *   connection cannot roll back, so "unknown" is the only honest read and Release is
 *   withheld. The Rust guard fails closed on the same input; this keeps the two in
 *   agreement rather than having the UI offer what the backend would refuse.
 *
 * Dialog-wide:
 * - **Keep working** - dismiss and leave the locks held.
 *
 * There is deliberately NO bulk release. The server verb is all-or-nothing and this
 * prompt is exactly where locks sitting over un-pushed work show up, so a single
 * button that frees every lock is the one action most likely to strand the work the
 * per-row rules exist to protect. Releasing is per project, and only where there is
 * nothing local to lose.
 * - Closing the dialog (X) is the "do nothing" exit - same as Keep working.
 */
export default function HeldLockResumeDialog() {
  const { t } = useTranslation();
  // Independent top-layer overlay (mounted directly on `heldLockPromptOpen`,
  // outside `activeDialog`) - register occupancy so a cold-launch drain
  // doesn't raise a second modal over it (#445 review). Always true here:
  // App.tsx only mounts this component while the prompt is open.
  useScreenOccupant(true);
  const showToast = useUIStore((s) => s.showToast);
  const heldLocks = useSpeleoStore((s) => s.heldLocks);
  const releaseLock = useSpeleoStore((s) => s.releaseLock);
  const clearHeldLocks = useSpeleoStore((s) => s.clearHeldLocks);
  // Close this OVERLAY only (not the shared `closeDialog`, which would close the
  // dialog this prompt is layered over - e.g. Settings during an inline login).
  const closeHeldLockPrompt = useSpeleoStore((s) => s.closeHeldLockPrompt);
  const recentProjects = useProjectStore((s) => s.recentProjects);
  const loadProject = useProjectStore((s) => s.loadProject);
  const { startEditing } = useSpeleoActions();
  // Which project id is being acted on, or null when idle. (There is no bulk path -
  // see the note above on why "Release all" does not exist.)
  const [busyId, setBusyId] = useState<string | null>(null);

  const locks = useMemo(() => heldLocks ?? [], [heldLocks]);
  const busy = busyId !== null;

  // The local half of the join, fetched once for every listed lock (#270).
  const lockIds = useMemo(() => locks.map((l) => l.projectId), [locks]);
  const { states, loaded, failed } = useSpeleoLocalStates(lockIds);

  // The accent + "has changes" line are a POSITIVE report only: an empty `states`
  // means we have not been told, not that the project is clean.
  const dirty = (projectId: string) => hasUnpushedEdits(states, projectId);
  // A local copy the backend could not READ. It arrives with `has_unpushed_edits:
  // false` (the listing fails open per row), so without this it is indistinguishable
  // from clean and would offer Release. See `localStateUnknown`.
  const unknownState = (projectId: string) => localStateUnknown(states, projectId);

  /**
   * Which action a row offers. Release is the one irreversible button here - the
   * server verb has no veto, so releasing a lock that sits over un-pushed work
   * lets a colleague lock and push over it - so it is offered ONLY on a positive
   * "this project is clean" report, never on the absence of a report.
   *
   * That distinction is the whole point of `loaded`/`failed`:
   *   - in flight  -> `checking`. The scan is raised at app-open and walks one
   *     cache dir per listed id, so this window is real, and a click landing in
   *     it used to release a lock over hours of unsynced survey.
   *   - failed     -> `continue`. `states` is reset to `{}` on failure, which
   *     otherwise reads as "every project is clean" and deterministically flips
   *     every row to Release. Continue editing is the safe read of "unknown": it
   *     KEEPS the lock and opens the project so the user can look for themselves.
   *     A genuinely abandoned lock is still releasable from Project Settings, and
   *     Keep working leaves everything as it was.
   */
  const actionFor = (projectId: string): 'release' | 'continue' | 'checking' => {
    if (!loaded) return 'checking';
    if (failed) return 'continue';
    // An UNREADABLE copy is "unknown", not "clean": the whole-call `failed` branch
    // above and this per-row one are the same rule at two scopes, so they get the
    // same safe answer. Checked before `dirty` because an unreadable row cannot
    // report a dirty bit at all.
    if (unknownState(projectId)) return 'continue';
    return dirty(projectId) ? 'continue' : 'release';
  };

  // Auto-close once the last lock is gone - releasing them one by one shrinks
  // `heldLocks` in the store, and when it empties there is nothing left to resolve.
  useEffect(() => {
    if (locks.length === 0) closeHeldLockPrompt();
  }, [locks.length, closeHeldLockPrompt]);

  // Keep / Do-nothing: leave the locks held and just dismiss the prompt.
  const keep = () => {
    clearHeldLocks();
    closeHeldLockPrompt();
  };

  const releaseOne = async (projectId: string) => {
    setBusyId(projectId);
    try {
      // On success the store drops this lock from `heldLocks`; the effect above
      // closes the dialog when the last one goes.
      await releaseLock(projectId);
    } catch (e) {
      showToast(
        humanizeSpeleoError(e, t('speleodb.heldLocks.releaseFailed')),
        TOAST_KEYS.SPELEODB_RELEASE_ONE_ERROR,
        'error',
      );
    } finally {
      setBusyId(null);
    }
  };

  /**
   * Resume the session this lock belongs to: open the project (unless it already is
   * the open one) and re-acquire, keeping the lock. `speleodb_start_editing` is the
   * app's open-and-hold verb and acts on the OPEN project, so the load comes first.
   * Re-acquiring a lock we already hold is a no-op server-side - the acquire simply
   * succeeds (only ANOTHER holder produces the 400 that reports contention) - and the
   * reconcile that follows is exactly what a resumed session needs.
   */
  const continueEditing = async (lock: HeldLock) => {
    setBusyId(lock.projectId);
    try {
      const path = recentProjects.find((p) => p.speleodb_project_id === lock.projectId)?.file_path;
      if (path === undefined) {
        // We cannot name a local file for this project. Say so and change nothing -
        // releasing the lock here is exactly the outcome this row exists to prevent.
        // (Reachable from the `failed` branch too, where `actionFor` returns
        // 'continue' precisely because NOTHING is known about the local copy.)
        showToast(
          t('speleodb.localEdits.heldLocks.openFailed', { name: lock.projectName }),
          TOAST_KEYS.SPELEODB_CONTINUE_EDITING_ERROR,
          'warning',
        );
        return;
      }
      // Compare against the LIVE open path rather than `states[...].is_open`: that
      // map is a snapshot taken when the prompt mounted and is never revalidated, and
      // `loadProject` short-circuits on ALREADY_OPEN anyway. The path is also the only
      // thing that decides whether the load below would close another project.
      const openPath = useProjectStore.getState().project?.file_path ?? null;
      if (openPath !== path) {
        // This prompt is app-wide and is raised on LOGIN as well as app-open, and
        // login is reachable from Settings with a project open. `loadProject` for a
        // DIFFERENT project closes the current one backend-side - which is exactly why
        // `routeOpenedFile`'s `.stalx` branch routes through the exit guards. Refuse rather
        // than open a second modal over this overlay: the lock stays held and
        // resolvable, and nothing is lost.
        //
        // BOTH kinds of work at risk, in the guards' own order. Disk-dirty is the
        // obvious one. The other is the reason `needsSpeleoHandBackPrompt` exists:
        // the open project can be perfectly clean on disk and still hold un-pushed
        // edits or our write lock, and closing it backend-side hands neither back -
        // it just hides the local copy and drops the lock this very prompt is about.
        // Checking only `isDirty` made this a fifth exit that skipped the guard the
        // other four share, which is the drift that guard was extracted to stop.
        const openLink = useSpeleoStore.getState().link;
        if (useProjectStore.getState().isDirty) {
          showToast(
            t('speleodb.localEdits.heldLocks.unsavedChanges', { name: lock.projectName }),
            TOAST_KEYS.SPELEODB_CONTINUE_EDITING_DIRTY,
            'warning',
          );
          return;
        }
        if (needsSpeleoHandBackPrompt(openLink)) {
          showToast(
            t('speleodb.localEdits.heldLocks.unpushedWork', { name: lock.projectName }),
            TOAST_KEYS.SPELEODB_CONTINUE_EDITING_UNPUSHED,
            'warning',
          );
          return;
        }
        const result = await loadProject(path);
        // loadProject surfaces its own failure; leave the prompt up so the lock is
        // still resolvable.
        if (!result.success) return;
      }
      // Acquire + reconcile, with its own toasts / divergence routing. Only hand
      // the shell over if we actually came away holding the lock: a contended
      // lock, a reported divergence or a network failure all leave this prompt as
      // the only place the remaining locks are resolvable, and dismissing it here
      // would also DISCARD them (`clearHeldLocks` drops the whole list, including
      // rows that were cleanly releasable) with no way to re-raise the prompt
      // until the next login scan.
      // `resumeHeldLock`: the user resumes a lock this account holds (typically this
      // app before a restart), so the session claims it and Done releases it as usual
      // (#576 RV2-R-13-RESTART).
      if (!(await startEditing({ resumeHeldLock: true }))) return;
      // The lock stays held on purpose: hand the shell over to the project.
      clearHeldLocks();
      closeHeldLockPrompt();
    } finally {
      setBusyId(null);
    }
  };

  return (
    <DialogWrapper
      title={t('speleodb.heldLocks.title')}
      // Not dismissable while a row's verb is in flight - the Keep button below is
      // already `disabled={busy}` for it, and DialogWrapper routes BOTH Escape and a
      // backdrop click here, so ungated they did what the equivalent button refuses.
      // `keep` runs `clearHeldLocks()`, which drops the WHOLE list including rows that
      // were cleanly releasable; landing that mid-`releaseOne` leaves the remaining
      // locks with no way to re-raise the prompt until the next login scan (the same
      // hazard `continueEditing` guards against by not dismissing on a failed
      // acquire). Same hardening as SpeleoQuitDialog / SpeleoUnpushedLocalDialog.
      onClose={busy ? () => {} : keep}
      size="md"
      data-testid="speleodb-held-lock-dialog"
    >
      <div className="space-y-4">
        <p className="text-sm text-text-secondary">
          {t('speleodb.heldLocks.description', { count: locks.length })}
        </p>

        <ul className="max-h-40 overflow-y-auto rounded border border-border divide-y divide-border">
          {locks.map((lock) => {
            const hasEdits = dirty(lock.projectId);
            const stateUnknown = unknownState(lock.projectId);
            const action = actionFor(lock.projectId);
            return (
              <li
                key={lock.projectId}
                className="flex items-center justify-between gap-3 px-3 py-2 text-sm text-text-primary"
                data-testid={`speleodb-held-lock-${lock.projectId}`}
                data-local-edits={hasEdits ? 'true' : 'false'}
                data-local-state-unknown={stateUnknown ? 'true' : 'false'}
                data-row-action={action}
              >
                <span className="min-w-0 truncate">
                  {lock.projectName}
                  {hasEdits && (
                    <span
                      className="block text-xs"
                      style={{ color: UNPUSHED_ACCENT }}
                      data-testid={`speleodb-held-lock-local-edits-${lock.projectId}`}
                    >
                      {t('speleodb.localEdits.heldLocks.hasLocalChanges')}
                    </span>
                  )}
                  {/* Say WHY Release is missing on this row, so a withheld button does
                      not read as a bug. Mutually exclusive with the line above: an
                      unreadable copy cannot report a dirty bit. */}
                  {stateUnknown && !hasEdits && (
                    <span
                      className="block text-xs"
                      style={{ color: UNPUSHED_ACCENT }}
                      data-testid={`speleodb-held-lock-state-unknown-${lock.projectId}`}
                    >
                      {t('speleodb.localEdits.heldLocks.localStateUnknown')}
                    </span>
                  )}
                </span>
                {action === 'checking' ? (
                  <span
                    className="shrink-0 text-xs text-text-secondary"
                    data-testid={`speleodb-held-lock-checking-${lock.projectId}`}
                  >
                    {t('speleodb.localEdits.heldLocks.checking')}
                  </span>
                ) : action === 'continue' ? (
                  <button
                    type="button"
                    onClick={() => void continueEditing(lock)}
                    disabled={busy}
                    className="shrink-0 text-xs text-accent hover:underline disabled:opacity-50 disabled:no-underline"
                    aria-label={t('speleodb.localEdits.heldLocks.continueEditingAria', {
                      name: lock.projectName,
                    })}
                    data-testid={`speleodb-held-lock-continue-${lock.projectId}`}
                  >
                    {busyId === lock.projectId
                      ? t('speleodb.localEdits.heldLocks.opening')
                      : t('speleodb.localEdits.heldLocks.continueEditing')}
                  </button>
                ) : (
                  <button
                    type="button"
                    onClick={() => void releaseOne(lock.projectId)}
                    disabled={busy}
                    className="shrink-0 text-xs text-danger hover:underline disabled:opacity-50 disabled:no-underline"
                    aria-label={t('speleodb.heldLocks.releaseOneAria', { name: lock.projectName })}
                    data-testid={`speleodb-held-lock-release-${lock.projectId}`}
                  >
                    {busyId === lock.projectId
                      ? t('speleodb.heldLocks.releasing')
                      : t('speleodb.heldLocks.release')}
                  </button>
                )}
              </li>
            );
          })}
        </ul>

        {failed && (
          <p
            className="text-xs"
            style={{ color: UNPUSHED_ACCENT }}
            data-testid="speleodb-held-lock-local-unknown"
          >
            {t('speleodb.localEdits.heldLocks.unknown')}
          </p>
        )}

        <div className="flex justify-end gap-2 pt-2 border-t border-border">
          <Button onClick={keep} disabled={busy} data-testid="speleodb-held-lock-keep">
            {t('speleodb.heldLocks.keep')}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
