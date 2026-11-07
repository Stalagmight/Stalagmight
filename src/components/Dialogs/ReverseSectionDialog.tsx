import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { isEditingBlockedNow } from '../../hooks/useEditingBlocked';
import { getErrorMessage } from '../../utils/ipc';
import { trackDialogComplete } from '../../utils/telemetry';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import { useShallow } from 'zustand/react/shallow';


/**
 * Confirm dialog for reversing one or more sections end-to-end (#24).
 *
 * Reverse flips which physical end of a section is station 0 without moving the
 * cave: every station keeps its world coordinate, and merges/closures/datums/
 * comments follow their physical station. The one thing the app cannot decide
 * on the user's behalf is left/right - whether LRUD was recorded facing along
 * travel is a survey convention - so it is an explicit, unticked choice here.
 */
export default function ReverseSectionDialog() {
  const { t } = useTranslation();
  const { closeDialog, dialogData } = useUIStore(
    useShallow((s) => ({ closeDialog: s.closeDialog, dialogData: s.dialogData })),
  );
  const { reverseSection, executeBatch, project } = useProjectStore(
    useShallow((s) => ({ reverseSection: s.reverseSection, executeBatch: s.executeBatch, project: s.project })),
  );

  const sectionIds = (dialogData.sectionIds as number[] | undefined) ?? [];
  const [swapLeftRight, setSwapLeftRight] = useState(false);
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Everything below counts RESOLVED sections, never `sectionIds`. A section
  // deleted between opening the menu and confirming would otherwise put
  // "Reverse 3 Sections" over a 2-item list and then submit a batch the backend
  // rolls back whole. The stale entry disables confirm instead of silently
  // reverse-ing a different set than the one on screen.
  const sections = sectionIds
    .map((id) => project?.sections.find((s) => s.id === id))
    .filter((s): s is NonNullable<typeof s> => s !== undefined);
  const hasMissing = sections.length < sectionIds.length;
  // The sections this dialog would ACTUALLY renumber. A shot-less section is a single station
  // and reverses to itself, so the backend treats it as a no-op - counting it in the title, the
  // confirm label or the enabled state would promise work that will not happen, and selecting
  // only such sections would enable Confirm, fire a completed-reverse telemetry event and close
  // the dialog having done nothing.
  const changing = sections.filter((s) => s.shots.length > 0);
  const changingCount = changing.length;

  const isMulti = changingCount > 1;
  const title = isMulti ? t('dialogs.reverseSection.titleMulti', { count: changingCount }) : t('dialogs.reverseSection.title');

  const handleConfirm = async () => {
    if (isSubmitting || changingCount === 0 || hasMissing) return;

    // Read-only self-check at confirm time: a reverse has NO server backstop for
    // the local closure-adjusted / Plan / Export case (see useEditingBlocked's
    // backstop-asymmetry note), so a dialog left open across a view switch to a
    // read-only mode must refuse here rather than mutate. Re-checked now, not at
    // open time, because the mode can flip while the dialog is up.
    if (isEditingBlockedNow()) {
      setError(t('dialogs.reverseSection.editingDisabled'));
      return;
    }

    // A section with no shots is a single station and reverses to itself, so the
    // backend deliberately changes nothing (and does not even touch the project).
    // Sending it anyway would mark the project dirty - and cost an Undo - for an
    // action that provably did nothing.
    if (changing.length === 0) {
      trackDialogComplete('reverse-section');
      closeDialog();
      return;
    }

    setIsSubmitting(true);
    setError(null);
    // Captured BEFORE the await. Escape closes this dialog (useKeyboardShortcuts /
    // DialogWrapper) without cancelling the command already in flight, so a late
    // reply must not close whatever dialog is open by then, nor emit a second
    // trackDialogComplete. `dialogData` is a fresh object per openDialog call, so
    // reference identity distinguishes even a re-opened Reverse dialog.
    const dialogKey = dialogData;
    try {
      // Reversing a multi-selection is one user action, so it goes through a
      // batch: the backend rolls the whole thing back if any section fails, and
      // it lands as a single undo entry. Looping one command per section would
      // leave the project half-reversed on a mid-loop failure while reporting an
      // error, and would need one Undo per section to get back.
      const only = changing[0];
      const result =
        changing.length === 1 && only !== undefined
          ? await reverseSection(only.id, swapLeftRight)
          : await executeBatch(
              changing.map((s) => ({
                type: 'reverse_section' as const,
                section_id: s.id,
                swap_left_right: swapLeftRight,
              }))
            );
      if (!result.success) {
        setError(result.error ?? t('dialogs.reverseSection.reverseFailed'));
        setIsSubmitting(false);
        return;
      }
      // The aftermath - mirroring the frontend's own positional refs - is applied by
      // `executeCommand`/`executeUndoRedo` in the store, immediately after the refresh it
      // belongs to. Doing it here instead meant a continuation that outlives the dialog
      // (Escape closes without cancelling the in-flight command) could land AFTER a
      // concurrent undo had already mirrored the pins back, re-applying the forward shift
      // onto restored numbering - in range, and silent.

      if (useUIStore.getState().dialogData !== dialogKey) return;
      trackDialogComplete('reverse-section');
      closeDialog();
    } catch (e) {
      setError(getErrorMessage(e));
      setIsSubmitting(false);
    }
  };

  return (
    <DialogWrapper title={title} onClose={closeDialog} size="md" data-testid="reverse-section-dialog">
      <div className="space-y-4">
        <p className="text-text-secondary">
          {isMulti
            ? t('dialogs.reverseSection.introMulti', { count: changingCount })
            : t('dialogs.reverseSection.intro')}
        </p>

        {sections.length > 0 && (
          <ul
            className="list-disc list-inside text-sm text-text-secondary"
            data-testid="reverse-section-list"
          >
            {sections.map((s) => (
              <li key={s.id}>
                {s.name ?? t('dialogs.reverseSection.sectionFallback', { id: s.id })}
                {s.shots.length > 0 && (
                  <span className="text-text-tertiary">
                    {' '}
                    {t('dialogs.reverseSection.stationMapping', { last: s.shots.length })}
                  </span>
                )}
              </li>
            ))}
          </ul>
        )}

        <p className="text-sm text-text-tertiary">
          {t('dialogs.reverseSection.caveDoesNotMove')}
        </p>

        <label className="flex items-start gap-2 text-sm cursor-pointer">
          <input
            type="checkbox"
            checked={swapLeftRight}
            onChange={(e) => setSwapLeftRight(e.target.checked)}
            className="mt-0.5"
            data-testid="reverse-swap-lr"
          />
          <span>
            <span className="text-text-primary">{t('dialogs.reverseSection.swapLeftRight')}</span>
            <span className="block text-text-tertiary">
              {t('dialogs.reverseSection.swapLeftRightHint')}
            </span>
          </span>
        </label>

        {hasMissing && (
          <div
            className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm"
            data-testid="reverse-stale"
          >
            {t('dialogs.reverseSection.stale')}
          </div>
        )}

        {error && (
          <div
            className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm"
            data-testid="reverse-error"
          >
            {error}
          </div>
        )}

        <div className="flex justify-end gap-2 pt-4 border-t border-border">
          <Button onClick={closeDialog} data-testid="reverse-section-cancel">{t('common.actions.cancel')}</Button>
          <Button
            variant="primary"
            onClick={handleConfirm}
            disabled={isSubmitting || changingCount === 0 || hasMissing}
            data-primary-action=""
            data-testid="reverse-confirm"
          >
            {isMulti ? t('dialogs.reverseSection.confirmMulti', { count: changingCount }) : t('dialogs.reverseSection.confirm')}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
