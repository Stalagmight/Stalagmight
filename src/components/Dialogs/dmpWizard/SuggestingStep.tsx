/**
 * The DMP wizard's suggestion step (split out of DmpNewImportDialog with #576
 * D4): one suggested connection on screen - its preview with the context
 * around it, the connection card (merge station, overlap mode), the
 * alternatives, the section settings, and the shot table below.
 *
 * Presentational: the suggestion, the merge offset and every action belong to
 * the dialog (the `suggesting` phase of dmpWizard/wizardState.ts). The step
 * owns what only it draws: the preview configs and context, and whether the
 * alternatives list is open.
 */
import { useMemo, useRef, useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { Button } from '../../common/Button';
import { Select } from '../../common/Select';
import type { CrossSectionSuggestion, DmpFile, DmpSection } from '../../../types/commands';
import type { OverlapMode } from '../../../types/config';
import type { MergeOption } from '../../../utils/overlapHelpers';
import type { VirtualConnection } from '../../../utils/segmentContext';
import type { SectionEditState } from '../../../utils/batchImportHelpers';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { azimuthReferenceFor } from '../../../utils/stationWalk';
import { editableShotsToDmpSection } from '../../../utils/shotConversion';
import { WizardMapPanel } from '../WizardMapPanel';
// The preview cap comes straight from layoutConstants (see DmpNewImportDialog):
// two component tests mock WizardMapPanel, and a mocked module cannot supply
// the plain number the render arithmetic needs.
import { WIZARD_MAP_PANEL_MAX_WIDTH as MAP_PANEL_MAX_WIDTH } from '../../SingleSystemView/layoutConstants';
import { WizardScrollBody } from '../WizardScrollBody';
import SectionPreview, {
  type MergePreviewConfig,
  type DmpMergePreviewConfig,
  type ContextSectionGeometry,
  computeStationsFromDmpShots,
  computeStationsFromShots,
} from '../SectionPreview';
import { SuggestionSectionSettings } from '../SuggestionSectionSettings';
import { usePreviewSize } from '../usePreviewSize';
import { useSuggestionContext } from './useSuggestionContext';
import { WIZARD_SECONDARY_BUTTON_CLASS as importSecondaryButtonClass } from './WizardFooter';

/** The confidence badge's colors, by tier. */
const confidenceColors: Record<string, string> = {
  high: 'bg-success-10 text-success border-success-30',
  medium: 'bg-warning-10 text-warning border-warning-30',
  low: 'bg-background-50 text-text-secondary border-border',
};

const directionLabel = (direction: string) =>
  direction === 'forward' ? '\u2192' : '\u2190';

export interface SuggestingStepProps {
  currentSuggestion: CrossSectionSuggestion;
  /** Other suggestions for the same section, best first. */
  alternatives: CrossSectionSuggestion[];
  onAlternativeClick: (alternative: CrossSectionSuggestion) => void;
  mergeOptions: MergeOption[];
  selectedMergeOffset: number;
  selectedMergePair: MergeOption | null;
  onMergeOffsetChange: (offset: number) => void;
  overlapMode: OverlapMode;
  onOverlapModeChange: (mode: OverlapMode) => void;
  /** Interpolate / import data would break a constraint on the target. */
  isTargetDoublyAnchored: boolean;
  /** The run's failure, shown under the connection card. */
  error: string | null;
  editState: SectionEditState | null;
  onMetadataChange: (field: keyof SectionEditState, value: string | number | boolean) => void;
  /** The shot table of the section on screen. */
  table: ReactNode;
  connections: VirtualConnection[];
  dmpFile: DmpFile;
  selectedIndices: number[];
  sectionPalette: string[];
}

export function SuggestingStep({
  currentSuggestion,
  alternatives,
  onAlternativeClick,
  mergeOptions,
  selectedMergeOffset,
  selectedMergePair,
  onMergeOffsetChange,
  overlapMode,
  onOverlapModeChange,
  isTargetDoublyAnchored,
  error,
  editState,
  onMetadataChange,
  table,
  connections,
  dmpFile,
  selectedIndices,
  sectionPalette,
}: SuggestingStepProps) {
  const { t } = useTranslation();
  // The map column's width, shared with the manual-placing step and kept by the
  // store (WizardMapPanel persists it on divider release, #404).
  const mapPanelWidth = useUIStore((s) => s.wizardMapPanelWidth);
  const setMapPanelWidth = useUIStore((s) => s.setWizardMapPanelWidth);
  const suggestionBodyRef = useRef<HTMLDivElement | null>(null);
  const [suggestionPreviewRef, suggestionPreviewSize] = usePreviewSize(MAP_PANEL_MAX_WIDTH);
  const [showAlternatives, setShowAlternatives] = useState(true);
  const segmentContext = useSuggestionContext(currentSuggestion, connections, dmpFile);

  // Build preview section for import side from original shots (no mode modifications).
  // Both import and target sections display their original geometry so the user can
  // visually compare the overlap regardless of the selected overlap mode.
  const importPreviewSection = useMemo<DmpSection | null>(() => {
    if (!currentSuggestion) return null;
    const section = dmpFile.sections[currentSuggestion.importSectionIndex];
    if (!section) return null;
    if (!editState) return section;

    return {
      ...editableShotsToDmpSection(section, editState.shots),
      name: editState.name,
      direction: editState.direction,
    };
  }, [currentSuggestion, dmpFile.sections, editState]);

  // Build merge preview config for SectionPreview (existing target)
  const mergePreviewConfig = useMemo((): MergePreviewConfig | null => {
    if (!currentSuggestion || currentSuggestion.targetType !== 'existing') return null;
    if (currentSuggestion.existingSectionId == null) return null;

    const currentProject = useProjectStore.getState().project;
    if (!currentProject) return null;

    const targetSection = currentProject.sections.find(
      (s) => s.id === currentSuggestion.existingSectionId
    );
    if (!targetSection) return null;

    return {
      targetSection,
      importStationIndex: selectedMergePair?.importStation ?? currentSuggestion.importStationIndex,
      targetStationIndex: selectedMergePair?.existingStation ?? currentSuggestion.existingStationIndex,
      // The reference the backend walks the target with (#576 D15); the context
      // offset below walks it with the same one.
      targetAzimuthReference: azimuthReferenceFor(targetSection.azimuth_reference, currentProject.default_azimuth_reference),
    };
  }, [currentSuggestion, selectedMergePair]);

  // Build DMP merge preview config for import-to-import targets
  const dmpMergePreviewConfig = useMemo((): DmpMergePreviewConfig | null => {
    if (!currentSuggestion || currentSuggestion.targetType !== 'importing') return null;
    if (currentSuggestion.targetImportIndex == null) return null;

    const targetSection = dmpFile.sections[currentSuggestion.targetImportIndex];
    if (!targetSection) return null;

    const targetColorIdx = (currentSuggestion.targetImportIndex ?? 0) % sectionPalette.length;

    return {
      targetDmpSection: targetSection,
      targetColor: sectionPalette[targetColorIdx] ?? '#95a5a6',
      importStationIndex: selectedMergePair?.importStation ?? currentSuggestion.importStationIndex,
      targetStationIndex: selectedMergePair?.existingStation ?? currentSuggestion.existingStationIndex,
    };
  }, [currentSuggestion, selectedMergePair, dmpFile.sections, sectionPalette]);

  // Merge target-side + import-side context for SectionPreview.
  // Import-side context is relative to the import section's station 0 and needs the same
  // merge offset that SectionPreview applies to align the import section with the target.
  const mergedContextSections = useMemo((): ContextSectionGeometry[] | null => {
    const parts: ContextSectionGeometry[] = [];
    if (segmentContext?.targetSide) parts.push(...segmentContext.targetSide);
    if (segmentContext?.importSide && segmentContext.importSide.length > 0) {
      const activePreview = mergePreviewConfig ?? dmpMergePreviewConfig;
      let dx = 0;
      let dy = 0;
      if (activePreview && importPreviewSection) {
        // Compute the same importOffset that SectionPreview uses internally
        const importStations = computeStationsFromDmpShots(
          importPreviewSection.shots.filter((s) => s.shot_type === 'Standard')
        );
        let targetStations: Array<{ x: number; y: number }> = [];
        if (mergePreviewConfig) {
          targetStations = computeStationsFromShots(
            mergePreviewConfig.targetSection.shots, mergePreviewConfig.targetAzimuthReference
          );
        } else if (dmpMergePreviewConfig) {
          targetStations = computeStationsFromDmpShots(
            dmpMergePreviewConfig.targetDmpSection.shots.filter((s) => s.shot_type === 'Standard')
          );
        }
        const importStn = importStations[activePreview.importStationIndex];
        const targetStn = targetStations[activePreview.targetStationIndex];
        if (importStn && targetStn) {
          dx = targetStn.x - importStn.x;
          dy = targetStn.y - importStn.y;
        }
      }
      parts.push(...segmentContext.importSide.map((ctx) => ({
        color: ctx.color,
        shots: ctx.shots.map((shot) => ({
          from: [shot.from[0] + dx, shot.from[1] + dy] as [number, number],
          to: [shot.to[0] + dx, shot.to[1] + dy] as [number, number],
        })),
      })));
    }
    return parts.length > 0 ? parts : null;
  }, [segmentContext, mergePreviewConfig, dmpMergePreviewConfig, importPreviewSection]);

  // Import color for the current section (from editState, which tracks user changes)
  const importColor = useMemo(() => {
    if (editState?.color) return editState.color;
    const sectionIdx = currentSuggestion?.importSectionIndex ?? -1;
    const pos = selectedIndices.indexOf(sectionIdx);
    return sectionPalette[(pos >= 0 ? pos : 0) % sectionPalette.length] ?? '#4ecdc4';
  }, [editState?.color, currentSuggestion?.importSectionIndex, selectedIndices, sectionPalette]);

  // Target section name for display
  const targetSectionName = useMemo(() => {
    if (!currentSuggestion) return '';
    return currentSuggestion.existingSectionName;
  }, [currentSuggestion]);

  // Target type label for import-to-import connections
  const isImportTarget = currentSuggestion?.targetType === 'importing';

  return (
    <WizardScrollBody
      bodyRef={suggestionBodyRef}
      table={table}
    >
      {(bandHeight) => (
      <div className="flex" style={{ minHeight: bandHeight }}>
      <WizardMapPanel
        width={mapPanelWidth}
        onWidthChange={setMapPanelWidth}
        bodyRef={suggestionBodyRef} panelRef={suggestionPreviewRef} height={bandHeight}
      >
        <SectionPreview
          section={importPreviewSection}
          color={importColor}
          size={suggestionPreviewSize.width}
          height={suggestionPreviewSize.height}
          mergePreview={mergePreviewConfig}
          dmpMergePreview={dmpMergePreviewConfig}
          focusStationIndex={selectedMergePair?.importStation ?? currentSuggestion.importStationIndex}
          contextSections={mergedContextSections}
        />
      </WizardMapPanel>

      {/* Right column: connection details */}
      <div className="flex-1 min-w-0 p-5 space-y-4" data-testid="wizard-suggestion">
        {/* Suggested connection - prominent card */}
        <div className="rounded-lg border border-accent/30 bg-accent/5 p-4 space-y-3">
          <div>
            <h3 className="text-sm font-semibold text-text-primary">
              {t('dialogs.dmpNewImport.connectionHeading')}
            </h3>
            <p className="text-xs text-text-secondary mt-0.5">
              {t('dialogs.dmpNewImport.connectionDescription')}
            </p>
          </div>

          {/* Connection description */}
          <div className="rounded border border-border bg-background p-3" data-testid="wizard-connection-description">
            <div className="text-sm">
              <span className="font-medium">&ldquo;{currentSuggestion.importSectionName}&rdquo;</span>
              {' '}{t('dialogs.dmpNewImport.station', { index: selectedMergePair?.importStation ?? currentSuggestion.importStationIndex })}
            </div>
            <div className="text-text-secondary text-sm my-1">
              {t('dialogs.dmpNewImport.connectsTo')}
            </div>
            <div className="text-sm">
              <span className="font-medium">&ldquo;{targetSectionName}&rdquo;</span>
              {' '}{t('dialogs.dmpNewImport.station', { index: selectedMergePair?.existingStation ?? currentSuggestion.existingStationIndex })}
              {isImportTarget && (
                <span className="ml-1 text-xs text-text-secondary">{t('dialogs.dmpNewImport.importingTag')}</span>
              )}
            </div>
          </div>

          {/* Confidence badge */}
          <div className="flex items-center gap-2" data-testid="wizard-confidence">
            <span className={`px-2 py-1 rounded border text-xs font-medium ${confidenceColors[currentSuggestion.confidence] ?? ''}`}>
              {t(`dialogs.dmpNewImport.confidence.${currentSuggestion.confidence}`)}
            </span>
            <span className="text-xs text-text-secondary">
              · {t('dialogs.dmpNewImport.matchedShots', { count: currentSuggestion.matchedShots })} {directionLabel(currentSuggestion.direction)}
              {currentSuggestion.isPartial && <span className="ml-0.5 opacity-60">~</span>}
            </span>
          </div>

          {/* Merge station control */}
          <div className="flex items-center gap-2" data-testid="wizard-merge-station-control">
            <label className="text-xs text-text-secondary whitespace-nowrap">
              {t('dialogs.dmpNewImport.mergeStation')}
            </label>
            {mergeOptions.length > 1 ? (
              <Select
                value={String(selectedMergeOffset)}
                onChange={(v) => onMergeOffsetChange(Number(v))}
                options={mergeOptions.map((opt) => ({
                  value: String(opt.offset),
                  label: t('dialogs.dmpNewImport.mergeOption', { importStation: opt.importStation, target: targetSectionName, existingStation: opt.existingStation }),
                }))}
                size="sm"
                data-testid="merge-station-select"
              />
            ) : (
              <span className="text-xs text-text-secondary">
                {t('dialogs.dmpNewImport.mergeOption', {
                  importStation: selectedMergePair?.importStation ?? currentSuggestion.importStationIndex, target: targetSectionName,
                  existingStation: selectedMergePair?.existingStation ?? currentSuggestion.existingStationIndex,
                })}
              </span>
            )}
          </div>

          {/* Overlap control */}
          <div className="flex items-center gap-2" data-testid="wizard-overlap-control">
            <label className="text-xs text-text-secondary whitespace-nowrap">
              {t('dialogs.dmpNewImport.overlapMode')}
            </label>
            {currentSuggestion && currentSuggestion.matchedShots > 0 ? (
              <Select
                value={overlapMode}
                onChange={(v) => onOverlapModeChange(v as OverlapMode)}
                options={[
                  { value: 'interpolate', label: t('dialogs.dmpNewImport.overlapMode_interpolate'), disabled: isTargetDoublyAnchored },
                  { value: 'import_data', label: t('dialogs.dmpNewImport.overlapMode_importData'), disabled: isTargetDoublyAnchored },
                  { value: 'existing_data', label: t('dialogs.dmpNewImport.overlapMode_existingData') },
                  { value: 'keep_both', label: t('dialogs.dmpNewImport.overlapMode_keepBoth') },
                ]}
                size="sm"
                data-testid="overlap-mode-select"
              />
            ) : (
              <span className="text-xs text-text-secondary">
                {t('dialogs.dmpNewImport.noOverlapShots')}
              </span>
            )}
          </div>

          {/* Prompt */}
          <div className="text-sm font-medium">
            {t('dialogs.dmpNewImport.doesThisLookRight')}
          </div>
        </div>

        {/* Error */}
        {error && (
          <div className="text-sm text-error" data-testid="wizard-error">
            {error}
          </div>
        )}

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
                ? t('dialogs.dmpNewImport.hideAlternatives')
                : t('dialogs.dmpNewImport.showAlternatives', { count: alternatives.length })}
            </button>

            {showAlternatives && (
              <div className="mt-2 space-y-1" data-testid="alternatives-list">
                {alternatives.map((alt, idx) => (
                  <button
                    key={`${alt.importSectionIndex}-${alt.existingSectionId ?? alt.targetImportIndex}-${alt.existingStationIndex}-${alt.importStationIndex}-${alt.direction}`}
                    type="button"
                    className={`w-full p-2 rounded border text-left text-xs flex items-center gap-2 hover:bg-accent-5 transition-colors ${confidenceColors[alt.confidence] ?? ''}`}
                    onClick={() => onAlternativeClick(alt)}
                    data-testid={`alternative-${idx}`}
                  >
                    <span className={`px-1 py-0.5 rounded text-[10px] font-medium ${confidenceColors[alt.confidence] ?? ''}`}>
                      {t(`dialogs.dmpNewImport.confidence.${alt.confidence}`).split(' ')[0]}
                    </span>
                    <span className="flex-1 truncate">
                      <span className="text-text-secondary">{t('dialogs.dmpNewImport.stn', { index: alt.importStationIndex })}</span>
                      <span className="text-text-secondary"> → </span>
                      <span className="font-medium">{alt.existingSectionName}</span>
                      <span className="text-text-secondary"> {t('dialogs.dmpNewImport.stn', { index: alt.existingStationIndex })}</span>
                      {alt.targetType === 'importing' && (
                        <span className="text-text-secondary text-[10px] ml-0.5">{t('dialogs.dmpNewImport.importingShort')}</span>
                      )}
                    </span>
                    <span className="text-text-secondary">
                      {alt.matchedShots} {directionLabel(alt.direction)}
                    </span>
                  </button>
                ))}
              </div>
            )}
          </div>
        )}

        {/* Section Settings */}
        {/* #61 embedding slot: section-name + SectionMetadataForm + ShotTableEditor.
            Consumed as-is here; the reusable focus-mode section-edit surface
            (fields-right / table-bottom) drops in here later. Do not refactor.
            #567 item 2: wrapped in a collapsed-by-default disclosure so it
            stops competing with the connection card/confidence badge above -
            the wrapped content itself is unchanged. */}
        {editState && (
          <div className="border-t border-border pt-3">
            <SuggestionSectionSettings editState={editState} onMetadataChange={onMetadataChange} />
          </div>
        )}
      </div>
      </div>
      )}
    </WizardScrollBody>
  );
}

/** The suggestion step's footer actions: Skip and Confirm, shut while the run imports. */
export function SuggestingActions({ busy, onSkip, onConfirm }: { busy: boolean; onSkip: () => void; onConfirm: () => void }) {
  const { t } = useTranslation();
  return (
    <>
      <Button
        onClick={onSkip}
        disabled={busy}
        className={importSecondaryButtonClass}
        data-testid="skip-button"
      >
        {t('dialogs.dmpNewImport.skip')}
      </Button>
      <Button
        variant="primary"
        onClick={onConfirm}
        disabled={busy}
        data-testid="confirm-button"
      >
        {t('dialogs.dmpNewImport.confirm')}
      </Button>
    </>
  );
}
