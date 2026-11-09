import { useSpeleoStore } from '../../store/speleoStore';
import { useProjectStore } from '../../store/projectStore';
import { useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { useSpeleoActions } from '../../hooks/useSpeleoActions';
import { getErrorMessage } from '../../utils/ipc';
import { surfaceSaveFailure } from '../../utils/saveFeedback';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import { SpeleoPushFailureNotice } from './SpeleoPushFailureNotice';

/**
 * Commit-message confirm for the action shell (#204), in two modes read off
 * `dialogData.mode`:
 * - **release** (default): `speleodb_done` - render + VACUUM snapshot + push +
 *   release the lock, returning to read-only.
 * - **push** (T2d, fix 8): `speleodb_push` - the same push WITHOUT releasing the
 *   lock, so editing continues.
 *
 * Both collect a required message when uploading changes then route through `useSpeleoActions`. Closes
 * only when the verb actually LANDED (released / pushed). A `divergence_needed`
 * keeps the dialog's typed message alive and lets the App-level effect open the
 * resolution dialog over it (fix 3); a `push_unconfirmed` keeps it open for a retry
 * (the #200 banner surfaces the ambiguity); a thrown (network) error likewise keeps
 * it open so the message and a retry survive.
 *
 * [#260] A thrown error is ALSO rendered INLINE here (`SpeleoPushFailureNotice`):
 * the shell's push-failure banner sits behind this dialog's modal scrim, so it was
 * unreadable and unclickable exactly when it mattered. The retry is this dialog's
 * own primary button; the standalone banner takes over only once the dialog is
 * dismissed with the failure unresolved.
 */
export default function SpeleoDoneDialog() {
  const { t } = useTranslation();
  const closeDialog = useUIStore((s) => s.closeDialog);
  const dialogData = useUIStore((s) => s.dialogData);
  const mode = dialogData.mode === 'push' ? 'push' : 'release';
  const { busy, push, done } = useSpeleoActions();
  const [message, setMessage] = useState('');
  const [submitting, setSubmitting] = useState(false);
  const [saveFailure, setSaveFailure] = useState<string | null>(null);
  const attempt = useRef(false);
  const busyAny = busy || submitting;
  const unpushed = useSpeleoStore((s) => s.link?.hasUnpushedEdits);
  const unsaved = useProjectStore((s) => s.isDirty);
  const needsMessage = mode === 'push' || unpushed || unsaved;

  const handleConfirm = async () => {
    const text = message.trim();
    if (attempt.current || busy || (needsMessage && !text)) return;
    attempt.current = true;
    setSubmitting(true);
    setSaveFailure(null);
    const state = useProjectStore.getState();
    const path = state.project?.file_path;
    const op = state.loading.clientOpId;
    const generation = useSpeleoStore.getState().sessionGeneration;
    const current = () => useProjectStore.getState().project?.file_path === path &&
      useProjectStore.getState().loading.clientOpId === op &&
      useSpeleoStore.getState().sessionGeneration === generation &&
      useUIStore.getState().activeDialog === 'speleodb-done' &&
      useUIStore.getState().dialogData === dialogData;
    try {
      if (mode === 'push') {
        let saved;
        try { saved = await state.saveProject(); }
        catch (error) {
          if (current()) surfaceSaveFailure(getErrorMessage(error), useUIStore.getState().openDialog);
          return;
        }
        if (!current()) return;
        if (!saved?.success || saved.unlinked_copy_path) {
          surfaceSaveFailure(saved?.error ?? 'Could not save the linked project', useUIStore.getState().openDialog);
          return;
        }
        const resolved = await push(text, { isCurrent: current });
        if (!current()) return;
        if (resolved?.kind === 'pushed') closeDialog();
        else setSaveFailure(resolved === null || resolved.kind === 'push_unconfirmed'
          ? 'Saved locally; upload could not be confirmed.'
          : 'Saved locally, but not uploaded to SpeleoDB.');
      } else {
        const resolved = await done(text, { isCurrent: current });
        if (current() && resolved?.kind === 'released') closeDialog();
      }
    } finally { attempt.current = false; setSubmitting(false); }
  };

  return (
    <DialogWrapper
      title={mode === 'push' ? t('speleodb.done.pushTitle') : t('speleodb.done.title')}
      onClose={busyAny ? () => {} : closeDialog}
      size="md"
      data-testid="speleodb-done-dialog"
      data-mode={mode}
    >
      <div className="space-y-4">
        <p className="text-sm text-text-secondary">
          {mode === 'push' ? t('speleodb.done.pushDescription') : t('speleodb.done.description')}
        </p>
        <div>
          <label htmlFor="speleodb-done-message" className="block text-sm text-text-secondary mb-1">
            {t('speleodb.done.messageLabel')}
          </label>
          <textarea
            id="speleodb-done-message"
            disabled={busyAny}
            value={message}
            onChange={(e) => setMessage(e.target.value)}
            required
            rows={3}
            className="w-full px-2 py-1 bg-background border border-border rounded text-sm"
            placeholder={t('speleodb.done.messagePlaceholder')}
            data-testid="speleodb-done-message"
          />
        </div>
        {/* #260: the failure of the LAST attempt, right above the button that made
            it - the shell banner is unreachable behind this dialog's scrim. The
            same button below is the retry; re-pressing it clears this first. */}
        {saveFailure && <p role="alert" data-testid="speleodb-save-upload-failure">{saveFailure}</p>}
        <SpeleoPushFailureNotice />
        <div className="flex justify-end gap-2 pt-2 border-t border-border">
          <Button onClick={closeDialog} disabled={busyAny} data-testid="speleodb-done-cancel">
            {t('common.actions.cancel')}
          </Button>
          <Button
            variant="primary"
            onClick={() => void handleConfirm()}
            disabled={busyAny || (needsMessage && !message.trim())}
            data-testid="speleodb-done-confirm"
          >
            {busyAny
              ? t('speleodb.done.pushing')
              : mode === 'push'
                ? t('speleodb.done.pushConfirm')
                : t('speleodb.done.confirm')}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
