import { isProjectRequestCurrent, beginProjectMutation, isProjectMutationCurrent, reserveProjectRequest } from '../utils/projectTransition';
import { useCallback } from 'react';
import { confirmSpeleoOpen } from '../utils/confirmSpeleoOpen';
import { useTranslation } from 'react-i18next';
import { useUIStore, type ToastKind } from '../store/uiStore';
import { useProjectStore } from '../store/projectStore';
import {
  useSpeleoStore,
  speleoProjectSessionIssuedFor,
} from '../store/speleoStore';
import { invokeIpc } from '../utils/ipc';
import { applySpeleoTreeReplacement } from '../utils/speleoTreeReplacement';
import {
  speleoErrorAction,
  humanizeSpeleoError,
  unpushedLocalEdits,
} from '../utils/speleoError';
import { needsSpeleoHandBackPrompt } from '../utils/quit';
import { TOAST_KEYS } from '../utils/toastKeys';
import type {
  SpeleoProjectSummary,
  SpeleoOpenResult,
} from '../types/ipc.generated';

/** Result shared by the project browser, remote rows and local-cache recovery. */
export type OpenRemoteOutcome =
  | { status: 'opened' }
  | { status: 'cancelled' }

  | { status: 'load_failed' }
  | { status: 'unpushed_local' }
  | { status: 'error'; message: string; reLogin: boolean };

interface UseOpenRemoteProjectOpts {
  /** Run right before loadProject (e.g. closeDialog so the load progress shows). */
  onBeforeLoad?: () => void;
  /**
   * [#270] The dialog to hand the user back to if they cancel the refused-open
   * prompt this hook raises. The prompt replaces the host in the single
   * `activeDialog` slot, so without this a Cancel left `activeDialog: null` - the
   * browser gone, nothing opened, and no way back except re-opening the menu.
   * Omitted by hosts that are not dialogs (GlobalView's remote rows), where
   * cancelling correctly just returns to the view underneath.
   */
  cancelReturnsTo?: string;
}

/** One ownership token covers the warning, download, seed, and file load. */
function reserveRemoteOpen() {
  const request = reserveProjectRequest();
  const sessionGeneration = useSpeleoStore.getState().sessionGeneration;
  let dispatched = false;
  return { request,
    latest: () => isProjectRequestCurrent(request) && useSpeleoStore.getState().sessionGeneration === sessionGeneration,
    dispatch: () => { dispatched = beginProjectMutation(request); return dispatched; },
    current: () => (dispatched ? isProjectMutationCurrent(request) : isProjectRequestCurrent(request)) &&
      useSpeleoStore.getState().sessionGeneration === sessionGeneration };

}

