import { useState, useEffect } from 'react';
import { useTranslation } from 'react-i18next';
import i18n from '../../i18n';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { getErrorMessage } from '../../utils/ipc';
import { trackDialogComplete } from '../../utils/telemetry';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import { useShallow } from 'zustand/react/shallow';

export default function SplitSectionDialog() {
  const { t } = useTranslation();
  const { closeDialog, dialogData, setSelection } = useUIStore(
    useShallow((s) => ({ closeDialog: s.closeDialog, dialogData: s.dialogData, setSelection: s.setSelection })),
  );
  const deleteShotAndSplit = useProjectStore((s) => s.deleteShotAndSplit);

  const sectionId = dialogData.sectionId as number | undefined;
  const shotId = dialogData.shotId as number | undefined;
  const sectionName = dialogData.sectionName as string | undefined;

  const defaultNameA = sectionName ? `${sectionName}-A` : t('dialogs.splitSection.defaultNameA');
  const defaultNameB = sectionName ? `${sectionName}-B` : t('dialogs.splitSection.defaultNameB');

  const [nameA, setNameA] = useState(defaultNameA);
  const [nameB, setNameB] = useState(defaultNameB);
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Re-initialize names when sectionName changes. i18n.t, not the hook's t:
  // a language change must not reset names the user typed.
  useEffect(() => {
    setNameA(sectionName ? `${sectionName}-A` : i18n.t('dialogs.splitSection.defaultNameA'));
    setNameB(sectionName ? `${sectionName}-B` : i18n.t('dialogs.splitSection.defaultNameB'));
  }, [sectionName]);

  const isDirty = nameA !== defaultNameA || nameB !== defaultNameB;

  const isValid = nameA.trim().length > 0 && nameB.trim().length > 0;

  const handleConfirm = async () => {
    if (sectionId === undefined || shotId === undefined || !isValid) return;

    setIsLoading(true);
    setError(null);

    try {
      const result = await deleteShotAndSplit(sectionId, shotId, nameA.trim(), nameB.trim());
      if (result.success) {
        // Select section A (keeps original ID) after split
        setSelection({ type: 'section', sectionId });
        trackDialogComplete('split-section');
        closeDialog();
      } else {
        setError(result.error ?? t('dialogs.splitSection.errors.splitSection'));
      }
    } catch (e) {
      setError(getErrorMessage(e, t('dialogs.splitSection.errors.splitSection')));
    } finally {
      setIsLoading(false);
    }
  };

  const handleClose = isLoading ? () => {} : closeDialog;

  return (
    <DialogWrapper
      title={t('dialogs.splitSection.title')}
      onClose={handleClose}
      size="md"
      isDirty={isDirty}
      closeOnBackdropClick={!isLoading}
      data-testid="split-section-dialog"
    >
      <div className="space-y-4">
        <p className="text-text-secondary">
          {t('dialogs.splitSection.intro')}
        </p>

        {error && (
          <div className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm">
            {error}
          </div>
        )}

        <div className="space-y-3">
          <div>
            <label className="block text-sm text-text-secondary mb-1">{t('dialogs.splitSection.nameA')}</label>
            <input
              type="text"
              value={nameA}
              onChange={(e) => setNameA(e.target.value)}
              className="w-full"
              data-testid="split-name-a"
              disabled={isLoading}
            />
          </div>
          <div>
            <label className="block text-sm text-text-secondary mb-1">{t('dialogs.splitSection.nameB')}</label>
            <input
              type="text"
              value={nameB}
              onChange={(e) => setNameB(e.target.value)}
              className="w-full"
              data-testid="split-name-b"
              disabled={isLoading}
            />
          </div>
        </div>

        <div className="flex justify-end gap-2 pt-4 border-t border-border">
          <Button onClick={handleClose} disabled={isLoading} data-testid="split-section-cancel">
            {t('common.actions.cancel')}
          </Button>
          <Button
            variant="danger"
            onClick={handleConfirm}
            disabled={isLoading || !isValid}
            data-testid="split-confirm"
          >
            {isLoading ? t('dialogs.splitSection.splitting') : t('dialogs.splitSection.confirm')}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
