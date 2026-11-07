/**
 * The DMP wizard's manual-placing step (split out of DmpNewImportDialog with
 * #576 D4): the segment on screen - its preview, the four ways to place it
 * (reuse a skipped suggestion, attach to an existing section, place on the
 * map, discard), its section settings, and the shot table below.
 *
 * Presentational: the segment queue, the anchor, the mode and the From
 * Existing pick are the `manual_placing` phase's (dmpWizard/wizardState.ts),
 * and every change goes back through the callbacks. The step derives what only
 * it draws: the segment preview, the From Existing context, the section names.
 */
import { useCallback, useId, useMemo, useRef, type MutableRefObject, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import type { DmpFile } from '../../../types/commands';
import type { SectionEditState } from '../../../utils/batchImportHelpers';
import { type VirtualConnection, computeSegmentLayout } from '../../../utils/segmentContext';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { Button } from '../../common/Button';
import { SectionMetadataForm } from '../../common/SectionMetadataForm';
import { WizardMapPanel } from '../WizardMapPanel';
// The preview cap comes straight from layoutConstants (see DmpNewImportDialog):
// two component tests mock WizardMapPanel, and a mocked module cannot supply
// the plain number the render arithmetic needs.
import { WIZARD_MAP_PANEL_MAX_WIDTH as MAP_PANEL_MAX_WIDTH } from '../../SingleSystemView/layoutConstants';
import { WizardScrollBody } from '../WizardScrollBody';
import SectionPreview, { type ContextSectionGeometry } from '../SectionPreview';
import { SegmentPlacementModes, type SegmentPlacementMode } from '../SegmentPlacementModes';
import { usePreviewSize } from '../usePreviewSize';
import { buildFromExistingContext } from './fromExistingContext';
import { adjustConnectionsForEdits, buildEffectiveDmpSection } from './effectiveSections';
import { anchorSectionIn, type ManualPlacingPhase, type SegmentAnchor } from './wizardState';
import type { SkippedSuggestionPlacement } from './wizardTypes';
import { WIZARD_SECONDARY_BUTTON_CLASS as importSecondaryButtonClass } from './WizardFooter';

export interface ManualPlacingStepProps {
  phase: ManualPlacingPhase;
  /** A suggestion skipped for one of the segment's sections, offered for reuse. */
  segmentSkippedSuggestion: SkippedSuggestionPlacement | null;
  onModeChange: (mode: SegmentPlacementMode) => void;
  onCommitAnchor: (anchor: SegmentAnchor) => void;
  onAnchorSectionHover: (value: string | null) => void;
  onAnchorStationHover: (value: string | null) => void;
  onFromExistingSectionChange: (sectionId: number | null) => void;
  onFromExistingStationChange: (stationIndex: number) => void;
  /** "Place on Map", clicked. */
  onPlaceOnMap: () => void;
  /** The anchor station as the backend (and the preview) counts it. */
  emittedAnchorStation: (anchor: SegmentAnchor) => number;
  /** A station the preview drew, as the station dropdowns count it. */
  visibleAnchorStation: (sectionIndex: number, emittedStationIndex: number) => number;
  /** The run's failure (a refused segment import lands here, #576 D13). */
  error: string | null;
  editState: SectionEditState | null;
  onMetadataChange: (field: keyof SectionEditState, value: string | number | boolean) => void;
  /** The shot table of the section on screen. */
  table: ReactNode;
  /** The run's edits per DMP section index. */
  sectionEditsRef: MutableRefObject<Map<number, SectionEditState>>;
  connections: VirtualConnection[];
  dmpFile: DmpFile;
  selectedIndices: number[];
  sectionPalette: string[];
}

export function ManualPlacingStep({
  phase,
  segmentSkippedSuggestion,
  onModeChange,
  onCommitAnchor,
  onAnchorSectionHover,
  onAnchorStationHover,
  onFromExistingSectionChange,
  onFromExistingStationChange,
  onPlaceOnMap,
  emittedAnchorStation,
  visibleAnchorStation,
  error,
  editState,
  onMetadataChange,
  table,
  sectionEditsRef,
  connections,
  dmpFile,
  selectedIndices,
  sectionPalette,
}: ManualPlacingStepProps) {
  const { t } = useTranslation();
  const currentSegment = phase.queue[0];
  const segmentAnchor = phase.anchor;
  const segmentPlacementMode = phase.mode;
  const fromExistingSectionId = phase.fromExisting.sectionId;
  const fromExistingStationIdx = phase.fromExisting.stationIndex;

  // Responsive preview - fills the available column so it never overflows the
  // fullscreen dialog on short windows (#93 review) and never leaves the column
  // half empty on wide ones (#567). One per flow.
  // The map column's width, dragged by the divider between it and the settings.
  // One value for both side-by-side blocks (suggestion and manual placing), so
  // the wizard does not change shape as it steps between them - and store-owned
  // since #404, so it also survives the dialog closing and the session ending
  // (WizardMapPanel persists it on divider release).
  const mapPanelWidth = useUIStore((s) => s.wizardMapPanelWidth);
  const setMapPanelWidth = useUIStore((s) => s.setWizardMapPanelWidth);
  const manualBodyRef = useRef<HTMLDivElement | null>(null);
  const [manualPreviewRef, manualPreviewSize] = usePreviewSize(MAP_PANEL_MAX_WIDTH);

  // Project sections and merges for "From Existing" dropdown + preview
  const projectSections = useProjectStore((s) => s.project?.sections ?? []);
  const projectMerges = useProjectStore((s) => s.project?.station_merges ?? []);
  const projectDefault = useProjectStore((s) => s.project?.default_azimuth_reference);

  // Get target section stations for "From Existing" mode
  const fromExistingTargetSection = useMemo(() => {
    if (fromExistingSectionId == null) return null;
    return projectSections.find(s => s.id === fromExistingSectionId) ?? null;
  }, [fromExistingSectionId, projectSections]);

  const fromExistingStationCount = useMemo(() => {
    if (!fromExistingTargetSection) return 0;
    return fromExistingTargetSection.shots.length + 1;
  }, [fromExistingTargetSection]);

  // Compute segment preview for manual_placing.
  // Uses effective (post-treatment) sections so the preview reflects deleted
  // shots and interpolated values - matching what will be imported.
  const segmentPreviewSections = useMemo(() => {
    if (!currentSegment) return null;

    // Build effective DmpSections with edits applied
    const effectiveSections = dmpFile.sections.map((sec, idx) =>
      buildEffectiveDmpSection(sec, sectionEditsRef.current.get(idx)),
    );
    // Adjust connection station indices to match post-deletion geometry
    const effectiveConns = adjustConnectionsForEdits(connections, sectionEditsRef.current);
    const layout = computeSegmentLayout(currentSegment, effectiveConns, effectiveSections);

    return layout.map((entry) => {
      const section = effectiveSections[entry.sectionIndex];
      if (!section) return null;
      const edits = sectionEditsRef.current.get(entry.sectionIndex);
      const selectedPos = selectedIndices.indexOf(entry.sectionIndex);
      const sectionColor = (edits?.color || null) ?? sectionPalette[(selectedPos >= 0 ? selectedPos : entry.sectionIndex) % sectionPalette.length] ?? '#95a5a6';
      return {
        section,
        color: sectionColor,
        sectionIndex: entry.sectionIndex,
        offsetX: entry.offsetX,
        offsetY: entry.offsetY,
      };
    }).filter((x): x is NonNullable<typeof x> => x !== null);
  }, [currentSegment, connections, dmpFile.sections, sectionPalette, sectionEditsRef, selectedIndices]);

  // Compute target segment context for "From Existing" preview (shows the target section's
  // connected segment as dimmed lines behind the import segment preview)
  const fromExistingContextSections = useMemo((): ContextSectionGeometry[] | null => {
    if (segmentPlacementMode !== 'fromExisting' || fromExistingSectionId == null) return null;
    if (!segmentAnchor || !segmentPreviewSections || segmentPreviewSections.length === 0) return null;
    return buildFromExistingContext({
      fromExistingSectionId, fromExistingStationIdx, projectSections, projectMerges, projectDefault,
      anchorSectionIndex: segmentAnchor.sectionIndex, emittedAnchorStation: emittedAnchorStation(segmentAnchor), segmentPreviewSections,
    });
  }, [segmentPlacementMode, fromExistingSectionId, fromExistingStationIdx, projectSections, projectMerges, projectDefault, segmentAnchor, segmentPreviewSections, emittedAnchorStation]);

  // Station click handler for segment preview. The index arrives in the
  // preview's own (emitted) numbering - see the note on the two spaces.
  const handleSegmentStationClick = useCallback((stationIndex: number, sectionIndex?: number) => {
    const idx = sectionIndex ?? (currentSegment && currentSegment.length > 0 ? currentSegment[0]! : null);
    if (idx == null) return;
    onCommitAnchor({ sectionIndex: idx, stationIndex: visibleAnchorStation(idx, stationIndex) });
  }, [currentSegment, onCommitAnchor, visibleAnchorStation]);

  // [#576 NF-W9FE-3-D1] The payload builder's own condition: with no such section
  // in the file the segment cannot go on the map, and the screen says so.
  const cannotPlaceOnMap = anchorSectionIn(dmpFile.sections, segmentAnchor) === undefined;
  // [#576 RV10b-2] The line is also the disabled "Place on map" button's
  // accessible reason (aria-describedby), so it needs an id.
  const cannotPlaceOnMapId = useId();
  // [#576 RV10b-6] The line points at the preview for another anchor - but when
  // none of the segment's sections is in the file the preview draws nothing to
  // click, so that sub-case leaves the preview out and offers Do not place alone.
  const previewHasStations = (segmentPreviewSections?.length ?? 0) > 0;

  // Section names in current segment (for display)
  const segmentSectionNames = useMemo(() => {
    if (!currentSegment) return [];
    return currentSegment.map(idx => dmpFile.sections[idx]?.name ?? t('dialogs.dmpNewImport.sectionFallbackName', { id: idx }));
  }, [currentSegment, dmpFile.sections, t]);

  return (
    <WizardScrollBody
      bodyRef={manualBodyRef}
      table={table}
    >
      {(bandHeight) => (
      <div className="flex" style={{ minHeight: bandHeight }}>
      <WizardMapPanel
        width={mapPanelWidth}
        onWidthChange={setMapPanelWidth}
        bodyRef={manualBodyRef} panelRef={manualPreviewRef} height={bandHeight}
      >
        <SectionPreview
          segmentSections={segmentPreviewSections}
          contextSections={fromExistingContextSections}
          size={manualPreviewSize.width}
          height={manualPreviewSize.height}
          onStationClick={handleSegmentStationClick}
          selectedStationIndex={emittedAnchorStation(phase.previewAnchor)}
          selectedSectionIndex={phase.previewAnchor.sectionIndex}
        />
      </WizardMapPanel>

      {/* Right column: Placement configuration */}
      <div className="flex-1 min-w-0 p-5 space-y-4" data-testid="wizard-manual-placing">
        {/* Segment header */}
        <div>
          <div className="text-sm font-medium">
            {t('dialogs.dmpNewImport.manualPlacing.segmentHeader', {
              current: phase.queue.length > 0 ? 1 : 0,
              total: phase.queue.length,
            })}
          </div>
          <div className="text-xs text-text-secondary mt-0.5">
            {t('dialogs.dmpNewImport.manualPlacing.sections')}: {segmentSectionNames.join(', ')}
          </div>
        </div>

        {/* Error - at the top, where the screen opens: a segment import that
            failed after a drop on the map lands here (#576 D13), and below
            the section settings it was out of view. */}
        {error && (
          <div className="text-sm text-error" role="alert" data-testid="wizard-error">
            {error}
          </div>
        )}

        {cannotPlaceOnMap && (
          <div
            id={cannotPlaceOnMapId}
            className="text-sm text-warning"
            role="status"
            data-testid="wizard-cannot-place-on-map"
          >
            {previewHasStations
              ? t('dialogs.dmpNewImport.manualPlacing.cannotPlaceOnMap')
              : t('dialogs.dmpNewImport.manualPlacing.cannotPlaceOnMapNoPreview')}
          </div>
        )}

        {/* Placement mode radio group */}
        <SegmentPlacementModes
          segmentSkippedSuggestion={segmentSkippedSuggestion}
          placementMode={segmentPlacementMode}
          onPlacementModeChange={onModeChange}
          currentSegment={currentSegment}
          segmentAnchor={segmentAnchor}
          onCommitSegmentAnchor={onCommitAnchor}
          sectionEdits={sectionEditsRef.current}
          dmpSections={dmpFile.sections}
          onAnchorSectionHover={onAnchorSectionHover}
          onAnchorStationHover={onAnchorStationHover}
          projectSections={projectSections}
          fromExistingSectionId={fromExistingSectionId}
          onFromExistingSectionChange={onFromExistingSectionChange}
          fromExistingStationIdx={fromExistingStationIdx}
          onFromExistingStationChange={onFromExistingStationChange}
          fromExistingStationCount={fromExistingStationCount}
          onPlaceOnMap={onPlaceOnMap}
          placeOnMapBlockedBy={cannotPlaceOnMap ? cannotPlaceOnMapId : null}
        />

        {/* Section settings (same editing surface as section connection / zero datum flows) */}
        {/* #61 embedding slot: section-edit surface goes here later. Do not refactor. */}
        {editState && (
          <>
            <div className="space-y-2" data-testid="wizard-manual-section-settings">
              <div className="border-t border-border" />
              <div className="text-sm font-medium">
                {t('dialogs.dmpNewImport.sectionSettings')}
              </div>
              <div>
                <label className="block text-sm text-text-secondary mb-1">
                  {t('common.labels.name')}
                </label>
                <input
                  type="text"
                  value={editState.name}
                  onChange={(e) => onMetadataChange('name', e.target.value)}
                  className="w-full px-3 py-2 bg-background border border-border rounded"
                  placeholder={t('sectionMetadata.placeholders.name')}
                  data-testid="section-name-input"
                />
              </div>
            </div>
            <div className="space-y-4" data-testid="wizard-manual-editable-rest">
              <SectionMetadataForm
                name={editState.name}
                surveyor={editState.surveyor}
                date={editState.date}
                color={editState.color}
                direction={editState.direction}
                quality={editState.quality}
                onNameChange={(v) => onMetadataChange('name', v)}
                onSurveyorChange={(v) => onMetadataChange('surveyor', v)}
                onDateChange={(v) => onMetadataChange('date', v)}
                onColorChange={(v) => onMetadataChange('color', v)}
                onDirectionChange={(v) => onMetadataChange('direction', v)}
                onQualityChange={(v) => onMetadataChange('quality', v)}
                reverse={editState.reverse}
                onReverseChange={(v) => onMetadataChange('reverse', v)}
                showName={false}
                showDirection
                showReverse
              />
            </div>
          </>
        )}
      </div>
      </div>
      )}
    </WizardScrollBody>
  );
}

/** The manual-placing step's footer actions: Skip segment (with more than one
 *  queued) and Place - "Next" when the segment is being discarded. */
export function ManualPlacingActions({
  canSkip,
  onSkip,
  canPlace,
  discarding,
  onPlace,
}: {
  canSkip: boolean;
  onSkip: () => void;
  /** The chosen mode can be submitted (map placement goes through its own button). */
  canPlace: boolean;
  discarding: boolean;
  onPlace: () => void;
}) {
  const { t } = useTranslation();
  return (
    <>
      {canSkip && (
        <Button
          onClick={onSkip}
          className={importSecondaryButtonClass}
          data-testid="segment-skip-button"
        >
          {t('dialogs.dmpNewImport.manualPlacing.skip')}
        </Button>
      )}
      <Button
        variant="primary"
        onClick={onPlace}
        disabled={!canPlace}
        data-testid="segment-place-button"
      >
        {discarding
          ? t('dialogs.dmpNewImport.manualPlacing.next')
          : t('dialogs.dmpNewImport.manualPlacing.placeSegment')}
      </Button>
    </>
  );
}
