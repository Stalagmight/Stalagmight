import { create } from 'zustand';
import type {
  DoneResult,
  KeychainReport,
  Permission,
  PushResult,
  RemoteLinkBits,
  RemoteState,
  SpeleoDumpCandidate,
  SpeleoErrorDto,
  SpeleoLinkStatus,
  SpeleoLoginResult,
  SpeleoLogoutReport,
  SpeleoProjectSummary,
  SpeleoSessionStatus,
  SpeleoSignOutLocks,
  StartEditingResult,
  TokenStorage,
  UndoMode,
  UpdateResult,
} from '../types/ipc.generated';
import { invokeIpc, getErrorMessage } from '../utils/ipc';
import {
  trackAction,
  setDwellState,
  checkDialogCancel,
  trackDialogComplete,
} from '../utils/telemetry';
import { emailsMatch } from '../utils/email';
import { TOAST_KEYS } from '../utils/toastKeys';
import i18n from '../i18n';
import { useProjectStore } from './projectStore';
import { expectedOpenProject } from './expectedOpenProject';
import { useUIStore } from './uiStore';
import { registerSpeleoStore } from './speleoStoreHandle';

/** [#258] Which ambiguity the picker is resolving. Both share the mechanics and
    differ only in the verb the user is offered. */
export type DumpPickerKind = 'stalx' | 'ariane';

/** Telemetry key for the picker overlay. Must match the registry entry. */
const DUMP_PICKER_DIALOG = 'speleodb-dump-picker';

/**
 * Hand dwell back after the picker closes.
 *
 * Not `tool.<activeTool>` the way `uiStore.closeDialog` does it: this overlay
 * layers OVER other dialogs, so when one is still open underneath, dwell belongs to
 * it and not to the map tool nobody has returned to yet.
 */
function restoreDwellUnderOverlay(): void {
  const ui = useUIStore.getState();
  setDwellState(ui.activeDialog ? `dialog.${ui.activeDialog}` : `tool.${ui.activeTool}`);
}

/**
 * Shared SpeleoDB frontend state (#204/#203/#205). Holds the backend **session**
 * (logged-in identity, shared by the Settings tab + project browser) and the
 * open project's **link/edit** state (the action shell's read-only/editing
 * indicator + control gating).
 *
 * This store is UI-truth seeded from backend results (`speleodb_link_status`,
 * the hand-off verb results); it never contradicts the backend edit gate - the
 * `EDIT_DISABLED` mutation rejection (see `utils/editErrors.ts`) remains the hard
 * backstop. `link === null` means the shell renders nothing; whether that is
 * because the project is local or because nothing is established yet is carried
 * by `projectLinkKind`, and only the former is editable (#270).
 */

/** A remote project is either opened read-only or actively edited (lock held). */
export type SpeleoEditState = 'read-only' | 'editing';

/**
 * Whether the OPEN project is a SpeleoDB project, as a tri-state (#270).
 *
 * `link === null` used to carry two very different meanings - "confirmed local
 * project" and "we have not established what this project is yet" - and the
 * read-only selector read both as editable. That is a permission gate defaulting
 * OPEN: between `setView('single')` and the first `speleodb_link_status` reply,
 * a locked remote project rendered every edit affordance enabled, and the user
 * could start an action the backend then silently refused.
 *
 * So the unresolved case gets its own value and is treated as read-only:
 * - `unknown` - no project open, or one is open whose remoteness has not been
 *   established. Pessimistic: read-only.
 * - `local` - confirmed not a SpeleoDB project. Fully editable via this path.
 * - `remote` - a SpeleoDB project; `link` carries the lock/edit state.
 *
 * It is resolved SYNCHRONOUSLY at project-open time (`syncProjectLink`, fed by
 * the load result's `isRemote` - the very verdict that load set the BACKEND edit
 * gate from, read off the same `EXISTS(speleo_link)` row `speleodb_link_status`
 * reads), so a local project is never briefly read-only, a remote one is never
 * briefly editable, and the two gates cannot disagree.
 */
export type ProjectLinkKind = 'unknown' | 'local' | 'remote';

/** Which hand-off verb hit a divergence that needs the reconcile prompt (#198).
    `reverify` is the lazy 20-min lock re-verify (#202) catching a lost lock /
    moved tip mid-edit-session. */
export type DivergenceOrigin = 'start' | 'update' | 'done' | 'push' | 'reverify';

/** A banner the shell surfaces above the toolbar (#200/#204). `push_unconfirmed`
    and `concurrent_session` are transient action feedback; `rendering_disabled`
    (server `exclude_geojson`) and `no_anchor` (no georeferenced zero datum on
    push/unlock) are the #200 non-blocking notices; `push_failed` (#260) is the
    durable surface for a push/unlock that threw (the toast alone auto-clears
    after 3 s, leaving a failed upload with no trace). `lock_lost` (#576 B35(e))
    is the backend switching editing off because SpeleoDB says the write lock this
    session was editing under is gone or held by `holder`. */
export type SpeleoBanner =
  | { kind: 'push_unconfirmed' }
  | { kind: 'concurrent_session' }
  | { kind: 'rendering_disabled' }
  | { kind: 'no_anchor' }
  | { kind: 'lock_lost'; holder: string | null }
  | PushFailedBanner;

/**
 * [#576 B35(e)] Did this status reply end an edit session because the lock is not
 * ours any more? The backend closes the edit gate when a SUCCESSFUL read names
 * another holder or none (the status poll, or Done refusing to release); this is
 * the frontend half that says so instead of silently turning the toolbar
 * read-only.
 *
 * Read from the transition (we were editing under a lock, the backend now reports
 * the gate shut) plus what the server answered (`permission` is only set when the
 * read succeeded - a signed-out or offline reply carries none). Two shapes qualify:
 * another account named as holder, which no release of OURS produces; and no
 * holder over un-pushed edits, which is worth saying however it came about, since
 * those edits now need Start editing to reach the server. A clean copy whose lock
 * simply went away is left to the read-only toolbar - that is also exactly what a
 * deliberate release looks like.
 */
export function lockLostHolder(
  prior: Pick<SpeleoLinkState, 'editState' | 'forceEditing'> | null,
  s: SpeleoLinkStatus,
  myEmail: string | null,
): { lost: boolean; holder: string | null } {
  const wasHoldingLock = prior?.editState === 'editing' && !prior.forceEditing;
  if (!wasHoldingLock || !s.is_remote || s.edit_enabled || s.force_editing || s.permission === null) {
    return { lost: false, holder: null };
  }
  const holder = s.lock_holder;
  const holderIsOther = !!holder && !emailsMatch(holder, myEmail);
  const lost = holderIsOther || (holder === null && s.has_unpushed_edits);
  return { lost, holder: lost ? holder : null };
}

/** [#260] The last push/unlock failed. Carries the already-humanized error plus
    enough context for the banner's Retry to re-issue the SAME verb with the SAME
    commit message the user typed. */
export interface PushFailedBanner {
  kind: 'push_failed';
  /** Humanized error line from the failed attempt (shown in the banner). */
  message: string;
  /** Which verb failed, so Retry re-runs push (keep the lock) vs done (release it). */
  origin: 'push' | 'done';
  /** The commit message that attempt carried, replayed verbatim by Retry. */
  pushMessage: string | null;
}

/** A SpeleoDB project whose write lock the current user holds, surfaced by the
    login/app-open scan so the resume/release prompt can offer to release it (#201). */
export interface HeldLock {
  projectId: string;
  projectName: string;
}

export interface SpeleoSession {
  loggedIn: boolean;
  userEmail: string | null;
  /** The server this session is authenticated against (a change to the config
      endpoint only affects future logins, so we show where we're actually signed in). */
  endpoint: string | null;
  /** False when the token is held in memory only (won't survive a restart). */
  keychainBacked: boolean;
  /**
   * WHICH tier the API key actually lives in; `null` when signed out (there is no
   * key to place). `keychainBacked` only answers "durable?", which cannot tell a
   * file on disk from a session-only token - and those two have very different
   * consequences for someone deciding whether to worry.
   */
  storage: TokenStorage | null;
  /**
   * What is known about the OS keychain, learned only from access we already had
   * to make. `not_attempted` is a real answer we show: probing a locked Linux
   * keyring is itself what raises the desktop unlock prompt, so a Settings panel
   * that probed to draw a status line would re-create the nag it reports on.
   */
  keychain: KeychainReport;
}

/**
 * [#303] "This copy is dirty ONLY because a chain step walked it back" - the question the
 * push routes, the close prompt and the status button all ask.
 *
 * Derived here rather than reported pre-combined by the backend, because the two halves
 * arrive at different times: a step onto the tip leaves the copy CLEAN, so the composite
 * was false at the moment of the poll, and the Undo that follows sets `hasUnpushedEdits`
 * from its own answer (`remote_link`, #576 D22) before the next poll. Composing locally is what makes the
 * status button react to that Undo instead of waiting a poll interval - which is exactly
 * how it was still offering "Push & release" after an unwind.
 */
export function isUnwoundNotEdited(link: SpeleoLinkState | null | undefined): boolean {
  return !!link && link.unwound && link.hasUnpushedEdits;
}

export interface SpeleoLinkState {
  permission: Permission | null;
  editState: SpeleoEditState;
  /** Email of the current lock holder (may be us, or another user), if known. */
  lockHolder: string | null;
  /**
   * Someone ELSE holds the write lock. Derived once here rather than compared
   * against our email in every consumer: the backend enforces hold-lock ⟹
   * edit-enabled, so a read-only project with a holder is held by another - even
   * when that holder's email (or our own) couldn't be resolved.
   */
  lockedByOther: boolean;
  hasUnpushedEdits: boolean;
  /**
   * [#303] A chain step has moved this copy off the server tip - an Unwind (or a forward
   * step) is mid-walk. RAW, not combined with `hasUnpushedEdits`: use
   * {@link isUnwoundNotEdited} for the question the UI actually asks.
   */
  unwound: boolean;
  /**
   * [#213] We are editing this remote project WITHOUT the write lock (force-edit):
   * `editState === 'editing'` but no lock is held, so the `.stalx` cannot be merged
   * until we reconnect and acquire the lock. Drives the persistent big-red banner
   * and routes the status button / lock menu to Reconnect (the existing
   * `startEditing`) instead of Done. Transient (backend-derived; resets on
   * open/close/reconnect).
   */
  forceEditing: boolean;
  /** [#303] This project's explicit undo-history policy. */
  undoMode: UndoMode;
  /**
   * [#213] The `SpeleoDbConfig.allow_force_edit` opt-in (a global preference,
   * mirrored per-poll onto the link). Gates whether the read-only "Edit without
   * lock" affordance appears at all; false hides it and the backend also refuses.
   */
  forceEditAllowed: boolean;
  /** The local copy's relationship to the server tip (drives the button, T2c). */
  remoteState: RemoteState;
  /**
   * The project is linked but the server refused this user access to it (a 403) -
   * e.g. a managed .stalx shared as a bare file. Drives the button's red-
   * exclamation "no permission" state, whose only action is offering to convert
   * the file to a plain unmanaged local .stalx (fix 10). Distinct from an offline
   * miss (which leaves this false), so we never nag a merely-offline user.
   */
  noAccess: boolean;
  /**
   * An UNEXPECTED server error hit while probing the link (a status that is not the
   * handled 401 dead-token / 403 access-denied, and not a transient offline miss -
   * e.g. a 404 for a deleted project, a 422, or a 5xx). Drives the button's yellow
   * "?" state, whose click opens a dialog with this detail. `null` on success /
   * offline / the handled auth cases (see backend `speleodb_link_status`).
   */
  serverError: SpeleoErrorDto | null;
}

