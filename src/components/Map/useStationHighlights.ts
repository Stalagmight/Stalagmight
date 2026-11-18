import { useEffect, type RefObject } from 'react';
import { useShallow } from 'zustand/react/shallow';
import { useUIStore } from '../../store/uiStore';
import type { LineId, StationRef } from '../../types/project';
import type { SplitAt } from '../../types/ipc/SplitAt';
import type SurveyLayer from './SurveyLayer';

/** Highlight-layer id of a station (`line_id:station_index`). */
const stationRefToId = (ref: StationRef | null | undefined): string | null =>
  ref ? `${ref.line_id}:${ref.station_index}` : null;

/**
 * What an open structural dialog previews on the map: the stations it rings and
 * the lines it highlights. Split Line Here (#335) rings the split station;
 * Combine Lines rings the join and highlights both lines.
 */
export function dialogMapPreview(
  activeDialog: string | null,
  dialogData: Record<string, unknown>
): { stations: StationRef[]; lines: LineId[] } {
  if (activeDialog === 'split-line-here') {
    const at = dialogData.at as SplitAt | undefined;
    const lineId = dialogData.lineId as LineId | undefined;
    if (at?.type === 'station' && lineId !== undefined) {
      return { stations: [{ line_id: lineId, station_index: at.station_index }], lines: [] };
    }
  }
  if (activeDialog === 'combine-lines') {
    const preview = dialogData.preview as { stations?: StationRef[]; lines?: LineId[] } | undefined;
    return { stations: preview?.stations ?? [], lines: preview?.lines ?? [] };
  }
  return { stations: [], lines: [] };
}

/**
 * Sync the highlight layer with the snap/merge state and an open structural
 * dialog's preview. Shows snapped/origin stations even when the stations layer
 * is hidden.
 */
export function useStationHighlights(
  surveyLayerRef: RefObject<SurveyLayer | null>,
  mapLoaded: boolean,
  snappedStation: StationRef | null,
  mergeStationA: StationRef | null,
  mergeStationB: StationRef | null
): void {
  const { activeDialog, dialogData } = useUIStore(
    useShallow((s) => ({ activeDialog: s.activeDialog, dialogData: s.dialogData }))
  );

  useEffect(() => {
    const layer = surveyLayerRef.current;
    if (!layer) return;
    const preview = dialogMapPreview(activeDialog, dialogData);
    const highlightIds: string[] = [];
    for (const ref of [snappedStation, mergeStationA, mergeStationB, ...preview.stations]) {
      const id = stationRefToId(ref);
      if (id && !highlightIds.includes(id)) highlightIds.push(id);
    }
    layer.setHighlightedStations(highlightIds);
    layer.setPreviewLines(preview.lines);
  }, [surveyLayerRef, snappedStation, mergeStationA, mergeStationB, activeDialog, dialogData, mapLoaded]);
}
