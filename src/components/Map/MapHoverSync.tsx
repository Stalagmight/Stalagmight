import { useEffect, useRef } from 'react';
import { useUIStore } from '../../store/uiStore';
import type SurveyLayer from './SurveyLayer';

/**
 * Lightweight child component that subscribes to hover state and syncs it to
 * SurveyLayer. Extracted from MapCanvas so that hover changes (~60/sec during
 * mouse movement) only re-render this tiny component instead of the entire
 * 2700-line MapCanvas with ~50 hooks.
 */
export function MapHoverSync({
  surveyLayerRef,
  mapLoaded,
  anchoredSectionIds,
  getConnectedSectionIds,
  logMapInteractions,
}: {
  surveyLayerRef: React.RefObject<SurveyLayer | null>;
  mapLoaded: boolean;
  anchoredSectionIds: Set<number>;
  getConnectedSectionIds: (id: number) => Promise<number[]>;
  logMapInteractions: boolean;
}) {
  const hoveredStation = useUIStore((s) => s.hoveredStation);
  const hoveredShot = useUIStore((s) => s.hoveredShot);
  const hoveredSectionId = useUIStore((s) => s.hoveredSectionId);
  const activeTool = useUIStore((s) => s.activeTool);

  // Sync hoveredSectionId to SurveyLayer
  useEffect(() => {
    if (!surveyLayerRef.current) return;
    surveyLayerRef.current.setHoveredSection(hoveredSectionId);
  }, [hoveredSectionId, mapLoaded, surveyLayerRef]);

  // Sync hoveredShot to SurveyLayer (for DataTable row hover)
  useEffect(() => {
    if (!surveyLayerRef.current) return;
    surveyLayerRef.current.setHoveredShot(
      hoveredShot?.sectionId ?? null,
      hoveredShot?.shotId ?? null
    );
  }, [hoveredShot, mapLoaded, surveyLayerRef]);

  // Sync hoveredStation to SurveyLayer (for DetailLink hover)
  useEffect(() => {
    if (!surveyLayerRef.current) return;
    surveyLayerRef.current.setHoveredStation(hoveredStation);
  }, [hoveredStation, mapLoaded, surveyLayerRef]);

  // Resolve connected segment on hover when Move Segment tool is active.
  const segmentHoverTokenRef = useRef(0);
  useEffect(() => {
    if (!surveyLayerRef.current) return undefined;

    if (activeTool !== 'move_segment') {
      if (logMapInteractions) {
        console.debug('[Highlight] Segment hover cleared (tool inactive)', {
          activeTool,
        });
      }
      surveyLayerRef.current.setHoveredSegment([]);
      return undefined;
    }

    const sectionId = hoveredStation?.section_id ?? hoveredShot?.sectionId ?? null;

    if (sectionId === null) {
      segmentHoverTokenRef.current++;
      if (logMapInteractions) {
        console.debug('[Highlight] Segment hover pending clear (no hovered feature)', {});
      }
      const timer = setTimeout(() => {
        surveyLayerRef.current?.setHoveredSegment([]);
      }, 150);
      return () => clearTimeout(timer);
    }

    if (anchoredSectionIds.has(sectionId)) {
      if (logMapInteractions) {
        console.debug('[Highlight] Segment hover suppressed (anchored section)', {
          sectionId,
        });
      }
      surveyLayerRef.current.setHoveredSegment([]);
      return undefined;
    }

    const token = ++segmentHoverTokenRef.current;
    getConnectedSectionIds(sectionId).then((ids) => {
      if (segmentHoverTokenRef.current !== token) return;
      const movable = ids.filter((id) => !anchoredSectionIds.has(id));
      if (logMapInteractions) {
        console.debug('[Highlight] Segment hover resolved', {
          sectionId,
          connectedCount: ids.length,
          movableCount: movable.length,
        });
      }
      surveyLayerRef.current?.setHoveredSegment(movable);
    }).catch(() => {
      if (segmentHoverTokenRef.current !== token) return;
      if (logMapInteractions) {
        console.debug('[Highlight] Segment hover fallback (IPC failed)', {
          sectionId,
        });
      }
      surveyLayerRef.current?.setHoveredSegment([sectionId]);
    });
    return undefined;
  }, [activeTool, hoveredStation, hoveredShot, anchoredSectionIds, getConnectedSectionIds, mapLoaded, logMapInteractions, surveyLayerRef]);

  return null;
}