/**
 * A staged login intent (A2): entered in the Settings account panel but applied
 * only when the dialog's Save is pressed. Credentials are transient - cleared on
 * apply, cancel, or dialog close (never persisted).
 */
export interface PendingLogin {
  kind: 'login';
  email: string;
  password: string;
  token: string;
  useToken: boolean;
}

export type PendingAuth = PendingLogin | { kind: 'logout' } | null;

/** [#576 SIGNOUT-PROMPT-D1] The sign-out prompt's answers. When it offers nothing to
 *  release (D2), its "Sign out" answers `release`: the sign-out runs as it always
 *  did, and the un-pushed-edits veto keeps those locks. */
export type SignOutChoice = 'release' | 'keep' | 'cancel';

/**
 * [#576 SIGNOUT-PROMPT-D1] The sign-out prompt on screen: the write locks this
 * session holds, asked about before a logout (`logout`) or before a sign-in as
 * another account or server replaces the session (`switch`). `releasable` are the
 * locks the sign-out would hand back; [#576 SIGNOUT-PROMPT-D2] `unpushed` those it
 * keeps whatever the answer, because the project has edits not on SpeleoDB yet;
 * [#576 SDB-2MACHINE-D1] `in_use_elsewhere` those this session adopted from the
 * account and made no edit under, kept unless the user picks Release.
 */
export interface SignOutPrompt extends SpeleoSignOutLocks {
  kind: 'logout' | 'switch';
}

/** The pending `applyPendingAuth` waiting on the sign-out prompt's answer. */
let signOutPromptAnswer: ((choice: SignOutChoice) => void) | null = null;

/**
 * Ask before a sign-out when this session holds any lock - one to release, one
 * kept for un-pushed edits (D2), or one in use elsewhere (SDB-2MACHINE-D1) - and
 * wait for the answer; with none, `release` (nothing to ask). A prompt raised over
 * one still unanswered answers that one Cancel, so no sign-out is left waiting
 * forever.
 */
function askSignOut(
  set: (patch: { signOutPrompt: SignOutPrompt | null }) => void,
  kind: SignOutPrompt['kind'],
  locks: SpeleoSignOutLocks,
): Promise<SignOutChoice> {
  if (locks.releasable.length === 0 && locks.unpushed.length === 0 && locks.in_use_elsewhere.length === 0) {
    return Promise.resolve('release');
  }
  signOutPromptAnswer?.('cancel');
  return new Promise((resolve) => {
    signOutPromptAnswer = resolve;
    set({
      signOutPrompt: {
        kind,
        releasable: locks.releasable,
        unpushed: locks.unpushed,
        in_use_elsewhere: locks.in_use_elsewhere,
      },
    });
  });
}

/**
 * [#270] The link state an optimistic open seed replaces - everything
 * `setLinkOnOpen` writes, so restoring one is a complete undo of a seed. Captured
 * by {@link SpeleoStore.snapshotLink} and put back by
 * {@link SpeleoStore.restoreLink}.
 */
/**
 * The session a request is being issued under, to be checked again when its reply
 * lands. [#258]
 *
 * Ambiguity is a ROUTING signal, so the catch that receives it opens a dialog rather
 * than reporting anything - and every verb that can raise the picker makes a call of
 * unbounded length (a dump download) first. A 401 auto-drop on any concurrent call ends
 * the session underneath it, so the reply can arrive after `clearSession` has already
 * torn the surface down; raising the picker then puts a dialog on screen for an account
 * nobody is signed in to, where Confirm can only fail and a flow suspended on the
 * question waits for a manual cancel.
 *
 * Shared rather than repeated because it was repeated, and the copies did not agree:
 * the three open verbs had it and the three revision-family verbs did not.
 */
export function speleoSessionIssuedAt(): number {
  return useSpeleoStore.getState().sessionGeneration;
}

/**
 * Whether the session that issued a request is still the one signed in.
 *
 * `sessionGeneration` rather than `session` identity, for the reason the store
 * documents: it moves on a login or an ending and NOT on the benign refreshes a project
 * load and Settings both trigger, so an identity check would fire on those and suppress
 * a picker that should have opened.
 *
 * `!== false`, not `=== true`: a session that is merely ABSENT is not evidence of a
 * logout, it is a store that has not been seeded yet, and demanding one would make
 * raising the picker conditional on a status poll having landed first. Only a positive
 * `loggedIn: false` says the session ended under us.
 */
export function speleoSessionStillCurrent(issuedAt: number): boolean {
  const { sessionGeneration, session } = useSpeleoStore.getState();
  return sessionGeneration === issuedAt && session?.loggedIn !== false;
}

/**
 * Whether the project a request was issued FOR is still the one open. [#258]
 *
 * The companion to {@link speleoSessionStillCurrent}, for the verbs that act on the
 * OPEN project - restore, View, revert. Their session can be perfectly current while
 * the project underneath has changed: start a View, dismiss the SpeleoDB dialog, open
 * something else, and the download returns to raise project A's picker over project B,
 * whose retry the backend then refuses because a revision swap binds to what is open.
 *
 * Deliberately NOT applied to the open verbs - they exist to change which project is
 * open, so for them a mismatch is the goal rather than the hazard.
 */
export function speleoOpenProjectStillIs(projectId: string | null): boolean {
  return useSpeleoStore.getState().remoteProjectId === projectId;
}

export interface SpeleoProjectSessionIdentity {
  sessionGeneration: number;
  projectId: string | null;
}

/** Capture both identities a long-running open-project request is allowed to affect. */
export function speleoProjectSessionIssuedFor(
  projectId: string | null,
): SpeleoProjectSessionIdentity {
  return { sessionGeneration: speleoSessionIssuedAt(), projectId };
}

/** The shared guard for requests that bind to both the login and the open project. */
export function speleoProjectSessionStillCurrent(
  issuedFor: SpeleoProjectSessionIdentity,
): boolean {
  return (
    speleoSessionStillCurrent(issuedFor.sessionGeneration) &&
    speleoOpenProjectStillIs(issuedFor.projectId)
  );
}

export interface SpeleoLinkSnapshot {
  link: SpeleoLinkState | null;
  projectLinkKind: ProjectLinkKind;
  linkPath: string | null;
  remoteProjectId: string | null;
  divergence: DivergenceOrigin | null;
  banner: SpeleoBanner | null;
}

export interface SpeleoStore {
  /** Backend session, or `null` before the first `setSession`/`refreshSession`. */
  session: SpeleoSession | null;
  /**
   * Which session this IS, as a counter bumped on every identity change - a login
   * (`setSessionFromLogin`) or an ending (`clearSession`). Deliberately NOT bumped
   * by `setSession`/`refreshSession`, which only re-read the session already held.
   *
   * For staleness guards on long IPC calls that outlive the account they were made
   * for. Neither of the two things already on `session` can do that job:
   * `userEmail` cannot tell a re-login as the same account from the original
   * session (a different token, its own storage outcome), and the object identity
   * changes on every benign refresh - a project load or reopening Settings both
   * call `refreshSession`, so an identity check would read those as an account
   * change and suppress a guard that should have fired.
   *
   * State rather than a module-scoped counter like `busySeq`/`linkStatusSeq`,
   * because the reader is a COMPONENT (`SpeleoStorageChoiceDialog`), not the store
   * itself. Nothing renders it.
   */
  sessionGeneration: number;
  /**
   * [#303] Version of the backend state used to resolve the next Unwind/Rewind target.
   * Unlike `sessionGeneration`, this moves for link-only operations such as a push or
   * undo-policy change, where the open project and its rendered tree stay the same.
   */
  chainTargetGeneration: number;
  /** The open project's link/edit state, or `null` when it is local / none open. */
  link: SpeleoLinkState | null;
  /**
   * Is the open project a SpeleoDB project? Tri-state, so "not established yet"
   * is distinguishable from "confirmed local" - see `ProjectLinkKind`. Read by
   * `selectSpeleoReadOnly`, which treats `unknown` as read-only.
   */
  projectLinkKind: ProjectLinkKind;
  /**
   * Which file's `.stalx` the current `link` describes, so a leftover link can be
   * told from one seeded for the project now being opened. `null` when no link is
   * seeded, or when it was seeded by a path-less caller.
   */
  linkPath: string | null;
  /**
   * [#218] The open remote project's SpeleoDB project id (from the poll'd status),
   * so the in-project menu can open its revision history. `null` for a local project
   * / none open / before the first status seed.
   */
  remoteProjectId: string | null;
  /** Set when a hand-off verb reported divergence; consumed by #198 (stub for now). */
  divergence: DivergenceOrigin | null;
  /** Transient banner (#200 seam). */
  banner: SpeleoBanner | null;
  /**
   * Lock holder per accessible SpeleoDB project (`project_id` → holder email, or
   * `null` = free), from the single login/app-open `list_projects` scan (#201).
   * Overlays lock badges onto the recents / GlobalView list; empty until the first
   * scan (logged out / offline).
   */
  remoteLocks: Record<string, string | null>;
  /**
   * SpeleoDB project NAME per accessible project (`project_id` → server-side name),
   * from the same login/app-open scan (#201). The recents list shows this
   * ("SpeleoDB: {name}") in place of the local path, since the server name can
   * differ from the local survey name. Empty until the first scan.
   */
  remoteProjects: Record<string, string>;
  /**
   * The full project summary per accessible project (`project_id` → summary), from
   * the same scan (#218). Retains permission / type_tag / latest_commit_sha so
   * GlobalView can list purely-remote projects (not in local recents) and Open them
   * without a second `list_projects` round trip. Superset of `remoteLocks` /
   * `remoteProjects`; empty until the first scan (logged out / offline).
   */
  remoteSummaries: Record<string, SpeleoProjectSummary>;
  /**
   * Locks the current user holds, found by the scan (#201). Populates the
   * resume/release prompt's list; a login/app-open scan that finds any also bumps
   * `heldLockPromptNonce` to raise the prompt.
   */
  heldLocks: HeldLock[] | null;
  /**
   * Visibility of the held-lock resume/release prompt. Set true only by a
   * PROMPT-worthy scan (login, or the first app-open scan) that finds held locks -
   * never by a routine badge refresh (returning to recents, the refresh button),
   * which is what stops it re-appearing on every visit. App renders the prompt as
   * an independent overlay keyed off this (NOT via the single `activeDialog` slot),
   * so it layers OVER an open dialog (e.g. Settings during an inline login) instead
   * of replacing it - the underlying dialog stays mounted and intact.
   */
  heldLockPromptOpen: boolean;
  /**
   * True until the first lock scan of the app session runs. Lets that first scan
   * count as PROMPT-worthy ("application first started") without a second GET,
   * then it stays false so later badge refreshes don't re-prompt.
   */
  heldLockPromptPending: boolean;
  /**
   * Bumped after a hand-off verb to restart the status poll timer, so the next
   * poll is a full (≥30 s) interval away - a buffer that avoids re-reading a
   * server mutex that hasn't reflected our just-issued release yet (1b).
   */
  pollNonce: number;
  bumpPoll: () => void;
  /**
   * True while a hand-off verb (start/update/push/done) is in flight. Shared so
   * every entry point - the status button AND the hamburger submenu - gates on
   * the same lock and can't double-issue (fix 12).
   *
   * Consumers only READ it; every production write goes through
   * {@link beginBusy} / {@link endBusy} rather than setting the boolean, so a
   * late release cannot clear a flag it no longer owns (see {@link beginBusy}).
   */
  busy: boolean;
  /**
   * [#270] Claim `busy` for ONE verb, returning the token its release must
   * present.
   *
   * `busy` is a single shared boolean but has three writers: the verb's own
   * `finally`, `clearLink` (project close/switch) and `restoreLink` (a failed
   * remote open). The latter two reset it out of band, so the verb's `finally`
   * would later run against a flag some NEWER verb had since raised - and clear
   * it. That is not cosmetic: `SpeleoQuitDialog` / `SpeleoCloseDialog` refuse to
   * close while busy (`onClose={busyAny ? () => {} : closeDialog}`) precisely
   * because dismissing mid-push lets the in-flight `pushReleaseThenQuit` resolve
   * `released` and call `performQuit()`, whose one-shot flag makes the resulting
   * close skip `guardQuit` entirely. A stale clear re-opens exactly that door.
   *
   * So releases are ownership-checked: whoever claims last owns the flag, and an
   * older token's release is a no-op.
   */
  beginBusy: () => number;
  /** Release `busy` IF `token` still owns it; a stale token is a no-op. */
  endBusy: (token: number) => void;

