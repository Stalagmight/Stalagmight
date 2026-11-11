/**
 * Unsent SpeleoDB input ("workspace drafts"): what the user typed into a SpeleoDB
 * form that has not been sent yet - the description of an upload, the conversion
 * (publish) form - including a draft parked while Settings is open for sign-in.
 *
 * Each draft registers itself where it lives (`uploadContinuation`,
 * `publishContinuation`, the publish dialog). Every way out that would drop one
 * asks about it in-app (`utils/exitConfirm`), never in a native dialog, and ONE
 * question per step:
 *
 * - close, switch and quit ask {@link confirmWorkspaceDrafts} first, before the
 *   SpeleoDB exit prompt can upload, release or close anything;
 * - where the way out also asks about unsaved survey edits (the open from
 *   SpeleoDB, an account change), the drafts are folded into THAT question
 *   ({@link workspaceDraftNotice}) instead of a second one; its answers that go
 *   on - save on this computer, or discard - drop them.
 *
 * Once answered the drafts are discarded, so the later store-level checks
 * (`loadProject`, `createProject`, `closeProject`) find none and do not ask again.
 */
import i18n from '../i18n';
import { askExitConfirm } from './exitConfirm';

/**
 * The account a sign-in or sign-out moves to. `email: null` is a sign-in whose
 * account is not known yet (a pasted token resolves it on the server).
 */
export type AccountTarget =
  | { signedOut: true }
  | { signedOut?: false; email: string | null; endpoint: string | null };

export interface WorkspaceDraft {
  /** What the draft is, as it reads in the question's list. */
  label: () => string;
  /**
   * Holds unsent input. With an `account` it answers the narrower question
   * "does moving to this account drop it": a draft stays bound to the account it
   * was written for, so a sign-out keeps it and a different account drops it.
   */
  dirty: (account?: AccountTarget) => boolean;
  /** Drop the input (and close the form it lives in, if open). */
  discard: () => void;
}

const drafts = new Map<symbol, WorkspaceDraft>();

/** Register a draft; the returned function unregisters it. */
export function registerWorkspaceDraft(draft: WorkspaceDraft): () => void {
  const id = Symbol('workspace-draft');
  drafts.set(id, draft);
  return () => { drafts.delete(id); };
}

function pendingDrafts(account?: AccountTarget): WorkspaceDraft[] {
  return [...drafts.values()].filter((draft) => draft.dirty(account));
}

export function hasWorkspaceDrafts(account?: AccountTarget): boolean {
  return pendingDrafts(account).length > 0;
}

/** Discard the drafts this way out drops (every draft without an `account`). */
export function discardWorkspaceDrafts(account?: AccountTarget): void {
  for (const draft of pendingDrafts(account)) draft.discard();
}

function draftItems(pending: WorkspaceDraft[]): string {
  return [...new Set(pending.map((draft) => draft.label()))].join(', ');
}

/**
 * The sentence another exit question appends when this way out also drops unsent
 * SpeleoDB input, or `null` when it drops none. Whatever that question's answer,
 * if the way out goes on, call {@link discardWorkspaceDrafts} with the same
 * `account`.
 */
export function workspaceDraftNotice(account?: AccountTarget): string | null {
  const pending = pendingDrafts(account);
  return pending.length ? i18n.t('speleoExitPrompt.drafts.alsoLost', { items: draftItems(pending) }) : null;
}

/**
 * Ask, in-app, whether to discard the unsent SpeleoDB input this way out would
 * drop. `true` = nothing to lose, or the user discarded it; `false` = cancelled
 * (or the flow stopped being current while asking), nothing discarded.
 */
export async function confirmWorkspaceDrafts(
  isCurrent: () => boolean = () => true,
  account?: AccountTarget,
): Promise<boolean> {
  if (!isCurrent()) return false;
  const pending = pendingDrafts(account);
  if (!pending.length) return true;
  const items = draftItems(pending);
  const discard = await askExitConfirm({
    title: i18n.t('speleoExitPrompt.drafts.title'),
    message: i18n.t('speleoExitPrompt.drafts.message', { items }),
    confirmLabel: i18n.t('speleoExitPrompt.drafts.confirm'),
    cancelLabel: i18n.t('common.actions.cancel'),
  });
  if (!discard || !isCurrent()) return false;
  for (const draft of pending) draft.discard();
  return true;
}