/** Pull a native project, seed its access state, load it, and roll back a failed load. */
export function useOpenRemoteProject(opts: UseOpenRemoteProjectOpts = {}) {
  const { t } = useTranslation();
  const { onBeforeLoad, cancelReturnsTo } = opts;
  const showToast = useUIStore((s) => s.showToast);
  const openDialog = useUIStore((s) => s.openDialog);
  const loadProject = useProjectStore((s) => s.loadProject);
  const setLinkOnOpen = useSpeleoStore((s) => s.setLinkOnOpen);

  // Shared tail: a native pull produced a local .stalx - seed the link
  // optimistically from the summary permission, then load. Roll the seed back if the
  // load fails (loadProject shows its own error), so the shell never keeps a remote
  // read-only link for a project that never opened.
  const finishOpen = useCallback(
    async (
      result: { local_path: string; concurrent_session: boolean },
      permission: SpeleoProjectSummary['permission'],
      /** The project this open is FOR, so the seed names it rather than waiting for a poll. */
      projectId: string,
      ownership: ReturnType<typeof reserveRemoteOpen>,
      opts: {
        /**
         * `kind` defaults to `info` for an ordinary successful open; callers can
         * provide another severity when the result needs attention.
         */
        successToast?: { message: string; key: string; kind: ToastKind };
        /**
         * [#270] Seed the link's dirty bit as TRUE. The seed's default is a fresh
         * pull (nothing local the server has not seen); "keep the local copy" and
         * an open are the opposite by construction, and seeding them clean
         * disarmed the quit guard for a project we have proof is dirty.
         */
        hasUnpushedEdits?: boolean;
      } = {},
    ): Promise<OpenRemoteOutcome> => {
      const { successToast, hasUnpushedEdits } = opts;
      if (!ownership.current()) return { status: 'cancelled' };
      // #270: what to put back if the load fails. Captured BEFORE the seed, so the
      // rollback below is a true undo rather than a clear-and-hope. `priorPath` is
      // the project the snapshot describes, so the rollback can tell "the load
      // failed and we are still here" from "the ground moved" (see below).
      const priorLink = useSpeleoStore.getState().snapshotLink();
      const priorPath = useProjectStore.getState().project?.file_path ?? null;
      /**
       * [#270] The file being "opened" is the one already on screen, so do not
       * touch the link AT ALL: its live link is the authoritative state (lock
       * held, permission, dirty bit, all resolved). The optimistic seed is for a
       * project arriving from the server; applied to the open one it OVERWRITES
       * resolved truth with a pessimistic guess (`read-only`, nothing unpushed, no
       * force-edit), and `loadProject` then short-circuits on ALREADY_OPEN so
       * nothing re-resolves it synchronously - a read-only survey UI and disarmed
       * exit guards for a project we hold the lock on, until some later status
       * poll (30 s to a user-configurable 24 h) happens to correct it.
       *
       * Decided HERE rather than at a call site because all three routes into this
       * tail can name the open project: "Open from SpeleoDB" sits in the
       * IN-PROJECT menu, so `openRemote` re-opening the current project is an
       * ordinary mis-step, and the open it can route to lands on the same path.
       *
       * A LITERAL string comparison, deliberately - it is NOT a claim that the two
       * spellings name the same file on disk. What it has to predict is whether
       * `loadProject` will short-circuit, and the backend decides exactly that with
       * the same literal test (`do_load_project_full`:
       * `project.file_path.as_ref() == Some(&path)`), over the same two values:
       * `file_path` is stored verbatim from the path the load was handed, and both
       * sides here are minted by one process-wide `speleodb_cache_path`. So the
       * aliases a literal test cannot see through - a symlink, `..`, a Windows /
       * macOS case alias - are ones the BACKEND cannot see through either, and this
       * lands on the correct branch anyway: it really is a full reload, so the seed
       * really is needed. Canonicalizing only this side would desync the prediction
       * from the fact and skip the seed for a load that still happens, leaving
       * `syncProjectLink` (literal in the same way, on `linkPath === forPath`) to
       * resolve the fresh open to read-only / nothing-unpushed with no seed left to
       * carry the permission or the dirty bit. Alias tolerance, if it is ever
       * wanted, has to start in `do_load_project_full` - `project_io::same_file`
       * canonicalizes, and `close_project_if_open_at` already uses it - with this
       * side following, never leading. The webview cannot canonicalize a path at
       * all: it has no filesystem.
       */
      const alreadyOpen = priorPath === result.local_path;
      // #193 b8: a lock held by THIS account at open time = a second same-account
      // session; warn (keyed so repeat opens don't stack duplicate toasts).
      if (result.concurrent_session) {
        showToast(t('speleodb.browser.concurrentSession'), TOAST_KEYS.SPELEODB_CONCURRENT_SESSION, 'warning');
      }
      // `forPath` is the exact string handed to `loadProject` below, so the
      // `syncProjectLink` inside that load recognises this seed as belonging to
      // the project being opened rather than to whatever was open before.
      if (alreadyOpen) {
        // The seed is skipped here on purpose - the link on screen is this project's
        // own RESOLVED state and replacing it with an optimistic one is the downgrade
        // this branch exists to avoid. But the project ID is not part of that state,
        // it is just which project this is, and skipping the whole seed took it with
        // the rest: reopening from SpeleoDB a project already on screen from recents
        // left `remoteProjectId` stale until a status request happened to land. The
        // revision history is keyed on it, and the identity guards compare against it,
        // so the window showed no history and could refuse actions as
        // `project_changed`. One field, without touching the link.
        useSpeleoStore.setState({ remoteProjectId: projectId });
      } else {
        setLinkOnOpen({
          permission,
          forPath: result.local_path,
          hasUnpushedEdits: hasUnpushedEdits === true,
          projectId,
        });
      }
      onBeforeLoad?.();
      const loaded = await loadProject(result.local_path, { accessConfirmed: true, request: ownership.request });
      if (!ownership.current()) return { status: 'load_failed' };
      if (!loaded.success) {
        // Nothing to roll back when we never seeded: the link on screen is still
        // the open project's own, resolved state, and clearing it would be the very
        // downgrade this branch exists to undo.
        if (alreadyOpen) return { status: 'load_failed' };
        // #270: the snapshot describes a project that is no longer the one on
        // screen, so putting it back would overwrite the link of whatever the user
        // actually opened. `loadProject` reports `success: false` for two different
        // things, and only one of them is "the load failed and we are still where
        // we were": a SUPERSEDED load lost its slot to a newer open, which commits
        // its own project and link. (The path check covers the other way to move
        // the ground - a close landing mid-flight, which does not bump the load
        // id.) Restoring here put the pre-attempt snapshot - `projectLinkKind:
        // 'unknown'`, read-only under the fail-closed gate - over a freshly-opened
        // LOCAL project. GlobalView guards its remote rows with `remoteBusyRef`,
        // but `handleOpenFromList` and `handlePublishToSpeleoDb` do not, so a
        // second open during a slow pull is reachable.
        const groundMoved =
          loaded.superseded === true ||
          (useProjectStore.getState().project?.file_path ?? null) !== priorPath;
        if (groundMoved) return { status: 'load_failed' };
        // #270: the failed load left the PREVIOUS project on screen and still open
        // backend-side, so put its link state back exactly as it was.
        //
        // This used to be `clearLink()`, which sets `projectLinkKind: 'unknown'` -
        // read-only under the fail-closed gate - leaving the repair entirely to the
        // unawaited `refreshLinkStatus` below. That call swallows its errors by
        // design, and the conditions that break a load (offline, a bad disk) are the
        // same ones that break a status call; `SpeleoStatusButton`'s recovery poll
        // is gated on `link !== null`, which `clearLink` had just falsified. So a
        // local project the user was editing could go read-only for the rest of the
        // session because an unrelated remote open failed, with nothing on screen
        // explaining it. The restore needs no network and cannot fail.
        useSpeleoStore.getState().restoreLink(priorLink);
        // Still worth re-resolving from the backend - the snapshot is as fresh as
        // the moment before the attempt, not as fresh as now. Opportunistic, not
        // load-bearing: unawaited, and the restore above already stands on its own.
        void useSpeleoStore.getState().refreshLinkStatus();
        return { status: 'load_failed' };
      }
      // Reopening the file already on screen makes loadProject take its intentional
      // already-open fast path. The backend may still have replaced the whole tree, so
      // run the same post-replacement cleanup as fast-forward, discard, and restore.
      if (
        alreadyOpen &&
        !(await applySpeleoTreeReplacement(speleoProjectSessionIssuedFor(projectId), true))
      ) {
        return { status: 'load_failed' };
      }
      if (!ownership.current()) return { status: 'load_failed' };
      if (successToast) showToast(successToast.message, successToast.key, successToast.kind);
      return { status: 'opened' };
    },
    [showToast, setLinkOnOpen, loadProject, onBeforeLoad, t],
  );

  /**
   * [#270] Would opening `summary` strand work in the project already on screen?
   * Returns the message to refuse with, or null when the swap is safe.
   *
   * "Open from SpeleoDB" sits in the IN-PROJECT hamburger and this flow ends in
   * `loadProject`, which replaces the open project backend-side - so it is one of
   * the ways OUT of a project, and the only one that asked nothing at all. Unsaved
   * in-memory edits were discarded with no prompt, and a managed project was swapped
   * away with its un-pushed work left behind in the cache and its server lock still
   * held. Every other way out asks first: close, opening another `.stalx`, New
   * Project and the held-lock resume prompt on `needsSpeleoHandBackPrompt`, quit on
   * its unpushed-work half (the backend releases the lock on process exit).
   *
   * Checked at the ENTRY point rather than in the shared `finishOpen` tail, where the
   * sibling `alreadyOpen` test lives: the tail runs AFTER the pull, and its first act
   * is the optimistic seed that overwrites the very link this reads. So it runs at
   * BOTH entry points that issue a project-replacing command of their own -
   * `openRemote` - never in the tail they share.
   *
   * The refused-open prompt's two branches run it too. They answer the prompt a
   * guarded `openRemote` raised - but that
   * prompt is about the TARGET's cached copy, and this asks about the project on
   * SCREEN, which the answer says nothing about. The prompt is also a user
   * interaction of unbounded length: `SpeleoUnpushedLocalDialog` sits there while
   * `SpeleoStatusButton`'s poll (30 s to a user-configurable 24 h) keeps calling
   * `refreshLinkStatus`, whose `seedFromStatus` rewrites the very `link` this reads -
   * so the open project can acquire un-pushed work, or have its lock resolved,
   * between the check and the load that replaces it. Nothing shuts the door out of an
   * un-pushed cache copy, because the case that needs that door is the one where the
   * target IS the open project, and that is exactly what the same-project exemption
   * below lets through.
   *
   * `keepLocalCopy` is the one partial exception, and only when the path it opens is
   * the one already on screen: it issues no command and `loadProject` short-circuits
   * on ALREADY_OPEN, so it replaces nothing and there is nothing to guard - see
   * there.
   *
   * It REFUSES rather than raising the shared hand-back prompt, the way
   * `HeldLockResumeDialog`'s Continue-editing row does. `speleodb-close` replays what
   * raised it (`pendingOpenPath` / `pendingNewProject`) and has no notion of a remote
   * open, so raising it here would resolve the SpeleoDB side and then drop the user
   * on the home screen with nothing opened - trading a silent swap for a swallowed
   * request. Refusing leaves the browser, the open project and the work exactly as
   * they were, and names an action that is one menu away.
   *
   * SpeleoDB before disk-dirty, the order every other exit guard uses: Close resolves
   * BOTH (its prompt carries a "save changes locally" checkbox), so naming that first
   * settles a project that is dirty in both senses in one hop rather than two.
   */
  const handBackRefusal = useCallback(
    (summary: SpeleoProjectSummary): string | null => {
      const { project, isDirty } = useProjectStore.getState();
      if (!project) return null;
      const speleo = useSpeleoStore.getState();
      // Re-opening the project already on screen is exempt from the HAND-BACK check
      // only, not from the whole guard. Nothing is handed back to a project we are
      // re-opening, and that exemption is load-bearing: it is how the refused-open
      // prompt is reached for the OPEN project, whose pull is refused over its
      // un-pushed cache copy and whose only way out is that prompt's "take the server
      // copy". Refusing on un-pushed work here would shut the door this branch exists
      // behind. Decided from the id the status poll recorded, because that is all
      // there is to go on BEFORE the pull; `remoteProjectId` is null until the first
      // poll lands, so that window refuses an open it could have allowed - a menu
      // trip, against losing work the other way.
      const reopeningOpenProject = speleo.remoteProjectId === summary.id;
      if (!reopeningOpenProject && needsSpeleoHandBackPrompt(speleo.link))
        return t('speleodb.errors.openBlockedUnpushed', { name: summary.name });
      // Disk-dirty is NOT exempt, even re-opening. `speleodb_open_project` replaces
      // the cache `.stalx`, and `replace_cache_stalx_abandoning` closes the live
      // backend project without saving before the rename - so unsaved in-memory
      // survey is dropped. The persisted dirty bit normally makes the backend refuse
      // first, but `mark_remote_dirty` logs and swallows a failure to write it, and a
      // frontend-only edit can leave `isDirty` as the only evidence there is anything
      // to lose. Exempting the re-open from this too was the wrong half to widen.
      if (isDirty) return t('speleodb.errors.openBlockedUnsaved', { name: summary.name });
      return null;
    },
    [t],
  );

  /**
   * [#270] `handBackRefusal` again, on the far side of the command - for the pull
   * verbs, whose await is the DOWNLOAD.
   *
   * The entry check has to run before the command; it is the only place that can
   * stop it. But its answer is stale by the time the command returns, and here the
   * gap is not a user sitting on a dialog the way the open confirm and the
   * refused-open prompt are. It is the network, on EVERY open: `download_stalx` runs
   * with a 120-second timeout and a 512 MiB ceiling. The pull resolves the revision
   * history, downloads and validates native SQLite, then installs it. The status poll's
   * floor is 30 s, so at least one poll can land inside a single slow attempt.
   *
   * It does land, because `SpeleoStatusButton` stays MOUNTED underneath: `App`
   * renders the dialogs as SIBLINGS of `SingleSystemView`, so raising the browser
   * (or the refused-open prompt) does not unmount the toolbar, and its interval
   * keeps calling `refreshLinkStatus` throughout the wait. `seedFromStatus` rewrites
   * `link` wholesale - `hasUnpushedEdits`, `editState`, `lockHolder` - so the
   * project on screen can acquire un-pushed work, or have its lock resolved, between
   * the entry check and the load that closes it backend-side. (GlobalView's remote
   * rows are the one host where it cannot: `view === 'global'` renders GlobalView
   * INSTEAD of `SingleSystemView`, so the button is unmounted and the poll unarmed.
   * The hook is shared, so the guard belongs here rather than in the host that
   * happens to need it.)
   *
   * `finishOpen` cannot be left to notice, for the reason the entry check exists at
   * all: its first act is the optimistic seed, which overwrites exactly what the
   * poll just wrote. By the time `loadProject` replaces the project the evidence is
   * already gone - the newly-discovered work hidden, or the newly-discovered lock
   * silently released.
   *
   * Refusing here leaves the pulled copy on disk, and that is a price rather than a
   * second data risk. Under `LocalEditsPolicy::Protect` the backend runs
   * `guard_local_edits` before it downloads anything AND again inside
   * `replace_cache_stalx`, and that guard fails CLOSED (a local copy it cannot read
   * refuses too), so a replace only ever happens over a cache copy with
   * `has_unpushed_edits == false` - a clean copy of a server revision, re-downloadable
   * in full. `Discard` is the take-remote branch, where the user confirmed that exact
   * loss by name. The local `display_settings` are carried across the replace either
   * way. So nothing that exists only on this computer is spent by refusing, and the
   * project on screen - which is what this guard is about - is untouched.
   *
   * Skipped when the pull names the project ALREADY ON SCREEN, and that is not a
   * courtesy: refusing there would be worse than the swap it prevents.
   * `replace_cache_stalx_abandoning` calls `close_project_if_open_at(dest)` before
   * the rename, so a pull whose destination is the open project has already closed
   * it backend-side by the time we get here; returning an error would leave the shell
   * displaying a project the backend does not have open, where every later command,
   * auto-save included, fails with "no project open" - the exact state that
   * function's own failure path goes out of its way to undo. And there is nothing to
   * protect: `finishOpen` skips the seed for that path (`alreadyOpen`), so the poll's
   * update is not overwritten, and the `loadProject` that follows re-resolves the
   * same project's link anyway.
   *
   * Decided by the SAME literal path comparison `finishOpen` makes, deliberately, so
   * the two can never disagree - this skips exactly when that skips the seed, and
   * guards exactly when it seeds. See `alreadyOpen` for why literal is the correct
   * test and not merely the careless one. Known residual: `close_project_if_open_at`
   * canonicalizes (`project_io::same_file`), so an ALIAS spelling closes the project
   * backend-side while this side reads "not already open". Only the disk-dirty half
   * can then refuse (the hand-back half is exempt - an alias is the same project, so
   * `remoteProjectId === summary.id`), and only if `isDirty` flipped during the
   * download. Narrow, and the alternative - canonicalizing here - is the thing the
   * do-not-regress tests at the foot of the suite exist to block; alias tolerance has
   * to start in `do_load_project_full`, with this side following.
   */
  const handBackRefusalAfterPull = useCallback(
    (summary: SpeleoProjectSummary, localPath: string): string | null => {
      if ((useProjectStore.getState().project?.file_path ?? null) === localPath) return null;
      return handBackRefusal(summary);
    },
    [handBackRefusal],
  );


  const openRemote = useCallback(
    async (
      summary: SpeleoProjectSummary,
    ): Promise<OpenRemoteOutcome> => {
      // Before the pull, the seed and the load: nothing about the open project may be
      // given up silently (see `handBackRefusal`).
      const refusal = handBackRefusal(summary);
      if (refusal) return { status: 'error', message: refusal, reLogin: false };
      const ownership = reserveRemoteOpen();
      try {
        if (!await confirmSpeleoOpen(summary) || !ownership.current()) return { status: 'cancelled' };
      } catch (e) {
        if (!ownership.latest()) return { status: 'cancelled' };
        return { status: 'error', message: humanizeSpeleoError(e, t('speleodb.errors.openFailed')), reLogin: false };
      }
      const afterWarning = handBackRefusal(summary);
      if (afterWarning) return { status: 'error', message: afterWarning, reLogin: false };
      let result: SpeleoOpenResult;
      try {
        if (!ownership.dispatch()) return { status: 'cancelled' };
        result = await invokeIpc<SpeleoOpenResult>('speleodb_open_project', {
          // [Track B] (#189) pass name + latest SHA so the backend stamps the
          // speleo_link row (is_remote) on the pulled .stalx.
          projectId: summary.id,
          projectName: summary.name,
          latestCommitSha: summary.latest_commit_sha ?? null,
        });
      } catch (e) {
        if (!ownership.latest()) return { status: 'cancelled' };
        const unpushed = unpushedLocalEdits(e);
        if (unpushed) {
          openDialog('speleodb-unpushed-local', {
            summary,
            unpushedLocal: unpushed,
            // Where Cancel goes, so the prompt is not a dead end for a host it
            // displaced in the single dialog slot (#270).
            ...(cancelReturnsTo ? { cancelReturnsTo } : {}),
          });
          return { status: 'unpushed_local' };
        }
        return {
          status: 'error',
          message: humanizeSpeleoError(e, t('speleodb.errors.openFailed')),
          reLogin: speleoErrorAction(e) === 'ReLogin',
        };
      }
      // The link the entry check read is now as old as the download, and the poll
      // underneath has had the whole of it to rewrite the link (see
      // `handBackRefusalAfterPull`). Re-read it BEFORE the seed overwrites it and
      // before the load closes the open project.
      if (!ownership.current()) return { status: 'cancelled' };
      const landed = handBackRefusalAfterPull(summary, result.local_path);
      if (landed) return { status: 'error', message: landed, reLogin: false };
      return finishOpen(result, summary.permission, summary.id, ownership);
    },
    [
      finishOpen,
      handBackRefusal,
      handBackRefusalAfterPull,
      openDialog,
      cancelReturnsTo,
      t,
    ],
  );

  /**
   * [#270] "Keep the local copy" - the safe branch of the refused-open prompt.
   *
   * Deliberately calls NO SpeleoDB command: nothing is downloaded and nothing is
   * overwritten. The local `.stalx` named by the refusal is opened through the
   * SAME tail every successful pull uses (`finishOpen` -> loadProject, with the
   * link seed + rollback), so the two branches can never drift apart.
   *
   * One thing the shared tail must be told, because this branch is NOT a pull:
   * the copy is DIRTY, and we know it. This branch is reachable only because the
   * backend refused the open over edits the server has never seen. The tail's
   * default seed says `hasUnpushedEdits: false`, and that value is exactly what
   * `guardQuit` and the status button's warning read - so seeding it clean
   * disarmed the quit guard for the one project proven to have work at risk,
   * until some later status poll (30 s to a user-configurable 24 h) happened to
   * correct it.
   *
   * The other half - seeding NOTHING when the refusal names the already-open
   * project - is the tail's own job now, since every route into it can name the
   * open project (see `finishOpen`'s `alreadyOpen`).
   *
   * Guarded, but only when it would REPLACE something. The local copy it opens is
   * the TARGET's, so whenever that is a different project this ends in a
   * `loadProject` that closes the one on screen backend-side - and the answer the
   * user gave was about the target's cached copy, not about the work in that
   * project. Between the refusal and this click the status poll can have marked the
   * open project un-pushed or resolved its lock, so the guard has to be re-read here
   * rather than inherited from the `openRemote` that raised the prompt.
   */
  const keepLocalCopy = useCallback(
    async (summary: SpeleoProjectSummary, localPath: string): Promise<OpenRemoteOutcome> => {
      // Opening the path already on screen replaces NOTHING: this branch issues no
      // SpeleoDB command, and `loadProject` short-circuits on ALREADY_OPEN - it flips
      // the view and re-resolves the link, and never re-reads the file, so no unsaved
      // in-memory survey is dropped and no cache `.stalx` is overwritten. Refusing it
      // would raise "save your changes first" over an action that cannot lose them.
      // Decided by PATH, the same comparison `finishOpen` makes for the same reason
      // (`alreadyOpen`) - and unlike the id-based exemption inside the guard, it does
      // not depend on a status poll having landed to record `remoteProjectId`.
      //
      // Literal, for the reason spelled out at `alreadyOpen`: it predicts the
      // backend's own literal already-open test rather than asserting file identity.
      // A spelling that test would not match (symlink, `..`, a case alias) falls to
      // the guarded branch, and that is the CORRECT answer there, not merely the
      // careful one - the backend reloads such a path in full, which is exactly how
      // unsaved in-memory survey gets dropped, so the disk-dirty refusal the guard
      // raises is the one that belongs. Case-folding or collapsing dot segments here
      // would skip the guard for a load that still replaces the project.
      const replacesOpenProject =
        (useProjectStore.getState().project?.file_path ?? null) !== localPath;
      if (replacesOpenProject) {
        const refusal = handBackRefusal(summary);
        if (refusal) return { status: 'error', message: refusal, reLogin: false };
      }
      return finishOpen(
        { local_path: localPath, concurrent_session: false },
        summary.permission,
        summary.id,
        reserveRemoteOpen(),
        { hasUnpushedEdits: true },
      );
    },
    [finishOpen, handBackRefusal],
  );

  /**
   * [#270] "Take the server copy and discard my local changes" - the destructive
   * branch. Re-issues the refused open through the discarding command with exactly
   * the arguments it was given, then runs the same post-open follow-up.
   */
  const takeRemoteDiscardingLocal = useCallback(
    async (
      summary: SpeleoProjectSummary,
    ): Promise<OpenRemoteOutcome> => {
      /**
       * [#270] The same entry guard, re-evaluated HERE - the discarding open is a
       * second project-replacing command, not a continuation of the first one, and
       * the answer it carries is about the TARGET's cached copy, never about the
       * project on screen.
       *
       * The `openRemote` that raised the prompt ran the guard, but the prompt is a
       * user interaction of unbounded length and `SpeleoStatusButton`'s poll (30 s to
       * a user-configurable 24 h) keeps rewriting the link the guard reads while it is
       * up. Same gap the open route had, same fix; every other hand-back exit
       * evaluates its guard immediately before the destructive step.
       *
       * Both halves apply even when the target IS the open project. The hand-back
       * half is exempt there (that is the door this whole branch exists to open, over
       * a cache copy that is un-pushed BY DEFINITION), but the disk-dirty half is not:
       * the command routes through `replace_cache_stalx_abandoning`, which closes the
       * live project WITHOUT saving and installs the downloaded SQLite file, so
       * unsaved in-memory survey goes with it. Nothing is stranded by refusing - the
       * user saves and presses it again.
       */
      const refusal = handBackRefusal(summary);
      if (refusal) return { status: 'error', message: refusal, reLogin: false };
      const ownership = reserveRemoteOpen();
      let result: SpeleoOpenResult;
      try {
        if (!ownership.dispatch()) return { status: 'cancelled' };
        result = await invokeIpc<SpeleoOpenResult>('speleodb_open_project_discarding_local', {
          projectId: summary.id,
          projectName: summary.name,
          latestCommitSha: summary.latest_commit_sha ?? null,
        });
      } catch (e) {
        if (!ownership.latest()) return { status: 'cancelled' };
        return {
          status: 'error',
          message: humanizeSpeleoError(e, t('speleodb.errors.openFailed')),
          reLogin: speleoErrorAction(e) === 'ReLogin',
        };
      }
      // Same download, same window, same re-read - this verb is `openRemote`'s pull
      // with the guard waived for the TARGET's cache copy, which says nothing about
      // the project on screen. See `handBackRefusalAfterPull`.
      if (!ownership.current()) return { status: 'cancelled' };
      const landed = handBackRefusalAfterPull(summary, result.local_path);
      if (landed) return { status: 'error', message: landed, reLogin: false };
      return finishOpen(result, summary.permission, summary.id, ownership);
    },
    [
      finishOpen,
      handBackRefusal,
      handBackRefusalAfterPull,
      t,
    ],
  );

  return { openRemote, keepLocalCopy, takeRemoteDiscardingLocal };
}
