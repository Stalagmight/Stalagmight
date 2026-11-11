import i18n from '../i18n';
import { useUIStore } from '../store/uiStore';
import { captureUploadOwner, uploadOwnerDroppedBy, uploadOwnerStillCurrent, type UploadOwner } from './uploadContinuation';
import { registerWorkspaceDraft, type AccountTarget } from './workspaceDrafts';

export interface PublishDraft { description: string; country: string; latitude: string; longitude: string }
let pending: { draft: PublishDraft; owner: UploadOwner } | null = null;

/** Keep a creation form through Settings, bound to its original project/account. */
export function setPublishContinuation(draft: PublishDraft | null, owner = captureUploadOwner()): void {
  pending = draft && uploadOwnerStillCurrent(owner) ? { draft, owner } : null;
}
export function getPublishContinuation(): PublishDraft | null {
  if (pending && !uploadOwnerStillCurrent(pending.owner)) pending = null;
  return pending?.draft ?? null;
}

/** The creation form holds input - the same test its own dismiss prompt uses. */
export function publishDraftHasInput(draft: PublishDraft): boolean {
  return [draft.description, draft.country, draft.latitude, draft.longitude].some((value) => value.trim() !== '');
}

/**
 * The open creation form as a draft, unless {@link publishDraftHasInput} is false.
 * Discarded, it stops counting at once: the form it closes unmounts (and
 * unregisters) only on React's next render.
 */
export function publishFormDraft(owner: UploadOwner, form: () => PublishDraft | null) {
  let discarded = false;
  return {
    label: () => i18n.t('speleoExitPrompt.drafts.publish'),
    dirty: (account?: AccountTarget) => {
      const draft = form();
      if (discarded || !draft || !publishDraftHasInput(draft) || !uploadOwnerStillCurrent(owner)) return false;
      return account ? uploadOwnerDroppedBy(owner, account) : true;
    },
    discard: () => {
      discarded = true;
      pending = null;
      const ui = useUIStore.getState();
      if (ui.activeDialog === 'publish-to-speleodb') ui.closeDialog();
    },
  };
}

// The creation form parked while Settings is open for sign-in.
registerWorkspaceDraft({
  label: () => i18n.t('speleoExitPrompt.drafts.publish'),
  dirty: (account) => {
    const parked = pending;
    return !!parked && publishFormDraft(parked.owner, () => parked.draft).dirty(account);
  },
  discard: () => { pending = null; },
});
