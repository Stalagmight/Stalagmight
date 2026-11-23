import i18n from '../i18n';
import { emailsMatch } from './email';
import { permissionAtLeast } from './speleoPermission';
import type { SpeleoLinkStatus } from '../types/ipc.generated';
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
let ownReleasesInFlight = 0;

/** Run `work`, a release this session starts, under the marker. */
export async function ownSpeleoRelease<T>(work: () => Promise<T>): Promise<T> {
  ownReleaseEpoch += 1;
  ownReleasesInFlight += 1;
  try {
    return await work();
  } finally {
    ownReleasesInFlight -= 1;
    ownReleaseEpoch += 1;
  }
}

/** Taken when a status refresh is issued, for {@link ownReleaseSince}. */
export function ownReleaseMark(): number {
  return ownReleaseEpoch;
}

/** Whether a release of this session's own ran, or is running, since `mark`. */
export function ownReleaseSince(mark: number): boolean {
  return ownReleasesInFlight > 0 || ownReleaseEpoch !== mark;
}