  // --- session (Settings #205 / browser #203) ---
  setSession: (s: SpeleoSessionStatus) => void;
  setSessionFromLogin: (r: SpeleoLoginResult) => void;
  clearSession: () => void;
  /** Query `speleodb_session` and set it (best-effort; runs on project load so the
      action button knows who we are - e.g. to tell our own lock from another's). */
  refreshSession: () => Promise<void>;

  // --- credential storage (items 1/11/12) ---
  /**
   * The "where should your API key go?" prompt raised by a login whose keychain
   * write failed (`needs_storage_choice`).
   *
   * An INDEPENDENT overlay flag rather than the single `activeDialog` slot, for
   * the reason the held-lock prompt is (#201): the login that raises it is
   * usually made from inside the Settings account panel, and taking the
   * `activeDialog` slot would unmount Settings - discarding every other setting
   * the user had staged but not yet saved.
   */
  storageChoiceOpen: boolean;
  /** Dismiss the storage prompt WITHOUT choosing. Deliberately persists nothing:
      the session keeps working in memory and the question can be asked again on
      the next login, which is the safe direction for a prompt someone escaped out
      of rather than answered. */
  closeStorageChoice: () => void;

  /** [#258] The ambiguous-dump picker: the candidates a rejected pull offered, and
   *  the pending call to replay once the user has chosen.
   *
   *  An INDEPENDENT overlay for the same reason as the storage prompt above, plus
   *  one of its own: this question is raised from FOUR unrelated surfaces (the
   *  Open-from-SpeleoDB browser, GlobalView, the refused-open prompt, and the
   *  revision history inside the project dialog), and three of those already
   *  occupy the single `activeDialog` slot themselves. */
  dumpPickerOpen: boolean;
  /** The entries the backend offered, already ordered canonical-first. */
  dumpPickerCandidates: SpeleoDumpCandidate[];
  /** Which question is being asked - it changes the verb, not the mechanics. */
  dumpPickerKind: DumpPickerKind;
  /** A resolve is in flight, so the dialog must not be dismissed out from under it. */
  dumpPickerBusy: boolean;
  /** Why the last confirmation failed, rendered inside the picker. The retry runs
      from the DIALOG, not from a host, so a rejection has nowhere else to surface -
      it used to escape as an unhandled rejection and leave the picker open saying
      nothing. */
  dumpPickerError: string | null;
  /** Raise the picker for `candidates`, replaying `retry` with the chosen entry. */
  openDumpPicker: (
    candidates: SpeleoDumpCandidate[],
    kind: DumpPickerKind,
    retry: (entryPath: string) => Promise<void>,
    onCancel?: () => void,
    /**
     * WHICH flow is asking, for the hosts that suspend on the answer.
     *
     * A raise either CONTINUES the flow already suspended - a retry meeting a fresh
     * ambiguity re-asks the same question of a moved tree - or DISPLACES a different
     * flow, whose cancel is then the only thing that would ever have released it. The
     * two need opposite treatment and only the caller knows which it is.
     *
     * Timing cannot answer it, which is what the first attempt got wrong: a flag held
     * for the duration of an awaited retry marks every picker raised anywhere in that
     * window as a continuation, so a View retry re-offering while a Revert picker was
     * up suppressed the Revert's cancel and stranded it. Identity does answer it.
     *
     * Omitted by the flows that suspend on nothing - the open verbs pass no `onCancel`
     * either, so there is nothing to strand and nothing to preserve.
     */
    owner?: string,
  ) => void;
  /** Dismiss WITHOUT choosing. Nothing was downloaded over and nothing was
      replaced - the classify step runs before the cache is touched - so cancelling
      is genuinely free and the question can simply be asked again. */
  closeDumpPicker: () => void;
  /** Re-run the pending call with the chosen entry, then close. Left OPEN on
      failure so the choice is not silently lost; the caller surfaces the error. */
  resolveDumpPicker: (entryPath: string) => Promise<void>;
  /** The pending call, held so the dialog itself stays presentational. Not part of
      the picker's public surface - callers go through `openDumpPicker`. */
  dumpPickerRetry: ((entryPath: string) => Promise<void>) | null;
  /** Run when the picker goes away WITHOUT an answer - dismissed, or dropped by a
      lost session. It lives here rather than as an effect in each dialog because
      the session-loss reset below also has to fire it: a caller that suspended a
      flow on this question is left with a modal it cannot dismiss otherwise. */
  dumpPickerCancel: (() => void) | null;
  /**
   * Which flow the open picker belongs to, or `null` for one that suspends nothing.
   * Compared against an incoming raise's owner to tell a continuation from a
   * displacement - see {@link SpeleoStore.openDumpPicker}.
   */
  dumpPickerOwner: string | null;
  /** Answer the storage question: persist the tier + the settled refusal, and move
      the held token there (no re-login - the backend still has it).
      Resolves the tier the key ACTUALLY LANDED ON, which is not always the one that
      was asked for: a plaintext write onto a full or read-only filesystem falls back
      to session-only memory, and a caller that reports the request instead promises a
      file the user will not have on the next launch. `null` on failure, which leaves
      the prompt open so the choice isn't silently lost. */
  setTokenStorage: (mode: 'session_only' | 'plaintext') => Promise<TokenStorage | null>;
  /** Settings' "Try again": lift the refusal and re-store the held token, which now
      reaches the keychain again. Resolves false on failure. */
  retryKeychain: () => Promise<boolean>;

  // --- lock scan on login/app-open (#201, B1) ---
  /** One `list_projects` GET resolves lock state for ALL accessible projects:
      populates `remoteLocks` (badges) + `heldLocks` (the resume/release prompt).
      `prompt: true` (login) forces a prompt-worthy scan; the first app-open scan is
      prompt-worthy via `heldLockPromptPending`. Best-effort; a logged-out / offline
      scan clears the badges. */
  scanLocksOnOpen: (opts?: { prompt?: boolean }) => Promise<void>;
  /** Release ONE held lock by project id (the prompt's per-project "Release"); on
      success drops it from `heldLocks`, marks it free in `remoteLocks`, and refreshes
      the open project's link status. Throws on failure so the prompt can surface it. */
  releaseLock: (projectId: string) => Promise<void>;
  /** Dismiss the held-lock prompt without a server call (Do nothing / keep working). */
  clearHeldLocks: () => void;
  /** Hide the held-lock prompt overlay (its close paths use this, NOT the shared
      `closeDialog`, so it never closes the dialog it is layered over). */
  closeHeldLockPrompt: () => void;

  // --- staged account actions (Settings #205, A2/A3) ---
  /** The login/logout the user staged; applied on the dialog's Save. */
  pendingAuth: PendingAuth;
  /** Error from the last apply-on-save (already humanized), shown in the panel. */
  authError: string | null;
  setPendingAuth: (p: PendingAuth) => void;
  clearPendingAuth: () => void;
  setAuthError: (msg: string | null) => void;
  /** Apply the staged login/logout (A2). ok=false carries the raw error, or
      `cancelled` when the user cancelled the sign-out prompt (nothing happened,
      and the staged change is still staged). */
  applyPendingAuth: (
    endpoint: string | null,
  ) => Promise<{ ok: boolean; error?: unknown; cancelled?: boolean }>;
  /**
   * [#576 SIGNOUT-PROMPT-D1] The sign-out prompt `applyPendingAuth` is waiting on,
   * or null. An INDEPENDENT overlay, like the storage prompt: it is raised from the
   * Settings account panel, which must survive a Cancel with what it staged.
   */
  signOutPrompt: SignOutPrompt | null;
  /** Answer the sign-out prompt (its buttons; Escape, X and the backdrop cancel). */
  answerSignOutPrompt: (choice: SignOutChoice) => void;

