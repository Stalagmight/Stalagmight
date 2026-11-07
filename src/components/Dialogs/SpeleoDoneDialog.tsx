import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { doneFinished } from '../../store/speleoStore';
import { useSpeleoActions } from '../../hooks/useSpeleoActions';
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
 * Both collect an optional message then route through `useSpeleoActions`. Closes
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
  const mode = useUIStore((s) => s.dialogData.mode) === 'push' ? 'push' : 'release';
  const { busy, push, done } = useSpeleoActions();
  const [message, setMessage] = useState('');

  const handleConfirm = async () => {
    const text = message.trim() || null;
    if (mode === 'push') {
      // Close only when the push actually LANDED. On divergence_needed /
      // push_unconfirmed the store records the outcome and the App-level effect
      // opens the resolution dialog; closing here unconditionally would discard the
      // typed message and drop the user onto the crimson button with no next step
      // (fix 3). A thrown (network) error returns null - keep the dialog for a retry.
      const resolved = await push(text);
      if (resolved?.kind === 'pushed') closeDialog();
    } else {
      // Same for release: only leave once the lock is RELEASED. A divergence_needed
      // result is truthy but must NOT close the dialog - the divergence dialog takes
      // over (fix 3); a thrown error returns null and keeps this dialog open.
      const resolved = await done(text);
      if (doneFinished(resolved?.kind)) closeDialog();
    }
  };

  return (
    <DialogWrapper
      title={mode === 'push' ? t('speleodb.done.pushTitle') : t('speleodb.done.title')}
      onClose={closeDialog}
      size="md"
      data-testid="speleodb-done-dialog"
      data-mode={mode}
    >
      <div className="space-y-4">
        <p className="text-sm text-text-secondary">
          {mode === 'push' ? t('speleodb.done.pushDescription') : t('speleodb.done.description')}
        </p>
        <div>
          <label className="block text-sm text-text-secondary mb-1">
            {t('speleodb.done.messageLabel')}
          </label>
          <textarea
            value={message}
            onChange={(e) => setMessage(e.target.value)}
            rows={3}
            className="w-full px-2 py-1 bg-background border border-border rounded text-sm"
            placeholder={t('speleodb.done.messagePlaceholder')}
            data-testid="speleodb-done-message"
          />
        </div>
        {/* #260: the failure of the LAST attempt, right above the button that made
            it - the shell banner is unreachable behind this dialog's scrim. The
            same button below is the retry; re-pressing it clears this first. */}
        <SpeleoPushFailureNotice />
        <div className="flex justify-end gap-2 pt-2 border-t border-border">
          <Button onClick={closeDialog} disabled={busy} data-testid="speleodb-done-cancel">
            {t('common.actions.cancel')}
          </Button>
          <Button
            variant="primary"
            onClick={() => void handleConfirm()}
            disabled={busy}
            data-testid="speleodb-done-confirm"
          >
            {busy
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
