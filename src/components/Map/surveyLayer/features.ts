import type { GpsOrigin, DecimalPrecision } from '../../../types/config';
import type {
  LoopClosure,
  Marker,
  StationRef,
} from '../../../types/project';
import {
  type SurveyData,
  type SectionMetadata,
  SHOT_FLAG_IS_REFERENCE,
  STATION_FLAG_IS_ZERO_DATUM,
} from '../../../types/survey-data';
import { getDepthColor, getErrorColor, COMMENT_PALETTE } from '../../../utils/colors';
import { metersToMapCoords } from '../../../utils/coordinates';
import { formatFixed } from '../../../i18n/format';
import type { MapThemeColors } from '../../../utils/themeColors';
import { encodeFeatureId } from '../../../utils/ids';

export type DepthRange = { min: number; max: number };

/** FNV-1a 32-bit string hash. Deterministic and fast; used to pick a stable
 *  `COMMENT_PALETTE` slot from a comment's owner identity (#52) - see
 *  `commentColorForKey`. */
function fnv1a32(str: string): number {
  let hash = 0x811c9dc5;
  for (let i = 0; i < str.length; i++) {
    hash ^= str.charCodeAt(i);
    hash = Math.imul(hash, 0x01000193);
  }
  return hash >>> 0;
}

/** Deterministic `COMMENT_PALETTE` color for a comment, keyed by its OWNER
 *  identity rather than build order (#52), so the color stays stable when
 *  other comments are added/removed/moved elsewhere in the survey. Exported
 *  for test assertions; callers should use the section-qualified key shapes
 *  below (`${sectionId}:s${shotId}` for shot comments, `${sectionId}:st${
 *  stationIndex}` for station comments) so two sections can't collide on a
 *  shared local index. */
export function commentColorForKey(key: string): string {
  return COMMENT_PALETTE[fnv1a32(key) % COMMENT_PALETTE.length]!;
}

/**
 * Ghost section data for merge preview visualization.
 */
export interface GhostSectionData {
  /** Shot line coordinates (pairs of [from, to] positions in meters) */
  shots: Array<{ from: { x: number; y: number }; to: { x: number; y: number } }>;
  /** Station positions in meters */
  stations: Array<{ x: number; y: number }>;
  /** Section color */
  color: string;
  /** Merge station highlight position (the station being merged) */
  mergeStation?: { x: number; y: number };
}

export interface GhostFeatureSets {
  shotFeatures: GeoJSON.Feature[];
  stationFeatures: GeoJSON.Feature[];
  mergeStationFeatures: GeoJSON.Feature[];
}

export interface SurveyFeatureSets {
  shotFeatures: GeoJSON.Feature[];
  shotGradientFeatures: GeoJSON.Feature[];
  shotLabelFeatures: GeoJSON.Feature[];
  stationFeatures: GeoJSON.Feature[];
  sectionLabelFeatures: GeoJSON.Feature[];
  stationLabelFeatures: GeoJSON.Feature[];
  loopClosureFeatures: GeoJSON.Feature[];
  markerFeatures: GeoJSON.Feature[];
}

export function normalizeDepthRange(depthRange: DepthRange): DepthRange {
  const minD = Number.isFinite(depthRange.min) ? depthRange.min : 0;
  const maxD = Number.isFinite(depthRange.max) ? depthRange.max : 50;
  return {
    min: Math.min(minD, maxD),
    max: Math.max(minD, maxD),
  };
}

