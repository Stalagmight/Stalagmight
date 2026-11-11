import { useState, useMemo, useCallback } from 'react';
import { useTranslation } from 'react-i18next';
import { saveDialog } from '../../utils/fileDialogs';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { getErrorMessage } from '../../utils/ipc';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import type { ProjectMetadata } from '../../types/project';
import { ensureExtension, projectSaveFilters, PROJECT_EXTENSION } from '../../utils/fileDialogs';
import { formatFixed } from '../../i18n/format';
import { useDecimalPrecision } from '../../hooks/useDecimalPrecision';
import { trackDialogComplete, setProjectActionSource } from '../../utils/telemetry';
import { TOAST_KEYS } from '../../utils/toastKeys';

/**
 * MergeProjectsDialog allows users to merge multiple projects into a single new project.
 * It receives the selected projects via dialogData.selectedProjects.
 */
export default function MergeProjectsDialog() {
  const { t } = useTranslation();
  const { closeDialog, dialogData, showToast } = useUIStore();
  const { mergeProjects, loadProject } = useProjectStore();
  const precision = useDecimalPrecision();

  const selectedProjects = dialogData.selectedProjects as ProjectMetadata[] | undefined;

  const [outputName, setOutputName] = useState(() => {
    // Generate a default name from the first project
    if (selectedProjects && selectedProjects.length > 0 && selectedProjects[0]) {
      return `${selectedProjects[0].name}_merged`;
    }
    return 'merged_project';
  });
  const [archiveOriginals, setArchiveOriginals] = useState(false);
  const [openAfterMerge, setOpenAfterMerge] = useState(true);
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Calculate totals for display
  const totals = useMemo(() => {
    if (!selectedProjects || selectedProjects.length === 0) {
      return { sections: 0, length: 0 };
    }
    return selectedProjects.reduce(
      (acc, p) => ({
        sections: acc.sections + p.section_count,
        length: acc.length + p.total_length,
      }),
      { sections: 0, length: 0 }
    );
  }, [selectedProjects]);

  // Format length for display
  const formatLength = (meters: number): string => {
    if (meters >= 1000) {
      return `${formatFixed(meters / 1000, undefined, precision.length)} km`;
    }
    return `${formatFixed(meters, undefined, precision.length)} m`;
  };

  const handleClose = useCallback(() => {
    closeDialog();
  }, [closeDialog]);

  const handleMerge = useCallback(async () => {
    if (!selectedProjects || selectedProjects.length < 2) return;
    if (!outputName.trim()) {
      setError('Please enter a name for the merged project');
      return;
    }

    setIsLoading(true);
    setError(null);

    try {
      // Get output directory from the first project's location
      const firstProject = selectedProjects[0];
      if (!firstProject) {
        setError('No projects selected');
        setIsLoading(false);
        return;
      }
      const firstProjectPath = firstProject.file_path;
      const defaultDir = firstProjectPath.substring(0, firstProjectPath.lastIndexOf('/'));
      const defaultFileName = `${outputName.trim()}.stalx`;

      // Show save dialog to let user choose output location
      const outputPath = await saveDialog({
        title: 'Save Merged Project',
        defaultPath: `${defaultDir}/${defaultFileName}`,
        filters: projectSaveFilters(),
      });

      if (!outputPath) {
        // User cancelled the save dialog
        setIsLoading(false);
        return;
      }

      const finalPath = ensureExtension(outputPath, PROJECT_EXTENSION);
      const sourcePaths = selectedProjects.map((p) => p.file_path);
      const result = await mergeProjects(sourcePaths, finalPath, outputName.trim(), archiveOriginals);

      if (result.success) {
        trackDialogComplete('merge-projects');
        closeDialog();
        // A successful merge can still carry a warning (e.g. a no-clobber
        // entrance-elevation mismatch, #448) - the dialog is about to close, so
        // this is the only place watching `result.warnings`. Per
        // docs/notifications.md the dialog has already closed and there is no
        // decision left to make, so this rides a toast, not a second modal.
        // Only the first warning is surfaced: the backend itself only ever
        // reports the first disagreement.
        const [firstWarning] = result.warnings ?? [];
        if (firstWarning) {
          showToast(
            t('dialogs.mergeProjects.warningToast', { message: firstWarning.message }),
            TOAST_KEYS.MERGE_WARNING,
            'warning'
          );
        }
        // Open the merged project if requested
        // loadProject handles view switching internally
        if (openAfterMerge) {
          setProjectActionSource('dialog');
          await loadProject(finalPath);
        }
      } else {
        setError(result.error ?? 'Failed to merge projects');
      }
    } catch (e) {
      setError(getErrorMessage(e, 'Failed to merge projects'));
    } finally {
      setIsLoading(false);
    }
  }, [selectedProjects, outputName, archiveOriginals, openAfterMerge, mergeProjects, closeDialog, loadProject]);

  if (!selectedProjects || selectedProjects.length < 2) {
    return null;
  }

  return (
    <DialogWrapper
      title="Merge Projects"
      onClose={handleClose}
      size="lg"
      data-testid="merge-projects-dialog"
    >
      <div className="space-y-4">
        {error && (
          <div className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm">
            {error}
          </div>
        )}

        {/* Projects to merge */}
        <div>
          <h3 className="text-sm font-medium text-text-secondary mb-2">
            Projects to merge ({selectedProjects.length})
          </h3>
          <div className="space-y-2 max-h-48 overflow-auto border border-border rounded p-2">
            {selectedProjects.map((project) => (
              <div
                key={project.file_path}
                className="flex justify-between items-center py-1 px-2 bg-background-50 rounded"
              >
                <div className="truncate flex-1">
                  <span className="font-medium">{project.name}</span>
                  <span className="text-text-secondary text-xs ml-2">
                    ({project.section_count} sections, {formatLength(project.total_length)})
                  </span>
                </div>
              </div>
            ))}
          </div>
        </div>

        {/* Totals */}
        <div className="p-3 bg-background rounded">
          <div className="flex justify-between text-sm">
            <span className="text-text-secondary">Total sections:</span>
            <span className="font-medium">{totals.sections}</span>
          </div>
          <div className="flex justify-between text-sm">
            <span className="text-text-secondary">Total length:</span>
            <span className="font-medium">{formatLength(totals.length)}</span>
          </div>
        </div>

        {/* Output name */}
        <div>
          <label htmlFor="merge-output-name" className="block text-sm font-medium mb-1">
            Merged project name
          </label>
          <input
            id="merge-output-name"
            type="text"
            value={outputName}
            onChange={(e) => setOutputName(e.target.value)}
            className="w-full px-3 py-2 bg-background border border-border rounded focus:outline-none focus-visible:ring-1 focus-visible:ring-accent"
            placeholder="Enter project name"
            data-testid="merge-output-name-input"
          />
        </div>

        {/* Options */}
        <div className="space-y-2">
          <label className="flex items-center gap-2 cursor-pointer">
            <input
              type="checkbox"
              checked={archiveOriginals}
              onChange={(e) => setArchiveOriginals(e.target.checked)}
              className="rounded border-border"
              data-testid="merge-archive-checkbox"
            />
            <span className="text-sm">
              Archive original project files (rename to .stalx.archived)
            </span>
          </label>
          <label className="flex items-center gap-2 cursor-pointer">
            <input
              type="checkbox"
              checked={openAfterMerge}
              onChange={(e) => setOpenAfterMerge(e.target.checked)}
              className="rounded border-border"
              data-testid="merge-open-checkbox"
            />
            <span className="text-sm">Open merged project after creation</span>
          </label>
        </div>

        {/* Info about merge behavior */}
        <div className="p-3 bg-accent-10 border border-accent-20 rounded text-sm">
          <p className="text-text-secondary">
            Merging combines all sections from the selected projects into a new project file.
            Projects with a GPS zero point are placed at their real positions; a project without
            one keeps its own layout and may need connecting afterwards.
          </p>
        </div>

        {/* Actions */}
        <div className="flex justify-end gap-2 pt-4 border-t border-border">
          <Button onClick={handleClose} disabled={isLoading} data-testid="merge-projects-cancel">
            Cancel
          </Button>
          <Button
            variant="primary"
            onClick={handleMerge}
            disabled={isLoading || !outputName.trim()}
            data-testid="merge-projects-confirm-button"
          >
            {isLoading ? 'Merging...' : 'Merge Projects'}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
