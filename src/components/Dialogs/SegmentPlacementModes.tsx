/**
 * #567 item 3: on a short window the checked card's inline controls
 * (from-existing/manual pickers) plus the "Place on Map" action all used to
 * sit in the dialog's single body scroller, pushing the primary action well
 * below the fold behind three inactive cards. The card list now gets its own
 * bounded scroll region (engages only once content exceeds `max-h`), and
 * "Place on Map" is pulled out of the manual card to render right after that
 * region - always reachable without scrolling past the whole group.
 */
import { useTranslation } from 'react-i18next';
import { Button } from '../common/Button';
import { Select, type SelectOption } from '../common/Select';
import type { DmpFile } from '../../types/commands';
import type { Section } from '../../types/project';
import type { SectionEditState } from '../../utils/batchImportHelpers';
import type { SkippedSuggestionPlacement } from './DmpNewImportDialog';

/** One per radio below - a mode with no radio would leave none selected (#576 NF-AN4-N3). */
export type SegmentPlacementMode = 'suggestion' | 'fromExisting' | 'manual' | 'discard';

export interface SegmentAnchor {
  sectionIndex: number;
  stationIndex: number;
}

export interface SegmentPlacementModesProps {
  segmentSkippedSuggestion: SkippedSuggestionPlacement | null;
  placementMode: SegmentPlacementMode;
  onPlacementModeChange: (mode: SegmentPlacementMode) => void;
  currentSegment: number[] | null;
  segmentAnchor: SegmentAnchor | null;
  onCommitSegmentAnchor: (anchor: SegmentAnchor) => void;
  sectionEdits: Map<number, SectionEditState>;
  dmpSections: DmpFile['sections'];
  onAnchorSectionHover: (value: string | null) => void;
  onAnchorStationHover: (value: string | null) => void;
  projectSections: Section[];
  fromExistingSectionId: number | null;
  onFromExistingSectionChange: (id: number | null) => void;
  fromExistingStationIdx: number;
  onFromExistingStationChange: (idx: number) => void;
  fromExistingStationCount: number;
  onPlaceOnMap: () => void;
  /**
   * [#576 RV10b-2] The id of the line saying why this segment cannot go on the
   * map, or null when it can. While set, "Place on map" is disabled and that line
   * is its accessible description: the click could only log a warning.
   */
  placeOnMapBlockedBy?: string | null;
}

