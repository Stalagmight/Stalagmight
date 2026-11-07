import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import i18n from '../../i18n';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { invokeIpc, getErrorMessage } from '../../utils/ipc';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import DeleteShotImpactDetails from './DeleteShotImpactDetails';
import type { DeleteShotImpact } from '../../types/commands';
import { setSource, trackDialogComplete } from '../../utils/telemetry';
import { useShallow } from 'zustand/react/shallow';

export default function DeleteShotChoiceDialog() {
  const { t } = useTranslation();
  const { closeDialog, dialogData, openDialog, clearSelection } = useUIStore(
    useShallow((s) => ({
      closeDialog: s.closeDialog,
      dialogData: s.dialogData,
      openDialog: s.openDialog,
      clearSelection: s.clearSelection,
    })),
  );
  const deleteShot = useProjectStore((s) => s.deleteShot);

  const sectionId = dialogData.sectionId as number | undefined;
  const shotId = dialogData.shotId as number | undefined;
  const sectionName = dialogData.sectionName as string | undefined;

  const [impact, setImpact] = useState<DeleteShotImpact | null>(null);
  const [impactError, setImpactError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [isLoading, setIsLoading] = useState(false);
  const shiftBlocked = impact?.shift_blocked ?? false;
  const shiftBlockedReasons = impact?.shift_blocked_reasons ?? [];

  useEffect(() => {
    if (sectionId !== undefined && shotId !== undefined) {
      invokeIpc('preview_delete_shot', { sectionId, shotId }, { purpose: 'preview shot deletion' })
        .then((result) => {
          setImpact(result);
          setImpactError(null);
        })
        .catch((e) => {
          setImpact(null);
          // i18n.t, not the hook's t: this IPC effect must not re-fetch on a language change.
          setImpactError(getErrorMessage(e, i18n.t('dialogs.deleteShotChoice.errors.loadImpactsFailed')));
        });
    } else {
      setImpact(null);
      setImpactError(null);
    }
  }, [sectionId, shotId]);

  const handleSplit = () => {
    if (sectionId === undefined || shotId === undefined) return;
    setSource('dialog');
    openDialog('split-section', {
      sectionId,
      shotId,
      sectionName,
    });
  };

  const handleShift = async () => {
    if (sectionId === undefined || shotId === undefined) return;
    setIsLoading(true);
    setError(null);
    try {
      const result = await deleteShot(sectionId, shotId);
      if (result.success) {
        clearSelection();
        trackDialogComplete('delete-shot-choice');
        closeDialog();
      } else {
        setError(result.error ?? t('dialogs.deleteShotChoice.errors.deleteShot'));
      }
    } catch (e) {
      setError(getErrorMessage(e, t('dialogs.deleteShotChoice.errors.deleteShot')));
    } finally {
      setIsLoading(false);
    }
  };

  return (
    <DialogWrapper title={t('dialogs.deleteShotChoice.title')} onClose={isLoading ? () => {} : closeDialog} size="md" data-testid="delete-shot-choice-dialog">
      <div className="space-y-4">
        <p className="text-text-secondary">
          {t('dialogs.deleteShotChoice.intro')}
        </p>

        {error && (
          <div className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm">
            {error}
          </div>
        )}

        {impactError && (
          <div className="p-3 bg-warning-10 border border-warning-20 rounded text-warning text-sm">
            {impactError}
          </div>
        )}

        {shiftBlocked && (
          <div className="p-3 bg-warning-10 border border-warning-20 rounded text-warning text-sm">
            <p className="font-medium">{t('dialogs.deleteShotChoice.shiftUnavailable')}</p>
            {shiftBlockedReasons.length > 0 && (
              <ul className="list-disc list-inside">
                {shiftBlockedReasons.map((reason, index) => (
                  <li key={`shift-blocked-${index}`}>{reason}</li>
                ))}
              </ul>
            )}
          </div>
        )}

        <DeleteShotImpactDetails
          impact={impact}
          title={t('dialogs.deleteShotChoice.impactTitle')}
        />

        <div className="flex justify-end gap-2">
          <Button onClick={closeDialog} disabled={isLoading} data-testid="delete-shot-choice-cancel">
            {t('common.actions.cancel')}
          </Button>
          <Button onClick={handleSplit} disabled={isLoading} data-testid="delete-shot-choice-split">
            {t('dialogs.deleteShotChoice.split')}
          </Button>
          <Button
            data-testid="delete-shot-choice-shift"
            variant="danger"
            onClick={handleShift}
            disabled={isLoading || shiftBlocked}
            data-primary-action=""
          >
            {isLoading ? t('dialogs.deleteShotChoice.deleting') : t('dialogs.deleteShotChoice.shift')}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
