import { useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { getErrorMessage } from '../../utils/ipc';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import { ensureExtension, projectSaveFilters, saveDialog, PROJECT_EXTENSION } from '../../utils/fileDialogs';
import type { DmpFile, TmlFile, GpxFile, CompassFile, WallsFile } from '../../types/commands';
import { setSource, trackDialogComplete, peekProjectActionSource, setProjectActionSource } from '../../utils/telemetry';
import { computeTmlImportStats } from '../../utils/tmlImportStats';
import { useShallow } from 'zustand/react/shallow';

/**
 * New Project dialog variant used during the DMP/TML import flow.
 *
 * This dialog is shown when importing a file from the home screen (no project open).
 * After creating the project, it automatically opens the appropriate import dialog
 * to continue the import flow.
 */
export default function NewProjectForImportDialog() {
  const { t } = useTranslation();
  const { closeDialog, dialogData, openDialog, setImportContext } = useUIStore(
    useShallow((s) => ({
      closeDialog: s.closeDialog,
      dialogData: s.dialogData,
      openDialog: s.openDialog,
      setImportContext: s.setImportContext,
    })),
  );
  const createProject = useProjectStore((s) => s.createProject);

  const dmpFile = dialogData.dmpFile as DmpFile | undefined;
  const tmlFile = dialogData.tmlFile as TmlFile | undefined;
  const gpxFile = dialogData.gpxFile as GpxFile | undefined;
  const compassFile = dialogData.compassFile as CompassFile | undefined;
  const wallsFile = dialogData.wallsFile as WallsFile | undefined;
  const filePath = dialogData.filePath as string | undefined;
  const fileName = useMemo(
    () => (filePath ? filePath.split(/[/\\]/).pop() || filePath : ''),
    [filePath]
  );
  // Default-options count: exactly what the user gets if they click straight
  // through to the next screen without touching its toggles (#411).
  const tmlStats = useMemo(
    () => (tmlFile ? computeTmlImportStats(tmlFile, false) : null),
    [tmlFile]
  );
  const importType = dialogData.importType as string | undefined;
  const defaultName = (dialogData.defaultName as string) || '';
  const isTml = importType === 'tml' || !!tmlFile;
  const isGpx = importType === 'gpx' || !!gpxFile;
  const isCompass = importType === 'compass' || !!compassFile;
  const isWalls = importType === 'walls' || !!wallsFile;

  const [name, setName] = useState(defaultName);
  const [projectLocation, setProjectLocation] = useState('');
  const [savePath, setSavePath] = useState('');
  const [documentsDir, setDocumentsDir] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [isCreating, setIsCreating] = useState(false);

  const normalizedBaseName = useMemo(
    () => (name.trim() || defaultName || t('dialogs.newProjectForImport.fallbackFileName')).replace(/[^a-zA-Z0-9]/g, '_'),
    [defaultName, name, t]
  );

  const joinPath = (base: string, child: string): string => {
    const separator = base.includes('\\') ? '\\' : '/';
    const trimmedBase = base.replace(/[\\/]+$/, '');
    return `${trimmedBase}${separator}${child}`;
  };

  const truncatePathFromLeft = (path: string, maxChars: number): string => {
    if (path.length <= maxChars) {
      return path;
    }
    return `...${path.slice(path.length - (maxChars - 3))}`;
  };

  const resolvedDefaultSavePath = useMemo(() => {
    if (!documentsDir) {
      return null;
    }
    return joinPath(joinPath(documentsDir, 'stalagmight'), `${normalizedBaseName}.stalx`);
  }, [documentsDir, normalizedBaseName]);

  const displayedDefaultSavePath = useMemo(() => {
    if (!resolvedDefaultSavePath) {
      return null;
    }
    return truncatePathFromLeft(resolvedDefaultSavePath, 40);
  }, [resolvedDefaultSavePath]);

  useEffect(() => {
    let active = true;
    const loadDocumentsDir = async () => {
      try {
        const { documentDir, homeDir } = await import('@tauri-apps/api/path');

        try {
          const dir = await documentDir();
          if (active && typeof dir === 'string' && dir.trim()) {
            setDocumentsDir(dir);
            return;
          }
        } catch {
          // Fall through to a best-effort home directory fallback.
        }

        const home = await homeDir();
        if (active && typeof home === 'string' && home.trim()) {
          setDocumentsDir(joinPath(home, 'Documents'));
        }
      } catch {
        // Optional enhancement only; keep path unset if resolution fails.
      }
    };
    void loadDocumentsDir();
    return () => {
      active = false;
    };
  }, []);

  const handleSelectSavePath = async () => {
    try {
      const selected = await saveDialog({
        title: t('dialogs.newProjectForImport.saveDialogTitle'),
        filters: projectSaveFilters(),
        defaultPath: savePath.trim() || resolvedDefaultSavePath || `${normalizedBaseName}.stalx`,
      });
      if (typeof selected === 'string' && selected.trim()) {
        setSavePath(ensureExtension(selected, PROJECT_EXTENSION));
      }
    } catch (e) {
      setError(getErrorMessage(e, t('dialogs.newProjectForImport.errors.saveDialogFailed')));
    }
  };

  const handleCreate = async () => {
    if (!name.trim()) {
      setError(t('dialogs.newProjectForImport.errors.nameRequired'));
      return;
    }

    if (!filePath || (!dmpFile && !tmlFile && !gpxFile && !compassFile && !wallsFile)) {
      setError(t('dialogs.newProjectForImport.errors.missingImportData'));
      return;
    }

    setIsCreating(true);
    setError(null);

    try {
      // Capture the entry-point source before createProject: its setView('single')
      // clears the pending project-action source, but the follow-up import command
      // in THIS same flow (import_sections / execute*Import) peeks the SAME source.
      // Re-arm it after the project is created so the import is attributed to the
      // same entry point as create_project_full (the import command clears it).
      const importSource = peekProjectActionSource();
      // `deferInitialSave` unconditionally (#394 F14): this dialog is a STEP in
      // an import flow, not the New Project command, and the import that
      // justifies the project has not run yet. Writing the `.stalx` here made a
      // wizard abandoned at the file-selection or anchor step leave an EMPTY
      // project file behind, plus a Recent Projects entry pointing at it.
      // Deferring reserves the path and writes nothing; the file and the recents
      // entry appear on the first import that lands
      // (`finalize_deferred_import`). Without this flag the store infers
      // `deferred = !savePath`, so only the "pick a location later" branch was
      // ever safe.
      //
      // It does NOT make the whole wizard transactional, and does not claim to.
      // Once the eager anchor import lands, the file is real; cancelling after
      // that removes the sections and saves the emptied project through
      // (`withdrawImportedSections`), and the file stays where the user asked
      // for it - see docs/dmp-import.md.
      const result = await createProject(
        name.trim(),
        projectLocation.trim() || undefined,
        {
          ...(savePath.trim() ? { savePath: ensureExtension(savePath.trim(), PROJECT_EXTENSION) } : {}),
          deferInitialSave: true,
        }
      );

      if (result.success) {
        if (importSource) setProjectActionSource(importSource);
        trackDialogComplete('new-project-for-import');
        closeDialog();
        if (isGpx && gpxFile) {
          // Open GPX selection dialog
          setSource('dialog');
          openDialog('gpx-selection', { gpxFile, filePath });
        } else if (isTml && tmlFile) {
          // Open TML import dialog
          setSource('dialog');
          openDialog('tml-import', { tmlFile, filePath });
        } else if (isCompass && compassFile) {
          // Open Compass import dialog
          setSource('dialog');
          openDialog('compass-import', { compassFile, filePath });
        } else if (isWalls && wallsFile) {
          // Open Walls import dialog
          setSource('dialog');
          openDialog('walls-import', { wallsFile, filePath });
        } else if (dmpFile) {
          // Open DMP selection dialog
          setSource('dialog');
          openDialog('dmp-selection', { dmpFile, filePath });
        }
      } else {
        setError(result.error ?? t('dialogs.newProjectForImport.errors.createFailed'));
      }
    } catch (e) {
      setError(getErrorMessage(e, t('dialogs.newProjectForImport.errors.unknown')));
    } finally {
      setIsCreating(false);
    }
  };

  const handleCancel = () => {
    // Clear import context when canceling
    setImportContext(null);
    closeDialog();
  };

  // Track if form has been modified from defaults
  const isDirty =
    name.trim() !== defaultName ||
    projectLocation.trim() !== '' ||
    savePath.trim() !== '';

  return (
    <DialogWrapper
      title={t('dialogs.newProjectForImport.title')}
      onClose={handleCancel}
      size="md"
      isDirty={isDirty}
      data-testid="new-project-for-import-dialog"
    >
      <div className="space-y-4">
        {/* Each format's summary line is composed from pluralised count
            sub-keys joined by summary.separator, rather than one fully
            composed sentence per format/locale - en is the only locale
            today, so this is the pragmatic fix for #447's plural bug. A
            locale whose grammar can't just join clauses with a fixed
            separator (count/noun order, or noun-dependent joiners) would
            need per-format composed keys instead; not needed yet. */}
        {dmpFile && (
          <div className="p-3 bg-accent-10 border border-accent-20 rounded text-sm">
            <div className="font-medium mb-1">{t('dialogs.newProjectForImport.importingFrom.dmp')}</div>
            <div className="text-text-secondary">
              {t('dialogs.newProjectForImport.summary.sections', { count: dmpFile.sections.length })}
              {t('dialogs.newProjectForImport.summary.separator')}
              {t('dialogs.newProjectForImport.summary.shots', {
                count: dmpFile.sections.reduce(
                  (acc, s) => acc + s.shots.filter((sh) => sh.shot_type === 'Standard').length,
                  0
                ),
              })}
            </div>
          </div>
        )}
        {tmlFile && tmlStats && (
          <div className="p-3 bg-accent-10 border border-accent-20 rounded text-sm">
            <div className="font-medium mb-1">{t('dialogs.newProjectForImport.importingFrom.tml')}</div>
            <div className="text-text-secondary">
              {t('dialogs.newProjectForImport.summary.sections', { count: tmlStats.totalSections })}
              {t('dialogs.newProjectForImport.summary.separator')}
              {t('dialogs.newProjectForImport.summary.shots', { count: tmlStats.totalShots })}
              {tmlFile.cave_name
                ? `${t('dialogs.newProjectForImport.summary.separator')}${tmlFile.cave_name}`
                : ''}
            </div>
          </div>
        )}
        {gpxFile && (
          <div className="p-3 bg-accent-10 border border-accent-20 rounded text-sm">
            <div className="font-medium mb-1">{t('dialogs.newProjectForImport.importingFrom.gpx')}</div>
            <div className="text-text-secondary">
              {t('dialogs.newProjectForImport.summary.waypoints', { count: gpxFile.waypoints.length })}
              {t('dialogs.newProjectForImport.summary.separator')}
              {t('dialogs.newProjectForImport.summary.tracks', { count: gpxFile.tracks.length })}
              {t('dialogs.newProjectForImport.summary.separator')}
              {t('dialogs.newProjectForImport.summary.routes', { count: gpxFile.routes.length })}
              {gpxFile.name
                ? `${t('dialogs.newProjectForImport.summary.separator')}${gpxFile.name}`
                : ''}
            </div>
          </div>
        )}
        {compassFile && (
          <div className="p-3 bg-accent-10 border border-accent-20 rounded text-sm">
            <div className="font-medium mb-1">{t('dialogs.newProjectForImport.importingFrom.compass')}</div>
            <div className="text-text-secondary">
              {t('dialogs.newProjectForImport.summary.surveys', { count: compassFile.survey_count })}
              {t('dialogs.newProjectForImport.summary.separator')}
              {/* This dialog has no include-excluded / apply-corrections toggles
                  of its own - it hands off to CompassImportDialog, which
                  defaults to `include_excluded: false, apply_corrections:
                  true` - so the honest landing count here subtracts both
                  converter drop paths under that default pair (#437, review
                  of #550: `invalid_shot_counts.non_excluded_corrected` is the
                  ONE of the four counters that matches those defaults),
                  matching what the next dialog will show before the user
                  changes anything. */}
              {t('dialogs.newProjectForImport.summary.shots', {
                count:
                  compassFile.shot_count -
                  compassFile.excluded_shot_count -
                  compassFile.invalid_shot_counts.non_excluded_corrected,
              })}
              {compassFile.cave_name
                ? `${t('dialogs.newProjectForImport.summary.separator')}${compassFile.cave_name}`
                : ''}
            </div>
          </div>
        )}
        {wallsFile && (
          <div className="p-3 bg-accent-10 border border-accent-20 rounded text-sm">
            <div className="font-medium mb-1">{t('dialogs.newProjectForImport.importingFrom.walls')}</div>
            <div className="text-text-secondary">
              {t('dialogs.newProjectForImport.summary.surveys', { count: wallsFile.resolved_survey_count })}
              {t('dialogs.newProjectForImport.summary.separator')}
              {t('dialogs.newProjectForImport.summary.shots', { count: wallsFile.shot_count })}
              {wallsFile.project_name
                ? `${t('dialogs.newProjectForImport.summary.separator')}${wallsFile.project_name}`
                : ''}
            </div>
          </div>
        )}

        {fileName && (
          <div
            className="p-3 bg-accent-10 border border-accent rounded text-sm text-text-secondary"
            data-testid="new-project-for-import-one-way-promise"
          >
            {t('dialogs.newProjectForImport.oneWayPromise', { fileName })}
          </div>
        )}

        {error && (
          <div className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm">
            {error}
          </div>
        )}

        <div>
          <label className="block text-sm text-text-secondary mb-1">
            {t('dialogs.newProjectForImport.nameLabel')}
          </label>
          <input
            type="text"
            value={name}
            onChange={(e) => setName(e.target.value)}
            className="w-full"
            placeholder={t('dialogs.newProjectForImport.namePlaceholder')}
            data-testid="project-name-input"
          />
        </div>

        <div>
          <label className="block text-sm text-text-secondary mb-1">
            {t('dialogs.newProjectForImport.savePathLabel')}
          </label>
          <div className="flex gap-2" data-testid="project-save-path-controls">
            <input
              type="text"
              value={savePath}
              onChange={(e) => setSavePath(e.target.value)}
              className="w-full"
              placeholder={displayedDefaultSavePath || `${normalizedBaseName}.stalx`}
              data-testid="project-save-path-input"
            />
            <Button
              onClick={handleSelectSavePath}
              disabled={isCreating}
              data-testid="project-save-path-browse-button"
            >
              {t('dialogs.newProjectForImport.browse')}
            </Button>
          </div>
        </div>

        <div>
          <label className="block text-sm text-text-secondary mb-1">
            {t('dialogs.newProjectForImport.locationLabel')}
          </label>
          <input
            type="text"
            value={projectLocation}
            onChange={(e) => setProjectLocation(e.target.value)}
            className="w-full"
            placeholder={t('dialogs.newProjectForImport.locationPlaceholder')}
            data-testid="project-location-input"
          />
        </div>

        <div className="flex justify-end gap-2 pt-4 border-t border-border">
          <Button onClick={handleCancel} disabled={isCreating} data-testid="new-project-for-import-cancel">
            {t('dialogs.newProjectForImport.cancel')}
          </Button>
          <Button
            variant="primary"
            onClick={handleCreate}
            disabled={isCreating || !name.trim()}
            data-testid="create-and-continue-button"
          >
            {isCreating ? t('dialogs.newProjectForImport.creating') : t('dialogs.newProjectForImport.create')}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
