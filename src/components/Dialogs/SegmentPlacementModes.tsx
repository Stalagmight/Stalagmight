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
import type { Line } from '../../types/project';
import type { LineEditState } from '../../utils/batchImportHelpers';
import type { SkippedSuggestionPlacement } from './DmpNewImportDialog';
import { useIdentityLabels } from '../../hooks/useIdentityLabels';
import { displayStationName } from '../../utils/formatters';
import { lineDisplayName } from '../../utils/stationIdentity';
import { dmpFileStationName, importStationChoices, suggestionTargetStationLabel } from '../../utils/importStationNames';
import { toEmittedStationIndex } from '../common/emittedShots';

export type SegmentPlacementMode = 'suggestion' | 'origin' | 'fromExisting' | 'manual' | 'discard';

export interface SegmentAnchor {
  lineIndex: number;
  stationIndex: number;
}

export interface SegmentPlacementModesProps {
  segmentSkippedSuggestion: SkippedSuggestionPlacement | null;
  placementMode: SegmentPlacementMode;
  onPlacementModeChange: (mode: SegmentPlacementMode) => void;
  currentSegment: number[] | null;
  segmentAnchor: SegmentAnchor | null;
  onCommitSegmentAnchor: (anchor: SegmentAnchor) => void;
  lineEdits: Map<number, LineEditState>;
  dmpSections: DmpFile['lines'];
  onAnchorLineHover: (value: string | null) => void;
  onAnchorStationHover: (value: string | null) => void;
  projectLines: Line[];
  fromExistingLineId: number | null;
  onFromExistingLineChange: (id: number | null) => void;
  fromExistingStationIdx: number;
  onFromExistingStationChange: (idx: number) => void;
  fromExistingStationCount: number;
  onPlaceOnMap: () => void;
}

