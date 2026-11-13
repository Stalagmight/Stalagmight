import { useEffect, type RefObject } from 'react';
import { useShallow } from 'zustand/react/shallow';
import { useUIStore } from '../../store/uiStore';
import type { SectionId, StationRef } from '../../types/project';
import type { SplitAt } from '../../types/ipc/SplitAt';
import type SurveyLayer from './SurveyLayer';

/** Highlight-layer id of a station (`section_id:station_index`). */
const stationRefToId = (ref: StationRef | null | undefined): string | null =>
  ref ? `${ref.section_id}:${ref.station_index}` : null;

/**
 * What an open structural dialog previews on the map: the stations it rings and
 * the sections it highlights. Split Section Here (#335) rings the split station;
 * Combine Sections rings the join and highlights both sections.
 */
export function dialogMapPreview(
  activeDialog: string | null,
  dialogData: Record<string, unknown>
): { stations: StationRef[]; sections: SectionId[] } {
  if (activeDialog === 'split-section-here') {
    const at = dialogData.at as SplitAt | undefined;
    const sectionId = dialogData.sectionId as SectionId | undefined;
    if (at?.type === 'station' && sectionId !== undefined) {
      return { stations: [{ section_id: sectionId, station_index: at.station_index }], sections: [] };
    }
  }
  if (activeDialog === 'combine-sections') {
    const preview = dialogData.preview as { stations?: StationRef[]; sections?: SectionId[] } | undefined;
    return { stations: preview?.stations ?? [], sections: preview?.sections ?? [] };
  }
  return { stations: [], sections: [] };
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
    layer.setPreviewSections(preview.sections);
  }, [surveyLayerRef, snappedStation, mergeStationA, mergeStationB, activeDialog, dialogData, mapLoaded]);
}
