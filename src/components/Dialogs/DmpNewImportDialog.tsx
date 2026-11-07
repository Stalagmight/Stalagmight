import { useState, useEffect, useMemo, useCallback, useRef } from 'react';
import { useLatest } from '../../hooks/useLatest';
import { useTranslation } from 'react-i18next';
import { invokeIpc, getErrorMessage } from '../../utils/ipc';
import type { DmpFile, CrossSectionSuggestion, ImportSectionForSuggestion } from '../../types/commands';
import type { DmpSelectionConfig } from './DmpSelectionDialog';
import type { Project, Station, ShotData } from '../../types/project';
import type { ImportPlacement } from '../../types/commands';
import type { ZeroDatumConfigResult } from './ZeroDatumConfigDialog';
import type { OverlapMode } from '../../types/config';
import { DialogWrapper } from './DialogWrapper';
import type { SegmentPlacementMode } from './SegmentPlacementModes';
import { buildZeroDatumSectionConfig, DMP_AZIMUTH_REFERENCE } from '../../utils/importSectionConfig';
import { ShotTableEditor } from '../common/ShotTableEditor';
import { useProjectStore } from '../../store/projectStore';
import { convertToEditableShots, editableShotToShotData } from '../../utils/shotConversion';
import { computeMergeOptions, defaultMergeOffset } from '../../utils/overlapHelpers';
import { type VirtualConnection, computeUnreachableSegments, computeSegmentLayout } from '../../utils/segmentContext';
import { sectionColorPalette } from '../../utils/colors';
import { debugLog } from '../../utils/debugLog';
import { useUIStore, type PlacementResult } from '../../store/uiStore';
import { setSource, trackDialogComplete } from '../../utils/telemetry';
import {
  buildPlacementSectionData,
  redirectDeletedTargets,
  type SectionEditState,
  type ConfirmedOverlapInfo,
} from '../../utils/batchImportHelpers';
import {
  collectExistingAnchorsForTemporalRerank,
  distanceMapFromField,
  rerankSuggestionsByTemporalPlan,
} from '../../utils/dmpTemporalRerank';
import { fromEmittedStationIndex, isEmittedShot, toEmittedStationIndex } from '../common/emittedShots';
import { applyAllOverlapTreatments } from '../../utils/overlapTreatment';
import type { ImportReceipt } from '../../types/ipc/ImportReceipt';
import { TOAST_KEYS } from '../../utils/toastKeys';
import { useDmpImportRun } from './dmpWizard/useDmpImportRun';
import { buildBatchImport } from './dmpWizard/buildBatchImport';
import { SuggestingActions, SuggestingStep } from './dmpWizard/SuggestingStep';
import { ManualPlacingActions, ManualPlacingStep } from './dmpWizard/ManualPlacingStep';
import { ExitConfirmations } from './dmpWizard/ExitConfirmations';
import { WizardFooter } from './dmpWizard/WizardFooter';
import { LateImportRetryAction, StatusStep, SuggestionsFailedActions } from './dmpWizard/WizardStatus';
import { targetDoublyAnchored } from './dmpWizard/doublyAnchored';
import { useSectionEditor, type RowCountChange } from './dmpWizard/useSectionEditor';
import { anchorSectionIn, initialWizardState, segmentQueue, useWizardState, type SegmentChoices } from './dmpWizard/wizardState';
import { adjustConnectionsForEdits, buildEffectiveDmpSection } from './dmpWizard/effectiveSections';
import {
  normalizeSkippedSuggestionEntry,
  type DirectPlacement,
  type SavedManualPlacingState,
  type SkippedSuggestionPlacement,
} from './dmpWizard/wizardTypes';

interface DmpNewImportDialogProps {
  dmpFile: DmpFile;
  selectionConfig: DmpSelectionConfig;
  project: Project | null;
  stations: Station[];
  zeroDatumConfig?: ZeroDatumConfigResult | undefined;
  onCancel: () => void;
  onBack: () => void;
  onComplete: (newSectionIds?: number[], receipt?: ImportReceipt) => void;
  /** Saved state to restore after returning from manual map placement. */
  savedManualPlacingState?: SavedManualPlacingState | undefined;
  /** If true, immediately start map placement when the dialog opens in manual placing phase. */
  autoStartManualPlacement?: boolean;
}

// Moved to dmpWizard/ (#576 D4) and re-exported here, where App.tsx, the
// placement modes and the tests have always imported them from.
export type { DirectPlacement, SkippedSuggestionPlacement, SavedManualPlacingState };
export { buildBatchImport };

/**
 * Connection Wizard - pre-calculates all placements at startup, presents
 * suggestions sorted by score, records virtual connections on confirm,
 * then batch-places everything when suggestions are exhausted.
 */