export function SegmentPlacementModes({
  segmentSkippedSuggestion,
  placementMode,
  onPlacementModeChange,
  currentSegment,
  segmentAnchor,
  onCommitSegmentAnchor,
  sectionEdits,
  dmpSections,
  onAnchorSectionHover,
  onAnchorStationHover,
  projectSections,
  fromExistingSectionId,
  onFromExistingSectionChange,
  fromExistingStationIdx,
  onFromExistingStationChange,
  fromExistingStationCount,
  onPlaceOnMap,
  placeOnMapBlockedBy = null,
}: SegmentPlacementModesProps) {
  const { t } = useTranslation();

  const sectionOptions = (): SelectOption[] =>
    (currentSegment ?? []).map((idx): SelectOption => ({
      value: String(idx),
      label: sectionEdits.get(idx)?.name ?? dmpSections[idx]?.name ?? t('dialogs.dmpNewImport.sectionFallbackName', { id: idx }),
    }));

  const stationOptions = (): SelectOption[] => {
    const secIdx = segmentAnchor?.sectionIndex;
    const secEdits = secIdx != null ? sectionEdits.get(secIdx) : undefined;
    const sec = secIdx != null ? dmpSections[secIdx] : null;
    const count = secEdits
      ? secEdits.shots.filter((s) => !s.isDeleted).length + 1
      : sec ? sec.shots.filter((s) => s.shot_type === 'Standard').length + 1 : 0;
    return Array.from({ length: count }, (_, i): SelectOption => ({
      value: String(i),
      label: t('dialogs.dmpNewImport.manualPlacing.stationIndexed', { index: i }),
    }));
  };

  return (
    <div className="space-y-2" data-testid="segment-placement-modes">
      <div className="text-xs font-medium text-text-secondary">
        {t('dialogs.dmpNewImport.manualPlacing.placementMode')}
      </div>

      <div className="max-h-[45vh] overflow-y-auto space-y-2 pr-1" data-testid="segment-placement-modes-scroll">
        {segmentSkippedSuggestion && (
          <label className="flex items-start gap-2 p-2 rounded border border-border hover:bg-accent-5 cursor-pointer">
            <input
              type="radio"
              name="segmentPlacementMode"
              data-testid="segment-mode-suggestion-input"
              checked={placementMode === 'suggestion'}
              onChange={() => onPlacementModeChange('suggestion')}
              className="mt-0.5"
            />
            <div className="text-sm">
              <div className="font-medium">{t('dialogs.dmpNewImport.manualPlacing.suggestion')}</div>
              <div className="text-xs text-text-secondary">
                {t('dialogs.dmpNewImport.manualPlacing.suggestionTarget', {
                  section: segmentSkippedSuggestion.suggestion.existingSectionName,
                  station: segmentSkippedSuggestion.mergeExistingStation,
                })}
                {' · '}{t('dialogs.dmpNewImport.matchedShots', { count: segmentSkippedSuggestion.suggestion.matchedShots })}
              </div>
            </div>
          </label>
        )}

        {/* From existing station */}
        <label
          className="flex items-start gap-2 p-2 rounded border border-border hover:bg-accent-5 cursor-pointer"
          data-testid="segment-mode-from-existing"
        >
          <input
            type="radio"
            name="segmentPlacementMode"
            data-testid="segment-mode-from-existing-input"
            checked={placementMode === 'fromExisting'}
            onChange={() => onPlacementModeChange('fromExisting')}
            className="mt-0.5"
          />
          <div className="text-sm flex-1">
            <div>{t('dialogs.dmpNewImport.manualPlacing.fromExisting')}</div>
            {placementMode === 'fromExisting' && (
              <div className="mt-2 space-y-2">
                {/* Import side: segment section + station */}
                <div className="text-xs font-medium text-text-secondary mt-1">
                  {t('dialogs.dmpNewImport.manualPlacing.importStation')}
                </div>
                {currentSegment && currentSegment.length > 1 && (
                  <div className="flex items-center gap-2">
                    <label className="text-xs text-text-secondary whitespace-nowrap">
                      {t('dialogs.dmpNewImport.manualPlacing.section')}
                    </label>
                    <Select
                      value={String(segmentAnchor?.sectionIndex ?? '')}
                      onChange={(v) => onCommitSegmentAnchor({ sectionIndex: Number(v), stationIndex: 0 })}
                      options={sectionOptions()}
                      className="flex-1"
                      data-testid="from-existing-import-section-select"
                      onOptionHover={onAnchorSectionHover}
                    />
                  </div>
                )}
                <div className="flex items-center gap-2">
                  <label className="text-xs text-text-secondary whitespace-nowrap">
                    {t('dialogs.dmpNewImport.manualPlacing.station')}
                  </label>
                  <Select
                    value={String(segmentAnchor?.stationIndex ?? 0)}
                    onChange={(v) => {
                      if (!segmentAnchor) return;
                      onCommitSegmentAnchor({ sectionIndex: segmentAnchor.sectionIndex, stationIndex: Number(v) });
                    }}
                    options={stationOptions()}
                    className="flex-1"
                    data-testid="from-existing-import-station-select"
                    onOptionHover={onAnchorStationHover}
                  />
                </div>

                {/* Target side: existing project section + station */}
                <div className="text-xs font-medium text-text-secondary mt-1">
                  {t('dialogs.dmpNewImport.manualPlacing.targetStation')}
                </div>
                {projectSections.length > 0 ? (
                  <>
                    <div className="flex items-center gap-2">
                      <label className="text-xs text-text-secondary whitespace-nowrap">
                        {t('dialogs.dmpNewImport.manualPlacing.section')}
                      </label>
                      <Select
                        value={fromExistingSectionId != null ? String(fromExistingSectionId) : ''}
                        onChange={(v) => {
                          onFromExistingSectionChange(v ? Number(v) : null);
                          onFromExistingStationChange(0);
                        }}
                        options={[
                          { value: '', label: '-' } as SelectOption,
                          ...projectSections.map((s): SelectOption => ({
                            value: String(s.id),
                            label: s.name ?? t('dialogs.dmpNewImport.sectionFallbackName', { id: s.id }),
                          })),
                        ]}
                        className="flex-1"
                        data-testid="from-existing-section-select"
                      />
                    </div>
                    {fromExistingSectionId != null && (
                      <div className="flex items-center gap-2">
                        <label className="text-xs text-text-secondary whitespace-nowrap">
                          {t('dialogs.dmpNewImport.manualPlacing.station')}
                        </label>
                        <Select
                          value={String(fromExistingStationIdx)}
                          onChange={(v) => onFromExistingStationChange(Number(v))}
                          options={Array.from({ length: fromExistingStationCount }, (_, i): SelectOption => ({
                            value: String(i),
                            label: t('dialogs.dmpNewImport.manualPlacing.stationIndexed', { index: i }),
                          }))}
                          className="flex-1"
                          data-testid="from-existing-station-select"
                        />
                      </div>
                    )}
                  </>
                ) : (
                  <div className="text-xs text-text-secondary italic">
                    {t('dialogs.dmpNewImport.manualPlacing.noSections')}
                  </div>
                )}
              </div>
            )}
          </div>
        </label>

        {/* Manual placement */}
        <label
          className="flex items-start gap-2 p-2 rounded border border-border hover:bg-accent-5 cursor-pointer"
          data-testid="segment-mode-manual"
        >
          <input
            type="radio"
            name="segmentPlacementMode"
            data-testid="segment-mode-manual-input"
            checked={placementMode === 'manual'}
            onChange={() => onPlacementModeChange('manual')}
            className="mt-0.5"
          />
          <div className="text-sm flex-1">
            <div>{t('dialogs.dmpNewImport.manualPlacing.manual')}</div>
            {placementMode === 'manual' && (
              <div className="mt-2 space-y-2">
                {currentSegment && currentSegment.length > 1 && (
                  <div className="flex items-center gap-2">
                    <label className="text-xs text-text-secondary whitespace-nowrap">
                      {t('dialogs.dmpNewImport.manualPlacing.section')}
                    </label>
                    <Select
                      value={String(segmentAnchor?.sectionIndex ?? '')}
                      onChange={(v) => onCommitSegmentAnchor({ sectionIndex: Number(v), stationIndex: 0 })}
                      options={sectionOptions()}
                      className="flex-1"
                      data-testid="manual-import-section-select"
                      onOptionHover={onAnchorSectionHover}
                    />
                  </div>
                )}
                <div className="flex items-center gap-2">
                  <label className="text-xs text-text-secondary whitespace-nowrap">
                    {t('dialogs.dmpNewImport.manualPlacing.station')}
                  </label>
                  <Select
                    value={String(segmentAnchor?.stationIndex ?? 0)}
                    onChange={(v) => {
                      if (!segmentAnchor) return;
                      onCommitSegmentAnchor({ sectionIndex: segmentAnchor.sectionIndex, stationIndex: Number(v) });
                    }}
                    options={stationOptions()}
                    className="flex-1"
                    data-testid="manual-import-station-select"
                    onOptionHover={onAnchorStationHover}
                  />
                </div>
              </div>
            )}
          </div>
        </label>

        {/* Do not place (discard) */}
        <label
          className="flex items-start gap-2 p-2 rounded border border-border hover:bg-accent-5 cursor-pointer"
          data-testid="segment-mode-discard"
        >
          <input
            type="radio"
            name="segmentPlacementMode"
            data-testid="segment-mode-discard-input"
            checked={placementMode === 'discard'}
            onChange={() => onPlacementModeChange('discard')}
            className="mt-0.5"
          />
          <div className="text-sm">
            <div>{t('dialogs.dmpNewImport.manualPlacing.doNotPlace')}</div>
            <div className="text-xs text-text-secondary">
              {t('dialogs.dmpNewImport.manualPlacing.doNotPlaceDescription')}
            </div>
          </div>
        </label>
      </div>

      {/* Kept outside the scroll region above so it stays reachable without
          scrolling past the whole card list on short windows. */}
      {placementMode === 'manual' && (
        <div className="pt-1" data-testid="segment-manual-action-row">
          <Button
            variant="primary"
            size="sm"
            onClick={onPlaceOnMap}
            disabled={placeOnMapBlockedBy !== null}
            aria-describedby={placeOnMapBlockedBy ?? undefined}
            data-testid="segment-place-on-map-button"
          >
            {t('dialogs.dmpNewImport.manualPlacing.placeOnMap')}
          </Button>
        </div>
      )}
    </div>
  );
}
