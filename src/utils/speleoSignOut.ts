/**
 * Sign-out (and account switch, which signs the previous account out first) must
 * never strand a lock or un-uploaded work.
 *
 * Signing out drops the token, and with it the only way this app can hand a lock
 * back or upload anything for that account - SpeleoDB has no lock timeout. So
 * before the token goes, {@link guardSpeleoSignOut} surveys everything the
 * account still holds:
 *
 * (a) the OPEN project, when we hold its lock over un-uploaded work: Upload and
 *     disconnect (the normal upload, which releases), Keep on this computer, or
 *     Discard (second "lost forever" confirm, the open project only);
 * (b) every lock this run holds a claim on (`lock_claimed_here`: taken here, or
 *     kept by a close) with nothing un-uploaded behind it: handed back as part of
 *     the sign-out, no question asked;
 * (c) every OTHER cached project of the account with un-uploaded work: always
 *     kept on this computer (the dialog names them and says they can be opened
 *     and uploaded after connecting again); a lock this run claims on one is
 *     handed back with (b). The sign-out never discards another project's work;
 * (d) every other lock the account holds that this run holds no claim on, where
 *     this computer has something of the project (a copy, a recent file): it may
 *     be the lock Stalagmight on another computer with this account is editing
 *     under, or one a session that did not exit cleanly left (sdbdemo2 issue 5).
 *     The dialog says so in the held-lock prompt's words and offers "Release
 *     anyway" per project; Disconnect leaves it held. A lock this computer can
 *     positively not have taken (`lockNotTakenHere`: no claim, no copy, no recent
 *     file) strands nothing of this computer's and is left alone without a word
 *     (demo item 1).
 *
 * Keeping is the non-destructive answer when the upload is not possible
 * (decision 10: edits are never discard-only). The copy keeps its edits, its
 * un-uploaded mark and its recents row, and its lock is handed back with the
 * others through the explicit per-project release: confirming the sign-out
 * dialog, which says so, is the consent a silent lifecycle release never has
 * (that one still keeps the lock over un-uploaded edits). After the next sign-in
 * the ordinary open takes over: the local copy opens and resumes when SpeleoDB
 * has not moved, and a divergence offers "Open my local copy".
 *
 * There is deliberately no "keep the lock" here: a lock kept across a sign-out is
 * exactly a stranded lock. Nothing to do -> sign out directly, as before. The
 * releases run last, off the main thread and time-bounded; if any fails the user
 * is told and chooses "Try again" or "Disconnect anyway" (the locks then stay
 * held until they sign in again and release them).
 */
import { create } from 'zustand';
import { useProjectStore } from '../store/projectStore';
import { markOwnReleaseEnded, useSpeleoStore } from '../store/speleoStore';
import type { SpeleoProjectSummary } from '../types/ipc.generated';
import { emailsMatch } from './email';
import { invokeIpc } from './ipc';
import { describeSpeleoError } from './speleoError';
import { ownSpeleoRelease } from './speleoLockLost';
import { needsSpeleoExitPrompt } from './speleoExit';
import { fetchSpeleoLocalStates, lockNotTakenHere } from './speleoLocalState';

export type SignOutKind = 'signOut' | 'switch';

/** One project in the survey, named the way the server lists it. */
export interface SignOutProject {
  summary: SpeleoProjectSummary;
  /** This account holds the project's write lock on the server. */
  holdsLock: boolean;
  /** This run holds a claim on that lock (taken here, or kept by a close). */
  claimedHere: boolean;
  /** When the local copy was last edited (RFC 3339), if known. */
  lastEditAt: string | null;
}

export interface SignOutSurvey {
  /** (a) The open project, when we hold its lock over un-uploaded work. */
  openProject: SignOutProject | null;
  /** (b) Locks with nothing un-uploaded behind them: released on sign-out. */
  releasable: SignOutProject[];
  /** (c) Other cached projects with un-uploaded work (lock held or not). */
  unuploaded: SignOutProject[];
  /** (d) The account's locks this run never claimed, over a copy or a recent
      file here: may be another session's; left held unless released anyway. */
  heldUnclaimed: SignOutProject[];
}

/** The locks Disconnect hands back besides the open project's: the clean ones
    and those of the other copies with changes, each one this run claims. */
export function signOutLocks(survey: SignOutSurvey): SignOutProject[] {
  return [...survey.releasable, ...survey.unuploaded.filter((p) => p.holdsLock && p.claimedHere)];
}

/** How long one survey or one release may take before it counts as failed. */
export const SIGN_OUT_NETWORK_LIMIT_MS = 20_000;

function bounded<T>(work: Promise<T>, what: string): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const limit = new Promise<never>((_resolve, reject) => {
    timer = setTimeout(() => reject(new Error(`${what} timed out`)), SIGN_OUT_NETWORK_LIMIT_MS);
  });
  return Promise.race([work, limit]).finally(() => clearTimeout(timer));
}

export function signOutHasWork(survey: SignOutSurvey): boolean {
  return survey.openProject !== null || survey.releasable.length > 0 || survey.unuploaded.length > 0 ||
    survey.heldUnclaimed.length > 0;
}

