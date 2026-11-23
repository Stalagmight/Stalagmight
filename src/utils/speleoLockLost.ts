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
