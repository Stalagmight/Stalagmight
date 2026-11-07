/**
 * The data SurveyLayer pushes into its stock GeoJSON auxiliaries
 * (auxiliaryLayers.ts) (#576 D3 split): the loop-closure indicator lines and
 * their move-tool dimming, the gas route highlight, and the merge-preview ghost
 * section. Each reads only what it is handed.
 */
import type { Map as MaplibreMap } from 'maplibre-gl';
import type * as maplibregl from 'maplibre-gl';
import type { LoopClosure, Station, StationRef } from '../../../types/project';
import type { GpsOrigin } from '../../../types/config';
import type { RouteLeg } from '../../../types/ipc.generated';
import { metersToMapCoords } from '../../../utils/coordinates';
import { getErrorColor } from '../../../utils/colors';
import { FEATURE_ID_STRIDE } from '../../../utils/ids';
import { canWriteStyle, hasLiveStyle } from '../mapUtils';
import { AUX_IDS } from './auxiliaryLayers';
import type { GhostSectionData } from './displayColor';
import type { SurveyGeometryIndex } from './geometryIndex';
import { buildGhostFeatures } from './ghostFeatures';

/**
 * Loop-closure line opacity: 0.25 for a closure either move-tool rule dims - both
 * endpoints in anchored sections (Move Segment), or either endpoint immovable
 * (Move Station / Move Shot) - and 0.8 otherwise. ONE write from both sets: the
 * two rules used to write this paint property separately, so whichever ran last
 * erased the other's dimming - e.g. leaving Move Segment for Move Station cleared
 * the anchored set a render after the immovable set landed, and its plain 0.8
 * undimmed the immovable closures (#576 NF-MAP-4).
 */
export function applyClosureOpacity(
  map: MaplibreMap,
  anchoredSectionIds: ReadonlySet<number>,
  immovableStationFeatureIds: ReadonlySet<number>,
): void {
  // No style during a lost WebGL context, and a rebuilt one refuses writes until
  // it has loaded; replayAfterContextRestore re-applies (#576 RV18-MAP-1).
  if (!canWriteStyle(map)) return;
  const dimRules: maplibregl.ExpressionSpecification[] = [];
  if (anchoredSectionIds.size > 0) {
    const idsArray = [...anchoredSectionIds];
    // Both endpoints must be anchored to gray out the closure line
    dimRules.push(['all',
      ['in', ['get', 'sectionIdA'], ['literal', idsArray]],
      ['in', ['get', 'sectionIdB'], ['literal', idsArray]],
    ] as maplibregl.ExpressionSpecification);
  }
  if (immovableStationFeatureIds.size > 0) {
    // Station key strings matching the closure feature "sectionId:stationIndex" format
    const immovableKeyStrings: string[] = [];
    for (const fid of immovableStationFeatureIds) {
      const sectionId = Math.floor(fid / FEATURE_ID_STRIDE);
      const stationIndex = fid % FEATURE_ID_STRIDE;
      immovableKeyStrings.push(`${sectionId}:${stationIndex}`);
    }
    // Dim closure if either endpoint is immovable
    dimRules.push(['any',
      ['in', ['get', 'stationA'], ['literal', immovableKeyStrings]],
      ['in', ['get', 'stationB'], ['literal', immovableKeyStrings]],
    ] as maplibregl.ExpressionSpecification);
  }
  if (dimRules.length === 0) {
    map.setPaintProperty(AUX_IDS.loopClosuresLayerId, 'line-opacity', 0.8);
    return;
  }
  const dimmed = dimRules.length === 1 ? dimRules[0]! : (['any', ...dimRules] as maplibregl.ExpressionSpecification);
  map.setPaintProperty(AUX_IDS.loopClosuresLayerId, 'line-opacity', [
    'case', dimmed, 0.25, 0.8,
  ] as maplibregl.ExpressionSpecification);
}

/** Build one LineString per route leg (with `isJump` flagged) from `stations` and push them
 *  to the gas-route source. No-op without a live style or the source. */