export function buildGhostFeatures(
  data: GhostSectionData | null,
  gpsOrigin: GpsOrigin | null
): GhostFeatureSets {
  const shotFeatures: GeoJSON.Feature[] = [];
  const stationFeatures: GeoJSON.Feature[] = [];
  const mergeStationFeatures: GeoJSON.Feature[] = [];

  if (!data) {
    return { shotFeatures, stationFeatures, mergeStationFeatures };
  }

  for (const shot of data.shots) {
    shotFeatures.push({
      type: 'Feature',
      geometry: {
        type: 'LineString',
        coordinates: [
          metersToMapCoords(shot.from.x, shot.from.y, gpsOrigin),
          metersToMapCoords(shot.to.x, shot.to.y, gpsOrigin),
        ],
      },
      properties: { color: data.color },
    });
  }

  for (const station of data.stations) {
    stationFeatures.push({
      type: 'Feature',
      geometry: {
        type: 'Point',
        coordinates: metersToMapCoords(station.x, station.y, gpsOrigin),
      },
      properties: { color: data.color },
    });
  }

  if (data.mergeStation) {
    mergeStationFeatures.push({
      type: 'Feature',
      geometry: {
        type: 'Point',
        coordinates: metersToMapCoords(data.mergeStation.x, data.mergeStation.y, gpsOrigin),
      },
      properties: {},
    });
  }

  return { shotFeatures, stationFeatures, mergeStationFeatures };
}

/**
 * Options for `buildSurveyFeatures`. Splitting these out keeps the
 * signature flat as new toggles are added.
 */
export interface BuildSurveyFeaturesOptions {
  /** Whether to materialize shot-label and station-label features.
   *  Both layers have `minzoom: 14` (so do their comment-overlay layers
   *  that share the same source), and at fit-to-bounds on a large cave
   *  the camera starts well below that. Skipping the ~160 k label
   *  features when they wouldn't render anyway saves ~100 MB main+worker
   *  on a 75 k-shot project. SurveyLayer rebuilds when the camera
   *  crosses the threshold. Defaults to `true` for callers that don't
   *  care (tests, etc.). */
  buildLabels?: boolean;
  /** Issue-severity coloring mode (#22). When true, shots and stations are
   *  colored by validation severity (green/yellow/red) instead of depth /
   *  section color. Mutually exclusive with `showDepthGradient` (the caller
   *  guarantees depth is off in this mode). */
  showIssueSeverity?: boolean;
  /** Per global shot/station severity (0=clear, 1=warning, 2=error), aligned
   *  to this `SurveyData`. Produced by `computeIssueSeverity`. Required when
   *  `showIssueSeverity` is true. */
  issueSeverity?: { shotSeverity: Uint8Array; stationSeverity: Uint8Array } | undefined;
}

/**
 * SoA-based feature builder. Reads from the typed-array `SurveyData`
 * layout instead of iterating `Section.shots[]` / `Station[]` objects.
 * This is what `SurveyLayer.update()` and the features worker call
 * after Phase A.4.
 *
 * On a 75k-shot project the input no longer involves materialized JS
 * objects per shot/station — the worker receives ~10 transferable
 * typed-array buffers + a small `Map`/`SectionMetadata[]` envelope.
 */