  // --- link/edit (action shell #204) ---
  /**
   * Resolve the open project's remoteness SYNCHRONOUSLY from the load result's
   * `isRemote`, BEFORE the project view renders (#270).
   *
   * This is the render-order half of the fix: `refreshLinkStatus` is fired
   * unawaited after `setView('single')`, so the view used to paint with
   * `link === null` for a beat. Awaiting it instead is not an option -
   * `speleodb_link_status` is only PART local: for a remote project it also does
   * best-effort `get_project` + `get_revisions` HTTP round trips, so awaiting it
   * would stall every remote open behind the network.
   *
   * - remote: seeds a pessimistic read-only link if none is seeded yet, so the
   *   project is read-only from the first frame AND the status button still
   *   renders its way out ("Start editing"), even if the status call never
   *   answers. `refreshLinkStatus` refines it.
   * - local: drops any link left over from a previously-open remote project
   *   (same shape as `seedFromStatus` on a non-remote status), so a local
   *   project is fully editable from the first frame with no read-only flash.
   *
   * `forPath` is the file being loaded, and it is what makes "keep an
   * already-seeded link" safe. Without it, the remote branch kept ANY existing
   * link - including the previous project's - so opening remote project B while
   * A's link was live rendered B with A's lock holder, A's permission and A's
   * dirty bit (File > Open never cleared it, and `refreshLinkStatus` swallows its
   * errors, so offline the wrong state simply persisted). The link is kept only
   * when it was seeded FOR this path: the optimistic `setLinkOnOpen` seed
   * immediately before the load, or a reload of the same file (fast-forward /
   * revision swap).
   */
  syncProjectLink: (isRemote: boolean, forPath?: string | null) => void;
  /** Query `speleodb_link_status` and seed the link (best-effort; runs post-load). */
  refreshLinkStatus: () => Promise<void>;
  /** Seed the link from the backend status command (authoritative; runs post-load). */
  seedFromStatus: (s: SpeleoLinkStatus) => void;
  /**
   * Optimistic seed from the browser's summary before `loadProject` settles.
   * `forPath` is the local `.stalx` about to be loaded, so `syncProjectLink` can
   * tell this fresh seed from a previous project's leftover link.
   *
   * `hasUnpushedEdits` defaults to false because the seed's original caller is a
   * fresh PULL, which by construction has nothing local the server has not seen.
   * It is an explicit parameter (#270) because that is no longer the only caller:
   * "Keep the local copy" reuses this same tail while downloading NOTHING, and is
   * reachable only because the backend refused the open over un-pushed local
   * edits - so for it the dirty bit is KNOWN true. Hardcoding false there seeded
   * "nothing outstanding" for the one project we have proof is dirty, which
   * disarmed the quit guard (`guardQuit` reads exactly this flag) and dropped the
   * status button's unpushed warning until some later poll happened to correct it.
   */
  setLinkOnOpen: (a: {
    permission: Permission | null;
    forPath?: string | null;
    hasUnpushedEdits?: boolean;
    /**
     * WHICH SpeleoDB project this seed is for.
     *
     * The seed used to omit it, leaving `remoteProjectId` to the status poll - which
     * meant that after opening remote B from remote A the store still named A for as
     * long as a poll took (30 s to a user-configurable 24 h). Every consumer keyed on
     * it read the wrong project in that window, and the guards that compare against it
     * could not notice, because the id they were handed came from the same stale value.
     * The pull knows the answer at the call site; there is no reason to wait to be told.
     */
    projectId?: string | null;
  }) => void;
  clearLink: () => void;
  /**
   * [#270] Capture exactly the fields an optimistic open seed overwrites, so a
   * failed load can put back what was on screen instead of clearing to `unknown`.
   * See {@link SpeleoStore.restoreLink}.
   */
  snapshotLink: () => SpeleoLinkSnapshot;
  /**
   * [#270] Undo an optimistic seed by restoring a {@link SpeleoLinkSnapshot}.
   *
   * The rollback for a remote open whose `loadProject` failed used `clearLink`,
   * which resets `projectLinkKind` to `'unknown'` - read-only under the fail-closed
   * gate. The PREVIOUS project is still on screen and still open backend-side, so
   * that silently made a project the user was editing uneditable. The only repair
   * was an unawaited `refreshLinkStatus()` whose failures are swallowed by design,
   * in exactly the conditions that had just broken a load (offline, disk trouble),
   * and `SpeleoStatusButton`'s recovery poll is gated on `link !== null`, which
   * `clearLink` had just falsified - so nothing healed it for the rest of the
   * session, with no badge or message saying why.
   *
   * Restoring the snapshot needs no network and cannot fail: the state goes back to
   * precisely what it was before the attempt. `refreshLinkStatus` still runs after
   * it, but as an opportunistic re-resolve rather than the only way back.
   */
  restoreLink: (snapshot: SpeleoLinkSnapshot) => void;
  applyStartEditing: (r: StartEditingResult) => void;
  applyUpdate: (r: UpdateResult) => void;
  applyDone: (r: DoneResult) => void;
  /** Push-without-release (T2d): keeps the lock; clears the dirty bit on success. */
  applyPush: (r: PushResult) => void;
  /**
   * [#576 D22] The SpeleoDB link bits a survey mutation answered with - the BACKEND's
   * own `has_unpushed_edits` / `unwound` after the edit, an undo/redo, or a failure
   * that still mutated (read from its link row by the one helper that fills every
   * mutation's `remote_link`). Applied by `invokeIpc`, the only caller, so no editing
   * call site re-derives them: the hand-written mirrors this replaced (`markDirty` /
   * `markForwardEdit`) stamped a provable no-op as an un-pushed edit and dropped its
   * Unwind position while the backend stayed clean.
   *
   * `linkPathAtIssue` is the `linkPath` when the command was ISSUED: a reply for a
   * project the user has since left (the link now describes another file) is
   * dropped. No link (a local project, nothing open) is a no-op too. Returns `{}`
   * when the bits already match, so the object identity `refreshLinkStatus` uses to
   * spot a concurrent mutation is preserved; any change bumps
   * `chainTargetGeneration`, since an Unwind/Rewind target resolved before it is stale.
   */
  applyRemoteLink: (bits: RemoteLinkBits, linkPathAtIssue: string | null) => void;

  // --- seams ---
  clearDivergence: () => void;
  /**
   * #202: the lazy 20-min re-verify blocked an edit because the write lock was
   * lost or the server tip moved. Drop the shell to read-only + conflict (the
   * backend already dropped its edit gate) and raise the divergence prompt.
   */
  notifyReverifyDivergence: () => void;
  /** #200: after a push/unlock of a project with no georeferenced zero datum, note
      (non-blocking) that the server generated no new map GeoJSON. Only claims a free
      banner slot - never clobbers transient action feedback. */
  raiseNoAnchorBanner: () => void;
  /** #260: a push/unlock threw. Raise the PERSISTENT failure banner (the toast is
      immediate but auto-clears), carrying the humanized error + the verb/message so
      its Retry can replay the same attempt. Always claims the slot - a failed upload
      outranks the informational notices. */
  raisePushFailedBanner: (b: Omit<PushFailedBanner, 'kind'>) => void;
  /** #260: drop the push-failure banner (a new attempt supersedes the old failure;
      a successful push/release clears the whole slot anyway). No-op for other kinds,
      so it never wipes an unrelated banner. */
  clearPushFailedBanner: () => void;
  /** #200 Enable-rendering: PATCH `exclude_geojson = false` so the server resumes
      web-viewer GeoJSON on the NEXT push (not retroactively). Clears the
      rendering-disabled banner and re-seeds link status. Throws on failure so the
      banner can surface it. */
  enableRendering: () => Promise<void>;
  /** [#303] Set the open project's explicit undo-history policy. LOCK-REQUIRED: the value
      lands in the `.stalx` and rides to the server on the next push, so it becomes
      every collaborator's. Throws on refusal (no lock, or a local project) so the
      control can surface it, and re-seeds link status on success. */
  setUndoMode: (mode: UndoMode) => Promise<void>;
  /** Invalidate any cached Unwind/Rewind target after a successful tree replacement. */
  invalidateChainTargets: () => void;
  dismissBanner: () => void;
}

/**
 * [#260/#270] The push/release outcome kinds, widened by `not_editing`.
 *
 * `speleodb_push_only` / `speleodb_done` answer `NotEditing` when the backend's
 * edit gate was already closed: no lock was held, so NOTHING was uploaded and
 * nothing was released. They used to fold that case into `Pushed` / `Released`,
 * which told the frontend the work had landed - clearing `hasUnpushedEdits`,
 * disarming `guardQuit` and toasting success - over edits that never left this
 * machine. It is reachable in normal use: a 401 auto-drop closes the gate while
 * deliberately KEEPING the dirty bit, and the push-failure banner's Retry then
 * re-issues the verb from that state.
 *
 * The alias exists because the generated `PushResult` / `DoneResult` unions are
 * regenerated from Rust in a separate step, so this file must be able to name the
 * arm before (and after) that lands. Once the arm is in the generated union the
 * alias is a no-op, and any FUTURE variant flows into it and trips the
 * {@link unhandledHandoffKind} `never` check instead of silently taking a
 * success path.
 */
export type PushKind = PushResult['kind'] | 'not_editing';
export type DoneKind = DoneResult['kind'] | 'not_editing';

/**
 * Widen a result kind to {@link PushKind} / {@link DoneKind} for a switch.
 *
 * Functions rather than an annotated `const`: TS narrows an annotated binding
 * back to its initializer's type, which would drop `not_editing` off the union
 * again and make the arm unreachable. A call expression's type is its declared
 * return type, so the widening survives - and so does the `never` check.
 */
export const asPushKind = (kind: PushResult['kind']): PushKind => kind;
export const asDoneKind = (kind: DoneResult['kind']): DoneKind => kind;

/**
 * Whether a Done finished the edit session with the work on the server: `released`
 * (the lock handed back), or [#576 SDB-2MACHINE-D1] `lock_held_elsewhere` - nothing
 * to upload, and the lock, adopted from the account with no edit made here, left
 * with the session using it. The exit prompts and the Done dialog move on for both;
 * every other kind left the work (or the lock) where a retry still needs it.
 */
export function doneFinished(kind: DoneKind | null | undefined): boolean {
  return kind === 'released' || kind === 'lock_held_elsewhere';
}

/**
 * Exhaustiveness guard for the hand-off switches. Reached only if the backend
 * grows a variant the frontend has not been taught, so it is a COMPILE error at
 * the call site and, at runtime, changes no state at all - the conservative
 * answer, since every unknown outcome could be one that did not upload anything.
 */
export function unhandledHandoffKind(kind: never): Record<string, never> {
  console.warn('[speleoStore] unhandled SpeleoDB hand-off result kind:', kind);
  return {};
}

/// Link objects produced by `seedFromStatus`, so `refreshLinkStatus` can tell a
/// rival poll's seed apart from a LOCAL mutation of link truth.
///
/// The staleness guard has to drop a reply whose link changed under it - a push
/// landing inside the poll's two round trips clears the dirty bit, and a reply
/// carrying the pre-push snapshot would resurrect it. But `seedFromStatus`
/// allocates a fresh object every time, so a plain identity check also fired when
/// the change came from another poll: two overlapping refreshes invalidated each
/// other, and the LATER one - the one holding newer truth - was the one thrown
/// away. A `WeakSet` keeps that distinction without a counter every future link
/// mutator would have to remember to bump, and without retaining dead objects.
const seededLinks = new WeakSet<SpeleoLinkState>();

/// Issue order for `refreshLinkStatus`, so that when replies do overlap the newest
/// one wins instead of whichever happened to land last.
let linkStatusSeq = 0;
let linkStatusApplied = 0;

/// [#270] Ownership of the shared `busy` flag: the token of the claim that owns it
/// (0 = nobody). Module-scoped for the same reason `linkStatusSeq` is - it is
/// bookkeeping about in-flight calls, not UI state any component renders.
///
/// `busy` is one boolean shared by every hand-off verb, but `clearLink` (close /
/// project switch) and `restoreLink` (failed remote open) also clear it out of
/// band. Ownership makes the verb's own `finally` a no-op once either of those -
/// or a newer verb - has taken over, instead of clearing a flag that now belongs
/// to something else. See `SpeleoStore.beginBusy` for why that matters.
let busySeq = 0;
let busyOwner = 0;

/// Drop any current claim on `busy`, so a still-running verb's release cannot
/// clear the flag for whatever comes next. Callers pair this with `busy: false`
/// in the same `set`.
const disownBusy = (): void => {
  busyOwner = 0;
};

