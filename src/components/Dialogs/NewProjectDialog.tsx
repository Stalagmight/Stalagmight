import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { getErrorMessage } from '../../utils/ipc';
import { trackDialogComplete } from '../../utils/telemetry';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import { ensureExtension, projectSaveFilters, saveDialog, PROJECT_EXTENSION } from '../../utils/fileDialogs';

export default function NewProjectDialog() {
  const { t } = useTranslation();
  const closeDialog = useUIStore((s) => s.closeDialog);
  const setView = useUIStore((s) => s.setView);
  const openDialog = useUIStore((s) => s.openDialog);
  const createProject = useProjectStore((s) => s.createProject);
  // Only offer the follow-up SpeleoDB publish when signed in.
  const loggedIn = useSpeleoStore((s) => s.session?.loggedIn ?? false);

  const [name, setName] = useState('');
  const [projectLocation, setProjectLocation] = useState('');
  const [saveImmediately, setSaveImmediately] = useState(false);
  const [alsoPublish, setAlsoPublish] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [isCreating, setIsCreating] = useState(false);

  const handleCreate = async () => {
    if (!name.trim()) {
      setError(t('dialogs.newProject.errors.nameRequired'));
      return;
    }

    setIsCreating(true);
    setError(null);

    try {
      let explicitSavePath: string | undefined;
      if (saveImmediately) {
        const path = await saveDialog({
          title: t('dialogs.newProject.saveDialogTitle'),
          filters: projectSaveFilters(),
          defaultPath: `${name.replace(/[^a-zA-Z0-9]/g, '_')}.stalx`,
        });
        if (path) {
          explicitSavePath = ensureExtension(path, PROJECT_EXTENSION);
        }
      }

      const result = await createProject(
        name.trim(),
        projectLocation.trim() || undefined,
        explicitSavePath ? { savePath: explicitSavePath } : undefined
      );

      if (result.success) {
        // Switch to single system view
        setView('single');
        trackDialogComplete('new-project');
        // When asked (and signed in), hand off to the Publish dialog instead of just
        // closing - it replaces this dialog via the single activeDialog slot.
        if (alsoPublish && loggedIn) {
          openDialog('publish-to-speleodb');
        } else {
          closeDialog();
        }
      } else {
        setError(result.error ?? t('dialogs.newProject.errors.createFailed'));
      }
    } catch (e) {
      setError(getErrorMessage(e, t('dialogs.newProject.errors.unknown')));
    } finally {
      setIsCreating(false);
    }
  };

  // Track if form has been modified
  const isDirty = name.trim() !== '' || projectLocation.trim() !== '' || saveImmediately || alsoPublish;

  return (
    <DialogWrapper title={t('dialogs.newProject.title')} onClose={closeDialog} size="md" isDirty={isDirty} data-testid="new-project-dialog">
      <div className="space-y-4">
        {error && (
          <div className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm">
            {error}
          </div>
        )}

        <div>
          <label className="block text-sm text-text-secondary mb-1">
            {t('dialogs.newProject.nameLabel')}
          </label>
          <input
            type="text"
            value={name}
            onChange={(e) => setName(e.target.value)}
            className="w-full"
            placeholder={t('dialogs.newProject.namePlaceholder')}
            data-testid="new-project-name-input"
          />
        </div>

        <div>
          <label className="block text-sm text-text-secondary mb-1">
            {t('dialogs.newProject.locationLabel')}
          </label>
          <input
            type="text"
            value={projectLocation}
            onChange={(e) => setProjectLocation(e.target.value)}
            className="w-full"
            placeholder={t('dialogs.newProject.locationPlaceholder')}
            data-testid="new-project-location-input"
          />
        </div>

        <div className="flex items-center gap-2">
          <input
            type="checkbox"
            id="saveImmediately"
            checked={saveImmediately}
            onChange={(e) => setSaveImmediately(e.target.checked)}
            className="rounded"
            data-testid="new-project-save-immediately"
          />
          <label htmlFor="saveImmediately" className="text-sm">
            {t('dialogs.newProject.saveImmediately')}
          </label>
        </div>

        {loggedIn && (
          <div className="flex items-center gap-2">
            <input
              type="checkbox"
              id="alsoPublish"
              checked={alsoPublish}
              onChange={(e) => setAlsoPublish(e.target.checked)}
              className="rounded"
              data-testid="new-project-also-publish"
            />
            <label htmlFor="alsoPublish" className="text-sm">
              {t('dialogs.newProject.alsoPublish')}
            </label>
          </div>
        )}

        <div className="flex justify-end gap-2 pt-4 border-t border-border">
          <Button onClick={closeDialog} disabled={isCreating} data-testid="new-project-cancel-button">
            {t('common.actions.cancel')}
          </Button>
          <Button
            variant="primary"
            onClick={handleCreate}
            disabled={isCreating || !name.trim()}
            data-testid="new-project-create-button"
          >
            {isCreating ? t('dialogs.newProject.creating') : t('dialogs.newProject.create')}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
