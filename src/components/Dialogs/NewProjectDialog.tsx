import { useState } from 'react';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { getErrorMessage } from '../../utils/ipc';
import { trackDialogComplete } from '../../utils/telemetry';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';

export default function NewProjectDialog() {
  const { closeDialog } = useUIStore();
  const { setView } = useUIStore();
  const openDialog = useUIStore((s) => s.openDialog);
  const { createProject } = useProjectStore();
  // Only offer the follow-up SpeleoDB publish when signed in.
  const loggedIn = useSpeleoStore((s) => s.session?.loggedIn ?? false);

  const [name, setName] = useState('');
  const [projectLocation, setProjectLocation] = useState('');
  const [alsoPublish, setAlsoPublish] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [isCreating, setIsCreating] = useState(false);

  const handleCreate = async () => {
    if (!name.trim()) {
      setError('Project name is required');
      return;
    }

    setIsCreating(true);
    setError(null);

    try {
      const result = await createProject(
        name.trim(),
        projectLocation.trim() || undefined
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
        setError(result.error ?? 'Failed to create project');
      }
    } catch (e) {
      setError(getErrorMessage(e, 'An error occurred'));
    } finally {
      setIsCreating(false);
    }
  };

  // Track if form has been modified
  const isDirty = name.trim() !== '' || projectLocation.trim() !== '' || alsoPublish;

  return (
    <DialogWrapper title="New Project" onClose={closeDialog} size="md" isDirty={isDirty} data-testid="new-project-dialog">
      <div className="space-y-4">
        {error && (
          <div className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm">
            {error}
          </div>
        )}

        <div>
          <label className="block text-sm text-text-secondary mb-1">
            Project Name *
          </label>
          <input
            type="text"
            value={name}
            onChange={(e) => setName(e.target.value)}
            className="w-full"
            placeholder="e.g., Lechuguilla Cave"
            data-testid="new-project-name-input"
          />
        </div>

        <div>
          <label className="block text-sm text-text-secondary mb-1">
            Location (optional)
          </label>
          <input
            type="text"
            value={projectLocation}
            onChange={(e) => setProjectLocation(e.target.value)}
            className="w-full"
            placeholder="e.g., Carlsbad Caverns National Park, NM"
            data-testid="new-project-location-input"
          />
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
              Create as SpeleoDB project
            </label>
          </div>
        )}

        <div className="flex justify-end gap-2 pt-4 border-t border-border">
          <Button onClick={closeDialog} disabled={isCreating} data-testid="new-project-cancel-button">
            Cancel
          </Button>
          <Button
            variant="primary"
            onClick={handleCreate}
            disabled={isCreating || !name.trim()}
            data-testid="new-project-create-button"
          >
            {isCreating ? 'Creating...' : 'Create Project'}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