export const useSpeleoStore = create<SpeleoStore>((set, get) => ({
  session: null,
  sessionGeneration: 0,
  chainTargetGeneration: 0,
  link: null,
  // Pessimistic until a project open resolves it: an unresolved gate is closed.
  projectLinkKind: 'unknown',
  linkPath: null,
  remoteProjectId: null,
  divergence: null,
  banner: null,
  remoteLocks: {},
  remoteProjects: {},
  remoteSummaries: {},
  heldLocks: null,
  heldLockPromptOpen: false,
  heldLockPromptPending: true,
  pollNonce: 0,
  busy: false,
  pendingAuth: null,
  authError: null,

  bumpPoll: () => set((state) => ({ pollNonce: state.pollNonce + 1 })),

  beginBusy: () => {
    busyOwner = ++busySeq;
    set({ busy: true });
    return busyOwner;
  },

  endBusy: (token) => {
    // A stale release: `clearLink` / `restoreLink` disowned this claim, or a newer
    // verb took over. Either way the flag is not ours to clear.
    if (busyOwner !== token) return;
    disownBusy();
    set({ busy: false });
  },

  setPendingAuth: (pendingAuth) => set({ pendingAuth }),
  clearPendingAuth: () => set({ pendingAuth: null, authError: null }),
  setAuthError: (authError) => set({ authError }),

  signOutPrompt: null,
  answerSignOutPrompt: (choice) => {
    const answer = signOutPromptAnswer;
    signOutPromptAnswer = null;
    set({ signOutPrompt: null });
    answer?.(choice);
  },

  applyPendingAuth: async (endpoint) => {
    const p = get().pendingAuth;
    if (!p) return { ok: true };
    try {
      if (p.kind === 'logout') {
        // [#576 SIGNOUT-PROMPT-D1] Ask first when this session holds write locks:
        // release them, keep them held on SpeleoDB, or do nothing at all. No locks,
        // no prompt - the sign-out releases as it always did (nothing to release).
        // [#576 SIGNOUT-PROMPT-D2] Locks kept for un-pushed edits are named too, and
        // when they are all there is, the prompt only says so: Sign out or Cancel.
        const locks = await invokeIpc('speleodb_sign_out_locks');
        const choice = await askSignOut(set, 'logout', locks);
        if (choice === 'cancel') return { ok: false, cancelled: true };
        // #270: the backend drops the session and the persisted account email even
        // when it can't remove the credential from the OS keychain - it rejects only
        // to WARN that a credential may still be stored on this machine. So mirror
        // the logout locally in `finally`, then let the rejection propagate to the
        // catch below so the dialog still surfaces that warning. Clearing only on
        // success would leave the UI claiming a session the backend no longer has.
        let report: SpeleoLogoutReport | null = null;
        try {
          report = await invokeIpc('speleodb_logout', { releaseLocks: choice === 'release' });
        } finally {
          set({ pendingAuth: null, authError: null });
          get().clearSession();
        }
        // [#576 Q-W61-3-D1] Signed out, but a write lock the logout tried to hand back
        // is still held on the server: say which project, and how to release it. A
        // warning toast - the sign-out succeeded with a caveat, and nothing in the app
        // can show the lock once the session is gone.
        const leftHeld = report?.locks_left_held ?? [];
        if (leftHeld.length > 0) {
          useUIStore.getState().showToast(
            i18n.t('settings.speleodb.account.locksLeftHeld', {
              count: leftHeld.length,
              projects: leftHeld.map((left) => `"${left.project_name}"`).join(', '),
            }),
            TOAST_KEYS.SPELEODB_LOGOUT_LOCKS_HELD,
            'warning',
          );
        }
      } else {
        const credentials = {
          endpoint: (endpoint ?? '').trim() || undefined,
          email: p.useToken ? undefined : p.email.trim() || undefined,
          password: p.useToken ? undefined : p.password || undefined,
          token: p.useToken ? p.token.trim() || undefined : undefined,
        };
        // [#576 SIGNOUT-PROMPT-D1] A sign-in as another account (or on another
        // server) signs the live session out first: the same prompt about that
        // session's locks, asked before it is replaced. The backend says which
        // locks, if any - it alone knows whose account a pasted token is.
        const locks = await invokeIpc('speleodb_sign_in_hand_back_locks', {
          endpoint: credentials.endpoint,
          email: credentials.email,
          token: credentials.token,
        });
        const choice = await askSignOut(set, 'switch', locks);
        if (choice === 'cancel') return { ok: false, cancelled: true };
        const r = await invokeIpc('speleodb_login', {
          ...credentials,
          releaseLocks: choice === 'release',
        });
        set({ pendingAuth: null, authError: null });
        // [#576 FZ-SDB-3-D1] The login replaced a live session for another account or
        // server, and the backend ran logout's hand-back for it first (lock released
        // best-effort with the old token, edit gate shut, claim forgotten). Mirror that
        // logout here before the new session lands, then re-seed the link from the
        // backend, so the old session's held lock and edit state do not carry over.
        if (r.replaced_session) get().clearSession();
        get().setSessionFromLogin(r);
        if (r.replaced_session) void get().refreshLinkStatus();
        // [#576 NF-W72-1] Signed in, but a write lock that hand-back tried to release
        // is still held on the server: say which project, and how to release it, as
        // after a logout (Q-W61-3-D1). A warning toast - the sign-in succeeded with a
        // caveat, and the new session cannot show the old session's lock.
        if (r.locks_left_held.length > 0) {
          useUIStore.getState().showToast(
            i18n.t('settings.speleodb.account.locksLeftHeldOnSignIn', {
              count: r.locks_left_held.length,
              projects: r.locks_left_held.map((left) => `"${left.project_name}"`).join(', '),
            }),
            TOAST_KEYS.SPELEODB_SIGNIN_LOCKS_HELD,
            'warning',
          );
        }
        // The keychain failed on THIS login and the user has not already settled
        // where the key should go, so ask now, while the failure is in context and
        // the token is still in hand. Raised from here so BOTH login paths reach it
        // - the staged one applied on Settings' Save, and the "Log in now" button.
        // Assigned rather than only raised: a login that DID reach the keychain
        // also retires a prompt left over from an earlier failed one, which is
        // now asking about a key that is no longer where it says.
        set({ storageChoiceOpen: r.needs_storage_choice });
        // #201: a fresh login is an "on login" scan point - surface any locks this
        // user left held (and prime the recents badges). `prompt: true` so it raises
        // the resume/release prompt even after the app-open scan already ran.
        void get().scanLocksOnOpen({ prompt: true });
      }
      return { ok: true };
    } catch (error) {
      return { ok: false, error };
    }
  },

  syncProjectLink: (isRemote, forPath = null) =>
    set((state) => {
      if (!isRemote) {
        // Confirmed local: no link, and none of a previous remote project's
        // shell state may survive into it.
        return state.link === null && state.projectLinkKind === 'local'
          ? { projectLinkKind: 'local' }
          : {
              projectLinkKind: 'local',
              link: null,
              linkPath: null,
              remoteProjectId: null,
              divergence: null,
              banner: null,
            };
      }
      // Remote. Keep an already-seeded link only when it was seeded for THIS
      // file - the browser's `setLinkOnOpen` immediately before this load, or a
      // reload of the same path (fast-forward / revision swap).
      //
      // Keeping any link at all was the bug: opening remote project B while A's
      // link was still live rendered B with A's lock holder, A's permission and
      // A's dirty bit. File > Open never clears the link, and `refreshLinkStatus`
      // swallows its errors, so offline (or on any failed status call) the
      // previous project's state simply stayed on screen.
      if (state.link && forPath !== null && state.linkPath === forPath) {
        return { projectLinkKind: 'remote' };
      }
      return {
        projectLinkKind: 'remote',
        linkPath: forPath,
        remoteProjectId: null,
        divergence: null,
        banner: null,
        link: {
          // Read-only until the status confirms we hold the write lock. The
          // backend agrees: `project_load` re-derives its edit gate as
          // `!is_remote`, so a remote project loads with mutations rejected.
          editState: 'read-only',
          // Unknown rather than assumed: nothing has been asked of the server
          // yet. `null` permission reads as "unknown (offline)" in the status
          // button, which is exactly the truth here.
          permission: null,
          lockHolder: null,
          lockedByOther: false,
          hasUnpushedEdits: false,
          unwound: false,
          forceEditing: false,
          forceEditAllowed: false,
          // [#303] Placeholder until the first status reply supplies the project's
          // explicit value.
          undoMode: 'full',
          remoteState: 'unknown',
          noAccess: false,
          serverError: null,
        },
      };
    }),

  refreshLinkStatus: async () => {
    // Capture which project this poll/refresh is for BEFORE the await, and drop the
    // reply if the open project changed (switch / close / another load) while it was
    // in flight - so a stale status can't paint the previous project's lock holder /
    // remote-state / permission onto the newly-open one. Mirrors the load-time
    // `take_unrecognised_shot_kinds` + `checkProjectIntegrity` staleness guards. (fix S3)
    const requestedPath = useProjectStore.getState().project?.file_path ?? null;
    // Also drop the reply when the link CHANGED under it, not just when the
    // project did. The backend reads the link row and then makes two network
    // round trips, and `run_blocking` holds no command-level lock, so a push can
    // land inside that window; the reply then carries the PRE-push snapshot, and
    // `seedFromStatus` overwrites `hasUnpushedEdits` wholesale - resurrecting a
    // dirty bit the push had just cleared, which re-arms the quit guard and
    // re-paints the un-pushed warning over work that IS on the server.
    //
    // Object identity is the check on purpose: every action that mutates link
    // truth builds a NEW link object (they all spread `...state.link`), so any
    // future mutator is covered without having to remember to bump a counter.
    // The no-op arms that return `{}` (applyRemoteLink when the bits already match,
    // applyPush's not_editing) keep the same object, and a reply is genuinely still valid
    // there.
    //
    // A seed by ANOTHER refresh is explicitly not such a mutation (`seededLinks`).
    // Counting it as one is what made overlapping refreshes cancel each other:
    // `handleRelease` awaits its own `refreshLinkStatus` after releasing the lock,
    // and a periodic poll landing in that window reseeded `link`, so the release's
    // reply - the one that knew the lock was gone - was discarded. The toolbar kept
    // the green pen and the shell stayed editable over a lock that no longer
    // existed, until the next poll, which can be up to 24 hours out.
    const seq = ++linkStatusSeq;
    const requestedLink = get().link;
    try {
      const status = await invokeIpc('speleodb_link_status');
      if ((useProjectStore.getState().project?.file_path ?? null) !== requestedPath) return;
      const current = get().link;
      if (current !== requestedLink && !(current && seededLinks.has(current))) return;
      // Issue order, not arrival order: an older reply must never overwrite a
      // newer one that already landed.
      if (seq < linkStatusApplied) return;
      linkStatusApplied = seq;
      get().seedFromStatus(status);
    } catch {
      // Best-effort - leave the prior link state (the backend gate is the
      // real enforcement; the shell just can't refresh its indicator).
    }
  },

  setSession: (s) => {
    // [#258] A refresh that comes back logged-OUT is an ending, whatever raised it.
    // This action is the benign one - every project load and every Settings open calls
    // it, and it deliberately does not bump `sessionGeneration` - so it cannot simply
    // tear down. But the ONE transition it can carry is a real session loss, and an
    // overlay left standing across it is a picker offering to retry against an account
    // nobody is signed in to. Suppressing NEW pickers is not enough; one already open
    // has to go too.
    //
    // Routed through `clearSession` rather than a second teardown beside it: that is
    // where the pending cancel is fired, the dismissal telemetry recorded, and every
    // stale link field reset - all of which are just as due here.
    // Two ways in, because a known true->false transition is not the only one that
    // matters. `sessionStillCurrent` deliberately lets a picker be raised while
    // `session` is still null - an authenticated backend call can beat the first
    // `speleodb_session` reply - and for that picker the prior value is never `true`,
    // so a transition test alone skipped the teardown and left it open over a session
    // the very same refresh had just reported dead. Confirm could then only fail, and a
    // revert suspended on the question stayed busy until the user cancelled by hand.
    // An open picker is therefore its own reason to tear down.
    const wasLoggedIn = get().session?.loggedIn === true;
    if (!s.logged_in && (wasLoggedIn || get().dumpPickerOpen)) {
      get().clearSession();
    }
    set({
      session: {
        loggedIn: s.logged_in,
        userEmail: s.user_email ?? null,
        endpoint: s.endpoint ?? null,
        keychainBacked: s.keychain_backed,
        storage: s.storage ?? null,
        keychain: s.keychain,
      },
    });
  },

  refreshSession: async () => {
    try {
      const status = await invokeIpc('speleodb_session');
      get().setSession(status);
    } catch {
      // Best-effort - leave whatever the store had.
    }
  },

  storageChoiceOpen: false,

  closeStorageChoice: () => set({ storageChoiceOpen: false }),

  dumpPickerOpen: false,
  dumpPickerCandidates: [],
  dumpPickerKind: 'stalx',
  dumpPickerBusy: false,
  dumpPickerError: null,
  dumpPickerRetry: null,
  dumpPickerCancel: null,
  dumpPickerOwner: null,

  openDumpPicker: (candidates, kind, retry, onCancel, owner) => {
    // Whoever was suspended on the PREVIOUS question has to be released, because this
    // raise takes their picker away and their cancel with it.
    //
    // Two flows really can be in flight at once: the revision history disables View
    // while its own download runs but leaves Revert live, so a revert probe can raise a
    // picker and the older View download can then raise another over it. The displaced
    // revert kept `busy` deliberately - its cancel is what releases it - so dropping
    // that callback left a dialog with every control and every dismissal route
    // disabled.
    //
    // NOT on a continuation, though. When a retry re-raises with a fresh candidate set,
    // the same flow is still suspended on the same question, and firing its cancel would
    // release it out from under the picker that just replaced it.
    //
    // Told apart by OWNER, not by timing. The first attempt used a flag held for the
    // duration of an awaited retry, which marks every picker raised anywhere in that
    // window as a continuation - so a View retry re-offering while a Revert picker was
    // up suppressed the Revert's cancel and stranded exactly the dialog this is here to
    // release. An unowned raise (the open verbs, which suspend on nothing) never
    // matches an owned one, so it always displaces.
    const previousOwner = get().dumpPickerOwner;
    const continuing = owner !== undefined && owner === previousOwner;
    const displaced = continuing ? null : get().dumpPickerCancel;
    // The lifecycle `uiStore.openDialog` runs, done by hand because this overlay
    // deliberately does not take the `activeDialog` slot - and skipping it left the
    // four registered picker events dead, so incidence and cancel counts stayed zero
    // while dwell went on being attributed to whatever surface is underneath.
    trackAction(`dialog.${DUMP_PICKER_DIALOG}`);
    setDwellState(`dialog.${DUMP_PICKER_DIALOG}`);
    set({
      dumpPickerOpen: true,
      dumpPickerCandidates: candidates,
      dumpPickerKind: kind,
      dumpPickerBusy: false,
      dumpPickerError: null,
      dumpPickerRetry: retry,
      dumpPickerCancel: onCancel ?? null,
      dumpPickerOwner: owner ?? null,
    });
    // Clear FIRST, then notify - the same order `closeDumpPicker` documents, so a
    // handler that re-raises is not wiped by the call that released it.
    displaced?.();
  },

  closeDumpPicker: () => {
    // Dismissed without answering - the cancel half of the same lifecycle.
    checkDialogCancel(DUMP_PICKER_DIALOG);
    restoreDwellUnderOverlay();
    const onCancel = get().dumpPickerCancel;
    // Clear FIRST, then notify: a handler that re-raises the picker must not have
    // its new state wiped by this same call.
    set({
      dumpPickerOpen: false,
      dumpPickerCandidates: [],
      dumpPickerBusy: false,
      dumpPickerError: null,
      dumpPickerRetry: null,
      dumpPickerCancel: null,
      dumpPickerOwner: null,
    });
    onCancel?.();
  },

  resolveDumpPicker: async (entryPath) => {
    const retry = get().dumpPickerRetry;
    if (!retry) return;
    set({ dumpPickerBusy: true, dumpPickerError: null });
    try {
      await retry(entryPath);
      // Only act on a picker that is still THIS one. `retry` is the ownership token:
      // every route that ends a picker replaces it - `closeDumpPicker` and
      // `clearSession` null it, `openDumpPicker` installs a fresh closure - so an
      // identity check catches all three, where the `dumpPickerBusy` proxy this
      // replaces caught only the re-raise. A confirm is a network round trip of
      // unbounded length, and the session can die and be replaced under it (the
      // reLogin route does exactly that), after which closing, erroring or
      // un-busying would be acting on a DIFFERENT dialog's state.
      //
      // A fresh ambiguity is the ordinary case: the tree moved while the dialog was
      // open, so the retry re-raised the picker with a new candidate list, and
      // closing here would throw that away.
      if (get().dumpPickerRetry !== retry) return;
      // Both halves, in this order, exactly as `uiStore.openDialog` does for a
      // chained dialog. `trackDialogComplete` only SETS a module-global marker and
      // `checkDialogCancel` is what consumes it - so completing without consuming
      // left the marker standing, and the next picker's cancel was read as this
      // one's completion and emitted no cancel event.
      trackDialogComplete(DUMP_PICKER_DIALOG);
      checkDialogCancel(DUMP_PICKER_DIALOG);
      restoreDwellUnderOverlay();
      set({
        dumpPickerOpen: false,
        dumpPickerCandidates: [],
        dumpPickerRetry: null,
        dumpPickerCancel: null,
        dumpPickerOwner: null,
        // Cleared HERE rather than left to the `finally`, which no longer fires
        // unconditionally: this very `set` hands ownership away, so the guard below
        // correctly declines to touch a picker that is no longer ours.
        dumpPickerBusy: false,
      });
    } catch (e) {
      // The retry runs from the DIALOG, so its rejection has nowhere else to go: the
      // caller invokes this with `void` and no host receives an outcome. Left
      // unhandled it became an unhandled rejection and the picker sat there open,
      // saying nothing about why confirming did not work.
      // `getErrorMessage`, not `humanizeSpeleoError`: `utils/speleoError` imports
      // this module, so reaching for it here would close an import cycle. The
      // picker's message is the raw reason, which is what a user staring at a
      // failed confirmation needs.
      //
      // Same ownership check as the success path, and it is what keeps the reLogin
      // route quiet: that replay drops the session (which closes this picker) and
      // then rethrows, and painting its message onto whatever dialog came next -
      // or onto a closed one, to surface on the next open - helps nobody.
      if (get().dumpPickerRetry !== retry) return;
      set({ dumpPickerError: getErrorMessage(e) });
    } finally {
      // Never un-busy a picker we no longer own: a late loser settling under a
      // NEWER picker would otherwise re-enable its buttons mid-confirm.
      if (get().dumpPickerRetry === retry) set({ dumpPickerBusy: false });
    }
  },

  // Both credential verbs write `config.speleodb.{token_fallback,keychain_declined}`
  // on the backend, so the frontend's config copy is stale the moment they return.
  // `update_config` now preserves that pair server-side (the Settings form does not
  // own it), so a stale snapshot can no longer UNDO the answer - but re-reading the
  // config still keeps anything rendering off it honest about where the key went.
  setTokenStorage: async (mode) => {
    try {
      const status = await invokeIpc('speleodb_set_token_storage', { mode });
      // Seed from the ANSWER, not from the request: a plaintext write can itself
      // fail, and then the honest thing to show is session-only.
      get().setSession(status);
      set({ storageChoiceOpen: false });
      await useProjectStore.getState().reloadConfigInOrder();
      // The landed tier, handed back rather than collapsed to a bare `true`, so the
      // caller can say where the key went instead of where it was sent. `storage` is
      // null only when the backend reports logged-out, which this verb refuses
      // outright - it is treated as a failure rather than dropped on the floor.
      return status.storage ?? null;
    } catch {
      return null;
    }
  },

  retryKeychain: async () => {
    try {
      const status = await invokeIpc('speleodb_retry_keychain');
      get().setSession(status);
      await useProjectStore.getState().reloadConfigInOrder();
      return true;
    } catch {
      return false;
    }
  },

  scanLocksOnOpen: async (opts) => {
    // Ensure we know who we are before filtering held-by-me: on the app-open home
    // screen the frontend session may be cold even though the backend holds a
    // rehydrated token (which list_projects uses).
    if (!get().session) await get().refreshSession();
    const me = get().session?.userEmail?.toLowerCase();
    if (!get().session?.loggedIn || !me) {
      // Logged out: no badges, no prompt. Also force the overlay shut (not just its
      // pending flag) so a prompt open when the session ended can't linger. Consume
      // the app-open "pending" so a later return-to-recents (still using the pending
      // flag) can't retro-prompt; a subsequent login re-arms via its `prompt: true`.
      set({
        remoteLocks: {},
        remoteProjects: {},
        remoteSummaries: {},
        heldLocks: null,
        heldLockPromptPending: false,
        heldLockPromptOpen: false,
      });
      return;
    }
    try {
      const projects = await invokeIpc('speleodb_list_projects');
      // Staleness guard: a logout / account switch can land while the list_projects
      // GET is in flight. Bail if the session moved so we never repaint the ended
      // session's badges or re-raise its prompt (mirrors refreshLinkStatus's guard).
      if (!get().session?.loggedIn || get().session?.userEmail?.toLowerCase() !== me) {
        return;
      }
      const remoteLocks: Record<string, string | null> = {};
      const remoteProjects: Record<string, string> = {};
      const remoteSummaries: Record<string, SpeleoProjectSummary> = {};
      const heldLocks: HeldLock[] = [];
      for (const p of projects) {
        remoteLocks[p.id] = p.lock_holder;
        remoteProjects[p.id] = p.name;
        remoteSummaries[p.id] = p;
        // Same ownership check the recents badge uses (emailsMatch), so the prompt's
        // held list and the badge's "locked by you" can't disagree.
        if (emailsMatch(p.lock_holder, me)) {
          heldLocks.push({ projectId: p.id, projectName: p.name });
        }
      }
      set((state) => {
        // Prompt on an explicit login/app-open scan (`prompt`), or on the FIRST scan
        // of the session (`heldLockPromptPending`) - never on a routine badge refresh
        // (returning to recents, the refresh button). That is what stops the prompt
        // re-appearing on every recents visit while still firing on login + app-start.
        const promptWorthy = opts?.prompt === true || state.heldLockPromptPending;
        return {
          remoteLocks,
          remoteProjects,
          remoteSummaries,
          heldLocks: heldLocks.length > 0 ? heldLocks : null,
          heldLockPromptPending: false,
          ...(promptWorthy && heldLocks.length > 0 ? { heldLockPromptOpen: true } : {}),
        };
      });
    } catch {
      // Best-effort: keep prior badges and never raise a prompt on a failed scan.
    }
  },

  releaseLock: async (projectId) => {
    // Throws on failure so the dialog keeps itself open and surfaces the error.
    await invokeIpc('speleodb_release_lock', { projectId });
    // Optimistic local update (avoids a full re-scan per release when the user frees
    // several one by one): drop this lock from the prompt's list and mark the project
    // free in the badge map. `heldLocks` going empty is what auto-closes the dialog.
    set((state) => {
      const heldLocks = (state.heldLocks ?? []).filter((l) => l.projectId !== projectId);
      return {
        heldLocks: heldLocks.length > 0 ? heldLocks : null,
        remoteLocks: { ...state.remoteLocks, [projectId]: null },
      };
    });
    // If we just freed the OPEN project's own lock, re-seed its status indicator.
    await get().refreshLinkStatus();
  },

  clearHeldLocks: () => set({ heldLocks: null }),

  closeHeldLockPrompt: () => set({ heldLockPromptOpen: false }),

  setSessionFromLogin: (r) =>
    set((state) => ({
      session: {
        loggedIn: true,
        userEmail: r.user_email,
        endpoint: r.endpoint,
        keychainBacked: r.keychain_backed,
        storage: r.storage,
        keychain: r.keychain,
      },
      // A login is a NEW session even when it is the same account: a different
      // token, with its own storage outcome. Work still in flight for the previous
      // one must not act on this one.
      sessionGeneration: state.sessionGeneration + 1,
    })),

  // A logout / dead-token drop can't act on the remote project anymore - the
  // backend drops its edit gate AND releases the lock on the same event - so fully
  // reset the link to a clean read-only state (keeping only the last-known
  // permission tier so the view-only glyph stays right). Leaving stale lockHolder /
  // dirty / lockedByOther / remoteState / noAccess / serverError behind would render
  // a self-contradictory button (e.g. yellow "click to edit" while still claiming we
  // hold the lock and have un-pushed edits). The shell offers Start editing again on
  // re-login, and the next poll re-seeds. (fix 4)
  clearSession: () => {
    // [#258] Fire the picker's cancel BEFORE dropping it. A caller that suspended a
    // flow on this question (the revert holds `busy` across it) is otherwise left
    // with a modal whose every dismissal route is disabled, by the very reset that
    // was meant to tidy up after the session.
    const abandonedPicker = get().dumpPickerCancel;
    // A session-loss teardown is still a dismissal without an answer, so it owes the
    // same telemetry any other dismissal does - otherwise the cancel goes unrecorded
    // and dwell stays attributed to a picker that is no longer on screen.
    if (get().dumpPickerOpen) {
      checkDialogCancel(DUMP_PICKER_DIALOG);
      restoreDwellUnderOverlay();
    }
    set((state) => ({
      session: null,
      // The session ended, so nothing still in flight for it may act on whatever
      // signs in next (see `sessionGeneration`).
      sessionGeneration: state.sessionGeneration + 1,
      link: state.link
        ? {
            ...state.link,
            editState: 'read-only',
            lockHolder: null,
            lockedByOther: false,
            // `hasUnpushedEdits` is deliberately CARRIED THROUGH, not zeroed. It is
            // a fact about the local .stalx - edits that never reached the server -
            // and losing the token does not upload them. Clearing it here claimed
            // "nothing outstanding" at the exact moment the work became most
            // stranded, which disarmed the quit guard and hid the Toolbar's
            // unpushed-edits warning; the next authenticated poll re-seeds it.
            // The backend drops the edit gate (and any force-edit) on a token loss,
            // so mirror that here or the shell would keep offering Reconnect.
            forceEditing: false,
            // We can no longer verify the server relationship without a token, so
            // report Unknown (what a token-less backend poll returns) rather than
            // claiming up-to-date; the next poll re-resolves it.
            remoteState: 'unknown',
            noAccess: false,
            serverError: null,
          }
        : null,
      divergence: null,
      banner: null,
      // The revision-history section fetches per the open remote project id, which it
      // can no longer read token-lessly; drop it so it hides (rather than error-fetching)
      // until a re-login re-seeds it from the next status poll.
      remoteProjectId: null,
      // The lock scan was for this now-ended session; drop its badges + prompt.
      remoteLocks: {},
      remoteProjects: {},
      remoteSummaries: {},
      heldLocks: null,
      heldLockPromptOpen: false,
      // Same reasoning: the storage prompt asks where to put a key that no longer
      // exists once the session has ended.
      storageChoiceOpen: false,
      // The candidates describe a server tree this session can no longer reach.
      dumpPickerOpen: false,
      dumpPickerCandidates: [],
      dumpPickerBusy: false,
      dumpPickerRetry: null,
      dumpPickerCancel: null,
      dumpPickerOwner: null,
    }));
    abandonedPicker?.();
  },

  seedFromStatus: (s) =>
    set((state) => {
      // The status is authoritative on remoteness, so it also RESOLVES the
      // tri-state (both ways) - a status reply is never "unknown".
      if (!s.is_remote) {
        return {
          projectLinkKind: 'local' as const,
          link: null,
          remoteProjectId: null,
          divergence: null,
          banner: null,
        };
      }
      const editState = s.edit_enabled ? 'editing' : 'read-only';
      // #200: reflect the server's exclude_geojson as a persistent
      // rendering-disabled banner. It outranks the informational `no_anchor` notice
      // (rendering_disabled is actionable via Enable-rendering, and if both were true
      // no_anchor would otherwise mask the only way to reach that action), so claim
      // the slot when it is free OR held by no_anchor - but NEVER clobber transient
      // action feedback (push_unconfirmed / concurrent_session). Clear it only when
      // WE own the slot, so a routine poll never wipes another banner.
      const prior = state.banner;
      let banner = prior;
      if (s.exclude_geojson) {
        if (!prior || prior.kind === 'no_anchor') banner = { kind: 'rendering_disabled' };
      } else if (prior?.kind === 'rendering_disabled') {
        banner = null;
      }
      // [#576 B35(e)] The edit session just ended because the lock is not ours any
      // more. Outranks every other notice: whatever they said was about an edit
      // session that is over (a Done refused for exactly this reason raises a
      // push_failed first, and this explains it).
      const lockLost = lockLostHolder(state.link, s, state.session?.userEmail ?? null);
      if (lockLost.lost) banner = { kind: 'lock_lost', holder: lockLost.holder };
      const seeded: SpeleoLinkState = {
        editState,
        // The network fields degrade to null/unknown offline - keep the
        // last-known permission rather than blanking the gate on a transient miss.
        permission: s.permission ?? state.link?.permission ?? null,
        lockHolder: s.lock_holder,
        // Read-only + a resolved holder ⟹ someone else holds it (the backend
        // gives us the gate when the lock is ours). Derived PURELY from this
        // poll's (editState, lockHolder), with NO "preserve prior" carry: once the
        // other holder releases (a later poll resolves no holder) the button stops
        // reading "locked by other" instead of sticking orange until a poll both
        // reaches the server AND happens to resolve a holder (fix 7). A transient
        // offline poll may briefly drop it to read-only; the next poll re-resolves,
        // and the backend gate stays the real enforcement.
        lockedByOther: editState !== 'editing' && !!s.lock_holder,
        hasUnpushedEdits: s.has_unpushed_edits,
        unwound: s.unwound,
        // #213: force-edit (edit-enabled WITHOUT a held lock) is backend-authoritative
        // and reported every poll; a reconnect that acquires the lock clears it there.
        forceEditing: s.force_editing,
        forceEditAllowed: s.allow_force_edit,
        // [#303] Backend-resolved explicit project policy.
        undoMode: s.undo_mode,
        remoteState: s.remote_state,
        noAccess: s.access_denied,
        serverError: s.server_error,
      };
      // Register the object this seed produced. `refreshLinkStatus` drops a reply
      // whose link changed under it, and without this marker a seed by ANOTHER
      // refresh looked identical to a local mutation - so two overlapping polls
      // made each other's replies unusable and the fresher one was thrown away.
      seededLinks.add(seeded);
      return {
        banner,
        projectLinkKind: 'remote' as const,
        link: seeded,
        // #218: the open remote project's id, for the in-project revision-history entry.
        remoteProjectId: s.project_id,
      };
    }),

  setLinkOnOpen: ({ permission, forPath = null, hasUnpushedEdits = false, projectId = null }) =>
    set({
      // Committed in the SAME synchronous `set()` as the rest of the seed, so nothing
      // can observe the new project's link beside the old project's id. `syncProjectLink`
      // recognises this seed by `linkPath` and keeps it; the unmatched branch clears the
      // id, so a File > Open of some other remote `.stalx` still resets it.
      remoteProjectId: projectId,
      // We just pulled it from SpeleoDB, so it is remote by construction - and
      // saying so keeps the gate closed even if the load never completes.
      projectLinkKind: 'remote',
      // Whose link this is, so the `syncProjectLink` that runs inside the load
      // that follows recognises it as THIS project's seed and keeps it.
      linkPath: forPath,
      // Opens read-only until the status confirms a lock. `remoteState` is the
      // pull's truth (we just took the server tip) and stays optimistic; the dirty
      // bit is NOT assumed - see the interface note - because "keep the local copy"
      // reaches this seed with edits the server has never seen. `seedFromStatus`
      // refines lock holder / remote state on the first poll.
      link: {
        permission,
        editState: 'read-only',
        lockHolder: null,
        lockedByOther: false,
        hasUnpushedEdits,
        // A fresh pull is at the server tip, so no chain step has walked it anywhere.
        unwound: false,
        // A fresh pull always opens without the lock and not force-editing;
        // `seedFromStatus` refines the opt-in + any force-edit on the first poll.
        forceEditing: false,
        forceEditAllowed: false,
        undoMode: 'full',
        // A pull just took the server tip, so up-to-date is true by construction -
        // but only for a pull. A seed that already knows the copy holds un-pushed
        // edits is by definition NOT in sync, so claim nothing until the first poll
        // resolves it rather than render a self-contradictory "up to date + dirty".
        remoteState: hasUnpushedEdits ? 'unknown' : 'up_to_date',
        noAccess: false,
        serverError: null,
      },
      divergence: null,
      banner: null,
    }),

  snapshotLink: () => {
    const { link, projectLinkKind, linkPath, remoteProjectId, divergence, banner } = get();
    return { link, projectLinkKind, linkPath, remoteProjectId, divergence, banner };
  },

  // `busy: false` mirrors `clearLink`: the seed's caller is done either way, and a
  // restored link must not inherit a stuck spinner from the attempt that failed.
  // Disowning first is what keeps that from becoming the opposite bug - a verb
  // still in flight over the restored project would otherwise clear the flag out
  // from under whatever ran next (see `beginBusy`).
  restoreLink: (snapshot) => {
    disownBusy();
    set({ ...snapshot, busy: false });
  },

  // Reset `busy` too: a hand-off verb in flight when the project closes/switches
  // would otherwise leave a freshly-opened project's status button disabled until
  // that verb's `finally` runs (its `apply*` no-op safely, guarded on `link`). (N2)
  // That reasoning covers the verb's `apply*` but NOT its `finally`, which would
  // clear `busy` for a NEWER verb on a NEWER project - so disown the claim as well
  // as clearing the flag (#270; see `beginBusy`).
  // Back to `unknown`, not `local`: this runs on close (nothing open), and neither
  // outcome establishes that a local project is open, so it may not re-open the
  // gate. NOT for rolling back a failed remote open - that has a previous project
  // still on screen to put back, so it uses `restoreLink` instead.
  clearLink: () => {
    disownBusy();
    set({
      link: null,
      linkPath: null,
      projectLinkKind: 'unknown',
      remoteProjectId: null,
      divergence: null,
      banner: null,
      busy: false,
    });
  },

  applyStartEditing: (r) =>
    set((state) => {
      if (!state.link) return {};
      switch (r.kind) {
        case 'editing':
        case 'fast_forwarded': {
          // We now hold the lock: not locked-by-other, no remote-ahead/conflict,
          // no stale banner, and force-edit clears (Reconnect succeeded - #213).
          // (fast_forwarded also reloads + re-seeds.)
          //
          // EXCEPT a push failure (#260/#270). Read-only is exactly how a project
          // lands in that state - a 401 auto-drop runs `clearSession()`, which
          // KEEPS the unpushed edits - and from there the failure banner's only
          // control is "Start editing to retry", which calls this. Clearing the
          // slot therefore destroyed the very surface that sent the user here, the
          // moment it succeeded: the Retry control and their typed commit message
          // went with it, leaving them holding the lock with the edits still only
          // on this computer and nothing on screen saying so - the durable surface
          // #260 exists to provide. So the failure survives the gate opening, and
          // the Retry it carries is now usable.
          //
          // A fast-forward is different and still clears: it can only run over a
          // copy with nothing unpushed (`LocalEditsPolicy::Protect` refuses
          // otherwise) and it has just replaced the working copy with the server's,
          // so an older failure no longer describes anything on this machine.
          const keepFailure = r.kind === 'editing' && state.banner?.kind === 'push_failed';
          return {
            link: {
              ...state.link,
              editState: 'editing',
              lockedByOther: false,
              forceEditing: false,
              remoteState: 'up_to_date',
              serverError: null,
            },
            divergence: null,
            banner: keepFailure ? state.banner : null,
          };
        }
        case 'divergence_needed':
          return { divergence: 'start' };
        case 'locked_by':
          // Someone else holds it - mark locked even when the holder email is
          // unknown (keep the last-known email rather than nulling it).
          return {
            link: {
              ...state.link,
              // #213: a force-editor who tried to Reconnect and found the lock still
              // held keeps editing offline (the backend left its gate + force-edit on),
              // so preserve their editing state - only record the holder. A normal
              // read-only start downgrades to read-only as before.
              editState: state.link.forceEditing ? state.link.editState : 'read-only',
              lockHolder: r.email ?? state.link.lockHolder,
              lockedByOther: true,
            },
          };
        default:
          return {};
      }
    }),

  applyUpdate: (r) =>
    set((state) => {
      if (!state.link) return {};
      switch (r.kind) {
        case 'up_to_date':
          // Nothing to pull - clear any stale remote-ahead/conflict indicator so
          // the button doesn't keep showing "pull available" after a no-op refresh.
          return {
            link: { ...state.link, remoteState: 'up_to_date', serverError: null },
            divergence: null,
          };
        case 'divergence_needed':
          return { divergence: 'update' };
        // fast_forwarded: the caller reloads + re-seeds.
        default:
          return {};
      }
    }),

  applyDone: (r) =>
    set((state) => {
      if (!state.link) return {};
      // Widened so the not-editing arm is nameable and the `never` check below
      // stays honest across the type regeneration - see {@link DoneKind}.
      const kind = asDoneKind(r.kind);
      switch (kind) {
        case 'released':
          // Lock released and edits are on the server - clear the holder (or we'd
          // read as still holding it), drop the dirty bit + any push-unconfirmed
          // banner, and reset the remote state.
          return {
            link: {
              ...state.link,
              editState: 'read-only',
              hasUnpushedEdits: false,
              unwound: false,
              lockHolder: null,
              lockedByOther: false,
              remoteState: 'up_to_date',
              serverError: null,
            },
            chainTargetGeneration: state.chainTargetGeneration + 1,
            divergence: null,
            banner: null,
          };
        case 'lock_held_elsewhere':
          // [#576 SDB-2MACHINE-D1] Editing has ended here with nothing to upload, as
          // for `released` - but the lock was adopted from the account and no edit was
          // made under it here, so it stays with the session using it: the holder is
          // NOT cleared (the poll keeps it fresh).
          return {
            link: {
              ...state.link,
              editState: 'read-only',
              hasUnpushedEdits: false,
              unwound: false,
              lockedByOther: false,
              remoteState: 'up_to_date',
              serverError: null,
            },
            chainTargetGeneration: state.chainTargetGeneration + 1,
            divergence: null,
            banner: null,
          };
        case 'divergence_needed':
          return { divergence: 'done' };
        case 'push_unconfirmed':
          return { banner: { kind: 'push_unconfirmed' } };
        case 'not_editing':
          // Nothing was uploaded and nothing was released: the backend's edit gate
          // was already closed (typically a 401 auto-drop, which runs
          // `speleodb_forget` and deliberately LEAVES the dirty bit alone). Change
          // no link state - above all not `hasUnpushedEdits`, which is the flag
          // `guardQuit` and the status button's warning both read. The caller
          // surfaces the honest "reconnect and start editing again" line.
          return {};
        default:
          return unhandledHandoffKind(kind);
      }
    }),

  applyPush: (r) =>
    set((state) => {
      if (!state.link) return {};
      const kind = asPushKind(r.kind);
      switch (kind) {
        case 'pushed':
          // Lock retained (still editing); edits are now on the server. Clear a
          // prior push-unconfirmed banner since this push landed.
          return {
            link: {
              ...state.link,
              hasUnpushedEdits: false,
              unwound: false,
              remoteState: 'up_to_date',
              serverError: null,
            },
            chainTargetGeneration: state.chainTargetGeneration + 1,
            banner: null,
          };
        case 'divergence_needed':
          return { divergence: 'push' };
        case 'push_unconfirmed':
          return { banner: { kind: 'push_unconfirmed' } };
        case 'not_editing':
          // See applyDone: nothing left this machine, so nothing about the local
          // copy's relationship to the server may be marked resolved.
          return {};
        default:
          return unhandledHandoffKind(kind);
      }
    }),

  applyRemoteLink: (bits, linkPathAtIssue) =>
    set((state) => {
      const link = state.link;
      if (!link || state.linkPath !== linkPathAtIssue) return {};
      if (link.hasUnpushedEdits === bits.has_unpushed_edits && link.unwound === bits.unwound) return {};
      return {
        link: { ...link, hasUnpushedEdits: bits.has_unpushed_edits, unwound: bits.unwound },
        chainTargetGeneration: state.chainTargetGeneration + 1,
      };
    }),

  clearDivergence: () => set({ divergence: null }),

  notifyReverifyDivergence: () =>
    set((state) => ({
      divergence: 'reverify',
      link: state.link
        ? { ...state.link, editState: 'read-only', remoteState: 'conflict' }
        : state.link,
    })),

  raiseNoAnchorBanner: () =>
    set((state) => (state.banner ? {} : { banner: { kind: 'no_anchor' } })),

  raisePushFailedBanner: ({ message, origin, pushMessage }) =>
    set({ banner: { kind: 'push_failed', message, origin, pushMessage } }),

  clearPushFailedBanner: () =>
    set((state) => (state.banner?.kind === 'push_failed' ? { banner: null } : {})),

  enableRendering: async () => {
    // Throws on failure so the banner keeps itself open and shows the error.
    // [#576 RVF-c-C1] Names the project it was asked for: refused once another is open.
    await invokeIpc('speleodb_set_exclude_geojson', {
      exclude: false,
      expectedProject: expectedOpenProject(),
    });
    // Drop the rendering-disabled banner (this resolved it) and re-seed from the
    // server so the flag reflects the change.
    set((state) => (state.banner?.kind === 'rendering_disabled' ? { banner: null } : {}));
    await get().refreshLinkStatus();
  },

  setUndoMode: async (mode) => {
    // [#303] Throws on failure so the caller (the settings control) renders the
    // refusal - most usefully the lock one, since this writes the `.stalx` and so
    // becomes every collaborator's policy on the next push.
    await invokeIpc('speleodb_set_undo_mode', { mode, expectedProject: expectedOpenProject() });
    // The backend policy changed even if the best-effort status refresh below cannot
    // reach the server, so a target resolved under the old policy is already stale.
    get().invalidateChainTargets();
    // Re-read so the store reflects the committed backend value.
    await get().refreshLinkStatus();
  },

  invalidateChainTargets: () =>
    set((state) => ({ chainTargetGeneration: state.chainTargetGeneration + 1 })),

  dismissBanner: () => set({ banner: null }),
}));

