import i18n from '../i18n';
import { emailsMatch } from './email';
import { permissionAtLeast } from './speleoPermission';
import type { EditingStopped, SpeleoLinkStatus } from '../types/ipc.generated';
import type { SpeleoLinkState } from '../store/speleoStore';

/**
 * What to tell a session whose edit session a status refresh just ended because
 * SpeleoDB says the lock is no longer this account's, or its access no longer
 * allows editing (sdbdemo2 issue 4, screenshot 21). `null` when there is nothing
 * to say.
 *
 * SpeleoDB's lock belongs to the account, so a close, quit or Done in another
 * session of the same account hands back the lock this one edits under (the plan's
 * Left list: no release can tell the account's sessions apart). The status
 * refresh is where this session next learns it, and it used to switch to
 * "Ready for editing" and read-only with no word.
 *
 * Only a reply that heard from SpeleoDB (a permission came back) and was not
 * refused (`access_denied` has its own surface) says anything, and only when the
 * session was editing before the refresh and the reply ended it. A reply that
 * still names this account as the holder says nothing here: the gate shut for
 * another reason, whose own surface reports it.
 */
export function speleoLockLostMessage(
  before: SpeleoLinkState | null,
  status: SpeleoLinkStatus,
  myEmail: string | null,
): string | null {
  if (before?.editState !== 'editing' || !status.is_remote || status.edit_enabled) return null;
  if (status.access_denied || status.permission === null) return null;
  if (!permissionAtLeast(status.permission, 'READ_AND_WRITE')) {
    return i18n.t('speleodb.readOnlyNotice.lockLostPermission');
  }
  if (!status.lock_holder) return i18n.t('speleodb.readOnlyNotice.lockLostReleased');
  if (!emailsMatch(status.lock_holder, myEmail)) {
    return i18n.t('speleodb.readOnlyNotice.lockLostTaken', { email: status.lock_holder });
  }
  return null;
}

/**
 * What to tell a session whose in-place reload (Take the SpeleoDB version,
 * Reload Project) came back read-only because SpeleoDB, asked right before it,
 * no longer recorded this account's lock (`editing_stopped`; farm finding
 * P2-take-remote-editable). Worded like "Editing stopped" after a status
 * refresh, but about the SpeleoDB version now on screen, and never "your
 * changes are kept": Take the SpeleoDB version has just discarded them.
 */
export function editingStoppedMessage(stopped: EditingStopped): string {
  return stopped.kind === 'held_by'
    ? i18n.t('speleodb.readOnlyNotice.reloadLockTaken', { email: stopped.email })
    : i18n.t('speleodb.readOnlyNotice.reloadLockReleased');
}

/**
 * Releases this session starts itself (Discard and hand back the lock, Done,
 * Switch to Read Only, the held-lock prompt's and the sign-out's per-project
 * releases, the revert's unwind, a close or a switch settling the lock). A status
 * refresh that overlaps one reads "no lock for this account" because this session
 * just gave it back, which is no loss: it must not raise "Editing stopped"
 * (pre-PR review of the X3 round, must-fix 1). The callers also mark the session
 * read-only once their own release has landed, so a refresh issued after it never
 * sees an edit session to lose.
 */
let ownReleaseEpoch = 0;
/** Per project: releases of it in flight, and the epoch it was last touched at. */
const ownReleases = new Map<string, { inFlight: number; touched: number }>();
/** The key of a release whose project is not known: it overlaps every refresh. */
const ANY_PROJECT = '';

function touch(key: string, inFlight: number): void {
  ownReleaseEpoch += 1;
  const entry = ownReleases.get(key) ?? { inFlight: 0, touched: 0 };
  ownReleases.set(key, { inFlight: entry.inFlight + inFlight, touched: ownReleaseEpoch });
}

/**
 * Run `work`, a release this session starts of `projectId`'s lock, under the
 * marker. Keyed by project (delta review of X4, should-fix 1): a release of
 * another project's lock (the held-lock prompt's, the sign-out loop's) says
 * nothing about the open project's, and swallowed its genuine "Editing
 * stopped". `null` is a release whose project is not known, which overlaps
 * every refresh.
 */
export async function ownSpeleoRelease<T>(projectId: string | null, work: () => Promise<T>): Promise<T> {
  const key = projectId ?? ANY_PROJECT;
  touch(key, 1);
  try {
    return await work();
  } finally {
    touch(key, -1);
  }
}

/** Taken when a status refresh is issued, for {@link ownReleaseSince}. */
export function ownReleaseMark(): number {
  return ownReleaseEpoch;
}

/**
 * Whether a release of this session's own of `projectId`'s lock (or of an
 * unknown project's) ran, or is running, since `mark`. A refresh whose project
 * is not known counts every release, as the marker did before it was keyed.
 */
export function ownReleaseSince(mark: number, projectId: string | null): boolean {
  const overlaps = (entry: { inFlight: number; touched: number } | undefined) =>
    entry !== undefined && (entry.inFlight > 0 || entry.touched > mark);
  if (projectId === null) return [...ownReleases.values()].some(overlaps);
  return overlaps(ownReleases.get(projectId)) || overlaps(ownReleases.get(ANY_PROJECT));
}