export function buildSurveyFeatures(
  data: SurveyData,
  loopClosures: LoopClosure[] | undefined,
  gpsOrigin: GpsOrigin | null,
  showDepthGradient: boolean,
  depthRange: DepthRange,
  themeColors: MapThemeColors,
  precision?: DecimalPrecision,
  options: BuildSurveyFeaturesOptions = {},
  markers?: Marker[],
): SurveyFeatureSets {
  const buildLabels = options.buildLabels ?? true;
  // Issue-severity coloring (#22). `sevShot`/`sevStation` are aligned to the
  // global shot/station indices iterated below. `sevColor` maps 2→red, 1→yellow,
  // else→green. The caller guarantees `showDepthGradient` is false in this mode.
  const showIssueSeverity = (options.showIssueSeverity ?? false) && !!options.issueSeverity;
  const sevShot = options.issueSeverity?.shotSeverity;
  const sevStation = options.issueSeverity?.stationSeverity;
  const sevColor = (s: number | undefined): string =>
    s === 2
      ? themeColors.severityError
      : s === 1
        ? themeColors.severityWarning
        : themeColors.severityClear;
  // Theme colors (resolved from CSS variables by the caller)
  const depthShallow = themeColors.depthShallow;
  const depthDeep = themeColors.depthDeep;
  const labelColor = themeColors.labelColor;
  const zeroDatum = themeColors.zeroDatum;
  const strokeColor = themeColors.strokeColor;
  const sectionDefault = themeColors.stationDefault;
  const shotFeatures: GeoJSON.Feature[] = [];
  const shotGradientFeatures: GeoJSON.Feature[] = [];
  const shotLabelFeatures: GeoJSON.Feature[] = [];
  const stationFeatures: GeoJSON.Feature[] = [];
  const sectionLabelFeatures: GeoJSON.Feature[] = [];
  const stationLabelFeatures: GeoJSON.Feature[] = [];
  const loopClosureFeatures: GeoJSON.Feature[] = [];
  const markerFeatures: GeoJSON.Feature[] = [];

  // Shot coord lookup for marker position interpolation: "sectionId:shotId" → from/to XYZ.
  // Populated during the shot loop below and consumed when building marker features.
  const shotCoordLookup = markers && markers.length > 0
    ? new Map<string, { from: { x: number; y: number; z: number }; to: { x: number; y: number; z: number } }>()
    : null;

  // (sectionId, stationIndex) → global station index. Built once,
  // O(stationCount). Replaces the legacy `stationsBySection` sort.
  const stationLookup = new Map<number, Map<number, number>>();
  for (let g = 0; g < data.stationCount; g++) {
    const sid = data.stationSectionIds[g]!;
    const stIdx = data.stationIndices[g]!;
    let inner = stationLookup.get(sid);
    if (!inner) {
      inner = new Map();
      stationLookup.set(sid, inner);
    }
    inner.set(stIdx, g);
  }

  // sectionId → SectionMetadata. Built once, O(sections). The
  // station-features loop below runs ~stationCount times and was
  // previously calling `data.sections.find(...)` per iteration,
  // turning the build into O(stationCount × sectionCount). For the
  // 92.stalx fixture (89 k stations × 14 k sections) that was
  // ~1.3 G comparisons inside the MapLibre features worker — visible
  // as delayed first paint.
  const sectionMetaById = new Map<number, SectionMetadata>();
  for (const s of data.sections) sectionMetaById.set(s.id, s);

  /** Read station (x, y, z) from the SoA. Returns null if invalid global idx. */
  const stationXYZ = (gIdx: number): { x: number; y: number; z: number } => {
    const base = gIdx * 3;
    return {
      x: data.stationCoords[base]!,
      y: data.stationCoords[base + 1]!,
      z: data.stationCoords[base + 2]!,
    };
  };

  // Track stations that have at least one non-reference shot
  // (key: "sectionId:stationIndex").
  const stationsWithNonReferenceShot = new Set<string>();

  let globalShotOrdinal = 0; // for shot-label `index` property (matches legacy semantics)
  // Move-comment palette (#52): `stationCommentCarry` carries a station's
  // comment text + assigned color from the (always-built) station-circle loop
  // into the (zoom-gated) station-label loop below, so `data.stationComments`
  // is read once per station rather than twice. Only populated when
  // `buildLabels` is true - the label loop is its only reader.
  const stationCommentCarry = new Map<number, { comment: string; color: string }>();

  for (const section of data.sections) {
    const innerStationMap = stationLookup.get(section.id);
    if (!innerStationMap) continue;

    for (let i = 0; i < section.shotCount; i++) {
      const shotGlobalIdx = section.shotOffset + i;
      const fromGlobal = innerStationMap.get(i);
      const toGlobal = innerStationMap.get(i + 1);
      if (fromGlobal === undefined || toGlobal === undefined) continue;

      const shotId = data.shotIds[shotGlobalIdx]!;
      const isReference =
        (data.shotFlags[shotGlobalIdx]! & SHOT_FLAG_IS_REFERENCE) !== 0;
      const azimuth_in = data.shotAzimuthIn[shotGlobalIdx]!;
      const length = data.shotLength[shotGlobalIdx]!;
      const depth_in = data.shotDepthIn[shotGlobalIdx]!;
      const depth_out = data.shotDepthOut[shotGlobalIdx]!;

      if (!isReference) {
        stationsWithNonReferenceShot.add(`${section.id}:${i}`);
        stationsWithNonReferenceShot.add(`${section.id}:${i + 1}`);
      }

      const avgDepth = (depth_in + depth_out) / 2;
      const depthColor = showDepthGradient
        ? getDepthColor(avgDepth, depthShallow, depthDeep, depthRange.min, depthRange.max)
        : null;
      const shotColor = showIssueSeverity
        ? sevColor(sevShot![shotGlobalIdx]!)
        : (depthColor ?? section.color);
      const shotFeatureId = encodeFeatureId(section.id, shotId);

      const fromXYZ = stationXYZ(fromGlobal);
      const toXYZ = stationXYZ(toGlobal);

      const shotComment = data.shotComments.get(shotGlobalIdx);
      const shotCommentColor = shotComment
        ? commentColorForKey(`${section.id}:s${shotId}`)
        : undefined;

      const shotProps: Record<string, unknown> = {
        featureId: shotFeatureId,
        sectionId: section.id,
        shotId,
        color: shotColor,
        isReference,
      };
      if (shotCommentColor) shotProps.commentColor = shotCommentColor;

      if (shotCoordLookup) {
        shotCoordLookup.set(`${section.id}:${shotId}`, { from: fromXYZ, to: toXYZ });
      }

      shotFeatures.push({
        type: 'Feature',
        geometry: {
          type: 'LineString',
          coordinates: [
            metersToMapCoords(fromXYZ.x, fromXYZ.y, gpsOrigin),
            metersToMapCoords(toXYZ.x, toXYZ.y, gpsOrigin),
          ],
        },
        properties: shotProps,
      });

      if (showDepthGradient) {
        const segmentCount = Math.max(4, Math.min(12, Math.ceil(length / 2)));
        const dx = toXYZ.x - fromXYZ.x;
        const dy = toXYZ.y - fromXYZ.y;
        const depthDelta = depth_out - depth_in;

        for (let segmentIndex = 0; segmentIndex < segmentCount; segmentIndex++) {
          const startT = segmentIndex / segmentCount;
          const endT = (segmentIndex + 1) / segmentCount;
          const midT = (startT + endT) / 2;

          const segmentDepth = depth_in + depthDelta * midT;
          const segmentColor =
            getDepthColor(segmentDepth, depthShallow, depthDeep, depthRange.min, depthRange.max) ??
            shotColor;

          shotGradientFeatures.push({
            type: 'Feature',
            geometry: {
              type: 'LineString',
              coordinates: [
                metersToMapCoords(fromXYZ.x + dx * startT, fromXYZ.y + dy * startT, gpsOrigin),
                metersToMapCoords(fromXYZ.x + dx * endT, fromXYZ.y + dy * endT, gpsOrigin),
              ],
            },
            properties: {
              featureId: shotFeatureId,
              sectionId: section.id,
              shotId,
              segmentIndex,
              color: segmentColor,
              isReference,
            },
          });
        }
      }

      // Shot-label feature (zoom-gated; T3a)
      if (buildLabels) {
        const midX = (fromXYZ.x + toXYZ.x) / 2;
        const midY = (fromXYZ.y + toXYZ.y) / 2;
        const labelDepth = (fromXYZ.z + toXYZ.z) / 2;

        const label = `${formatFixed(azimuth_in, undefined, precision?.azimuth ?? 0)}° ${formatFixed(length, undefined, precision?.length ?? 1)}m ${formatFixed(labelDepth, undefined, precision?.depth ?? 1)}m`;

        const shotLabelDepthColor = showDepthGradient
          ? getDepthColor(avgDepth, depthShallow, depthDeep, depthRange.min, depthRange.max)
          : null;
        const shotLabelColor = shotLabelDepthColor ?? labelColor;

        const shotLabelProps: Record<string, unknown> = {
          label,
          index: globalShotOrdinal,
          sectionId: section.id,
          shotId,
          color: shotLabelColor,
          isReference,
        };
        if (shotComment) {
          shotLabelProps.comment = shotComment;
          if (shotCommentColor) shotLabelProps.commentColor = shotCommentColor;
        }
        shotLabelFeatures.push({
          type: 'Feature',
          geometry: {
            type: 'Point',
            coordinates: metersToMapCoords(midX, midY, gpsOrigin),
          },
          properties: shotLabelProps,
        });
      }

      globalShotOrdinal++;
    }
  }

  // ─── Station features (per-station circles) ────────────────────
  for (let g = 0; g < data.stationCount; g++) {
    const sectionId = data.stationSectionIds[g]!;
    const stationIndex = data.stationIndices[g]!;
    const section = sectionMetaById.get(sectionId);
    const sectionColor = section?.color ?? sectionDefault;
    const isZeroDatumStation = (data.stationFlags[g]! & STATION_FLAG_IS_ZERO_DATUM) !== 0;
    const xyz = stationXYZ(g);

    let color: string;
    if (showIssueSeverity) {
      // Severity fill overrides depth/section/zero-datum coloring; the
      // zero-datum size/stroke below is preserved.
      color = sevColor(sevStation![g]!);
    } else if (showDepthGradient) {
      color =
        getDepthColor(xyz.z, depthShallow, depthDeep, depthRange.min, depthRange.max) ??
        sectionColor;
    } else {
      color = isZeroDatumStation ? zeroDatum : sectionColor;
    }

    const featureStrokeColor = showDepthGradient && isZeroDatumStation ? zeroDatum : strokeColor;
    const featureId = encodeFeatureId(sectionId, stationIndex);

    const stationComment = data.stationComments.get(g);
    const stationCommentColor = stationComment
      ? commentColorForKey(`${sectionId}:st${stationIndex}`)
      : undefined;
    if (buildLabels && stationComment) {
      stationCommentCarry.set(g, { comment: stationComment, color: stationCommentColor! });
    }

    const stationProps: Record<string, unknown> = {
      featureId,
      sectionId,
      stationIndex,
      isZeroDatum: isZeroDatumStation,
      color,
      strokeColor: featureStrokeColor,
      allShotsReference: !stationsWithNonReferenceShot.has(`${sectionId}:${stationIndex}`),
    };
    if (stationCommentColor) stationProps.commentColor = stationCommentColor;

    stationFeatures.push({
      type: 'Feature',
      geometry: {
        type: 'Point',
        coordinates: metersToMapCoords(xyz.x, xyz.y, gpsOrigin),
      },
      properties: stationProps,
    });
  }

  // ─── Section labels ────────────────────────────────────────────
  for (const section of data.sections) {
    const innerStationMap = stationLookup.get(section.id);
    if (!innerStationMap || innerStationMap.size === 0) continue;

    // Prefer the first non-reference station for label placement so it
    // doesn't float mid-air when reference shots are hidden.
    let anchorGlobalIdx: number | undefined;
    let anchorStationIdx = Infinity;
    let foundNonRef = false;
    for (const [stIdx, gIdx] of innerStationMap.entries()) {
      const hasNonRef = stationsWithNonReferenceShot.has(`${section.id}:${stIdx}`);
      // Once we see a non-reference station, only consider those.
      if (foundNonRef && !hasNonRef) continue;
      if (hasNonRef && !foundNonRef) {
        foundNonRef = true;
        anchorStationIdx = Infinity; // restart min search among non-ref
      }
      if (stIdx < anchorStationIdx) {
        anchorStationIdx = stIdx;
        anchorGlobalIdx = gIdx;
      }
    }
    if (anchorGlobalIdx === undefined) continue;

    const xyz = stationXYZ(anchorGlobalIdx);
    const allStationsReference = !foundNonRef;
    sectionLabelFeatures.push({
      type: 'Feature',
      geometry: {
        type: 'Point',
        coordinates: metersToMapCoords(xyz.x, xyz.y, gpsOrigin),
      },
      properties: {
        name: section.name,
        color: section.color,
        sectionId: section.id,
        allStationsReference,
      },
    });
  }

  // ─── Station labels (zoom-gated; T3a) ─────────────────────────
  if (buildLabels) {
    // Per-section min station_index (== "is first in section" predicate)
    const sectionMinIndices = new Map<number, number>();
    for (let g = 0; g < data.stationCount; g++) {
      const sid = data.stationSectionIds[g]!;
      const idx = data.stationIndices[g]!;
      const cur = sectionMinIndices.get(sid);
      if (cur === undefined || idx < cur) sectionMinIndices.set(sid, idx);
    }

    for (let g = 0; g < data.stationCount; g++) {
      const sectionId = data.stationSectionIds[g]!;
      const stationIndex = data.stationIndices[g]!;
      const isFirstInSection = stationIndex === sectionMinIndices.get(sectionId);
      const xyz = stationXYZ(g);

      const stationLabelDepthColor = showDepthGradient
        ? getDepthColor(xyz.z, depthShallow, depthDeep, depthRange.min, depthRange.max)
        : null;
      const stationLabelColor = stationLabelDepthColor ?? labelColor;

      const stationLabelProps: Record<string, unknown> = {
        label: String(stationIndex),
        stationIndex,
        sectionId,
        globalOrdinal: g,
        isFirstInSection,
        color: stationLabelColor,
        allShotsReference: !stationsWithNonReferenceShot.has(`${sectionId}:${stationIndex}`),
      };
      const carriedComment = stationCommentCarry.get(g);
      if (carriedComment) {
        stationLabelProps.comment = carriedComment.comment;
        stationLabelProps.commentColor = carriedComment.color;
      }
      stationLabelFeatures.push({
        type: 'Feature',
        geometry: {
          type: 'Point',
          coordinates: metersToMapCoords(xyz.x, xyz.y, gpsOrigin),
        },
        properties: stationLabelProps,
      });
    }
  }

  // ─── Loop closures ────────────────────────────────────────────
  if (loopClosures && loopClosures.length > 0) {
    /** Look up station coords via (sectionId, stationIndex) — used by
     *  loop-closure endpoints that reference stations by ref. */
    const findStationXYZByRef = (ref: StationRef): { x: number; y: number; z: number } | undefined => {
      const gIdx = stationLookup.get(ref.section_id)?.get(ref.station_index);
      return gIdx === undefined ? undefined : stationXYZ(gIdx);
    };

    for (const closure of loopClosures) {
      const a = findStationXYZByRef(closure.station_a);
      const b = findStationXYZByRef(closure.station_b);
      if (a && b) {
        loopClosureFeatures.push({
          type: 'Feature',
          geometry: {
            type: 'LineString',
            coordinates: [
              metersToMapCoords(a.x, a.y, gpsOrigin),
              metersToMapCoords(b.x, b.y, gpsOrigin),
            ],
          },
          properties: {
            stationA: `${closure.station_a.section_id}:${closure.station_a.station_index}`,
            stationB: `${closure.station_b.section_id}:${closure.station_b.station_index}`,
            sectionIdA: closure.station_a.section_id,
            sectionIdB: closure.station_b.section_id,
            errorPercentage: closure.error_percentage,
            misclosureDistance: closure.misclosure_distance,
            pathLength: closure.path_length,
            color: getErrorColor(closure.error_percentage),
          },
        });
      }
    }
  }

  // ─── Markers ──────────────────────────────────────────────────
  if (markers && markers.length > 0 && shotCoordLookup) {
    for (const marker of markers) {
      const coords = shotCoordLookup.get(`${marker.section_id}:${marker.shot_id}`);
      if (!coords) continue;
      const t = marker.position / 100;
      const x = coords.from.x + (coords.to.x - coords.from.x) * t;
      const y = coords.from.y + (coords.to.y - coords.from.y) * t;
      markerFeatures.push({
        type: 'Feature',
        geometry: {
          type: 'Point',
          coordinates: metersToMapCoords(x, y, gpsOrigin),
        },
        properties: {
          markerId: Number(marker.id),
          sectionId: marker.section_id,
          shotId: marker.shot_id,
          color: marker.color,
          markerType: marker.marker_type,
          text: marker.text,
          direction: marker.direction ?? null,
        },
      });
    }
  }

  return {
    shotFeatures,
    shotGradientFeatures,
    shotLabelFeatures,
    stationFeatures,
    sectionLabelFeatures,
    stationLabelFeatures,
    loopClosureFeatures,
    markerFeatures,
  };
}