// projectStore reaches this store through speleoStoreHandle instead of importing
// it (this module imports projectStore; the two used to be a value-import cycle,
// #576 D27-32(b)).
registerSpeleoStore(() => useSpeleoStore.getState());

/**
 * [#260] The dialogs that OWN a push/unlock failure while they are open: they
 * issue the verb, stay open when it fails, and render the failure INLINE (see
 * `SpeleoPushFailureNotice`). While one of them is the active dialog the shell
 * banner suppresses its `push_failed` kind - rendered there it sits behind the
 * modal scrim, dimmed and unreadable, with its Retry/Dismiss unclickable, exactly
 * when the user needs it. Closing the dialog with the failure unresolved hands
 * the banner straight back, so the "your changes are only on this computer"
 * reminder is never lost.
 *
 * Every entry retries through its own primary button (`Upload & release` /
 * `Upload & keep editing` / `Push & release` / `Push, release & quit` /
 * `Retry the upload`), so none needs a second retry control and none loses the
 * commit message the user typed.
 *
 * `speleodb-revert` earns its place for a sharper reason than the rest. Its push
 * happens inside the restore flow (`useRevertRevision` -> `done`), and the dialog
 * renders through `AlertDialog`'s full-viewport scrim (`fixed inset-0 z-[60]`),
 * which covers the shell banner completely - so leaving the banner as the only
 * surface would put the failure, and its Retry, behind glass. Suppressing it here
 * lets the dialog show the failure inline with its own Retry, and closing the
 * dialog hands the banner back.
 */
