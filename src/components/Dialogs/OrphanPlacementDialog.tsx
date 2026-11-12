import { useState, useCallback, useMemo, useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { Button } from '../common/Button';
import { Select } from '../common/Select';
import { DialogWrapper } from './DialogWrapper';
import SectionPreview from './SectionPreview';
import { usePreviewSize } from './usePreviewSize';
import type { MergePreviewConfig } from './SectionPreview';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import { isEditingBlockedNow } from '../../hooks/useEditingBlocked';
import type { OrphanPlacementSuggestion } from '../../types/commands';
import type { ImportReceipt } from '../../types/ipc/ImportReceipt';
import type { Section } from '../../types/project';
import type { DmpSection } from '../../types/ipc/DmpSection';
import { trackDialogComplete } from '../../utils/telemetry';
import { presentImportReceipt } from '../../utils/saveFeedback';
import { formatShotId } from '../../utils/formatters';
import { computeOverlapPairs, type MergeOption } from '../../utils/overlapHelpers';
import { repeatedShots, type OverlapChoice } from '../../utils/overlapChoices';
import { isRepeatDoublyAnchored, mergeOptionsOf, overlapParamsOf, placeSuggestion } from '../../utils/orphanPlacement';
import OverlapShotTable, { type OverlapShotRow } from './OverlapShotTable';
import { sectionFirstDate } from '../../utils/sectionCredit';

interface OrphanPlacementDialogProps {
  suggestions: OrphanPlacementSuggestion[];
  /** Import receipt (#391) to surface once orphan placement finishes, if any. */
  receipt?: ImportReceipt | undefined;
}

/** Group key for an orphan end: sectionId + stationIndex. */
interface OrphanEnd {
  key: string;
  orphanSectionId: number;
  orphanStationIndex: number;
  suggestions: OrphanPlacementSuggestion[];
}

const confidenceColors: Record<string, string> = {
  high: 'bg-success-10 text-success border-success-30',
  medium: 'bg-warning-10 text-warning border-warning-30',
  low: 'bg-background-50 text-text-secondary border-border',
};

const directionLabel = (direction: string) =>
  direction === 'forward' ? '\u2192' : '\u2190';

/**
 * A suggestion's score as a whole percent, capped at 100. The matcher boosts a
 * station with matching survey on both sides by up to 1.18x (placement_suggest.rs
 * `station_context_multiplier`), which ranks it but is not a percentage. Capped
 * for display only, so ranking and confidence are untouched (#681).
 */
const scorePercent = (score: number): number => Math.round(Math.min(score, 1) * 100);

/** Convert a project Section to DmpSection format for SectionPreview. */
function sectionToDmpSection(section: Section): DmpSection {
  return {
    version: 1,
    name: section.name ?? '',
    direction: section.direction,
    date: sectionFirstDate(section),
    content_hash: '',
    shots: section.shots.map((s) => ({
      shot_type: 'Standard' as const,
      heading_in: s.azimuth_in,
      heading_out: s.azimuth_out,
      length: s.length,
      depth_in: s.depth_in,
      depth_out: s.depth_out,
      pitch_in: s.pitch_in ?? 0,
      pitch_out: s.pitch_out ?? 0,
      left: s.left ?? 0,
      right: s.right ?? 0,
      up: s.up ?? 0,
      down: s.down ?? 0,
      temperature: s.temperature ?? 0,
      hour: 0, minute: 0, second: 0,
      marker_index: s.marker_index ?? 0,
      ...(s.comment != null ? { comment: s.comment } : {}),
    })),
  };
}

export default function OrphanPlacementDialog({ suggestions, receipt }: OrphanPlacementDialogProps) {
  const { t } = useTranslation();
  const confidenceLabel = useCallback(
    (c: string): string => {
      switch (c) {
        case 'high': return t('dialogs.orphanPlacement.confidence.high');
        case 'medium': return t('dialogs.orphanPlacement.confidence.medium');
        case 'low': return t('dialogs.orphanPlacement.confidence.low');
        default: return c;
      }
    },
    [t],
  );
  const { mergeStations, updateShot, setShotKind, refreshProjectState, project } = useProjectStore();
  const { closeDialog, setView, triggerFitToBounds, openDialog, showToast } = useUIStore();

  const [processing, setProcessing] = useState(false);
  const [currentEndIndex, setCurrentEndIndex] = useState(0);
  const [acceptedOrphans, setAcceptedOrphans] = useState<Set<number>>(new Set());
  const [skippedCount, setSkippedCount] = useState(0);
  const [showAlternatives, setShowAlternatives] = useState(false);
  // Track which suggestion index within the current group is selected
  const [selectedSuggestionIndex, setSelectedSuggestionIndex] = useState(0);
  // Merge station pair, and the per-shot choice for repeated shots (#681),
  // keyed by orphan shot index. A row with no entry is Keep existing.
  const [selectedMergeOffset, setSelectedMergeOffset] = useState(0);
  const [overlapChoices, setOverlapChoices] = useState<ReadonlyMap<number, OverlapChoice>>(new Map());
  // Why the last Accept / Accept All stopped: a merge the backend refused.
  const [placeError, setPlaceError] = useState<string | null>(null);
  const [previewRef, previewSize] = usePreviewSize(340, 200);
  // Set while finish() runs or once it closed the dialog, so its several callers
  // (the auto-close effect, Skip on the last section, Accept All, Skip All, the
  // X) save, toast and report completion once.
  const finishingRef = useRef(false);
  // Set while Accept / Accept All place: a second click in the same task lands
  // before the re-render that disables the buttons.
  const placingRef = useRef(false);

  // Group suggestions by orphan end, sorted by score descending within each group
  const allOrphanEnds = useMemo((): OrphanEnd[] => {
    const map = new Map<string, OrphanPlacementSuggestion[]>();
    for (const s of suggestions) {
      const key = `${s.orphanSectionId}-${s.orphanStationIndex}`;
      const list = map.get(key) ?? [];
      list.push(s);
      map.set(key, list);
    }
    const ends: OrphanEnd[] = [];
    for (const [key, group] of map) {
      group.sort((a, b) => b.score - a.score);
      ends.push({
        key,
        orphanSectionId: group[0]!.orphanSectionId,
        orphanStationIndex: group[0]!.orphanStationIndex,
        suggestions: group,
      });
    }
    return ends;
  }, [suggestions]);

  // Filter out ends whose orphan section has already been accepted
  const remainingEnds = useMemo(
    () => allOrphanEnds.filter((e) => !acceptedOrphans.has(e.orphanSectionId)),
    [allOrphanEnds, acceptedOrphans],
  );

  const currentEnd = remainingEnds[currentEndIndex] ?? null;
  const currentSuggestion = currentEnd?.suggestions[selectedSuggestionIndex] ?? currentEnd?.suggestions[0] ?? null;
  const alternatives = useMemo(() => {
    if (!currentEnd) return [];
    return currentEnd.suggestions.filter((_, i) => i !== selectedSuggestionIndex);
  }, [currentEnd, selectedSuggestionIndex]);

  const isFinished = currentEnd == null;

  // Look up sections from project
  const orphanSection = useMemo(
    () => project?.sections.find((s) => s.id === currentSuggestion?.orphanSectionId) ?? null,
    [project, currentSuggestion],
  );
  const targetSection = useMemo(
    () => project?.sections.find((s) => s.id === currentSuggestion?.targetSectionId) ?? null,
    [project, currentSuggestion],
  );

  // Compute merge options for the current suggestion
  const importShotCount = useMemo(() => {
    if (!orphanSection) return 0;
    return orphanSection.shots.length;
  }, [orphanSection]);

  const mergeOptions = useMemo(
    () => (currentSuggestion ? mergeOptionsOf(currentSuggestion, importShotCount) : []),
    [currentSuggestion, importShotCount],
  );

  const selectedMergePair = useMemo(() => {
    if (mergeOptions.length === 0) return null;
    return mergeOptions.find((o) => o.offset === selectedMergeOffset) ?? mergeOptions[mergeOptions.length - 1]!;
  }, [mergeOptions, selectedMergeOffset]);

  // Default the merge to the last option: the END of the repeat, the last
  // station both surveys share (#681). The matcher's `preferredMergeOffset` is
  // deliberately not used here - it favours a station with matching survey on
  // both sides, i.e. the middle of any repeat of two shots or more. Every choice
  // also resets to Keep existing for the new suggestion.
  useEffect(() => {
    if (mergeOptions.length > 0) {
      setSelectedMergeOffset(mergeOptions[mergeOptions.length - 1]!.offset);
    }
    setOverlapChoices(new Map());
  }, [currentSuggestion, mergeOptions]);

  // A target anchored at both ends of the repeat allows Keep existing only.
  const isTargetDoublyAnchored = useMemo(
    () => (currentSuggestion && project ? isRepeatDoublyAnchored(currentSuggestion, project) : false),
    [currentSuggestion, project],
  );

  const mergeImportStation = selectedMergePair?.importStation ?? currentSuggestion?.orphanStationIndex ?? 0;

  // The repeated shots up to the "Merge at" station, which sets how long the
  // repeat is, existing reading beside the new one oriented the same way.
  const overlapRows = useMemo((): OverlapShotRow[] => {
    if (!currentSuggestion || !orphanSection || !targetSection) return [];
    const params = overlapParamsOf(currentSuggestion);
    return repeatedShots(params, mergeImportStation, orphanSection.shots, targetSection.shots).map((r) => ({
      orphanShotIndex: r.orphanShotIndex,
      existingLabel: formatShotId(targetSection.id, r.targetShotIndex + 1),
      newLabel: formatShotId(orphanSection.id, r.orphanShotIndex + 1),
      existing: r.targetShot,
      incoming: r.incoming,
    }));
  }, [currentSuggestion, orphanSection, targetSection, mergeImportStation]);

  // The choices that will be applied: a doubly-anchored target forces Keep
  // existing on every row, as the single overlap mode did before.
  const effectiveChoices = useMemo((): ReadonlyMap<number, OverlapChoice> => {
    const map = new Map<number, OverlapChoice>();
    for (const row of overlapRows) {
      const chosen = overlapChoices.get(row.orphanShotIndex) ?? 'keep_existing';
      map.set(row.orphanShotIndex, isTargetDoublyAnchored ? 'keep_existing' : chosen);
    }
    return map;
  }, [overlapRows, overlapChoices, isTargetDoublyAnchored]);

  const handleChoiceChange = useCallback((orphanShotIndex: number, choice: OverlapChoice) => {
    setOverlapChoices((prev) => new Map(prev).set(orphanShotIndex, choice));
  }, []);
  const handleSetAll = useCallback((choice: OverlapChoice) => {
    setOverlapChoices(new Map(overlapRows.map((row) => [row.orphanShotIndex, choice])));
  }, [overlapRows]);
  // A new merge station re-derives the rows. A row still listed keeps its
  // choice; a row that leaves the table forgets it, and returns as Keep existing.
  const handleMergeChange = useCallback((offset: number) => {
    setSelectedMergeOffset(offset);
    const option = mergeOptions.find((o) => o.offset === offset);
    if (!currentSuggestion || !option) return;
    const listed = new Set(
      computeOverlapPairs(overlapParamsOf(currentSuggestion), option.importStation).map((p) => p.orphanShotIndex),
    );
    setOverlapChoices((prev) => new Map([...prev].filter(([index]) => listed.has(index))));
  }, [currentSuggestion, mergeOptions]);

  // Convert orphan section to DmpSection for SectionPreview
  const orphanDmpSection = useMemo(
    () => (orphanSection ? sectionToDmpSection(orphanSection) : null),
    [orphanSection],
  );

  // Build merge preview config using selected merge pair
  const mergePreviewConfig = useMemo((): MergePreviewConfig | null => {
    if (!targetSection || !currentSuggestion) return null;
    return {
      targetSection,
      importStationIndex: selectedMergePair?.importStation ?? currentSuggestion.orphanStationIndex,
      targetStationIndex: selectedMergePair?.existingStation ?? currentSuggestion.targetStationIndex,
    };
  }, [targetSection, currentSuggestion, selectedMergePair]);

  const finish = useCallback(async () => {
    if (finishingRef.current) return;
    finishingRef.current = true;
    await refreshProjectState();
    // Persist after orphan placement (#92): import + orphan-merge changes are
    // otherwise held in memory only. On a failed save an error/permission dialog
    // is shown; leave it visible rather than dismissing it via closeDialog, and
    // let the X or Skip All try again.
    if (!(await useProjectStore.getState().saveAfterImport())) {
      finishingRef.current = false;
      return;
    }
    // The import flow ends here on this branch (it never reaches
    // completeImportToSingleView, which drops the context on the other one), and
    // a context left set outlives its flow: the next import whose route sets
    // none inherits 'new-project' and takes saveAfterImport's full-save branch,
    // committing the user's unrelated pending edits. Cleared AFTER the save
    // above, which reads it. (#435 tracks the duplicated exit sequence itself.)
    useUIStore.getState().setImportContext(null);
    trackDialogComplete('orphan-placement');
    closeDialog();
    setView('single');
    triggerFitToBounds();
    // Deliberately no "N sections unanchored" info line here, even if the
    // user skipped/declined every suggestion and sections remain unanchored:
    // this dialog existing at all means suggestions WERE offered and the
    // user made an explicit choice about them - unlike
    // `finishImportWithoutOrphanSuggestions` (saveFeedback.ts), whose info
    // line exists specifically for the case where the suggestion pass had
    // nothing to offer and the silence itself needed a receipt line (#446).
    if (receipt) {
      presentImportReceipt(receipt, { openDialog, showToast });
    }
  }, [refreshProjectState, closeDialog, setView, triggerFitToBounds, receipt, openDialog, showToast]);

  // Auto-close when all suggestions have been processed. This is the one owner
  // of finishing after Accept: accepting the last section empties
  // `remainingEnds`, which lands here.
  useEffect(() => {
    if (isFinished) {
      finish();
    }
  }, [isFinished, finish]);

  const advance = useCallback(() => {
    setShowAlternatives(false);
    setSelectedSuggestionIndex(0);
    // If we're at the last remaining end, finish
    if (currentEndIndex >= remainingEnds.length - 1) {
      finish();
    } else {
      setCurrentEndIndex((i) => i + 1);
    }
  }, [currentEndIndex, remainingEnds.length, finish]);

  /** Place one suggestion; the message to show if its merge was refused, else null. */
  const place = useCallback(async (
    suggestion: OrphanPlacementSuggestion,
    mergePair: MergeOption | null,
    choices: ReadonlyMap<number, OverlapChoice>,
  ): Promise<string | null> => {
    const merged = await placeSuggestion(suggestion, mergePair, choices, {
      mergeStations, setShotKind, updateShot, project: () => useProjectStore.getState().project,
    });
    if (merged.success) return null;
    return t('dialogs.orphanPlacement.mergeFailed', {
      name: suggestion.orphanSectionName,
      error: merged.error ?? t('editGate.commandFailed'),
    });
  }, [mergeStations, setShotKind, updateShot, t]);

  // Read-only self-check at Accept time: the store actions placing runs carry
  // no gate of their own, and the mode can change while the dialog is up.
  const editingAllowed = useCallback((): boolean => {
    if (!isEditingBlockedNow()) return true;
    setPlaceError(t('dialogs.orphanPlacement.readOnly'));
    return false;
  }, [t]);

  const handleAccept = useCallback(async () => {
    if (!currentSuggestion || placingRef.current || !editingAllowed()) return;
    placingRef.current = true;
    setProcessing(true);
    setPlaceError(null);
    try {
      const error = await place(currentSuggestion, selectedMergePair, effectiveChoices);
      if (error) {
        setPlaceError(error);
        return;
      }
      setAcceptedOrphans((prev) => new Set(prev).add(currentSuggestion.orphanSectionId));
      setShowAlternatives(false);
      setSelectedSuggestionIndex(0);
    } finally {
      placingRef.current = false;
      setProcessing(false);
    }
  }, [currentSuggestion, selectedMergePair, place, effectiveChoices, editingAllowed]);

  const handleSkip = useCallback(() => {
    setPlaceError(null);
    setSkippedCount((c) => c + 1);
    advance();
  }, [advance]);

  const handleAcceptAll = useCallback(async () => {
    if (placingRef.current || !editingAllowed()) return;
    placingRef.current = true;
    setProcessing(true);
    setPlaceError(null);
    try {
      const placed = new Set<number>();
      for (let i = currentEndIndex; i < remainingEnds.length; i++) {
        const end = remainingEnds[i]!;
        if (placed.has(end.orphanSectionId)) continue;
        let error: string | null;
        if (i === currentEndIndex && currentSuggestion) {
          // The section on screen: exactly what the user set up on it.
          error = await place(currentSuggestion, selectedMergePair, effectiveChoices);
        } else {
          // Every other section: its best suggestion, the default merge pair
          // (end of the repeat) and Keep existing on every repeated shot.
          const best = end.suggestions[0];
          if (!best) continue;
          const shotCount = project?.sections.find((s) => s.id === best.orphanSectionId)?.shots.length ?? 0;
          const opts = mergeOptionsOf(best, shotCount);
          error = await place(best, opts[opts.length - 1] ?? null, new Map());
        }
        if (error) {
          // Stop on the section whose merge was refused, on screen with the
          // error; the sections joined before it count as placed.
          setPlaceError(error);
          if (placed.size > 0) {
            setAcceptedOrphans((prev) => new Set([...prev, ...placed]));
            setCurrentEndIndex(remainingEnds.filter((e) => !placed.has(e.orphanSectionId)).indexOf(end));
            setShowAlternatives(false);
            setSelectedSuggestionIndex(0);
          }
          return;
        }
        placed.add(end.orphanSectionId);
      }
      await finish();
    } finally {
      placingRef.current = false;
      setProcessing(false);
    }
  }, [currentEndIndex, remainingEnds, currentSuggestion, selectedMergePair, effectiveChoices, place, project, finish, editingAllowed]);

  const handleAlternativeClick = useCallback((altIndex: number) => {
    if (!currentEnd) return;
    let realIndex = 0;
    let altCount = 0;
    for (let i = 0; i < currentEnd.suggestions.length; i++) {
      if (i === selectedSuggestionIndex) continue;
      if (altCount === altIndex) {
        realIndex = i;
        break;
      }
      altCount++;
    }
    setSelectedSuggestionIndex(realIndex);
  }, [currentEnd, selectedSuggestionIndex]);

  // Progress: how many we've processed (accepted + skipped) out of total original ends
  const processedCount = currentEndIndex + (allOrphanEnds.length - remainingEnds.length);
  const totalOriginal = allOrphanEnds.length;

  // Display stations from selected merge pair
  const displayOrphanStation = selectedMergePair?.importStation ?? currentSuggestion?.orphanStationIndex ?? 0;
  const displayTargetStation = selectedMergePair?.existingStation ?? currentSuggestion?.targetStationIndex ?? 0;

  return (
    <DialogWrapper
      title={t('dialogs.orphanPlacement.title')}
      onClose={finish}
      fixedSize={{ width: 'min(900px, 95vw)', height: 'min(650px, 90vh)' }}
      overflow="hidden"
      bodyClassName="flex-1 flex flex-col p-0 min-h-0"
      data-testid="orphan-placement-dialog"
    >
      {/* Progress header */}
      <div className="px-4 py-2 border-b border-border text-sm text-text-secondary">
        <span data-testid="wizard-progress">
          {t('dialogs.orphanPlacement.progress', {
            current: Math.min(processedCount + 1, totalOriginal),
            total: totalOriginal,
          })}
        </span>
        {skippedCount > 0 && (
          <span className="ml-2" data-testid="wizard-skipped">
            &middot; {t('dialogs.orphanPlacement.skipped', { count: skippedCount })}
          </span>
        )}
      </div>

      {isFinished ? null : (
        /* Side-by-side layout */
        <div className="flex-1 flex min-h-0 overflow-hidden relative">
          {/* Left column: SectionPreview. 40% of the dialog, not a fixed 380 px,
              so the details column keeps room for the repeat table at the
              800 px window minimum (#681). */}
          <div
            ref={previewRef}
            className="shrink-0 w-[40%] flex flex-col items-center justify-center p-4 border-r border-border"
          >
            {orphanDmpSection && (
              <SectionPreview
                section={orphanDmpSection}
                color={orphanSection?.color ?? '#4a9eff'}
                size={previewSize.width}
                height={previewSize.height}
                mergePreview={mergePreviewConfig}
                focusStationIndex={currentSuggestion?.orphanStationIndex ?? null}
              />
            )}
            {!orphanDmpSection && (
              <div
                className="border border-border rounded flex items-center justify-center text-text-secondary text-sm"
                style={{ width: previewSize.width, height: previewSize.height }}
              >
                {t('dialogs.orphanPlacement.sectionNotFound')}
              </div>
            )}
          </div>

          {/* Right column: connection details */}
          <div className="flex-1 min-w-0 overflow-y-auto p-5 space-y-4" data-testid="wizard-suggestion">
            {/* Connection heading */}
            <div>
              <h3 className="text-sm font-semibold text-text-secondary uppercase tracking-wide">
                {t('dialogs.orphanPlacement.suggestedConnection')}
              </h3>
            </div>

            {/* Connection description */}
            <div className="p-3 rounded border border-border bg-surface-alt space-y-2">
              <div className="text-sm">
                <span className="font-medium">{currentSuggestion!.orphanSectionName}</span>
                <span className="text-text-tertiary"> {t('dialogs.orphanPlacement.station', { index: displayOrphanStation })}</span>
              </div>
              <div className="text-xs text-text-secondary">
                &rarr; <span className="font-medium text-text-primary">{currentSuggestion!.targetSectionName}</span>
                <span className="text-text-tertiary"> {t('dialogs.orphanPlacement.station', { index: displayTargetStation })}</span>
              </div>
            </div>

            {/* Confidence badge + matched shots */}
            <div className="flex items-center gap-3 flex-wrap">
              <span
                className={`px-2 py-1 rounded border text-xs font-medium ${confidenceColors[currentSuggestion!.confidence] ?? ''}`}
                data-testid="confidence-badge"
              >
                {confidenceLabel(currentSuggestion!.confidence)}
              </span>
              <span className="text-xs text-text-secondary" data-testid="orphan-matched-shots">
                {/* The matcher's count, and how much of it this merge station
                    resolves when that is not all of it. */}
                {overlapRows.length < currentSuggestion!.matchedShots
                  ? t('dialogs.orphanPlacement.matchedShotsResolved', {
                    count: currentSuggestion!.matchedShots,
                    resolved: overlapRows.length,
                    direction: directionLabel(currentSuggestion!.direction),
                  })
                  : t('dialogs.orphanPlacement.matchedShots', {
                    count: currentSuggestion!.matchedShots,
                    direction: directionLabel(currentSuggestion!.direction),
                  })}
              </span>
              <span className="text-xs text-text-tertiary">
                {t('dialogs.orphanPlacement.score', {
                  percent: scorePercent(currentSuggestion!.score),
                })}
              </span>
            </div>

            {/* Merge station picker */}
            {mergeOptions.length > 1 && (
              <div className="flex items-center gap-2" data-testid="merge-station-picker">
                <label className="text-xs text-text-secondary whitespace-nowrap">
                  {t('dialogs.orphanPlacement.mergeAt')}
                </label>
                <Select
                  value={String(selectedMergeOffset)}
                  onChange={(v) => handleMergeChange(Number(v))}
                  options={mergeOptions.map((opt) => ({
                    value: String(opt.offset),
                    label: t('dialogs.orphanPlacement.mergeOption', {
                      importStation: opt.importStation,
                      targetName: currentSuggestion!.targetSectionName,
                      existingStation: opt.existingStation,
                    }),
                  }))}
                  size="sm"
                  data-testid="merge-station-select"
                />
              </div>
            )}

            {/* A merge at the start of the repeat resolves none of it (#681). */}
            {overlapRows.length === 0 && currentSuggestion!.matchedShots > 0 && (
              <p className="text-xs text-warning-text" data-testid="overlap-unresolved-note">
                {t('dialogs.orphanPlacement.repeat.unresolved', { count: currentSuggestion!.matchedShots })}
              </p>
            )}

            {/* Per-shot decisions for repeated survey (#681) */}
            {overlapRows.length > 0 && (
              <OverlapShotTable
                rows={overlapRows}
                choices={effectiveChoices}
                onChoiceChange={handleChoiceChange}
                onSetAll={handleSetAll}
                replaceDisabled={isTargetDoublyAnchored}
              />
            )}

            {/* Prompt */}
            <p className="text-sm text-text-secondary">
              {t('dialogs.orphanPlacement.prompt')}
            </p>

            {placeError && (
              <p role="alert" className="text-sm text-error" data-testid="orphan-place-error">{placeError}</p>
            )}

            {/* Accept / Skip buttons */}
            <div className="flex gap-2">
              <Button
                data-testid="orphan-accept"
                onClick={handleAccept}
                disabled={processing}
                data-primary-action=""
              >
                {t('dialogs.orphanPlacement.accept')}
              </Button>
              <Button
                data-testid="orphan-skip"
                onClick={handleSkip}
                disabled={processing}
              >
                {t('dialogs.orphanPlacement.skip')}
              </Button>
            </div>

            {/* Alternatives */}
            {alternatives.length > 0 && (
              <div>
                <button
                  type="button"
                  onClick={() => setShowAlternatives(!showAlternatives)}
                  className="text-xs text-accent hover:text-accent-hover flex items-center gap-1"
                  data-testid="toggle-alternatives"
                >
                  <span className={`transition-transform ${showAlternatives ? 'rotate-90' : ''}`}>&#9654;</span>
                  {showAlternatives
                    ? t('dialogs.orphanPlacement.hideAlternatives')
                    : t('dialogs.orphanPlacement.showAlternatives', { count: alternatives.length })}
                </button>

                {showAlternatives && (
                  <div className="mt-2 space-y-1" data-testid="alternatives-list">
                    {alternatives.map((alt, idx) => (
                      <button
                        key={`${alt.targetSectionId}-${alt.targetStationIndex}-${alt.direction}`}
                        type="button"
                        className={`w-full p-2 rounded border text-left text-xs flex items-center gap-2 hover:bg-accent-5 transition-colors ${confidenceColors[alt.confidence] ?? ''}`}
                        onClick={() => handleAlternativeClick(idx)}
                        data-testid={`alternative-${idx}`}
                      >
                        <span className={`px-1 py-0.5 rounded text-[10px] font-medium ${confidenceColors[alt.confidence] ?? ''}`}>
                          {confidenceLabel(alt.confidence)}
                        </span>
                        <span className="truncate">
                          {t('dialogs.orphanPlacement.alternativeTarget', {
                            name: alt.targetSectionName,
                            index: alt.targetStationIndex,
                          })}
                        </span>
                        <span className="text-text-tertiary ml-auto shrink-0">
                          {t('dialogs.orphanPlacement.alternativeSummary', {
                            count: alt.matchedShots,
                            direction: directionLabel(alt.direction),
                            percent: scorePercent(alt.score),
                          })}
                        </span>
                      </button>
                    ))}
                  </div>
                )}
              </div>
            )}
          </div>
        </div>
      )}

      {/* Footer */}
      {!isFinished && (
        <div className="flex justify-end gap-2 px-4 py-3 border-t border-border shrink-0">
          <Button
            data-testid="orphan-accept-all"
            onClick={handleAcceptAll}
            disabled={processing}
          >
            {t('dialogs.orphanPlacement.acceptAll')}
          </Button>
          <Button
            data-testid="orphan-close"
            onClick={finish}
            disabled={processing}
          >
            {t('dialogs.orphanPlacement.skipAll')}
          </Button>
        </div>
      )}
    </DialogWrapper>
  );
}