export function applyGasRoute(
  map: MaplibreMap,
  legs: (RouteLeg & { color: string })[],
  stations: Station[],
  gpsOrigin: GpsOrigin | null,
): void {
  if (!map || !hasLiveStyle(map)) return;
  const source = map.getSource(AUX_IDS.sourceId + '-gas-route');
  if (!source || source.type !== 'geojson') return;
  const geojsonSource = source as maplibregl.GeoJSONSource;
  // Index the stations once (keyed by section:index) so the per-leg endpoint lookups are O(1) -
  // the route can be long and this re-runs after every setData() rebuild, so a linear scan per
  // endpoint would be quadratic on large surveys.
  const coordByRef = new Map<string, [number, number]>();
  for (const s of stations) {
    coordByRef.set(
      `${s.station_ref.section_id}:${s.station_ref.station_index}`,
      metersToMapCoords(s.coordinates.x, s.coordinates.y, gpsOrigin) as [number, number],
    );
  }
  const coordFor = (ref: StationRef): [number, number] | null =>
    coordByRef.get(`${ref.section_id}:${ref.station_index}`) ?? null;
  const features = legs.flatMap((leg) => {
    const from = coordFor(leg.from);
    const to = coordFor(leg.to);
    if (!from || !to) return [];
    return [
      {
        type: 'Feature' as const,
        geometry: { type: 'LineString' as const, coordinates: [from, to] },
        properties: { isJump: leg.is_jump, segmentColor: leg.color },
      },
    ];
  });
  geojsonSource.setData({ type: 'FeatureCollection', features });
}

/**
 * Set ghost section data for merge preview visualization.
 * Pass null to clear the ghost section.
 */
export function applyGhostSection(map: MaplibreMap, data: GhostSectionData | null, gpsOrigin: GpsOrigin | null): void {
  const { shotFeatures, stationFeatures, mergeStationFeatures } = buildGhostFeatures(
    data,
    gpsOrigin
  );

  // Update sources
  const ghostShotsSource = map.getSource(AUX_IDS.sourceId + '-ghost-shots');
  if (ghostShotsSource && 'setData' in ghostShotsSource) {
    (ghostShotsSource as maplibregl.GeoJSONSource).setData({
      type: 'FeatureCollection',
      features: shotFeatures,
    });
  }

  const ghostStationsSource = map.getSource(AUX_IDS.sourceId + '-ghost-stations');
  if (ghostStationsSource && 'setData' in ghostStationsSource) {
    (ghostStationsSource as maplibregl.GeoJSONSource).setData({
      type: 'FeatureCollection',
      features: stationFeatures,
    });
  }

  const ghostMergeSource = map.getSource(AUX_IDS.sourceId + '-ghost-merge');
  if (ghostMergeSource && 'setData' in ghostMergeSource) {
    (ghostMergeSource as maplibregl.GeoJSONSource).setData({
      type: 'FeatureCollection',
      features: mergeStationFeatures,
    });
  }
}

/** Publish the loop-closure indicator lines to their GeoJSON source from the
 *  cached closures + the geometry index. The custom renderer draws everything
 *  else; loop closures stay a small stock GeoJSON overlay, like the gas route. */
export function publishLoopClosures(
  map: MaplibreMap,
  loopClosures: LoopClosure[] | undefined,
  geoIndex: SurveyGeometryIndex | null,
): void {
  const source = map.getSource(AUX_IDS.sourceId + '-loop-closures');
  if (!source || source.type !== 'geojson' || !geoIndex) return;
  const features: GeoJSON.Feature[] = [];
  for (const lc of loopClosures ?? []) {
    const a = geoIndex.stationCoords(lc.station_a);
    const b = geoIndex.stationCoords(lc.station_b);
    if (!a || !b) continue;
    features.push({
      type: 'Feature',
      geometry: { type: 'LineString', coordinates: [a, b] },
      properties: {
        stationA: `${lc.station_a.section_id}:${lc.station_a.station_index}`,
        stationB: `${lc.station_b.section_id}:${lc.station_b.station_index}`,
        sectionIdA: lc.station_a.section_id,
        sectionIdB: lc.station_b.section_id,
        errorPercentage: lc.error_percentage,
        misclosureDistance: lc.misclosure_distance,
        pathLength: lc.path_length,
        color: getErrorColor(lc.error_percentage),
      },
    });
  }
  (source as maplibregl.GeoJSONSource).setData({ type: 'FeatureCollection', features });
}