export default function DmpNewImportDialog({
  dmpFile,
  selectionConfig,
  project: _project,
  stations: _stations,
  zeroDatumConfig,
  onCancel,
  onBack,
  onComplete,
  savedManualPlacingState,
  autoStartManualPlacement = false,
}: DmpNewImportDialogProps) {
  const { t } = useTranslation();


  // Store selectors
  const config = useProjectStore((s) => s.config);
  const patchConfig = useProjectStore((s) => s.patchConfig);
  const resolvedTheme = useUIStore((s) => s.resolvedTheme);
  // #357 item 9: seed new sections from the palette legible on this theme's
  // MAP CANVAS, not its chrome - a section keeps whatever color it was
  // created with, so this only ever affects colors handed to NEW sections.
  const sectionPalette = useMemo(() => sectionColorPalette(resolvedTheme), [resolvedTheme]);

  // Shorthand for restoring state
  const saved = savedManualPlacingState;

  // The run: its imports, what they banked, the withdrawal, and the failure of
  // whichever step failed last (#576 D4).
  const {
    isImporting,
    failure,
    fail,
    clearFailure,
    importAnchor,
    importBatch,
    importSegment,
    withdrawImportedSections,
    withdrawInFlight,
    withdrawalFailure,
    clearWithdrawalFailure,
    importedCount,
    completion,
    carry,
    earlyImported,
  } = useDmpImportRun({ dmpFile, selectionConfig, carried: saved, importsOnMount: saved?.placedSegment != null });

  // The step the wizard is on, with the data that step has (#576 D4): see
  // dmpWizard/wizardState.ts. `dispatch` returns where an action left it.
  const { state: wizard, dispatch, current: currentWizard } = useWizardState(() =>
    initialWizardState(saved, autoStartManualPlacement)
  );
  const { phase } = wizard;
  const suggesting = phase.kind === 'suggesting' ? phase : null;
  const manual = phase.kind === 'manual_placing' ? phase : null;
  /** The phases with no panel of their own: the status body shows them. */
  const statusPhase = phase.kind === 'init' || phase.kind === 'placing' || phase.kind === 'done' ? phase : null;
  const currentSuggestion = suggesting?.suggestion ?? null;
  const alternatives = suggesting?.alternatives ?? [];
  const isCalculating = phase.kind === 'init' && phase.calculating;

  // The run's plan so far - virtual connections instead of imported indices
  const [connections, setConnections] = useState<VirtualConnection[]>(saved?.connections ?? []);
  const [skippedIndices, setSkippedIndices] = useState<Set<number>>(
    saved ? new Set(saved.skippedIndices) : new Set()
  );
  const error = failure?.message ?? null;

  // Overlap handling mode (persisted to config)
  const [overlapMode, setOverlapMode] = useState<OverlapMode>(
    config?.import?.overlap_mode ?? 'import_data'
  );

  const sectionEditsRef = useRef<Map<number, SectionEditState>>(
    saved ? new Map(saved.sectionEdits) : new Map()
  );
  const confirmedOverlapsRef = useRef<Map<number, ConfirmedOverlapInfo>>(
    saved ? new Map(saved.confirmedOverlaps ?? []) : new Map()
  );

  // Cache all cross-section suggestions from startup calculation
  const suggestionPoolRef = useRef<CrossSectionSuggestion[]>([]);
  // Keep full pool (unfiltered) so buildBatchImport can find existing-target anchors
  const fullPoolRef = useRef<CrossSectionSuggestion[]>([]);
  const temporalDistanceCacheRef = useRef<Map<string, number>>(new Map());
  const temporalDistanceFetchedOriginsRef = useRef<Set<string>>(new Set());

  // ─── Manual placing phase ──────────────────────────────────────────────────
  // The queue, the anchor, the preview anchor, the mode and the From Existing
  // pick are the `manual_placing` phase's own (wizardState.ts).
  const currentSegment = manual?.queue[0] ?? null;
  const segmentAnchor = manual?.anchor ?? null;

  const commitSegmentAnchor = useCallback((anchor: { sectionIndex: number; stationIndex: number }) => {
    dispatch({ type: 'commitAnchor', anchor });
  }, [dispatch]);

  // A dropdown hovering an option previews it on the map; closing without a
  // choice reverts to the committed anchor. The reducer reads that anchor, so no
  // handler can act on a stale copy of it.
  const handleAnchorSectionHover = useCallback((value: string | null) => {
    dispatch({ type: 'hoverAnchorSection', sectionIndex: value === null ? null : Number(value) });
  }, [dispatch]);

  const handleAnchorStationHover = useCallback((value: string | null) => {
    dispatch({ type: 'hoverAnchorStation', stationIndex: value === null ? null : Number(value) });
  }, [dispatch]);

  // ─── The anchor's two station spaces ──────────────────────────────────────
  //
  // The pick is STORED in visible row space: that is what the station dropdowns
  // list, what the map preview's own click handler talks in once translated, and
  // the space the shot-table mutators re-anchor it in. Two surfaces speak
  // emitted space instead - the payloads, which carry only the rows the import
  // will create, and the previews, which draw those same rows - so each converts
  // at its edge rather than storing a second number that can drift.

  /** The anchor station as the BACKEND counts it. One derivation for every
   *  payload site: the two that shipped the raw pick merged a station
   *  downstream of the one clicked, and a pick past the emitted end made
   *  `merge_stations` refuse the whole batch. */
  const emittedAnchorStation = useCallback(
    (anchor: { sectionIndex: number; stationIndex: number }): number => {
      const edits = sectionEditsRef.current.get(anchor.sectionIndex);
      return edits ? toEmittedStationIndex(edits.shots, anchor.stationIndex) : anchor.stationIndex;
    },
    []
  );

  /** The other direction, for a station the PREVIEW drew: it renders the emitted
   *  rows, so a dot below a blank row is numbered one lower than the row the
   *  user is looking at in the grid. */
  const visibleAnchorStation = useCallback(
    (sectionIndex: number, emittedStationIndex: number): number => {
      const edits = sectionEditsRef.current.get(sectionIndex);
      return edits ? fromEmittedStationIndex(edits.shots, emittedStationIndex) : emittedStationIndex;
    },
    []
  );

  // Track original suggestions for sections skipped during suggesting phase
  const skippedSuggestionsRef = useRef<Map<number, SkippedSuggestionPlacement>>(
    new Map((saved?.skippedSuggestions ?? []).map(([idx, entry]) => [idx, normalizeSkippedSuggestionEntry(entry)]))
  );

  // Accumulated direct placements for manual_placing phase
  const directPlacementsRef = useRef<DirectPlacement[]>(saved?.directPlacements ?? []);

  // DMP indices of segments the user chose to discard ("Do not place")
  const discardedIndicesRef = useRef<Set<number>>(
    new Set(saved?.discardedIndices ?? [])
  );

  // Skip counter per segment (prevent infinite loops)
  const segmentSkipCountRef = useRef<Map<string, number>>(
    saved ? new Map(saved.segmentSkipCounts) : new Map()
  );

  /**
   * Re-entry latch for the suggestion actions and the early import. Confirm and
   * Skip hold it ('picking') while the next suggestion is chosen - a double
   * click must not record the same section twice - and the early import holds
   * it ('importing') until it has answered, which no second Confirm, Skip or
   * Place manually may start again. A ref: two clicks in one tick both run
   * before React re-renders.
   */
  const advanceLatchRef = useRef<'idle' | 'picking' | 'importing'>('idle');

  // Refs to avoid stale closures
  const connectionsRef = useLatest(connections);
  const skippedIndicesRef = useLatest(skippedIndices);

  /** What decides where a segment opens (wizardState.ts `SegmentChoices`), read live. */
  const segmentChoices = useCallback((): SegmentChoices => ({
    skipped: skippedIndicesRef.current,
    withSuggestion: new Set(skippedSuggestionsRef.current.keys()),
  }), [skippedIndicesRef]);

  const selectedIndices = useMemo(
    () => Array.from(selectionConfig.selectedIndices).sort((a, b) => a - b),
    [selectionConfig.selectedIndices]
  );

  // Section currently being edited in the right-side editor panel.
  // In suggesting mode this follows the current suggestion section.
  // In manual placing mode this follows the selected segment anchor section.
  const activeEditableSectionIndex = suggesting
    ? suggesting.suggestion.importSectionIndex
    : manual
      ? manual.anchor.sectionIndex
      : null;

  const totalSelected = selectedIndices.length;

  // Set of section indices that already have a connection
  const connectedIndices = useMemo(() => {
    return new Set(connections.map((c) => c.importSectionIndex));
  }, [connections]);

  // Compute import shot count for current suggestion
  const importShotCount = useMemo(() => {
    if (!currentSuggestion) return 0;
    const section = dmpFile.sections[currentSuggestion.importSectionIndex];
    if (!section) return 0;
    return section.shots.filter((s) => s.shot_type === 'Standard').length;
  }, [currentSuggestion, dmpFile.sections]);

  // Compute merge options for the current suggestion
  const mergeOptions = useMemo(() => {
    if (!currentSuggestion) return [];
    return computeMergeOptions(currentSuggestion, importShotCount);
  }, [currentSuggestion, importShotCount]);

  // Selected merge pair
  const selectedMergeOffset = suggesting?.mergeOffset ?? 0;
  const selectedMergePair = useMemo(() => {
    if (mergeOptions.length === 0) return null;
    return mergeOptions.find((o) => o.offset === selectedMergeOffset) ?? mergeOptions[mergeOptions.length - 1]!;
  }, [mergeOptions, selectedMergeOffset]);

  // The section on screen and its rows (dmpWizard/useSectionEditor.ts). Called
  // here, before the wizard's other effects: its effect that seeds the section's
  // edits has to run before the start-of-run effect imports anything built from
  // them.
  const onRowsChanged = useCallback(
    (change: RowCountChange) => dispatch({ type: 'rowsChanged', ...change }),
    [dispatch],
  );
  const {
    editState,
    effectiveShots,
    frozenShotIndices,
    handleMetadataChange,
    handleShotChange,
    handleAddShot,
    handleAddShots,
    handleInsertShots,
    handleDeleteShot,
    handleDeleteShots,
    refuseIncompleteShots,
  } = useSectionEditor({
    activeEditableSectionIndex,
    currentSuggestion,
    selectedMergePair,
    overlapMode,
    dmpFile,
    selectionConfig,
    selectedIndices,
    sectionPalette,
    sectionEditsRef,
    onRowsChanged,
  });

  // No effect resets the offset when the suggestion changes: the two places that
  // set a suggestion (`pickFromPool`, `handleAlternativeClick`) set its default
  // offset in the same batch (`computeDefaultMergeOffset`). An effect here used to
  // run after them and overwrite the backend's preferred offset with the deepest
  // one (#576 D12).

  // Doubly anchored target: only 'existing_data' and 'keep_both' are valid
  // (dmpWizard/doublyAnchored.ts). Read from the project when the suggestion
  // changes, as it always was.
  const isTargetDoublyAnchored = useMemo(
    () => targetDoublyAnchored(currentSuggestion, useProjectStore.getState().project),
    [currentSuggestion],
  );



  // Persist the overlap mode on change: this one setting, sent as chosen (#576
  // RV6b-1). Handing `updateConfig` the whole config sent only what differed
  // from the store's copy, which is re-read only once a save has landed - so
  // switching the mode and straight back found nothing to send, and the first
  // choice was the one saved. Config writes run one at a time in the order they
  // were made (`chainSettingWrite`), so the last choice is the last write.
  const configLoaded = config != null;
  const handleOverlapModeChange = useCallback((value: OverlapMode) => {
    setOverlapMode(value);
    if (configLoaded) void patchConfig([{ path: ['import', 'overlap_mode'], value }]);
  }, [configLoaded, patchConfig]);

  // Auto-switch to 'existing_data' when a doubly-anchored target disables the current mode
  useEffect(() => {
    if (isTargetDoublyAnchored && (overlapMode === 'interpolate' || overlapMode === 'import_data')) {
      handleOverlapModeChange('existing_data');
    }
  }, [isTargetDoublyAnchored, overlapMode, handleOverlapModeChange]);

  // Pick the next suggestion from the cached pool, filtering out connected/skipped sections.
  const computeDefaultMergeOffset = useCallback((suggestion: CrossSectionSuggestion): number => {
    const section = dmpFile.sections[suggestion.importSectionIndex];
    const importShotCount = section
      ? section.shots.filter((s) => s.shot_type === 'Standard').length
      : 0;
    const options = computeMergeOptions(suggestion, importShotCount);
    return defaultMergeOffset(options, suggestion.preferredMergeOffset) ?? 0;
  }, [dmpFile.sections]);

  const ensureTemporalDistances = useCallback(async (pool: CrossSectionSuggestion[]) => {
    const anchors = collectExistingAnchorsForTemporalRerank(pool, connectionsRef.current, dmpFile.sections);
    const missing = anchors.filter((ref) => {
      const key = `${ref.section_id}:${ref.station_index}`;
      return !temporalDistanceFetchedOriginsRef.current.has(key);
    });

    await Promise.all(missing.map(async (origin) => {
      const key = `${origin.section_id}:${origin.station_index}`;
      try {
        const field = await invokeIpc(
          'get_station_distances_from',
          { from: origin },
          { purpose: 'temporal DMP placement rerank', fields: { origin: key } },
        );
        // null only when no project is open (#576 C3): no distances, not a crash.
        const distances = distanceMapFromField(origin, field);
        for (const [distanceKey, distance] of distances) {
          temporalDistanceCacheRef.current.set(distanceKey, distance);
        }
        temporalDistanceFetchedOriginsRef.current.add(key);
      } catch (e) {
        debugLog('[dmp-temporal-rerank] distance fetch failed', origin, e);
      }
    }));
  }, [connectionsRef]);

  const pickFromPool = useCallback(async () => {
    const connected = new Set(connectionsRef.current.map((c) => c.importSectionIndex));
    const skipped = skippedIndicesRef.current;

    // Build reverse map: for each import-to-import connection, record who targets whom.
    // Used to detect cycles: if A→B exists and we're considering B→A, skip it.
    const importingTargets = new Map<number, Set<number>>();
    for (const c of connectionsRef.current) {
      if (c.targetType === 'importing' && c.targetImportIndex != null) {
        let s = importingTargets.get(c.targetImportIndex);
        if (!s) { s = new Set(); importingTargets.set(c.targetImportIndex, s); }
        s.add(c.importSectionIndex);
      }
    }

    // Filter pool to only remaining sections, excluding cycle-forming connections
    const pool = suggestionPoolRef.current.filter((s) => {
      if (connected.has(s.importSectionIndex) || skipped.has(s.importSectionIndex)) return false;
      // Skip import-to-import suggestions that would create a direct cycle:
      // if the target already has an importing connection pointing back to this section
      if (s.targetType === 'importing' && s.targetImportIndex != null) {
        const sourcesTargetingMe = importingTargets.get(s.importSectionIndex);
        if (sourcesTargetingMe?.has(s.targetImportIndex)) return false;
      }
      return true;
    });
    suggestionPoolRef.current = pool;

    if (pool.length === 0) return false;

    await ensureTemporalDistances(pool);
    const reranked = rerankSuggestionsByTemporalPlan(
      pool,
      dmpFile.sections,
      connectionsRef.current,
      temporalDistanceCacheRef.current,
    );
    suggestionPoolRef.current = reranked.suggestions;
    if (reranked.rejectedCount > 0) {
      debugLog(`[dmp-temporal-rerank] rejected ${reranked.rejectedCount} impossible candidate(s)`);
    }
    if (reranked.suggestions.length === 0) return false;

    const top = reranked.suggestions[0]!;
    // The default merge offset rides the same action as the suggestion, so
    // Confirm can never read the previous suggestion's offset.
    dispatch({
      type: 'suggest',
      suggestion: top,
      alternatives: reranked.suggestions
        .filter((s) => s !== top && s.importSectionIndex === top.importSectionIndex)
        .slice(0, 4),
      mergeOffset: computeDefaultMergeOffset(top),
    });
    return true;
  }, [computeDefaultMergeOffset, dmpFile.sections, ensureTemporalDistances, connectionsRef, skippedIndicesRef, dispatch]);

  /** A section that reached the emit loop and produced nothing would otherwise
   *  disappear between the wizard and the map: it is not in the `unreachable`
   *  report either, which lists only what was never placed. */
  const reportEmptySections = useCallback((indices: number[]) => {
    if (indices.length === 0) return;
    const names = indices.map(
      (i) => sectionEditsRef.current.get(i)?.name ?? dmpFile.sections[i]?.name ?? t('dialogs.dmpNewImport.sectionFallbackName', { id: i }),
    );
    useUIStore.getState().showToast(
      t('dmpWizard.toasts.emptySections', { names: names.join(', ') }),
      TOAST_KEYS.DMP_WIZARD_EMPTY_SECTIONS,
      'warning',
    );
  }, [dmpFile.sections, t]);

  /** Hand `unplaced` to manual placing, one segment at a time, starting with the
   *  auto-start handoff of the first to map placement. `mode` 'manual' opens it
   *  on map placement whatever it holds (the early import's way in); 'bySegment'
   *  opens a segment holding a skipped suggestion on reusing it. False when the
   *  sections form no segment. */
  const placeRemainingManually = useCallback((unplaced: number[], mode: 'manual' | 'bySegment'): boolean => {
    const queue = segmentQueue(computeUnreachableSegments(unplaced, connectionsRef.current));
    if (!queue) return false;
    segmentSkipCountRef.current = new Map();
    // The suggestion is the `suggesting` phase's own and leaves with it: the shot
    // table used to keep following the last SUGGESTED section into manual
    // placing while the anchor named another.
    dispatch({ type: 'placeManually', queue, choices: segmentChoices(), mode });
    return true;
  }, [connectionsRef, dispatch, segmentChoices]);

  /**
   * Nothing left to suggest: the run is done - unless sections are still not
   * placed (connected to nothing), which go to manual placing instead. Called
   * where the wizard gets there (the anchor was the whole import, or nothing
   * was left to suggest); it used to be an effect watching for `done`, which
   * ran again whenever the run's busy flag flipped - a withdrawal from that
   * screen completed the run a second time.
   */
  const finishRun = useCallback(() => {
    dispatch({ type: 'done' });
    const connected = new Set(connectionsRef.current.map((c) => c.importSectionIndex));
    const remaining = selectedIndices.filter(
      (idx) => !connected.has(idx) && idx !== zeroDatumConfig?.sectionIndex
    );
    if (remaining.length > 0 && placeRemainingManually(remaining, 'bySegment')) {
      clearFailure();
      return;
    }
    trackDialogComplete('dmp-import-new');
    // With the receipt, like the other three completion sites (#487). This one
    // is reached after the zero-datum import has already banked one - a
    // single-selected-section file, or a selection with nothing left to
    // suggest - so dropping it silenced the receipt for those imports too.
    onComplete(...completion());
  }, [clearFailure, completion, connectionsRef, dispatch, onComplete, placeRemainingManually, selectedIndices, zeroDatumConfig?.sectionIndex]);

  // Transition from suggesting phase: import reachable sections immediately (early import),
  // then enter manual_placing for unreachable segments, or complete if none.
  const transitionToPlacingOrManual = useCallback(async () => {
    if (advanceLatchRef.current === 'importing') return;
    advanceLatchRef.current = 'importing';

    try {
      // Import every reachable section now (the early import).
      const plan = await importBatch('early', () => {
        // 1. Overlap treatments (single source of truth): they replace the run's
        //    pending updates to existing shots.
        const existingShotUpdates = fullPoolRef.current.length > 0
          ? applyAllOverlapTreatments(
              fullPoolRef.current,
              confirmedOverlapsRef.current,
              sectionEditsRef.current,
              dmpFile,
              overlapMode,
            )
          : undefined;

        // 2. Redirect connections targeting deleted overlap zones
        redirectDeletedTargets(
          connectionsRef.current,
          sectionEditsRef.current,
          confirmedOverlapsRef.current,
          fullPoolRef.current,
        );

        // 3. Build batch import configs for reachable sections
        const batch = buildBatchImport(
          connectionsRef.current,
          zeroDatumConfig?.sectionIndex ?? null,
          dmpFile,
          selectionConfig,
          sectionEditsRef.current,
          fullPoolRef.current,
          directPlacementsRef.current,
          undefined,
          skippedIndicesRef.current,
          resolvedTheme,
          discardedIndicesRef.current,
        );
        reportEmptySections(batch.emptySections);
        return { ...batch, ...(existingShotUpdates ? { existingShotUpdates } : {}) };
      });
      if (!plan) return;
      const { unreachable } = plan;

      // 5. What is still unreachable goes to manual placing, map placement first.
      if (unreachable.length > 0 && placeRemainingManually(unreachable, 'manual')) return;

      // No unreachable - all done
      trackDialogComplete('dmp-import-new');
      onComplete(...completion());
    } finally {
      advanceLatchRef.current = 'idle';
    }
  }, [zeroDatumConfig, dmpFile, selectionConfig, overlapMode, onComplete, importBatch, completion, reportEmptySections, resolvedTheme, connectionsRef, skippedIndicesRef, placeRemainingManually]);

  /**
   * Ask the backend where every remaining section might connect, then present
   * the best match - or, with nothing to suggest, move on to placing.
   *
   * A failed call STOPS here (#576 D24, decided "Stop and offer Retry"). It used
   * to be swallowed as "nothing matched", so the wizard hid itself and started
   * map placement of every section by hand: a broken matcher looked exactly
   * like a survey with no overlaps. The user now sees why, and chooses between
   * Retry (this again) and Place manually (that old path, on purpose).
   */
  const computeSuggestions = useCallback(async () => {
    clearFailure();
    dispatch({ type: 'calculating', calculating: true });
    const zeroDatumIdx = zeroDatumConfig?.sectionIndex ?? null;

    const remainingSections: ImportSectionForSuggestion[] = [];
    for (const idx of selectedIndices) {
      if (idx === zeroDatumIdx) continue;
      const section = dmpFile.sections[idx];
      if (!section) continue;

      const editableShots = convertToEditableShots(section.shots, section.version);
      const shotData: ShotData[] = editableShots.map(editableShotToShotData);
      if (shotData.length === 0) continue;

      remainingSections.push({
        index: idx,
        name: section.name,
        shots: shotData,
        // Scored in the frame these readings are actually in (#438). Without
        // it the backend falls back to the project default, and a DMP section
        // would be matched against an existing Compass section across a
        // declination - a real overlap scoring as a mismatch.
        // camelCase here, snake_case on `ImportSectionConfig`: the two IPC
        // structs differ in their serde rename, and the generated types are
        // the authority.
        azimuthReference: DMP_AZIMUTH_REFERENCE,
      });
    }

    if (remainingSections.length === 0) {
      dispatch({ type: 'calculating', calculating: false });
      finishRun();
      return;
    }

    try {
      const suggestions = await invokeIpc(
        'suggest_all_import_placements',
        { importSections: remainingSections },
        { purpose: 'suggest import placements for remaining sections' },
      );

      suggestionPoolRef.current = suggestions;
      fullPoolRef.current = suggestions;
    } catch (e) {
      const reason = getErrorMessage(e);
      console.warn('[dmpImport] placement suggestions could not be computed:', reason);
      dispatch({ type: 'calculating', calculating: false });
      fail('suggestions', t('dialogs.dmpNewImport.suggestionsFailed', { reason }));
      return;
    }

    dispatch({ type: 'calculating', calculating: false });

    // A suggestion picked moves the wizard to `suggesting`; none left goes on
    // to the early import.
    if (!(await pickFromPool())) {
      transitionToPlacingOrManual();
    }
  }, [clearFailure, dispatch, dmpFile.sections, fail, finishRun, pickFromPool, selectedIndices, t, transitionToPlacingOrManual, zeroDatumConfig?.sectionIndex]);

  /**
   * The late import: whatever manual placement added, in one batch (overlap
   * treatment and the connection redirect already ran with the early import).
   * Called on entering `placing` - the queue emptied, or the instance reopened
   * on an empty one - and by Retry; it used to be an effect watching for
   * `placing` and a retry counter. What it leaves unreachable, bar what the user
   * discarded, goes back to manual placing.
   */
  const runLateImport = useCallback(async () => {
    const plan = await importBatch('late', () => {
      const batch = buildBatchImport(
        connectionsRef.current,
        zeroDatumConfig?.sectionIndex ?? null,
        dmpFile,
        selectionConfig,
        sectionEditsRef.current,
        fullPoolRef.current,
        directPlacementsRef.current,
        earlyImported,
        undefined,
        resolvedTheme,
        discardedIndicesRef.current,
      );
      reportEmptySections(batch.emptySections);
      return batch;
    });
    if (!plan) return;

    // Filter out discarded segments - they should not be placed later.
    const remaining = plan.unreachable.filter(idx => !discardedIndicesRef.current.has(idx));
    if (remaining.length > 0 && placeRemainingManually(remaining, 'bySegment')) {
      clearFailure();
      return;
    }

    trackDialogComplete('dmp-import-new');
    onComplete(...completion());
  }, [clearFailure, completion, connectionsRef, dmpFile, earlyImported, importBatch, onComplete, placeRemainingManually, reportEmptySections, resolvedTheme, selectionConfig, zeroDatumConfig?.sectionIndex]);

  /** Start the run: the zero-datum anchor, then the suggestions. */
  const startRun = useCallback(async () => {
    // 1. Import zero datum section if provided (this is the anchor)
    if (zeroDatumConfig) {
      const { sectionIndex } = zeroDatumConfig;
      if (!(await importAnchor(() => buildZeroDatumSectionConfig(zeroDatumConfig)))) return;

      // Snap-fit to the newly placed zero datum section
      useUIStore.getState().triggerFitToBounds({ snap: true });

      // Check if only 1 section was selected - we're done
      if (selectedIndices.length === 1 && selectedIndices[0] === sectionIndex) {
        finishRun();
        return;
      }
    } else {
      // No zero datum - check if we can suggest
      const existingProject = useProjectStore.getState().project;
      const hasExistingSections = (existingProject?.sections.length ?? 0) > 0;

      if (!hasExistingSections && selectedIndices.length <= 1) {
        finishRun();
        return;
      }
    }

    // 2. Fire suggest_all_import_placements with all remaining sections
    await computeSuggestions();
  }, [computeSuggestions, finishRun, importAnchor, selectedIndices, zeroDatumConfig]);


  // Confirm handler - records a virtual connection + overlap info (instant, no backend call).
  // Overlap treatment is deferred to applyAllOverlapTreatments which runs before batch import.

  const handleConfirm = useCallback(async () => {
    if (!currentSuggestion) return;
    if (refuseIncompleteShots()) return;
    if (advanceLatchRef.current !== 'idle') return;
    advanceLatchRef.current = 'picking';
    try {
      debugLog(
        `[handleConfirm] import=${currentSuggestion.importSectionIndex} "${currentSuggestion.importSectionName}"`,
        `→ target=${currentSuggestion.targetType === 'importing' ? currentSuggestion.targetImportIndex : currentSuggestion.existingSectionId}`,
        `"${currentSuggestion.existingSectionName}"`,
        `type=${currentSuggestion.targetType}`,
        `impStn=${currentSuggestion.importStationIndex} exStn=${currentSuggestion.existingStationIndex}`,
        `matched=${currentSuggestion.matchedShots} dir=${currentSuggestion.direction}`,
      );
      clearFailure();

      // Read fresh edit state from the ref (synchronously up-to-date) instead of
      // the editState React state which may be one render behind when the user
      // confirms quickly after a suggestion change.
      const importIdx = currentSuggestion.importSectionIndex;
      const freshEditState = sectionEditsRef.current.get(importIdx);
      if (!freshEditState) return; // shouldn't happen - init-effect always populates

      // Save raw user edits unchanged - NO overlap treatment here.
      // applyAllOverlapTreatments will handle all overlap mutation before batch import.
      sectionEditsRef.current.set(importIdx, freshEditState);

      // Record confirmed overlap info so applyAllOverlapTreatments knows the merge station
      const mergeImportStation = selectedMergePair?.importStation ?? currentSuggestion.importStationIndex;
      confirmedOverlapsRef.current.set(importIdx, {
        importStationIndex: currentSuggestion.importStationIndex,
        matchedShots: currentSuggestion.matchedShots,
        existingMatchedShots: currentSuggestion.existingMatchedShots,
        direction: currentSuggestion.direction,
        existingStationIndex: currentSuggestion.existingStationIndex,
        targetType: currentSuggestion.targetType,
        existingSectionId: currentSuggestion.existingSectionId ?? null,
        targetImportIndex: currentSuggestion.targetImportIndex ?? null,
        mergeImportStation,
      });

      // Use selected merge pair for station indices (falls back to suggestion's original)
      const mergeExistingStation = selectedMergePair?.existingStation ?? currentSuggestion.existingStationIndex;

      const newConn: VirtualConnection = {
        importSectionIndex: currentSuggestion.importSectionIndex,
        importStationIndex: mergeImportStation,
        targetType: currentSuggestion.targetType,
        ...(currentSuggestion.existingSectionId != null && { existingSectionId: currentSuggestion.existingSectionId }),
        ...(currentSuggestion.targetType === 'existing' && { existingStationIndex: mergeExistingStation }),
        ...(currentSuggestion.targetImportIndex != null && { targetImportIndex: currentSuggestion.targetImportIndex }),
        ...(currentSuggestion.targetType === 'importing' && { targetImportStationIndex: mergeExistingStation }),
      };

      setConnections((prev) => [...prev, newConn]);
      // Update ref eagerly
      connectionsRef.current = [...connectionsRef.current, newConn];

      // Check if all sections are now connected or skipped
      const newConnected = new Set(connectionsRef.current.map((c) => c.importSectionIndex));
      const zeroDatumIdx = zeroDatumConfig?.sectionIndex ?? null;
      const remaining = selectedIndices.filter(
        (idx) => idx !== zeroDatumIdx && !newConnected.has(idx) && !skippedIndicesRef.current.has(idx)
      );

      if (remaining.length === 0) {
        // All handled - transition to placing (or manual_placing if unreachable)
        transitionToPlacingOrManual();
        return;
      }

      // Pick next from pool
      if (!(await pickFromPool())) {
        // Pool exhausted - check for unreachable sections
        transitionToPlacingOrManual();
        return;
      }
    } finally {
      // Still held by this action unless the early import took it over.
      if (advanceLatchRef.current === 'picking') advanceLatchRef.current = 'idle';
    }
  }, [currentSuggestion, selectedMergePair, zeroDatumConfig, selectedIndices, pickFromPool, transitionToPlacingOrManual, clearFailure, refuseIncompleteShots, connectionsRef, skippedIndicesRef]);

  // Skip handler (suggesting phase)
  const handleSkip = useCallback(async () => {
    if (refuseIncompleteShots()) return;
    if (!currentSuggestion) return;
    if (advanceLatchRef.current !== 'idle') return;
    advanceLatchRef.current = 'picking';
    try {
      const skippedIdx = currentSuggestion.importSectionIndex;
      const mergeImportStation = selectedMergePair?.importStation ?? currentSuggestion.importStationIndex;
      const mergeExistingStation = selectedMergePair?.existingStation ?? currentSuggestion.existingStationIndex;

      // Record the suggestion so it can be re-shown in manual_placing
      skippedSuggestionsRef.current.set(skippedIdx, {
        suggestion: currentSuggestion,
        mergeImportStation,
        mergeExistingStation,
      });

      setSkippedIndices((prev) => {
        const next = new Set(prev);
        next.add(skippedIdx);
        return next;
      });

      // Update ref eagerly
      const newSkipped = new Set(skippedIndicesRef.current);
      newSkipped.add(skippedIdx);
      skippedIndicesRef.current = newSkipped;

      const zeroDatumIdx = zeroDatumConfig?.sectionIndex ?? null;
      const newConnected = new Set(connectionsRef.current.map((c) => c.importSectionIndex));
      const remaining = selectedIndices.filter(
        (idx) => idx !== zeroDatumIdx && !newConnected.has(idx) && !newSkipped.has(idx)
      );

      if (remaining.length === 0) {
        transitionToPlacingOrManual();
        return;
      }

      if (!(await pickFromPool())) {
        transitionToPlacingOrManual();
        return;
      }
    } finally {
      if (advanceLatchRef.current === 'picking') advanceLatchRef.current = 'idle';
    }
  }, [currentSuggestion, selectedMergePair, zeroDatumConfig, selectedIndices, pickFromPool, transitionToPlacingOrManual, connectionsRef, skippedIndicesRef]);

  const handleMergeOffsetChange = useCallback(
    (mergeOffset: number) => dispatch({ type: 'setMergeOffset', mergeOffset }),
    [dispatch],
  );

  // Alternative click handler
  const handleAlternativeClick = useCallback((alt: CrossSectionSuggestion) => {
    // The reducer swaps it with the suggestion on screen, with its default offset.
    dispatch({ type: 'chooseAlternative', alternative: alt, mergeOffset: computeDefaultMergeOffset(alt) });
  }, [computeDefaultMergeOffset, dispatch]);

  // ─── Manual placing phase handlers ────────────────────────────────────────

  // Access UI store for map placement (use selectors to avoid full-store subscription)
  const startPlacementMode = useUIStore((s) => s.startPlacementMode);
  const closeDialog = useUIStore((s) => s.closeDialog);
  const openDialog = useUIStore((s) => s.openDialog);
  const showToast = useUIStore((s) => s.showToast);
  const setDmpImportWizardPhase = useUIStore((s) => s.setDmpImportWizardPhase);
  const onboardingWizardPhase =
    phase.kind === 'suggesting'
      ? ('suggesting' as const)
      : phase.kind === 'manual_placing' && !phase.handoffPending
        ? ('manual_placing' as const)
        : null;

  useEffect(() => {
    setDmpImportWizardPhase(onboardingWizardPhase);
  }, [onboardingWizardPhase, setDmpImportWizardPhase]);

  useEffect(
    () => () => {
      setDmpImportWizardPhase(null);
    },
    [setDmpImportWizardPhase]
  );


  // The manual_placing phase's mode and From Existing pick
  const segmentPlacementMode = manual?.mode ?? 'manual';
  const fromExistingSectionId = manual?.fromExisting.sectionId ?? null;
  const fromExistingStationIdx = manual?.fromExisting.stationIndex ?? 0;
  const setSegmentPlacementMode = useCallback(
    (mode: SegmentPlacementMode) => dispatch({ type: 'setMode', mode }),
    [dispatch],
  );
  const setFromExistingSectionId = useCallback(
    (sectionId: number | null) => dispatch({ type: 'setFromExistingSection', sectionId }),
    [dispatch],
  );
  const setFromExistingStationIdx = useCallback(
    (stationIndex: number) => dispatch({ type: 'setFromExistingStation', stationIndex }),
    [dispatch],
  );


  // Current segment's skipped suggestion (if any section in the segment had one)
  const segmentSkippedSuggestion = useMemo(() => {
    if (!currentSegment) return null;
    for (const idx of currentSegment) {
      const suggestion = skippedSuggestionsRef.current.get(idx);
      if (suggestion) return suggestion;
    }
    return null;
  }, [currentSegment]);

  // Advance to the next segment, or - the queue empty - to the late import.
  const advanceSegmentQueue = useCallback(() => {
    const before = currentWizard().phase.kind;
    const next = dispatch({ type: 'segmentDone', choices: segmentChoices() });
    if (before === 'manual_placing' && next.phase.kind === 'placing') void runLateImport();
  }, [currentWizard, dispatch, runLateImport, segmentChoices]);

  // Place segment handler
  const handleSegmentPlace = useCallback(() => {
    if (refuseIncompleteShots()) return;
    if (!currentSegment || !segmentAnchor) return;
    const anchorStation = emittedAnchorStation(segmentAnchor);

    let placement: ImportPlacement;
    if (segmentPlacementMode === 'suggestion' && segmentSkippedSuggestion) {
      // Use the skipped suggestion as a from_existing connection
      const suggestion = segmentSkippedSuggestion.suggestion;
      if (suggestion.targetType === 'existing' && suggestion.existingSectionId != null) {
        // Add as a virtual connection instead
        const newConn: VirtualConnection = {
          importSectionIndex: suggestion.importSectionIndex,
          importStationIndex: segmentSkippedSuggestion.mergeImportStation,
          targetType: 'existing',
          existingSectionId: suggestion.existingSectionId,
          existingStationIndex: segmentSkippedSuggestion.mergeExistingStation,
        };
        setConnections((prev) => [...prev, newConn]);
        connectionsRef.current = [...connectionsRef.current, newConn];
        advanceSegmentQueue();
        return;
      } else if (suggestion.targetType === 'importing' && suggestion.targetImportIndex != null) {
        const newConn: VirtualConnection = {
          importSectionIndex: suggestion.importSectionIndex,
          importStationIndex: segmentSkippedSuggestion.mergeImportStation,
          targetType: 'importing',
          targetImportIndex: suggestion.targetImportIndex,
          targetImportStationIndex: segmentSkippedSuggestion.mergeExistingStation,
        };
        setConnections((prev) => [...prev, newConn]);
        connectionsRef.current = [...connectionsRef.current, newConn];
        advanceSegmentQueue();
        return;
      }
      return;
    } else if (segmentPlacementMode === 'fromExisting') {
      if (fromExistingSectionId == null) return;
      placement = {
        type: 'from_existing',
        import_station: anchorStation,
        existing_station: {
          section_id: fromExistingSectionId,
          station_index: fromExistingStationIdx,
        },
      };
    } else if (segmentPlacementMode === 'discard') {
      // Discard: skip this segment entirely without creating a placement
      for (const idx of currentSegment) {
        discardedIndicesRef.current.add(idx);
      }
      advanceSegmentQueue();
      return;
    } else if (segmentPlacementMode === 'manual') {
      // Manual mode is handled by handleStartSegmentPlacement
      return;
    } else {
      return;
    }

    const dp: DirectPlacement = {
      anchorSectionIndex: segmentAnchor.sectionIndex,
      anchorStationIndex: anchorStation,
      placement,
    };
    directPlacementsRef.current = [...directPlacementsRef.current, dp];
    advanceSegmentQueue();
  }, [currentSegment, segmentAnchor, segmentPlacementMode, segmentSkippedSuggestion, fromExistingSectionId, fromExistingStationIdx, advanceSegmentQueue, emittedAnchorStation, refuseIncompleteShots, connectionsRef]);

  // Skip segment handler - appends to end of queue
  const handleSegmentSkip = useCallback(() => {
    if (!currentSegment) return;
    const key = currentSegment.join(',');
    const count = (segmentSkipCountRef.current.get(key) ?? 0) + 1;
    segmentSkipCountRef.current.set(key, count);

    // To the back of the queue (the only segment left stays). The next one
    // opens like every other way into a segment: 'manual' unless it holds a
    // skipped suggestion - Skip used to set an 'origin' mode the radio group
    // has no option for, so nothing came up selected (#576 NF-AN4-N3).
    dispatch({ type: 'segmentSkipped', choices: segmentChoices() });
  }, [currentSegment, dispatch, segmentChoices]);


  const buildSegmentPlacementPayload = useCallback(
    (segment: number[], anchor: { sectionIndex: number; stationIndex: number }) => {
      // The manual-placing screen reads the same condition for its reason line.
      const anchorDmpSection = anchorSectionIn(dmpFile.sections, anchor);
      if (!anchorDmpSection) {
        return null;
      }

      const edits = sectionEditsRef.current.get(anchor.sectionIndex);
      const shots: ShotData[] = edits
        ? edits.shots.filter(isEmittedShot).map(editableShotToShotData)
        : convertToEditableShots(anchorDmpSection.shots, anchorDmpSection.version).map(editableShotToShotData);
      const emittedStationIndex = emittedAnchorStation(anchor);

      const sectionData = buildPlacementSectionData({
        dmpSection: anchorDmpSection,
        edits,
        defaultSurveyor: selectionConfig.defaultSurveyor,
        defaultDate: selectionConfig.defaultDate,
        palette: sectionPalette,
        paletteIndex: selectedIndices.indexOf(anchor.sectionIndex),
        shots,
      });

      // Build additional sections for segment shadow using effective (post-treatment)
      // sections so the overlay reflects deleted shots and interpolated values.
      const effectiveSections = dmpFile.sections.map((sec, idx) =>
        buildEffectiveDmpSection(sec, sectionEditsRef.current.get(idx)),
      );
      const effectiveConns = adjustConnectionsForEdits(connectionsRef.current, sectionEditsRef.current);
      const layout = computeSegmentLayout(segment, effectiveConns, effectiveSections);
      // Offsets from computeSegmentLayout are relative to station 0 of the root
      // section (segmentIndices[0]). Re-base them relative to the anchor section's
      // station 0 so PlacementOverlay positions them correctly.
      const anchorLayoutEntry = layout.find((entry) => entry.sectionIndex === anchor.sectionIndex);
      const anchorLayoutOffsetX = anchorLayoutEntry?.offsetX ?? 0;
      const anchorLayoutOffsetY = anchorLayoutEntry?.offsetY ?? 0;
      const additionalSections = layout
        .filter((entry) => entry.sectionIndex !== anchor.sectionIndex)
        .map((entry) => {
          const sec = effectiveSections[entry.sectionIndex];
          if (!sec) return null;
          const secEdits = sectionEditsRef.current.get(entry.sectionIndex);
          const secSelectedPos = selectedIndices.indexOf(entry.sectionIndex);
          const secColor =
            (secEdits?.color || null) ??
            sectionPalette[(secSelectedPos >= 0 ? secSelectedPos : entry.sectionIndex) % sectionPalette.length] ??
            '#95a5a6';
          const secShots = secEdits
            ? secEdits.shots.filter(isEmittedShot).map(editableShotToShotData)
            : convertToEditableShots(sec.shots, sec.version).map(editableShotToShotData);
          return {
            sectionData: {
              name: secEdits?.name ?? sec.name,
              surveyor: null,
              date: null,
              color: secColor,
              direction: secEdits?.direction ?? sec.direction,
              quality: secEdits?.quality ?? 1.0,
              shots: secShots,
            },
            color: secColor,
            offsetX: entry.offsetX - anchorLayoutOffsetX,
            offsetY: entry.offsetY - anchorLayoutOffsetY,
          };
        })
        .filter((value): value is NonNullable<typeof value> => value !== null);

      return { sectionData, additionalSections, emittedStationIndex };
    },
    [dmpFile.sections, emittedAnchorStation, selectionConfig.defaultDate, selectionConfig.defaultSurveyor, selectedIndices, sectionPalette, connectionsRef],
  );

  /** Import the segment just placed on the map: the whole batch as it stands
   *  with that placement added, narrowed by the run to the segment's sections. */
  const importPlacedSegmentNow = useCallback(
    (segmentIndices: number[], directPlacements: DirectPlacement[]) =>
      importSegment(segmentIndices, () => {
        const batch = buildBatchImport(
          connectionsRef.current,
          zeroDatumConfig?.sectionIndex ?? null,
          dmpFile,
          selectionConfig,
          sectionEditsRef.current,
          fullPoolRef.current,
          directPlacements,
          earlyImported,
          undefined,
          resolvedTheme,
          discardedIndicesRef.current,
        );
        reportEmptySections(batch.emptySections);
        return batch;
      }),
    [dmpFile, earlyImported, importSegment, reportEmptySections, selectionConfig, zeroDatumConfig?.sectionIndex, resolvedTheme, connectionsRef],
  );

  /**
   * Hand this segment off to map placement.
   *
   * Carries NO payload guard, deliberately (#476). `refuseIncompleteShots`
   * exists to protect an IPC payload, and this hands off nothing: the payload
   * leaves later, from `importPlacedSegmentNow`, once the user has dropped the
   * section. Gating the handoff on the completeness of the WHOLE selection is
   * what turned one zero-length row in the file into a dead end - the auto-start
   * effect below runs this while the dialog is hidden, so a refusal here was
   * invisible AND unrecoverable. The guard stays where a payload is actually at
   * stake: `handlePlaceOnMapClick` (the user-initiated button), the segment
   * submit, and `handleConfirm`/`handleSkip`.
   *
   * It does not end the hidden handoff window either (the manual_placing
   * phase's `handoffPending`): the effect that attempts the handoff does - see
   * there.
   */
  const handleStartSegmentPlacement = useCallback(() => {
    // The payload is still guarded: importPlacedSegmentNow -> buildBatchImport -> assertSendableShots.
    if (!manual) return;
    const { queue: segmentQueue, anchor: segmentAnchor } = manual;
    const currentSegment = segmentQueue[0];

    // Reopen data (same as what App.tsx passes)
    const reopenData = {
      dmpFile,
      filePath: (useUIStore.getState().dialogData as Record<string, unknown>).filePath,
      selectionConfig,
      zeroDatumConfig,
    };

    const savedState = (
      directPlacements: DirectPlacement[],
      placedSegment?: SavedManualPlacingState['placedSegment'],
    ): SavedManualPlacingState => ({
      connections: [...connectionsRef.current],
      skippedIndices: Array.from(skippedIndicesRef.current),
      segmentQueue,
      segmentAnchor,
      directPlacements,
      skippedSuggestions: Array.from(skippedSuggestionsRef.current.entries()),
      segmentSkipCounts: Array.from(segmentSkipCountRef.current.entries()),
      sectionEdits: Array.from(sectionEditsRef.current.entries()),
      confirmedOverlaps: Array.from(confirmedOverlapsRef.current.entries()),
      discardedIndices: Array.from(discardedIndicesRef.current),
      ...carry(),
      ...(placedSegment ? { placedSegment } : {}),
    });

    const directPlacements = [...directPlacementsRef.current];
    const reopen = (autoStart: boolean, placedSegment?: SavedManualPlacingState['placedSegment']) => {
      setSource('dialog');
      openDialog('dmp-import-new', {
        ...reopenData,
        autoStartManualPlacement: autoStart,
        savedManualPlacingState: savedState(directPlacements, placedSegment),
      });
    };

    const payload = buildSegmentPlacementPayload(currentSegment, segmentAnchor);
    if (!payload) {
      // [#576 NF-W9FE-3] Nothing to put on the map (the anchor section is not in
      // the file). Leave the wizard as it is: reopening it and then closing it
      // below ended the run without a word, and from the auto-start handoff the
      // settled window now shows this segment's manual-placing screen instead,
      // which says why (#576 NF-W9FE-3-D1).
      console.warn('[dmpImport] no placement payload for anchor section', segmentAnchor.sectionIndex);
      return;
    }
    const anchor = segmentAnchor;
    startPlacementMode(
      payload.sectionData,
      payload.emittedStationIndex,
      (result: PlacementResult) => {
        // The SAME station the overlay was aligned to. Storing the visible
        // index here while placing against the emitted one meant the preview
        // and the import disagreed, and the segment landed a station too far
        // downstream whenever a blank row preceded the anchor.
        const alignStation = payload.emittedStationIndex;
        const placement: ImportPlacement = result.snappedToStation
          ? { type: 'manual_snap', align_station_index: alignStation, snapped_to_station: result.snappedToStation }
          : { type: 'manual', align_station_index: alignStation, offset: result.position };
        // The drop only RECORDS the placement (#576 D13): the reopened wizard
        // imports it, hidden like any auto-started handoff, so a failure shows
        // on this segment's screen and a success hands off the next segment.
        reopen(true, {
          segment: currentSegment,
          placement: { anchorSectionIndex: anchor.sectionIndex, anchorStationIndex: alignStation, placement },
        });
      },
      () => reopen(false),
      payload.additionalSections.length > 0 ? payload.additionalSections : undefined,
      DMP_AZIMUTH_REFERENCE,
    );

    showToast(
      t('dmpWizard.toasts.placementHint'),
      TOAST_KEYS.DMP_MANUAL_PLACEMENT_HINT,
      'info'
    );

    // Close dialog so user can interact with the map
    trackDialogComplete('dmp-import-new');
    closeDialog();
  }, [
    manual,
    buildSegmentPlacementPayload,
    dmpFile,
    selectionConfig,
    zeroDatumConfig,
    carry,
    startPlacementMode,
    closeDialog,
    openDialog,
    showToast,
    connectionsRef,
    skippedIndicesRef,
  ]);

  /**
   * Where this instance starts the run, once (a ref, so StrictMode's second
   * mount does not start it again):
   *
   * - a fresh wizard imports the anchor and asks for suggestions (`startRun`);
   * - #576 D13: one reopened after a drop on the map imports that segment HERE -
   *   the drop reopened the wizard with it pending
   *   (`SavedManualPlacingState.placedSegment`), and the run starts busy, so the
   *   exits and the auto-start handoff wait for the answer. Imported: the
   *   placement is recorded and the queue advances, and the handoff takes the
   *   next segment (or the late import runs). Refused: the segment stays queued,
   *   its placement is not kept, and the wizard shows the failure on its screen;
   * - one reopened on an empty queue goes straight to the late import.
   *
   * The continuations read the LATEST callbacks: the import answers after
   * renders this closure never saw.
   */
  const runStartedRef = useRef(false);
  const advanceSegmentQueueRef = useLatest(advanceSegmentQueue);
  useEffect(() => {
    if (runStartedRef.current) return;
    runStartedRef.current = true;
    if (!saved) {
      void startRun();
      return;
    }
    const placed = saved.placedSegment;
    if (placed) {
      const placements = [...directPlacementsRef.current, placed.placement];
      void importPlacedSegmentNow(placed.segment, placements).then((imported) => {
        if (!imported) {
          dispatch({ type: 'handoffSettled' });
          return;
        }
        directPlacementsRef.current = placements;
        advanceSegmentQueueRef.current();
      });
      return;
    }
    if (currentWizard().phase.kind === 'placing') void runLateImport();
  // eslint-disable-next-line react-hooks/exhaustive-deps -- mount only: `saved` is this instance's restore data, and the run starts once
  }, []);

  /**
   * "Place on Map", clicked by the user. This is where the payload guard
   * belongs: the click is a deliberate step toward an import, the dialog is
   * VISIBLE, and the refusal toast names a section the user can then go and fix
   * (#476 moved it off the auto-start path, where none of that was true).
   */
  const handlePlaceOnMapClick = useCallback(() => {
    if (refuseIncompleteShots()) return;
    handleStartSegmentPlacement();
  }, [refuseIncompleteShots, handleStartSegmentPlacement]);

  /**
   * The auto-start handoff: what ends the manual_placing phase's
   * `handoffPending` (with the placed-segment import's refusal).
   *
   * That flag hides the whole dialog (`hidden` -> `display:none` on
   * DialogWrapper's container), so the window it opens MUST be closed by
   * whatever happens next. Its honest meaning is "the handoff has not been
   * ATTEMPTED yet" - a state that any single attempt leaves, whatever the
   * attempt decides - and it exists only inside manual placing, so leaving that
   * phase ends it too.
   *
   * It used to be cleared inside `handleStartSegmentPlacement`, after two early
   * returns, so a callee that declined left the dialog mounted, `display:none`,
   * with `activeDialog` still set - an invisible modal that made the whole app
   * `inert` with no mouse affordance and no retry, because nothing in this
   * effect's dependencies changes on a refusal (#476). Keeping ownership here
   * makes the window unstrandable by any future early return, which is the
   * actual invariant; clearing above each individual `return` would be correct
   * today and re-breakable tomorrow.
   *
   * `isImporting` is the ONLY reason to wait - it flips false and re-runs this
   * effect. The terminal ones (no segment, no anchor, another phase) cannot be
   * represented while the handoff is pending (wizardState.ts).
   */
  const handoffPending = manual?.handoffPending ?? false;
  useEffect(() => {
    if (!handoffPending || isImporting) return;

    const timeoutId = window.setTimeout(() => {
      // Attempted, so the window is over either way. In the success case the
      // handoff closes the dialog in this same commit, so nothing flashes; in
      // any other case the wizard becomes VISIBLE on the manual-placing screen,
      // which is where the user can act.
      try {
        handleStartSegmentPlacement();
      } finally {
        dispatch({ type: 'handoffSettled' });
      }
    }, 0);

    return () => {
      window.clearTimeout(timeoutId);
    };
  }, [handoffPending, isImporting, handleStartSegmentPlacement, dispatch]);

  // ─── Cancel / Back ────────────────────────────────────────────────────────
  //
  // This wizard imports EAGERLY - the zero-datum anchor lands in the project on
  // mount, and each reachable segment as it is placed, because the map has to
  // render what it is placing. So by the time either exit is reached the project
  // may already hold sections the user never agreed to keep, and neither exit
  // used to say so: the gate asked only about `connections` (client-side records
  // that have imported nothing), so cancelling right after the zero-datum import
  // closed silently and left the section behind (#394 F2).
  //
  // Both exits therefore offer to WITHDRAW what was imported
  // (`withdrawImportedSections`, the run's one batched `delete_section` - one
  // undo entry for one user action).

  /**
   * Neither exit may be OPENED while an import is in flight.
   *
   * An import banks its section ids only when it resolves, so a withdrawal
   * decided now would snapshot a list that is about to grow, and the late
   * arrival would survive in the project with nothing tracking it - the exact
   * shape F2 exists to prevent. The footer buttons are disabled for the same
   * reason; this also covers the routes that bypass them (the dialog's own
   * close button and Escape, both of which land on `handleCancel`).
   */
  const exitBlocked = isImporting;

  /** Leaving a confirmation also drops the reason its last removal failed: it
   *  belongs to that dialog, and the next one asks afresh. */
  const closeExitConfirm = useCallback(() => {
    dispatch({ type: 'closeExit' });
    clearWithdrawalFailure();
  }, [clearWithdrawalFailure, dispatch]);

  const handleCancel = useCallback(() => {
    if (isImporting) return;
    const count = importedCount();
    if (connections.length > 0 || count > 0) {
      clearWithdrawalFailure();
      dispatch({ type: 'openExit', exit: { kind: 'cancel', count } });
    } else {
      onCancel();
    }
  }, [connections.length, onCancel, isImporting, importedCount, clearWithdrawalFailure, dispatch]);

  /** Close the wizard, first removing what it imported. */
  const handleCancelWithRemoval = useCallback(async () => {
    if (!(await withdrawImportedSections())) return;
    dispatch({ type: 'closeExit' });
    onCancel();
  }, [withdrawImportedSections, onCancel, dispatch]);

  const handleBack = useCallback(() => {
    if (isImporting) return;
    const count = importedCount();
    if (connections.length > 0 || count > 0) {
      clearWithdrawalFailure();
      dispatch({ type: 'openExit', exit: { kind: 'back', count } });
    } else {
      onBack();
    }
  }, [connections.length, onBack, isImporting, importedCount, clearWithdrawalFailure, dispatch]);

  /**
   * Back re-enters the flow at a step BEFORE anything was imported (zero-datum
   * config, or the section selection), and the forward pass re-imports from
   * scratch - the anchor on mount, then the whole suggest/early-import pipeline.
   * So Back has to withdraw first, or the second pass duplicates every section
   * the first one created (#394 F1). Carrying "already imported, don't redo it"
   * across instead cannot work: the mount effect that imports the anchor is the
   * same one that fires `suggest_all_import_placements`, so suppressing it
   * strands the reopened wizard in `init` with nothing to do - and it would also
   * pin the anchor to the GPS/name/shot edits of the FIRST pass, which is
   * exactly what the user pressed Back to change.
   */
  const handleBackWithRemoval = useCallback(async () => {
    if (!(await withdrawImportedSections())) return;
    dispatch({ type: 'closeExit' });
    onBack();
  }, [withdrawImportedSections, onBack, dispatch]);


  // Progress - during manual_placing, compute from queue (which has ALL unplaced sections).
  // Otherwise, count connected sections + zero datum.
  const sectionsInQueue = useMemo(() => (manual ? manual.queue.flat().length : 0), [manual]);
  const placedCount = phase.kind === 'manual_placing' || phase.kind === 'placing' || phase.kind === 'done'
    ? totalSelected - sectionsInQueue
    : connectedIndices.size + (zeroDatumConfig ? 1 : 0);
  const skippedCount = skippedIndices.size;



  // Whether current placement mode is valid for submission
  const isSegmentPlacementValid = useMemo(() => {
    if (!segmentAnchor) return false;
    if (segmentPlacementMode === 'suggestion') return segmentSkippedSuggestion != null;
    if (segmentPlacementMode === 'fromExisting') return fromExistingSectionId != null;
    if (segmentPlacementMode === 'manual') return false; // must use "Place on Map" button
    if (segmentPlacementMode === 'discard') return true;
    return false;
  }, [segmentAnchor, segmentPlacementMode, segmentSkippedSuggestion, fromExistingSectionId]);

  // The section on screen's rows, the same table under either step's body.
  const shotTable = editState ? (
    <ShotTableEditor
      shots={effectiveShots}
      onShotChange={handleShotChange}
      onAddShot={handleAddShot}
      onAddShots={handleAddShots}
      onInsertShots={handleInsertShots}
      onDeleteShot={handleDeleteShot}
      onDeleteShots={handleDeleteShots}
      frozenShotIndices={frozenShotIndices}
    />
  ) : null;

  return (
    <DialogWrapper
      title={t('dialogs.dmpNewImport.title')}
      onClose={handleCancel}
      size="fullscreen"
      overflow="hidden"
      bodyClassName="flex-1 flex flex-col p-0 min-h-0"
      hidden={handoffPending}
      data-testid="dmp-new-import-dialog"
    >
      <div className="flex-1 flex flex-col min-h-0">
        {/* Progress header */}
        <div className="px-4 py-2 border-b border-border text-sm text-text-secondary">
          <span data-testid="wizard-progress">
            {t('dialogs.dmpNewImport.progress', { placed: placedCount, total: totalSelected })}
          </span>
          {skippedCount > 0 && (
            <span className="ml-2" data-testid="wizard-skipped">
              · {t('dialogs.dmpNewImport.skipped', { count: skippedCount })}
            </span>
          )}
        </div>

        {/* Body: side-by-side when showing suggestion, centered otherwise */}
        {suggesting ? (
          <SuggestingStep
            currentSuggestion={suggesting.suggestion}
            alternatives={alternatives}
            onAlternativeClick={handleAlternativeClick}
            mergeOptions={mergeOptions}
            selectedMergeOffset={selectedMergeOffset}
            selectedMergePair={selectedMergePair}
            onMergeOffsetChange={handleMergeOffsetChange}
            overlapMode={overlapMode}
            onOverlapModeChange={handleOverlapModeChange}
            isTargetDoublyAnchored={isTargetDoublyAnchored}
            error={error}
            editState={editState}
            onMetadataChange={handleMetadataChange}
            table={shotTable}
            connections={connections}
            dmpFile={dmpFile}
            selectedIndices={selectedIndices}
            sectionPalette={sectionPalette}
          />
        ) : manual ? (
          <ManualPlacingStep
            phase={manual}
            segmentSkippedSuggestion={segmentSkippedSuggestion}
            onModeChange={setSegmentPlacementMode}
            onCommitAnchor={commitSegmentAnchor}
            onAnchorSectionHover={handleAnchorSectionHover}
            onAnchorStationHover={handleAnchorStationHover}
            onFromExistingSectionChange={setFromExistingSectionId}
            onFromExistingStationChange={setFromExistingStationIdx}
            onPlaceOnMap={handlePlaceOnMapClick}
            emittedAnchorStation={emittedAnchorStation}
            visibleAnchorStation={visibleAnchorStation}
            error={error}
            editState={editState}
            onMetadataChange={handleMetadataChange}
            table={shotTable}
            sectionEditsRef={sectionEditsRef}
            connections={connections}
            dmpFile={dmpFile}
            selectedIndices={selectedIndices}
            sectionPalette={sectionPalette}
          />
        ) : statusPhase ? (
          <StatusStep
            phase={statusPhase}
            isImporting={isImporting}
            failure={failure}
            placingCount={connectedIndices.size}
            allPlaced={placedCount === totalSelected}
          />
        ) : null}

        <WizardFooter exitBlocked={exitBlocked} onCancel={handleCancel} onBack={handleBack}>
          {suggesting && <SuggestingActions busy={isImporting} onSkip={handleSkip} onConfirm={handleConfirm} />}
          {manual && (
            <ManualPlacingActions
              canSkip={manual.queue.length > 1}
              onSkip={handleSegmentSkip}
              canPlace={isSegmentPlacementValid}
              discarding={segmentPlacementMode === 'discard'}
              onPlace={handleSegmentPlace}
            />
          )}
          {/* #576 D24: the suggestions failed - ask again, or place by hand. */}
          {failure?.kind === 'suggestions' && !isCalculating && !isImporting && (
            <SuggestionsFailedActions
              onPlaceManually={() => { void transitionToPlacingOrManual(); }}
              onRetry={() => { void computeSuggestions(); }}
            />
          )}
          {phase.kind === 'placing' && error && !isImporting && (
            <LateImportRetryAction onRetry={() => { void runLateImport(); }} />
          )}
        </WizardFooter>
      </div>

      <ExitConfirmations
        exit={wizard.exit}
        withdrawInFlight={withdrawInFlight}
        withdrawalFailure={withdrawalFailure}
        onContinue={closeExitConfirm}
        onKeepAndClose={onCancel}
        onRemoveAndClose={() => { void handleCancelWithRemoval(); }}
        onRemoveAndBack={() => { void handleBackWithRemoval(); }}
      />
    </DialogWrapper>
  );
}
