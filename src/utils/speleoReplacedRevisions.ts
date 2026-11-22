import type { TFunction } from 'i18next';
import type { DoneResult, PushResult, SpeleoRevision } from '../types/ipc.generated';

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

/**
 * [R3] The upload landed, but on top of revisions another session uploaded
 * meanwhile: `applyDone` / `applyPush` raised the banner that names them.
 *
 * Every way out of the project that ran the upload stops on it, so that banner
 * is read before the project closes: the exit prompt and the sign-out, Save and
 * Close, and the older-version confirmation finishing a close or quit. The lock
 * is back and nothing is left to upload, so the next close asks nothing and
 * proceeds. One predicate, so no close path can drift from the others.
 */
export function landedOverReplacedRevisions(result: DoneResult | PushResult | null | undefined): boolean {
  if (result?.kind !== 'released' && result?.kind !== 'pushed') return false;
  return (result.replaced_revisions?.length ?? 0) > 0;
}
