import type { TFunction } from 'i18next';
import type { SpeleoRevision } from '../types/ipc.generated';

/** The short revision id the Revision History list shows. */
const SHORT_SHA = 8;

/**
 * [R3] The report of an upload that landed on top of revisions another session
 * uploaded meanwhile (the backend's `replaced_revisions`, newest first).
 *
 * SpeleoDB's lock is the account's, so a second session of the same account
 * holds it too. The push refuses to upload when SpeleoDB's latest revision is not
 * the copy's own base, but SpeleoDB has no conditional upload: a revision landing
 * between that check and the upload is found only afterwards, in the history.
 * The sentence says what happened (naming the revision the way the Revision
 * History shows it: its description, its short id, who uploaded it), that its
 * changes are not in ours, and where it still is.
 */
export function replacedRevisionsNotice(t: TFunction, revisions: readonly SpeleoRevision[]): string {
  const [latest] = revisions;
  if (!latest) return '';
  const sha = latest.id.slice(0, SHORT_SHA);
  const message = latest.message?.trim();
  return t('speleodb.replaced.notice', {
    count: revisions.length,
    name: message ? t('speleodb.replaced.named', { message, sha }) : sha,
    author: latest.author_email.trim() || t('speleodb.revisions.unknownAuthor'),
  });
}