export function SegmentPlacementModes({
  segmentSkippedSuggestion,
  placementMode,
  onPlacementModeChange,
  currentSegment,
  segmentAnchor,
  onCommitSegmentAnchor,
  lineEdits,
  dmpSections,
  onAnchorLineHover,
  onAnchorStationHover,
  projectLines,
  fromExistingLineId,
  onFromExistingLineChange,
  fromExistingStationIdx,
  onFromExistingStationChange,
  fromExistingStationCount,
  onPlaceOnMap,
}: SegmentPlacementModesProps) {
  const { t } = useTranslation();
  const labels = useIdentityLabels();
  const stationOption = (i: number, name: string): SelectOption => ({
    value: String(i),
    label: t('dialogs.dmpNewImport.manualPlacing.stationNamed', { name: displayStationName(name) }),
  });

  const lineOptions = (): SelectOption[] =>
    (currentSegment ?? []).map((idx): SelectOption => ({
      value: String(idx),
      label: lineDisplayName(lineEdits.get(idx)?.name ?? dmpSections[idx]?.name),
    }));

  // Each station of the anchor line by the name the import gives it: an
  // edited line's emitted stations (a blank added row emits none, so it adds
  // no option), an unedited one by the file's MNemo numbers. The pick is
  // stored over the rows on screen, so the shown value is the option of the
  // station the stored pick lands on.
  const secIdx = segmentAnchor?.lineIndex;
  const secEdits = secIdx != null ? lineEdits.get(secIdx) : undefined;
  const editedChoices = secEdits ? importStationChoices(secEdits.shots, secEdits.reverse) : null;
  const stationOptions = (): SelectOption[] => {
    if (editedChoices) return editedChoices.map((c) => stationOption(c.index, c.name));
    const sec = secIdx != null ? dmpSections[secIdx] : null;
    const count = sec ? sec.shots.filter((s) => s.shot_type === 'Standard').length + 1 : 0;
    return Array.from({ length: count }, (_, i) => stationOption(i, dmpFileStationName(i)));
  };
  const stationValue = (): string => {
    const picked = segmentAnchor?.stationIndex ?? 0;
    if (!editedChoices || !secEdits) return String(picked);
    const emitted = toEmittedStationIndex(secEdits.shots, picked);
    return String(editedChoices.find((c) => c.emitted === emitted)?.index ?? picked);
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
                → {suggestionTargetStationLabel(
                  labels,
                  segmentSkippedSuggestion.suggestion,
                  segmentSkippedSuggestion.mergeExistingStation,
                  (i) => lineEdits.get(i)?.name,
                )}
                {' · '}{segmentSkippedSuggestion.suggestion.matchedShots} shots matched
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
                {/* Import side: segment line + station */}
                <div className="text-xs font-medium text-text-secondary mt-1">
                  {t('dialogs.dmpNewImport.manualPlacing.importStation')}
                </div>
                {currentSegment && currentSegment.length > 1 && (
                  <div className="flex items-center gap-2">
                    <label className="text-xs text-text-secondary whitespace-nowrap">
                      {t('dialogs.dmpNewImport.manualPlacing.section')}
                    </label>
                    <Select
                      value={String(segmentAnchor?.lineIndex ?? '')}
                      onChange={(v) => onCommitSegmentAnchor({ lineIndex: Number(v), stationIndex: 0 })}
                      options={lineOptions()}
                      className="flex-1"
                      data-testid="from-existing-import-line-select"
                      onOptionHover={onAnchorLineHover}
                    />
                  </div>
                )}
                <div className="flex items-center gap-2">
                  <label className="text-xs text-text-secondary whitespace-nowrap">
                    {t('dialogs.dmpNewImport.manualPlacing.station')}
                  </label>
                  <Select
                    value={stationValue()}
                    onChange={(v) => {
                      if (!segmentAnchor) return;
                      onCommitSegmentAnchor({ lineIndex: segmentAnchor.lineIndex, stationIndex: Number(v) });
                    }}
                    options={stationOptions()}
                    className="flex-1"
                    data-testid="from-existing-import-station-select"
                    onOptionHover={onAnchorStationHover}
                  />
                </div>

                {/* Target side: existing project line + station */}
                <div className="text-xs font-medium text-text-secondary mt-1">
                  {t('dialogs.dmpNewImport.manualPlacing.targetStation')}
                </div>
                {projectLines.length > 0 ? (
                  <>
                    <div className="flex items-center gap-2">
                      <label className="text-xs text-text-secondary whitespace-nowrap">
                        {t('dialogs.dmpNewImport.manualPlacing.section')}
                      </label>
                      <Select
                        value={fromExistingLineId != null ? String(fromExistingLineId) : ''}
                        onChange={(v) => {
                          onFromExistingLineChange(v ? Number(v) : null);
                          onFromExistingStationChange(0);
                        }}
                        options={[
                          { value: '', label: '-' } as SelectOption,
                          ...projectLines.map((s): SelectOption => ({
                            value: String(s.id),
                            label: lineDisplayName(s.name),
                          })),
                        ]}
                        className="flex-1"
                        data-testid="from-existing-line-select"
                      />
                    </div>
                    {fromExistingLineId != null && (
                      <div className="flex items-center gap-2">
                        <label className="text-xs text-text-secondary whitespace-nowrap">
                          {t('dialogs.dmpNewImport.manualPlacing.station')}
                        </label>
                        <Select
                          value={String(fromExistingStationIdx)}
                          onChange={(v) => onFromExistingStationChange(Number(v))}
                          options={Array.from({ length: fromExistingStationCount }, (_, i) =>
                            stationOption(i, labels.stationName({ line_id: fromExistingLineId, station_index: i })),
                          )}
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
                      value={String(segmentAnchor?.lineIndex ?? '')}
                      onChange={(v) => onCommitSegmentAnchor({ lineIndex: Number(v), stationIndex: 0 })}
                      options={lineOptions()}
                      className="flex-1"
                      data-testid="manual-import-line-select"
                      onOptionHover={onAnchorLineHover}
                    />
                  </div>
                )}
                <div className="flex items-center gap-2">
                  <label className="text-xs text-text-secondary whitespace-nowrap">
                    {t('dialogs.dmpNewImport.manualPlacing.station')}
                  </label>
                  <Select
                    value={stationValue()}
                    onChange={(v) => {
                      if (!segmentAnchor) return;
                      onCommitSegmentAnchor({ lineIndex: segmentAnchor.lineIndex, stationIndex: Number(v) });
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
            data-testid="segment-place-on-map-button"
          >
            {t('dialogs.dmpNewImport.manualPlacing.placeOnMap')}
          </Button>
        </div>
      )}
    </div>
  );
}
