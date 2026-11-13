import { useState, useEffect, useCallback, useMemo, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import { invokeIpc, getErrorMessage } from '../../utils/ipc';
import {
  completeImportToSingleView,
  finalizeDeferredSaveWarnings,
  finishImportWithoutOrphanSuggestions,
  importFailureMessage,
  surfaceImportFailure,
  type ImportFailure,
} from '../../utils/saveFeedback';
import { formatPhaseLabel } from '../../utils/loadingPhaseLabel';
import { extractFilename } from '../../utils/path';
import { clamp } from '../../utils/clamp';
import { formatNumber } from '../../i18n/format';
import { Button } from '../common/Button';
import { Select } from '../common/Select';
import { DialogWrapper } from './DialogWrapper';
import { WizardMapPanel } from './WizardMapPanel';
import { WizardScrollBody } from './WizardScrollBody';
import { WIZARD_MAP_PANEL_MAX_WIDTH } from '../SingleSystemView/layoutConstants';
import { usePreviewSize } from './usePreviewSize';
import SectionPreview from './SectionPreview';
import { TmlSectionTable } from './TmlSectionTable';
import {
  TmlPlacementStep,
  EMPTY_PLACEMENT,
  placementPayload,
  type TmlFileAnchor,
  type TmlPlacementState,
} from './TmlPlacementStep';
import type { TmlFile, OrphanPlacementSuggestion } from '../../types/commands';
import { setSource, trackDialogComplete } from '../../utils/telemetry';
import type { VirtualShotMode } from '../../types/project';
import { computeTmlImportStats } from '../../utils/tmlImportStats';
import { computeTmlPreviewLayout, isOfferableCoordinate } from '../../utils/tmlPreviewLayout';

interface TmlImportDialogProps {
  tmlFile: TmlFile;
  filePath: string;
}

/**
 * How to get rid of the just-created project, captured when the prompt is
 * raised: `savedPath` is set only when a `.stalx` exists on disk, which is what
 * decides delete-vs-close. Snapshotting it (rather than re-reading the store at
 * click time) keeps the two arms from swapping under an async store update.
 */
interface DiscardTarget {
  /** null while the initial save is deferred - nothing on disk, so close, don't delete. */
  savedPath: string | null;
  /** Project identity (the token `lostProject` uses), so a project switch between
   *  raising the prompt and clicking Discard cannot redirect the delete. */
  createdAt: string | null;
}

/** Why the dialog is asking what to do with the just-created empty project. */
type Residue = { kind: 'cancel' | 'failed'; message?: string; project: DiscardTarget };

/**
 * True when the open project was created FOR this import (`importContext ===
 * 'new-project'`, set by the unified picker route) and still holds nothing.
 * Cancelling or failing out of here would otherwise strand the user inside an
 * empty project they never asked for (#411 C2).
 */
function isEmptyImportProject(): boolean {
  return (
    useUIStore.getState().importContext === 'new-project' &&
    (useProjectStore.getState().project?.sections.length ?? 0) === 0
  );
}

/**
 * Which discard arm the open project needs.
 *
 * `databaseOpen` is false exactly while the initial save is deferred - the
 * common new-project-import case, where nothing has been written and closing the
 * project IS the discard. It cannot go stale under a failed import: the
 * backend's deferred-save safety net (`finalize_deferred_import`,
 * src-tauri/src/commands/import.rs) returns early on `!result.success`, so a
 * failing import never writes the `.stalx` behind the frontend's back, and a
 * fresh empty project is not dirty, so autosave never fires either.
 */
function discardTarget(): DiscardTarget {
  const { project, databaseOpen } = useProjectStore.getState();
  return {
    savedPath: databaseOpen ? (project?.file_path ?? null) : null,
    createdAt: project?.created_at ?? null,
  };
}

export default function TmlImportDialog({ tmlFile, filePath }: TmlImportDialogProps) {
  const { t } = useTranslation();
  const { executeTmlImport, calculateFileHash, startLoading, setLoadingProgress, resetLoading } = useProjectStore();
  const loading = useProjectStore((state) => state.loading);
  const { closeDialog, openDialog, setView, triggerFitToBounds, setImportContext, showToast } = useUIStore();

  const [isImporting, setIsImporting] = useState(false);
  const [isDiscarding, setIsDiscarding] = useState(false);
  const [discardError, setDiscardError] = useState<string | null>(null);
  const [fileHash, setFileHash] = useState<string | null>(null);
  const [residue, setResidue] = useState<Residue | null>(null);
  const [includeExcluded, setIncludeExcluded] = useState(false);
  const [commentsOnStations, setCommentsOnStations] = useState(true);
  // #27: Ariane has one VIRTUAL category, so the chain rule decides Jump vs
  // Reference by default. A user who knows the file can override it here.
  const [virtualShotMode, setVirtualShotMode] = useState<VirtualShotMode>('Auto');

  // Section indices (into `tmlFile.sections`) to import - same shape as the
  // Walls dialog's `selectedSurveys`. Default: everything, so the import
  // matches prior behavior until the user unticks something.
  const allIndices = useMemo(
    () => tmlFile.sections.map((_, i) => i),
    [tmlFile.sections]
  );
  const [selectedSections, setSelectedSections] = useState<Set<number>>(
    () => new Set(allIndices)
  );
  useEffect(() => {
    setSelectedSections(new Set(allIndices));
  }, [allIndices]);
  const allSelected =
    allIndices.length > 0 && allIndices.every((i) => selectedSections.has(i));
  const toggleSection = useCallback((i: number) => {
    setSelectedSections((prev) => {
      const next = new Set(prev);
      if (next.has(i)) next.delete(i);
      else next.add(i);
      return next;
    });
  }, []);
  const toggleAllSections = useCallback(() => {
    setSelectedSections((prev) => {
      const on = allIndices.length > 0 && allIndices.every((i) => prev.has(i));
      return on ? new Set() : new Set(allIndices);
    });
  }, [allIndices]);

  const [placement, setPlacement] = useState<TmlPlacementState>(EMPTY_PLACEMENT);
  // Whether the user has answered the placement question themselves. Without
  // it, the default-seeding effect below would re-run on every section toggle
  // (the anchor is recomputed from the new root) and silently overwrite an
  // answer they had already given.
  const [placementTouched, setPlacementTouched] = useState(false);
  const choosePlacement = useCallback((next: TmlPlacementState) => {
    setPlacementTouched(true);
    setPlacement(next);
  }, []);

  // The map column, and the box the preview draws into (it fills the column,
  // #567). Width and its persistence are owned by uiStore/WizardMapPanel (#404).
  const mapPanelWidth = useUIStore((s) => s.wizardMapPanelWidth);
  const setMapPanelWidth = useUIStore((s) => s.setWizardMapPanelWidth);
  const bodyRef = useRef<HTMLDivElement | null>(null);
  const [previewRef, previewSize] = usePreviewSize(WIZARD_MAP_PANEL_MAX_WIDTH);

  // What the import will build, in one frame, under the current toggles. The
  // preview and the section rows' "moved to origin" flags both read this, so
  // the picture and the annotations cannot disagree.
  const layout = useMemo(
    () =>
      computeTmlPreviewLayout(tmlFile, {
        includeExcluded,
        selected: selectedSections,
      }),
    [tmlFile, includeExcluded, selectedSections]
  );
  const previewSections = useMemo(
    () =>
      layout.sections.map((s) => ({
        section: s.section,
        color: s.color,
        sectionIndex: s.index,
        offsetX: s.offsetX,
        offsetY: s.offsetY,
      })),
    [layout]
  );
  const reanchoredIndices = useMemo(
    () => new Set(layout.sections.filter((s) => s.reanchored).map((s) => s.index)),
    [layout]
  );

  // The section the placement choice applies to: the first one the import will
  // CREATE - selected AND still holding shots after the exclusion filter.
  //
  // That is now the ONE definition of root, shared verbatim with
  // `computeTmlPreviewLayout` (whose first entry this is) and with the backend,
  // where the conversion loop reports it as `is_root: configs.is_empty()`. It
  // used to be three definitions: the backend and the preview layout each said
  // "lowest selected index", which disagrees with this one whenever the
  // exclusion filter empties that section - and the placement collected here
  // was then applied to a section that was never built, so it vanished.
  const rootSection = layout.sections[0] ?? null;

  /**
   * The coordinates the file offers for the root, and where they came from.
   *
   * `root`: the root section's own first shot carries them, which is exactly
   * what the importer picks with no override - choosing this option changes
   * nothing but makes it visible.
   * `elsewhere`: the file has a georeference, but on a section that is not the
   * root. The importer IGNORES that today (`determine_placement` only ever
   * looks at the section's own first shot), so offering it is new reach, not a
   * restatement.
   */
  const fileAnchor: TmlFileAnchor | null = useMemo(() => {
    const first = rootSection ? tmlFile.sections[rootSection.index]?.shots[0] : undefined;
    // Validity, not just non-zero: the backend runs both of these through
    // `GpsCoordinates::new`, so offering coordinates it will refuse would put
    // an option on screen that disables the Import button when chosen.
    if (first && isOfferableCoordinate(first.latitude, first.longitude)) {
      return { latitude: first.latitude, longitude: first.longitude, source: 'root' };
    }
    const geo = tmlFile.geo_coding;
    if (geo && isOfferableCoordinate(geo.latitude, geo.longitude)) {
      return { latitude: geo.latitude, longitude: geo.longitude, source: 'elsewhere' };
    }
    return null;
  }, [rootSection, tmlFile.sections, tmlFile.geo_coding]);

  const hasFileAnchor = fileAnchor !== null;
  // Seed the default: the file's own answer when it has one, the origin
  // otherwise. Keyed on the BOOLEAN, not the anchor object, and skipped once
  // the user has answered - both so that toggling sections cannot walk over
  // their choice.
  useEffect(() => {
    if (placementTouched) return;
    setPlacement((prev) => ({ ...prev, choice: hasFileAnchor ? 'file' : 'origin' }));
  }, [hasFileAnchor, placementTouched]);
  // A CORRECTION, not a default: deselecting the section that carried the
  // coordinates withdraws the option the user picked, and leaving `choice` on
  // it would disable Import with no visible reason (the radio is gone too).
  useEffect(() => {
    if (hasFileAnchor) return;
    setPlacement((prev) => (prev.choice === 'file' ? { ...prev, choice: 'origin' } : prev));
  }, [hasFileAnchor]);

  // An import in flight outlives this component when something else takes the
  // dialog slot (dropping another importable file re-routes it), so the failure
  // path has to know whether its in-component surface can still land.
  const isMountedRef = useRef(true);
  useEffect(() => {
    isMountedRef.current = true;
    return () => {
      isMountedRef.current = false;
    };
  }, []);

  // The hash is import PROVENANCE (it rides along to execute_tml_import), never
  // a gate: this dialog only ever opens on a brand-new project, so a duplicate
  // import is unreachable here and nothing waits on the round-trip.
  useEffect(() => {
    (async () => {
      try {
        setFileHash(await calculateFileHash(filePath));
      } catch {
        // Non-fatal: import without provenance rather than blocking on it.
      }
    })();
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const fileName = useMemo(() => {
    return extractFilename(filePath);
  }, [filePath]);

  // Every figure tracks the SELECTION, not the file: unticking the last
  // shot-bearing section has to disable Import and say the import is empty,
  // exactly as unticking "include excluded shots" would (same rule as the Walls
  // dialog's selected counts).
  const stats = useMemo(
    () => computeTmlImportStats(tmlFile, includeExcluded, selectedSections, virtualShotMode),
    [tmlFile, includeExcluded, selectedSections, virtualShotMode]
  );
  const fileStats = useMemo(
    () => computeTmlImportStats(tmlFile, includeExcluded),
    [tmlFile, includeExcluded]
  );

  const isEmptyImport = stats.totalShots === 0;
  // The placement payload the backend will get, or null while the user's typed
  // coordinates do not parse - which is a reason to hold the Import button, not
  // to send "you decide".
  const rootPlacement = placementPayload(placement, fileAnchor);

  // `clamp` passes NaN through, and an unseeded/garbled progress event would
  // then render as "NaN%" and an invalid bar width.
  const overallProgress = Number.isFinite(loading.overallProgress)
    ? Math.round(clamp(loading.overallProgress, 0, 100))
    : 0;

  // Escape / backdrop must not silently drop the option choices the user made.
  // The placement choice counts only once it diverges from the default this
  // file would have taken anyway, so merely opening the dialog is not "dirty".
  const optionsChanged =
    includeExcluded ||
    !commentsOnStations ||
    virtualShotMode !== 'Auto' ||
    !allSelected ||
    placementTouched;

  const handleImport = useCallback(async () => {
    setIsImporting(true);
    // Drive the in-dialog progress line: Reading -> Converting -> Applying ->
    // Finalizing phase ticks from the backend ProgressEmitter.
    startLoading(tmlFile.cave_name || fileName, filePath);
    setLoadingProgress({ phase: 'import_parsing' });
    const importUi = { closeDialog, setView, triggerFitToBounds, openDialog, showToast };
    /**
     * Terminal failure. The shared router owns the wording and the exit rules
     * (docs/notifications.md); the one thing it cannot do is ask about the empty
     * project this import created, so a failure - backend OR transport - with
     * such a project is reported by the residue prompt instead - only one
     * dialog fits on screen (#411 C2).
     *
     * `openDialog` is a single slot: App.tsx mounts this component only while
     * `activeDialog === 'tml-import'`, so routing a TRANSPORT failure to the
     * router's error dialog unmounts THIS component - there is no dialog stack
     * underneath it to fall back to. Dismissing that error dialog leaves
     * `activeDialog: null` with the empty project still open and `importContext`
     * already cleared, so `isEmptyImportProject()` can never recognise it as
     * residue again - the keep/discard prompt becomes unreachable for the rest
     * of the session. So a transport failure gets the SAME local handling as a
     * backend one whenever there's an empty import project to protect.
     *
     * The router (`surfaceImportFailure`) still handles the failure directly
     * when either condition below is false:
     *  - the project already holds sections - not this dialog's residue to ask
     *    about.
     *  - this component is no longer mounted. A drop of another importable
     *    file replaces the dialog slot mid-import, and `setResidue` on an
     *    unmounted component is a no-op - i.e. silence. The router is store-level
     *    and always lands.
     */
    const reportFailure = (failure: ImportFailure) => {
      setIsImporting(false);
      if (
        (failure.phase === 'command' || failure.phase === 'transport') &&
        isMountedRef.current &&
        isEmptyImportProject()
      ) {
        // The residue prompt IS this dialog's completion: stamp it here so a
        // failure reports the same way whichever surface takes it (the router
        // stamps `trackDialogComplete` too).
        trackDialogComplete('tml-import');
        setResidue({
          kind: 'failed',
          message: importFailureMessage(failure),
          project: discardTarget(),
        });
        return;
      }
      surfaceImportFailure('tml-import', importUi, failure);
    };
    try {
      // Omit (null) the section list when everything is ticked, so a full
      // import stays unfiltered on the wire - same convention as Walls.
      const sectionSelection = allSelected
        ? null
        : [...selectedSections].sort((a, b) => a - b);
      const result = await executeTmlImport(
        filePath,
        fileHash,
        includeExcluded,
        commentsOnStations,
        virtualShotMode,
        sectionSelection,
        rootPlacement
      );
      resetLoading();

      if (!result.success) {
        reportFailure({ phase: 'command', error: result.error });
        return;
      }

      // A partial-success import (new-project deferred save failed) returns
      // success WITH a "SYSTEM" save-failure warning. Surface ONLY that, before
      // the optional orphan step, so "unsaved - save manually" isn't buried.
      // Routine per-shot validation warnings (LONG_SHOT, …) also ride along on a
      // successful import but are surfaced via the data-table UI - they must not
      // modal here or skip orphan placement (#110).
      if (finalizeDeferredSaveWarnings(result.warnings, 'tml-import', importUi, result.import_receipt)) {
        return;
      }

      // `section_ids` used to be `skip_serializing_if = "Vec::is_empty"`, so an
      // import that created NO section omitted the key entirely while the
      // generated type said `Array<number>`. A bare `.length` here threw, the
      // catch below reported it as a transport failure, and the receipt
      // explaining the empty import was thrown away (#411). #441 removed the
      // skip, so the key is always on the wire: this `?? []` is defensive only
      // and unreachable in the shipped pairing (one binary, one frontend).
      // Same shape as WallsImportDialog.
      const sectionIds = result.section_ids ?? [];

      // Check for orphan placement opportunities. The suggest call is the
      // ONLY thing wrapped in try/catch here (non-fatal - a failed lookup
      // just skips the suggestion step); `finishImportWithoutOrphanSuggestions`
      // sits outside it and completes the import exactly once, so a throw
      // partway through IT can never fall through to a duplicate completion.
      if (sectionIds.length > 0) {
        let suggestions: OrphanPlacementSuggestion[] = [];
        try {
          suggestions = await invokeIpc<OrphanPlacementSuggestion[]>(
            'suggest_orphan_placements',
            { newlyImportedSectionIds: sectionIds },
            { purpose: 'suggest orphan placements after TML import' }
          );
        } catch {
          // Non-fatal: proceed without orphan detection
        }
        if (suggestions.length > 0) {
          setSource('dialog');
          openDialog('orphan-placement', { suggestions, receipt: result.import_receipt });
          return;
        }
        await finishImportWithoutOrphanSuggestions('tml-import', importUi, sectionIds, result.import_receipt);
        return;
      }

      completeImportToSingleView('tml-import', importUi, result.import_receipt);
    } catch (e) {
      resetLoading();
      reportFailure({ phase: 'transport', error: e, format: 'TML' });
    }
  }, [executeTmlImport, filePath, fileHash, includeExcluded, commentsOnStations, virtualShotMode, allSelected, selectedSections, rootPlacement, tmlFile.cave_name, fileName, closeDialog, openDialog, setView, triggerFitToBounds, startLoading, setLoadingProgress, resetLoading]);

  const handleCancel = useCallback(() => {
    if (isImporting) return;
    if (isEmptyImportProject()) {
      setResidue({ kind: 'cancel', project: discardTarget() });
      return;
    }
    closeDialog();
  }, [isImporting, closeDialog]);

  /** Keep the project: the import flow is over either way, so its context goes. */
  const handleKeepProject = useCallback(() => {
    setImportContext(null);
    closeDialog();
  }, [closeDialog, setImportContext]);

  const handleDiscardProject = useCallback(async () => {
    if (!residue || isDiscarding) return;
    // This is the app's only file-deleting API, so the "created for this import
    // and still empty" condition is re-checked HERE, at the click, not just when
    // the prompt was raised; and the project must still be the one the snapshot
    // was taken from, so a switch in between cannot redirect the delete at
    // another file. (`file_path` is no identity: the backend RESERVES one for a
    // deferred project that has never been written.)
    if (
      !isEmptyImportProject() ||
      (useProjectStore.getState().project?.created_at ?? null) !== residue.project.createdAt
    ) {
      // The flow is over here too: leaving `importContext` set would have the
      // next import that sets none (GPX/Compass/Walls) inherit 'new-project'
      // and take `saveAfterImport`'s full-save branch, committing the user's
      // unrelated pending edits.
      setImportContext(null);
      closeDialog();
      return;
    }
    setIsDiscarding(true);
    try {
      const { closeProject, deleteProject } = useProjectStore.getState();
      // A new-project import defers the initial save, so there is usually no
      // `.stalx` yet and closing the project IS the discard. One created with an
      // explicit save path was written to disk, so it goes through the same
      // delete path the Delete Project dialog uses.
      const result = residue.project.savedPath
        ? await deleteProject(residue.project.savedPath)
        : await closeProject();
      if (!result.success) {
        // Both arms can fail with the session already torn down (the backend
        // closes BEFORE handing the file to the OS trash), so "it failed" is not
        // "nothing happened" - say so instead of closing success-shaped over a
        // file that is still on disk and still in Recents.
        setDiscardError(result.error ?? t('dialogs.tmlImport.residue.discardFailed'));
        return;
      }
      setImportContext(null);
      setView('global');
      closeDialog();
    } catch (e) {
      setDiscardError(getErrorMessage(e, t('dialogs.tmlImport.residue.discardFailed')));
    } finally {
      setIsDiscarding(false);
    }
  }, [closeDialog, isDiscarding, residue, setImportContext, setView, t]);

  if (residue) {
    return (
      <DialogWrapper
        title={t('dialogs.tmlImport.residue.title')}
        // Escape mid-delete would otherwise run Keep while the delete is still
        // in flight, leaving nothing watching its result.
        onClose={isDiscarding ? () => {} : handleKeepProject}
        size="md"
        data-testid="tml-import-residue-dialog"
      >
        <div className="space-y-4">
          {residue.kind === 'failed' && residue.message && (
            <div
              className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm whitespace-pre-wrap max-h-40 overflow-y-auto"
              data-testid="tml-import-residue-error"
            >
              {residue.message}
            </div>
          )}
          <p className="text-text-secondary">{t('dialogs.tmlImport.residue.body')}</p>

          {discardError && (
            <div
              className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm whitespace-pre-wrap max-h-40 overflow-y-auto"
              data-testid="tml-import-residue-discard-error"
            >
              {discardError}
            </div>
          )}

          <div className="flex justify-end gap-2 pt-4 border-t border-border">
            <Button
              variant="danger"
              onClick={handleDiscardProject}
              disabled={isDiscarding}
              data-testid="tml-import-residue-discard"
            >
              {t('dialogs.tmlImport.residue.discard')}
            </Button>
            <Button
              data-primary-action=""
              onClick={handleKeepProject}
              disabled={isDiscarding}
              data-testid="tml-import-residue-keep"
            >
              {t('dialogs.tmlImport.residue.keep')}
            </Button>
          </div>
        </div>
      </DialogWrapper>
    );
  }

  return (
    <DialogWrapper
      title={t('dialogs.tmlImport.title')}
      onClose={handleCancel}
      size="5xl"
      overflow="hidden"
      bodyClassName="flex-1 flex flex-col p-0 min-h-0"
      isDirty={optionsChanged}
      data-testid="tml-import-dialog"
    >
      <div className="flex-1 flex flex-col min-h-0">
      <WizardScrollBody bodyRef={bodyRef}>
        {(bandHeight) => (
      <div className="flex" style={{ minHeight: bandHeight }}>
        {/* Map column: the file laid out as the import will build it, so the
            section toggles and the placement choice are answered by looking
            rather than by imagining. Same panel (and now the same remembered
            width, #404) as the mNemo wizard's. */}
        <WizardMapPanel
          width={mapPanelWidth}
          onWidthChange={setMapPanelWidth}
          bodyRef={bodyRef}
          panelRef={previewRef}
          height={bandHeight}
        >
          <SectionPreview
            segmentSections={previewSections}
            size={previewSize.width}
            height={previewSize.height}
          />
        </WizardMapPanel>

      <div className="flex-1 min-w-0 p-4 space-y-4">
        {/* Header info */}
        <div className="space-y-1">
          <div className="text-sm text-text-secondary">{fileName}</div>
          {tmlFile.cave_name && (
            <div className="text-lg font-medium">{tmlFile.cave_name}</div>
          )}
          {tmlFile.unit === 'Imperial' && (
            <div className="text-xs text-text-tertiary">
              {t('dialogs.tmlImport.unitConversion')}
            </div>
          )}
        </div>

        {/* Summary stats - what actually imports under the current toggles */}
        <div className="grid grid-cols-4 gap-3 p-3 bg-surface-alt rounded">
          <div>
            <div className="text-xs text-text-tertiary">{t('dialogs.tmlImport.stats.sections')}</div>
            <div className="text-sm font-medium" data-testid="tml-section-count">{stats.totalSections}</div>
          </div>
          <div>
            <div className="text-xs text-text-tertiary">{t('dialogs.tmlImport.stats.shots')}</div>
            <div className="text-sm font-medium" data-testid="tml-shot-count">{stats.totalShots}</div>
          </div>
          <div>
            <div className="text-xs text-text-tertiary">{t('dialogs.tmlImport.stats.totalLength')}</div>
            <div className="text-sm font-medium">
              {t('dialogs.tmlImport.stats.lengthValue', { length: stats.totalLength })}
            </div>
          </div>
          <div>
            <div className="text-xs text-text-tertiary">{t('dialogs.tmlImport.stats.depthRange')}</div>
            <div className="text-sm font-medium">{stats.depthRange ?? t('dialogs.tmlImport.stats.none')}</div>
          </div>
        </div>

        {/* Nothing survives the current options - there is no import to run */}
        {isEmptyImport && (
          <div
            className="p-3 bg-warning-10 border border-warning-20 rounded text-sm"
            data-testid="tml-import-empty"
          >
            {t('dialogs.tmlImport.empty')}
          </div>
        )}

        {/* Section list, with a checkbox per row (#425) */}
        <TmlSectionTable
          tmlFile={tmlFile}
          includeExcluded={includeExcluded}
          selected={selectedSections}
          onToggle={toggleSection}
          onToggleAll={toggleAllSections}
          reanchored={reanchoredIndices}
          disabled={isImporting}
        />

        {/* Sections whose parent is not being imported land at the origin
            rather than where the file put them. Said once here as well as
            per-row, because it is a consequence of the selection the user just
            made, not a property of any one row. */}
        {layout.reanchoredCount > 0 && (
          <div
            className="p-3 bg-warning-10 border border-warning-20 rounded text-sm"
            data-testid="tml-reanchored-notice"
          >
            {t('dialogs.tmlImport.reanchored', { count: layout.reanchoredCount })}
          </div>
        )}

        {/* Placement step (#425) */}
        <TmlPlacementStep
          state={placement}
          onChange={choosePlacement}
          anchor={fileAnchor}
          rootSectionName={rootSection?.name ?? null}
          disabled={isImporting}
        />

        {/* Closures info */}
        {tmlFile.closures.length > 0 && (
          <div className="text-xs text-text-secondary" data-testid="tml-closure-count">
            {t('dialogs.tmlImport.closures', { count: tmlFile.closures.length })}
          </div>
        )}

        {/* Excluded shots toggle. Gated on the FILE's count, not the
            selection's: a toggle that vanishes because the sections holding the
            excluded shots are unticked would be unreachable exactly when the
            user goes looking for it. The LABEL still quotes the selection. */}
        {fileStats.excludedCount > 0 && (
          <label className="flex items-center gap-2 text-sm text-text-secondary cursor-pointer">
            <input
              type="checkbox"
              checked={includeExcluded}
              onChange={(e) => setIncludeExcluded(e.target.checked)}
              data-testid="tml-include-excluded"
            />
            {t('dialogs.tmlImport.includeExcluded', { count: stats.excludedCount })}
          </label>
        )}

        {/* Comments placement toggle - gated on the file for the same reason. */}
        {fileStats.commentCount > 0 && (
          <label className="flex items-center gap-2 text-sm text-text-secondary cursor-pointer">
            <input
              type="checkbox"
              checked={!commentsOnStations}
              onChange={(e) => setCommentsOnStations(!e.target.checked)}
              data-testid="tml-comments-on-shots"
            />
            {t('dialogs.tmlImport.keepCommentsOnShots')}
          </label>
        )}

        {/* Virtual-shot classification (#27). One control for the whole file,
            rather than the per-shot picker the issue first proposed: the chain
            rule gets it right in the common case, and the data table's J/R cell
            handles the exceptions with the map visible alongside. */}
        {stats.virtualCount > 0 && (
          <label className="flex items-center gap-2 text-sm text-text-secondary">
            <span>{t('dialogs.tmlImport.virtualShots', { count: stats.virtualCount })}</span>
            <Select
              value={virtualShotMode}
              onChange={(v) => setVirtualShotMode(v as VirtualShotMode)}
              options={[
                { value: 'Auto', label: t('dialogs.tmlImport.virtualMode.auto') },
                { value: 'AllJumps', label: t('dialogs.tmlImport.virtualMode.allJumps') },
                { value: 'AllReferences', label: t('dialogs.tmlImport.virtualMode.allReferences') },
              ]}
              size="sm"
              aria-label={t('dialogs.tmlImport.virtualShots', { count: stats.virtualCount })}
              data-testid="tml-virtual-shot-mode"
            />
          </label>
        )}

        {/* In-dialog progress: by import time the view is 'single', so the
            GlobalView overlay that renders these phases is not mounted and a
            large file would otherwise show only a busy button (#411 C3). */}
        {isImporting && (
          <div className="space-y-1" data-testid="tml-import-progress">
            <div className="flex justify-between text-xs text-text-secondary">
              <span data-testid="tml-import-progress-phase">
                {formatPhaseLabel(t, loading.phase, loading.phaseIndex, loading.phaseCount)}
              </span>
              <span>{formatNumber(overallProgress)}%</span>
            </div>
            <div className="h-1.5 bg-border rounded-full overflow-hidden">
              <div
                className="h-full bg-accent transition-all duration-300"
                style={{ width: `${overallProgress}%` }}
                role="progressbar"
                aria-label={t('loading.overall.aria')}
                aria-valuenow={overallProgress}
                aria-valuemin={0}
                aria-valuemax={100}
              />
            </div>
          </div>
        )}

      </div>
      </div>
        )}
      </WizardScrollBody>

      {/* Footer. The dialog now owns its own scroller (WizardScrollBody), so
          this is a flex SIBLING of it rather than the sticky-inside-the-body
          idiom the smaller import dialogs use - it is outside the scroll, which
          is what keeps the primary action reachable however long the section
          list gets. */}
        <div className="flex justify-end gap-2 px-4 py-4 border-t border-border bg-panel">
          <Button onClick={handleCancel} disabled={isImporting} data-testid="tml-import-cancel">
            {t('dialogs.tmlImport.cancel')}
          </Button>
          <Button
            data-primary-action=""
            data-testid="tml-import-confirm"
            onClick={handleImport}
            // `rootPlacement === null` is the placement step refusing to answer
            // (coordinates typed but unparseable), NOT "let the backend decide"
            // - holding the button is the only way to say so without discarding
            // what the user typed.
            disabled={isImporting || isEmptyImport || rootPlacement === null}
          >
            {isImporting ? t('dialogs.tmlImport.importing') : t('dialogs.tmlImport.import')}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
