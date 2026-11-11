import i18n from '../i18n';
import { useProjectStore } from '../store/projectStore';
import { useSpeleoStore } from '../store/speleoStore';
import { useUIStore } from '../store/uiStore';
import { emailsMatch } from './email';
import { endpointsMatch } from './speleoEndpoint';
import { registerWorkspaceDraft, type AccountTarget } from './workspaceDrafts';

interface UploadAccount { endpoint: string; email: string }
export interface UploadOwner { path: string | null; epoch: number; account: UploadAccount | null }
interface UploadDraft { path: string; message: string; closeAfterSave: boolean }
let continuation: (UploadDraft & { owner: UploadOwner }) | null = null;
// A description the user set aside with the upload dialog's "Resolve now", to fix
// the survey's errors first. Unlike `continuation` it is not a pending upload:
// nothing reopens the dialog for it (finishSpeleoSettings reads only the
// continuation) and no exit prompt counts it as a draft. The next upload dialog
// (or the push-failure banner's Retry) starts from it and takes it over by
// writing a continuation; it lapses once nothing is left to upload.
let setAside: { path: string; message: string; owner: UploadOwner } | null = null;
let projectEpoch = 0;
let subscribed = false;

function currentAccount(): UploadAccount | null {
  const session = useSpeleoStore.getState().session;
  return session?.loggedIn && session.endpoint && session.userEmail
    ? { endpoint: session.endpoint, email: session.userEmail.trim().toLowerCase() }
    : null;
}

/** Subscribe lazily: stores import this module while their own initialization runs. */
function observeProjectLifetime(): void {
  if (subscribed) return;
  subscribed = true;
  useProjectStore.subscribe((next, previous) => {
    if (!!next.project !== !!previous.project ||
        next.project?.file_path !== previous.project?.file_path ||
        next.project?.created_at !== previous.project?.created_at) {
      projectEpoch += 1;
      continuation = null;
      setAside = null;
    }
  });
  useSpeleoStore.subscribe(() => {
    if (continuation && !uploadOwnerStillCurrent(continuation.owner)) continuation = null;
    if (setAside && !uploadOwnerStillCurrent(setAside.owner)) setAside = null;
  });
}

export function captureUploadOwner(): UploadOwner {
  observeProjectLifetime();
  return { path: useProjectStore.getState().project?.file_path ?? null, epoch: projectEpoch, account: currentAccount() };
}

/** Reauthentication may recover the same account, but never retarget a draft. */
export function uploadOwnerStillCurrent(owner: UploadOwner): boolean {
  observeProjectLifetime();
  if (!useProjectStore.getState().project || owner.epoch !== projectEpoch ||
      owner.path !== (useProjectStore.getState().project?.file_path ?? null)) return false;
  const account = currentAccount();
  if (!account) return true;
  if (!owner.account) { owner.account = account; return true; }
  return owner.account.email === account.email && endpointsMatch(owner.account.endpoint, account.endpoint);
}

/**
 * Whether moving to `target` drops a draft owned by `owner` - the account half of
 * {@link uploadOwnerStillCurrent}, asked before the move instead of after it.
 */
export function uploadOwnerDroppedBy(owner: UploadOwner, target: AccountTarget): boolean {
  // Signed out, a draft waits: the next sign-in decides.
  if (target.signedOut) return false;
  const account = owner.account ?? currentAccount();
  // A draft not tied to an account yet adopts the one that signs in.
  if (!account) return false;
  // A pasted token names its account only once the server has answered.
  if (target.email === null) return true;
  return !emailsMatch(account.email, target.email) ||
    !endpointsMatch(account.endpoint, target.endpoint ?? account.endpoint);
}

/** Recovery may replace the upload dialog. Reopening still requires confirmation. */
export function getUploadContinuation(path: string | null | undefined) {
  observeProjectLifetime();
  if (continuation && (continuation.path !== path || !uploadOwnerStillCurrent(continuation.owner))) continuation = null;
  return continuation;
}
export function setUploadContinuation(value: UploadDraft | null, owner = captureUploadOwner()): void {
  continuation = value && value.path === owner.path && uploadOwnerStillCurrent(owner) ? { ...value, owner } : null;
  if (value) setAside = null;
}

/** Once the work is on SpeleoDB, a description written for it describes nothing. */
function nothingToUpload(): boolean {
  return !useProjectStore.getState().isDirty && !useSpeleoStore.getState().link?.hasUnpushedEdits;
}

/** "Resolve now": keep the typed description for the next upload dialog, with no upload pending. */
export function setAsideUploadMessage(message: string, owner: UploadOwner): void {
  continuation = null;
  setAside = owner.path !== null && message.trim() && uploadOwnerStillCurrent(owner)
    ? { path: owner.path, message, owner }
    : null;
}

/** The description "Resolve now" set aside for this project, if it still belongs to it. */
export function getSetAsideUploadMessage(path: string | null | undefined): string | null {
  observeProjectLifetime();
  if (setAside && (setAside.path !== path || !uploadOwnerStillCurrent(setAside.owner) || nothingToUpload())) setAside = null;
  return setAside?.message ?? null;
}

// The description typed for an upload is unsent input until the upload lands: the
// open upload dialog mirrors it here, and it waits here while Settings is open for
// sign-in or an error replaced the dialog. A description with nothing left to
// upload loses nothing, and one being uploaded right now is not a draft.
registerWorkspaceDraft({
  label: () => i18n.t('speleoExitPrompt.drafts.upload'),
  dirty: (account) => {
    const pending = getUploadContinuation(useProjectStore.getState().project?.file_path);
    if (!pending?.message.trim() || useSpeleoStore.getState().busy) return false;
    if (!useProjectStore.getState().isDirty && !useSpeleoStore.getState().link?.hasUnpushedEdits) return false;
    return account ? uploadOwnerDroppedBy(pending.owner, account) : true;
  },
  discard: () => {
    continuation = null;
    const ui = useUIStore.getState();
    if (ui.activeDialog === 'speleodb-done') ui.closeDialog();
  },
});