export const PUSH_FAILURE_DIALOGS: readonly string[] = [
  'speleodb-done',
  'speleodb-close',
  'speleodb-quit',
  'speleodb-revert',
  'speleodb-unwound-push',
];

/**
 * The active push/unlock failure (the `push_failed` banner payload), or `null`.
 * Shared so the inline dialog notice and the shell banner read ONE source of
 * truth: they can never disagree, and there is no window where both are visible.
 */
export const selectPushFailure = (s: SpeleoStore): PushFailedBanner | null =>
  s.banner?.kind === 'push_failed' ? s.banner : null;

/**
 * A remote SpeleoDB project opened without the write lock is read-only. Shared so
 * the toolbar, map read-only badge, and context menu all derive "read-only"
 * identically (the backend edit gate is the real enforcement; this is the visual
 * parity).
 *
 * Fails CLOSED (#270). Only two things open this gate: a `local` project, or a
 * link that says we hold the write lock. Everything else - including the
 * `unknown` window before a project's remoteness is established - reads as
 * read-only, because a permission gate must not default open. The old form was
 * `s.link ? s.link.editState !== 'editing' : false`, which handed the editable
 * answer to every state where `link` had simply not been seeded yet, and a
 * reopen-from-recents of a locked remote project spent the whole
 * `speleodb_link_status` round trip in exactly that state.
 *
 * This is strictly MORE read-only than the old form, never less: whenever `link`
 * is set the verdict is unchanged, so no existing read-only path is weakened.
 */
export const selectSpeleoReadOnly = (s: SpeleoStore): boolean => {
  if (s.link) return s.link.editState !== 'editing';
  return s.projectLinkKind !== 'local';
};