/**
 * Ask SpeleoDB which locks the account holds and the pull cache which copies
 * have un-uploaded work, and sort the answer into the three groups above.
 * Throws when the server cannot be asked: "no locks" must never be read into a
 * failure to look.
 */
export async function surveySignOut(): Promise<SignOutSurvey> {
  const me = useSpeleoStore.getState().session?.userEmail ?? null;
  const listed = await bounded(
    invokeIpc<SpeleoProjectSummary[]>('speleodb_list_projects', undefined, {
      purpose: 'find the locks this account holds before signing out',
    }),
    'Checking your SpeleoDB projects',
  );
  const local = await fetchSpeleoLocalStates(listed.map((p) => p.id));

  const { project, isDirty, recentProjects } = useProjectStore.getState();
  const { link, remoteProjectId } = useSpeleoStore.getState();
  const openId = project ? remoteProjectId : null;
  const openNeedsAnswer = openId !== null && needsSpeleoExitPrompt(link, isDirty);

  const survey: SignOutSurvey = { openProject: null, releasable: [], unuploaded: [], heldUnclaimed: [] };
  for (const summary of listed) {
    const state = local[summary.id];
    const entry: SignOutProject = {
      summary,
      holdsLock: emailsMatch(summary.lock_holder, me),
      claimedHere: state?.lock_claimed_here === true,
      lastEditAt: state?.last_edit_at ?? null,
    };
    const linkedHere = recentProjects.some((p) => p.speleodb_project_id === summary.id);
    // (d): the account's lock, never claimed by this run, over something of the
    // project here. Listed whether or not the copy has changes (those are kept
    // either way): only its release is a question.
    if (entry.holdsLock && !entry.claimedHere && !lockNotTakenHere(local, summary.id, linkedHere) &&
      !(summary.id === openId && openNeedsAnswer)) {
      survey.heldUnclaimed.push(entry);
    }
    if (summary.id === openId && openNeedsAnswer) {
      survey.openProject = entry;
      continue;
    }
    // An UNREADABLE copy is not a clean one: it may hold work, so it is treated
    // as un-uploaded rather than releasing the lock that may be guarding it.
    const unuploaded = state?.has_unpushed_edits === true || state?.state_unknown === true;
    if (unuploaded) survey.unuploaded.push(entry);
    else if (entry.holdsLock && entry.claimedHere) survey.releasable.push(entry);
  }
  return survey;
}

/**
 * Hand back every lock in `projects`, each bounded and off the main thread
 * (`speleodb_release_lock` runs on a blocking worker). Returns the ones that
 * could NOT be released, so the dialog can name them.
 */
export async function releaseSignOutLocks(projects: SignOutProject[]): Promise<SignOutProject[]> {
  const failed: SignOutProject[] = [];
  for (const project of projects) {
    try {
      await ownSpeleoRelease(project.summary.id, () => bounded(
        invokeIpc('speleodb_release_lock', { projectId: project.summary.id }, {
          purpose: 'hand a lock back before signing out',
        }),
        'Handing the project back',
      ));
      // The open project's lock, handed back here: no "Editing stopped" for it.
      if (project.summary.id === useSpeleoStore.getState().remoteProjectId) markOwnReleaseEnded();
    } catch {
      failed.push(project);
    }
  }
  return failed;
}

interface SignOutRequest {
  id: number;
  kind: SignOutKind;
  /** `null` while the first survey runs; the dialog re-surveys on "Try again". */
  survey: SignOutSurvey | null;
  /** Why the first survey failed, worded by cause (sdbdemo2 P3, screenshot 26:
      the cause used to appear only after Try again). */
  error?: string | null;
  /** Finish: `true` = go ahead and sign out, `false` = the sign-out is cancelled. */
  settle: (proceed: boolean) => void;
}

export const useSpeleoSignOut = create<{ request: SignOutRequest | null }>(() => ({ request: null }));

let nextId = 0;

function openSignOutDialog(kind: SignOutKind, survey: SignOutSurvey | null, error: string | null = null): Promise<boolean> {
  useSpeleoSignOut.getState().request?.settle(false);
  return new Promise<boolean>((resolve) => {
    const id = ++nextId;
    let settled = false;
    const settle = (proceed: boolean) => {
      if (settled) return;
      settled = true;
      if (useSpeleoSignOut.getState().request?.id === id) useSpeleoSignOut.setState({ request: null });
      resolve(proceed);
    };
    useSpeleoSignOut.setState({ request: { id, kind, survey, error, settle } });
  });
}

/**
 * Run before the token is dropped. Resolves `true` when the sign-out may go
 * ahead (nothing held, or everything answered and released - or the user chose
 * "Disconnect anyway"), `false` when it was cancelled.
 */
export async function guardSpeleoSignOut(kind: SignOutKind): Promise<boolean> {
  if (useSpeleoStore.getState().session?.loggedIn !== true) return true;
  let survey: SignOutSurvey | null;
  let error: string | null = null;
  try {
    survey = await surveySignOut();
  } catch (e) {
    // Could not look: say so, and why, rather than guess. The dialog offers Try again.
    survey = null;
    error = describeSpeleoError(e);
  }
  if (survey && !signOutHasWork(survey)) return true;
  return openSignOutDialog(kind, survey, error);
}
