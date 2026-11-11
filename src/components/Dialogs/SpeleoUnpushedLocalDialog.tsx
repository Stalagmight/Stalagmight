import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { useOpenRemoteProject } from '../../hooks/useOpenRemoteProject';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import type { SpeleoProjectSummary, UnpushedLocalEdits } from '../../types/ipc.generated';

/**
 * Refuse to overwrite unsynced work. Raised when the server has moved past the
 * local copy's base (or the base cannot be checked), and when the copy is not
 * recorded as the signed-in account's (another account's, or one downloaded before
 * owners were recorded): only this account's un-uploaded work sitting on the
 * server tip opens directly instead. "Open my local copy" opens it as it is (no
 * download) so the user can carry on through the ordinary resume / upload flows;
 * Cancel preserves it; replacing it requires confirmation.
 *
 * When the copy's owner record names ANOTHER account (`other_owner`), the edits
 * are that account's: the prompt names it instead of saying "your changes", and
 * the non-destructive answer becomes "Open it read-only" - no lock is taken, and
 * the backend refuses Start Editing on it, so the work can be looked at and saved
 * as a copy but never edited or uploaded as this account's.
 */
export default function SpeleoUnpushedLocalDialog() {
  const { t } = useTranslation();
  const closeDialog = useUIStore((s) => s.closeDialog);
  const openDialog = useUIStore((s) => s.openDialog);
  const dialogData = useUIStore((s) => s.dialogData);
  // A dead token here must drop the session, exactly as every other consumer of
  // `takeRemoteDiscardingLocal`'s outcome does (`OpenFromSpeleoDbDialog` routes it
  // through `surfaceOpenError`). Reading only `outcome.message` left the shell
  // claiming a session the backend no longer has.
  const clearSession = useSpeleoStore((s) => s.clearSession);
  // Close the prompt right before loadProject so its progress is visible (the same
  // hand-off the browser dialog makes).
  const { takeRemoteDiscardingLocal, openLocalCopy } = useOpenRemoteProject({
    onBeforeLoad: closeDialog,
  });

  const [busy, setBusy] = useState(false);
  const [discardArmed, setDiscardArmed] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    setBusy(false);
    setDiscardArmed(false);
    setError(null);
  }, [dialogData]);
  const stillPrompting = () => useUIStore.getState().activeDialog === 'speleodb-unpushed-local' &&
    useUIStore.getState().dialogData === dialogData;

  const summary = dialogData.summary as SpeleoProjectSummary | undefined;
  const unpushedLocal = dialogData.unpushedLocal as UnpushedLocalEdits | undefined;
  // Which dialog displaced us in the single `activeDialog` slot, if any (#270).
  const cancelReturnsTo =
    typeof dialogData.cancelReturnsTo === 'string' ? dialogData.cancelReturnsTo : null;
  if (!summary || !unpushedLocal) return null;

  /**
   * Cancel / X / backdrop. Hands the user back to the dialog this prompt replaced
   * (the SpeleoDB browser), because cancelling here opened nothing: dropping to
   * `activeDialog: null` left the browser gone, no project opened, and no way back
   * except re-opening the menu. Hosts that are not dialogs pass nothing and get the
   * plain close, which correctly reveals the view underneath.
   */
  const dismiss = () => {
    if (cancelReturnsTo) openDialog(cancelReturnsTo);
    else closeDialog();
  };

  // The copy's owner record names another account: the edits are that account's
  // work. Say so, and open it read-only rather than as this account's to edit.
  const owner = unpushedLocal.other_owner ?? null;

  const lastEditAt = unpushedLocal.last_edit_at;
  const parsed = lastEditAt ? new Date(lastEditAt) : null;
  const lastEditedLine =
    parsed && !Number.isNaN(parsed.getTime())
      ? t('speleodb.unpushedLocal.lastEdited', { when: parsed.toLocaleString() })
      : t('speleodb.unpushedLocal.lastEditedUnknown');

  // Both branches keep this prompt open with the reason on screen when they fail:
  // closing it would leave the user with nothing opened and no explanation.
  const settleOutcome = (outcome: Awaited<ReturnType<typeof openLocalCopy>>) => {
    if (!stillPrompting()) return;
    if (outcome.status === 'cancelled' || outcome.status === 'load_failed') {
      setBusy(false);
    } else if (outcome.status === 'error') {
      setError(outcome.message);
      // A `ReLogin` outcome means the token is gone; keeping the session would have
      // the shell offering lock verbs against an identity the backend has dropped.
      if (outcome.reLogin) clearSession();
      setBusy(false);
    }
  };

  // Non-destructive: open the copy on this computer, untouched. Nothing is
  // downloaded and nothing is thrown away.
  const handleOpenLocal = async () => {
    setBusy(true);
    setError(null);
    settleOutcome(await openLocalCopy(summary, unpushedLocal));
  };

  // Destructive branch. The discard stays armed so a failed attempt's button is a
  // retry, not a re-arm.
  const handleTakeRemote = async () => {
    setBusy(true);
    setError(null);
    settleOutcome(await takeRemoteDiscardingLocal(summary));
  };

  return (
    <DialogWrapper
      title={t('speleodb.unpushedLocal.title')}
      // Not dismissable while a branch is in flight - the same hardening
      // SpeleoQuitDialog / SpeleoCloseDialog carry. DialogWrapper routes BOTH
      // Escape and a backdrop click here, so an Escape while
      // `speleodb_open_project_discarding_local` was awaiting unmounted the only
      // surface its error had: `setError` landed on an unmounted component and a
      // failed discard-my-edits open produced no message anywhere.
      onClose={busy ? () => {} : dismiss}
      // Wide enough for the three answers on one footer line, armed or not.
      size="lg"
      data-testid="speleodb-unpushed-local-dialog"
    >
      <div className="space-y-4">
        <p className="text-sm text-text-secondary" data-testid="speleodb-unpushed-local-message">
          {owner
            ? t('speleodb.unpushedLocal.messageOtherAccount', { name: summary.name, owner })
            : t('speleodb.unpushedLocal.message', { name: summary.name })}
        </p>
        <p className="text-sm text-text-primary" data-testid="speleodb-unpushed-local-last-edited">
          {lastEditedLine}
        </p>
        {/* What each answer does, said once in the body (the app's dialog layout). */}
        <p className="text-sm text-text-secondary" data-testid="speleodb-unpushed-local-open-hint">
          {owner
            ? t('speleodb.unpushedLocal.choicesOtherAccount', { owner })
            : t('speleodb.unpushedLocal.choices')}
        </p>

        {discardArmed && (
          <div
            className="rounded border border-error bg-error-20 text-error text-sm px-3 py-2"
            role="alert"
            data-testid="speleodb-unpushed-local-discard-warning"
          >
            {t('speleodb.unpushedLocal.discardWarning', { name: summary.name })}
          </div>
        )}

        {error && (
          <div
            className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm"
            role="alert"
            data-testid="speleodb-unpushed-local-error"
          >
            {error}
          </div>
        )}

        <div className="flex flex-wrap justify-end gap-2 pt-4 border-t border-border" data-testid="speleodb-unpushed-local-footer">
          <Button onClick={dismiss} disabled={busy} data-testid="speleodb-unpushed-local-cancel">
            {t('common.actions.cancel')}
          </Button>
          <Button
            variant="danger"
            onClick={() => {
              if (!discardArmed) {
                setDiscardArmed(true);
                return;
              }
              void handleTakeRemote();
            }}
            disabled={busy}
            data-testid="speleodb-unpushed-local-take-remote"
          >
            {owner
              ? t(discardArmed
                ? 'speleodb.unpushedLocal.takeRemoteConfirmOtherAccount'
                : 'speleodb.unpushedLocal.takeRemoteOtherAccount')
              : t(discardArmed ? 'speleodb.unpushedLocal.takeRemoteConfirm' : 'speleodb.unpushedLocal.takeRemote')}
          </Button>
          <Button variant="primary" data-primary-action="" onClick={() => void handleOpenLocal()} disabled={busy}
            data-testid="speleodb-unpushed-local-open-local">
            {owner ? t('speleoExitPrompt.unpushedLocal.openReadOnly') : t('speleoExitPrompt.unpushedLocal.openLocal')}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
