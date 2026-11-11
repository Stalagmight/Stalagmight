/**
 * Screen-space hit-test resolver over the SoA `SurveyData` (#mem).
 *
 * With merged shot/station features (see `features.ts` `mergeFeatures`), the
 * rendered hitbox layers can only tell us WHICH GROUPS (sections) are under
 * the cursor - the per-item identity (`shotId` / `stationIndex`) is no longer
 * a feature property. This module resolves the exact item geometrically:
 * MapLibre's rendered query gates the candidate sections, then we search only
 * those sections' shots/stations in screen space with the same
 * point-to-segment / point-distance math the old per-feature resolvers used
 * (`hitTest.ts`), so hover/click/snap targets and tolerances are identical.
 *
 * Equivalence argument (same pick as the per-feature path): the rendered
 * query returns a group iff at least one member is within the rendered hit
 * area (circle radius / half line width). The geometric argmin over ALL
 * members of returned groups therefore has distance <= that member's, i.e.
 * it lies within the rendered hit area too - and every item within the
 * rendered hit area belongs to a returned group. So the argmin sets coincide.
 * The one deliberate improvement: distances/`t` are computed from the TRUE
 * endpoints instead of tile-clipped rendered geometry, which fixes the
 * documented long-shot clip edge case in `queryNearestShotPoint`.
 *
 * The resolver also mirrors the reference-visibility filters the hitbox
 * layers apply (`isReference` shots / `allShotsReference` stations), so
 * hidden reference legs stay non-interactive.
 */
import type { GpsOrigin } from '../../../types/config';
import type { StationRef } from '../../../types/project';
import {
  type SurveyData,
  type SectionMetadata,
  isVirtualKind,
  shotKindFromFlags,
  stationKindRank,
  STATION_FLAG_IS_ZERO_DATUM,
} from '../../../types/survey-data';
import { metersToMapCoords } from '../../../utils/coordinates';
import { encodeFeatureId } from '../../../utils/ids';
import { distanceToLineSegment, projectParamOnSegment } from './hitTest';

/** Screen projection: [lng, lat] -> pixel coords. Wraps `map.project`. */
export type ProjectFn = (lngLat: [number, number]) => { x: number; y: number };

export interface NearestStationHit {
  ref: StationRef;
  /** Screen-space distance to the station center, px. */
  distance: number;
  /** The station's map coordinates. */
  lngLat: [number, number];
}

export interface NearestShotHit {
  sectionId: number;
  shotId: number;
  /** Screen-space distance to the segment, px. */
  distance: number;
  /** Fractional position of the projected point along the shot (0..1). */
  t: number;
  /** True (unclipped) shot endpoints in map coordinates. */
  from: [lng: number, lat: number];
  to: [lng: number, lat: number];
}

/**
 * Which virtual shot kinds are currently drawn (#27). Hit-testing must agree
 * with what is on screen, so a hidden kind is also non-interactive.
 */
export interface VisibleKinds {
  jump: boolean;
  reference: boolean;
}

/** True when a shot of this kind is currently drawn. */
export function isKindVisible(flags: number, visible: VisibleKinds): boolean {
  const kind = shotKindFromFlags(flags);
  if (kind === 'Jump') return visible.jump;
  if (kind === 'Reference') return visible.reference;
  return true;
}

export interface ResolveOptions {
  /** Mirror of the per-kind visibility toggles: a hidden kind's shots (and
   *  stations touched only by hidden kinds) are skipped, since they are
   *  filtered out of the hitbox layers and must stay non-interactive. */
  visibleKinds: VisibleKinds;
  /** Drawn radius of a station marker at size 1 (disc + stroke), px. When set,
   *  {@link SurveyGeometryIndex.nearestStation} gives a marker whose disc is under
   *  the cursor to the best-ranked station, the one drawn on top. Omitted: plain
   *  nearest. */
  stationDiscPx?: number;
  /** The size multiplier a station's marker is drawn at (the circle shader's:
   *  hover / selection size, else 2 for a zero datum, else 1), scaling
   *  `stationDiscPx`. Omitted: 2 for a zero datum, else 1. */
  stationSizeMul?: (sectionId: number, zeroDatum: boolean) => number;
  /** Drawn half-width of a shot line at width 1, px. When set,
   *  {@link SurveyGeometryIndex.nearestShot} gives a line whose drawn width is
   *  under the cursor to the best-ranked shot, the one drawn on top. Omitted:
   *  plain nearest. */
  shotHalfWidthPx?: number;
  /** The width multiplier a section's lines are drawn at (the line shader's:
   *  hover / selection width, else 1), scaling `shotHalfWidthPx`. Omitted: 1. */
  shotWidthMul?: (sectionId: number) => number;
  /** The section drawn above every rank (the selection's,
   *  `SurveyCustomController.setRaisedSection`): its stations and shots outrank
   *  all others in {@link SurveyGeometryIndex.nearestStation} /
   *  {@link SurveyGeometryIndex.nearestShot}, the way they are drawn. */
  raisedSectionId?: number | null;
}

/** Pick rank of a raised section's features: better than real line (0). */
const RAISED_RANK = -1;

/** Draw rank of a shot kind: 0 Normal, 1 Jump, 2 Reference (the line draw order
 *  and the shot pick; `stationKindRank` is the station counterpart). */
function shotKindRank(flags: number): number {
  const kind = shotKindFromFlags(flags);
  return kind === 'Normal' ? 0 : kind === 'Jump' ? 1 : 2;
}

/**
 * Cheap snapshot of the map camera for {@link SurveyGeometryIndex.sectionsIntersectingBox}'s
 * projected-AABB cache. `key` is any stable string that changes iff `project`
 * would map a given lng/lat to a different pixel (zoom / center / bearing /
 * pitch / viewport). `axisAligned` is true when the projection preserves axis
 * order (bearing 0 and pitch 0), which lets the reprojection use only the two
 * diagonal AABB corners instead of all four - exact under a top-down mercator
 * view, so the conservative-superset contract is unchanged. Omit the snapshot
 * to force a full 4-corner reprojection every call (the legacy behavior).
 */
export interface CameraSnapshot {
  key: string;
  axisAligned: boolean;
}

export class SurveyGeometryIndex {
  private readonly data: SurveyData;
  private readonly gpsOrigin: GpsOrigin | null;

  // Lazily built (first query) so update() flushes stay cheap.
  private built = false;
  private sectionById = new Map<number, SectionMetadata>();
  /** sectionId -> global station indices (iteration order). */
  private sectionStationIdxs = new Map<number, number[]>();
  /** sectionId -> (stationIndex -> global station index). */
  private stationLookup = new Map<number, Map<number, number>>();
  /** Per-section cache: stationIndex values that touch >=1 non-reference shot
   *  (both endpoints present - mirrors the builder's feature-emission guard). */
  private stationKindMaskCache = new Map<number, Map<number, number>>();
  /** Per-section world-coord (map coords) AABB over its stations, stored as flat
   *  parallel arrays (index 0..boundsCount-1) for cache-friendly projection.
   *  Lazily built by {@link ensureBounds} the first time the gate runs. */
  private boundsSid: Uint32Array = new Uint32Array(0);
  private boundsMinLng: Float64Array = new Float64Array(0);
  private boundsMinLat: Float64Array = new Float64Array(0);
  private boundsMaxLng: Float64Array = new Float64Array(0);
  private boundsMaxLat: Float64Array = new Float64Array(0);
  private boundsCount = 0;
  private boundsBuilt = false;
  /** Projected screen-space AABB per section (parallel to the bounds arrays),
   *  reused across probes of the SAME camera. {@link sectionsIntersectingBox}
   *  reprojects only when the caller's camera key changes - during hover the
   *  camera is static, so every probe after the first is pure AABB overlap. */
  private projMinX: Float32Array = new Float32Array(0);
  private projMinY: Float32Array = new Float32Array(0);
  private projMaxX: Float32Array = new Float32Array(0);
  private projMaxY: Float32Array = new Float32Array(0);
  private cachedCameraKey: string | null = null;

  constructor(data: SurveyData, gpsOrigin: GpsOrigin | null) {
    this.data = data;
    this.gpsOrigin = gpsOrigin;
  }

  private ensureBuilt(): void {
    if (this.built) return;
    this.built = true;
    for (const s of this.data.sections) this.sectionById.set(s.id, s);
    for (let g = 0; g < this.data.stationCount; g++) {
      const sid = this.data.stationSectionIds[g]!;
      let idxs = this.sectionStationIdxs.get(sid);
      if (!idxs) {
        idxs = [];
        this.sectionStationIdxs.set(sid, idxs);
      }
      idxs.push(g);
      let inner = this.stationLookup.get(sid);
      if (!inner) {
        inner = new Map();
        this.stationLookup.set(sid, inner);
      }
      inner.set(this.data.stationIndices[g]!, g);
    }
  }

  /**
   * Build the per-section world-coord bounding boxes used by
   * {@link sectionsIntersectingBox}. Kept separate from + lazier than
   * {@link ensureBuilt}: only the candidate-section gate needs projected bounds,
   * so the per-item resolvers must not pay this pass. One metersToMapCoords per
   * station, on the first gate query only.
   */
  private ensureBounds(): void {
    if (this.boundsBuilt) return;
    this.boundsBuilt = true;
    // Accumulate per-section AABBs in a Map, then flatten to parallel arrays.
    const acc = new Map<
      number,
      { minLng: number; minLat: number; maxLng: number; maxLat: number }
    >();
    for (let g = 0; g < this.data.stationCount; g++) {
      const sid = this.data.stationSectionIds[g]!;
      const [lng, lat] = this.stationLngLat(g);
      const b = acc.get(sid);
      if (!b) {
        acc.set(sid, { minLng: lng, minLat: lat, maxLng: lng, maxLat: lat });
        continue;
      }
      if (lng < b.minLng) b.minLng = lng;
      if (lng > b.maxLng) b.maxLng = lng;
      if (lat < b.minLat) b.minLat = lat;
      if (lat > b.maxLat) b.maxLat = lat;
    }
    const n = acc.size;
    this.boundsCount = n;
    this.boundsSid = new Uint32Array(n);
    this.boundsMinLng = new Float64Array(n);
    this.boundsMinLat = new Float64Array(n);
    this.boundsMaxLng = new Float64Array(n);
    this.boundsMaxLat = new Float64Array(n);
    this.projMinX = new Float32Array(n);
    this.projMinY = new Float32Array(n);
    this.projMaxX = new Float32Array(n);
    this.projMaxY = new Float32Array(n);
    let i = 0;
    for (const [sid, b] of acc) {
      this.boundsSid[i] = sid;
      this.boundsMinLng[i] = b.minLng;
      this.boundsMinLat[i] = b.minLat;
      this.boundsMaxLng[i] = b.maxLng;
      this.boundsMaxLat[i] = b.maxLat;
      i++;
    }
    // Force a reprojection on the first query (no camera key matches null yet).
    this.cachedCameraKey = null;
  }

  /**
   * Reproject every section's world-coord AABB into the screen-space cache.
   * With `axisAligned` (bearing 0, pitch 0) the projection preserves axis order,
   * so the two diagonal corners bound the footprint exactly; otherwise all four
   * corners are projected to keep the superset conservative under an affine map.
   */
  private projectBounds(project: ProjectFn, axisAligned: boolean): void {
    for (let i = 0; i < this.boundsCount; i++) {
      const minLng = this.boundsMinLng[i]!;
      const minLat = this.boundsMinLat[i]!;
      const maxLng = this.boundsMaxLng[i]!;
      const maxLat = this.boundsMaxLat[i]!;
      if (axisAligned) {
        const p1 = project([minLng, minLat]);
        const p2 = project([maxLng, maxLat]);
        this.projMinX[i] = Math.min(p1.x, p2.x);
        this.projMaxX[i] = Math.max(p1.x, p2.x);
        this.projMinY[i] = Math.min(p1.y, p2.y);
        this.projMaxY[i] = Math.max(p1.y, p2.y);
      } else {
        const p1 = project([minLng, minLat]);
        const p2 = project([minLng, maxLat]);
        const p3 = project([maxLng, minLat]);
        const p4 = project([maxLng, maxLat]);
        this.projMinX[i] = Math.min(p1.x, p2.x, p3.x, p4.x);
        this.projMaxX[i] = Math.max(p1.x, p2.x, p3.x, p4.x);
        this.projMinY[i] = Math.min(p1.y, p2.y, p3.y, p4.y);
        this.projMaxY[i] = Math.max(p1.y, p2.y, p3.y, p4.y);
      }
    }
  }

  /** Station map coords for a global index. */
  private stationLngLat(g: number): [number, number] {
    const base = g * 3;
    return metersToMapCoords(
      this.data.stationCoords[base]!,
      this.data.stationCoords[base + 1]!,
      this.gpsOrigin,
    );
  }

  /** Which kinds of shot touch each station in `sectionId`, keyed by station
   *  index (#27). Same encoding as the circle shader's `a_all_reference`:
   *  0 = at least one Normal shot, 1 = only Jumps, 2 = only References,
   *  3 = both virtual kinds (also the no-legs default). Toggle-independent, so
   *  it caches per section; the toggles are applied by
   *  [`isStationVisible`](Self.isStationVisible) at query time. */
  private stationKindMask(sectionId: number): Map<number, number> {
    let cached = this.stationKindMaskCache.get(sectionId);
    if (cached) return cached;
    cached = new Map<number, number>();
    const section = this.sectionById.get(sectionId);
    const lookup = this.stationLookup.get(sectionId);
    if (section && lookup) {
      const touch = (idx: number, bit: number) => {
        const prev = cached!.get(idx);
        if (prev === 0) return; // a Normal shot already won
        cached!.set(idx, bit === 0 ? 0 : prev === undefined ? bit : prev | bit);
      };
      for (let i = 0; i < section.shotCount; i++) {
        const shotGlobalIdx = section.shotOffset + i;
        if (lookup.get(i) === undefined || lookup.get(i + 1) === undefined) continue;
        const kind = shotKindFromFlags(this.data.shotFlags[shotGlobalIdx]!);
        const bit = kind === 'Jump' ? 1 : kind === 'Reference' ? 2 : 0;
        touch(i, bit);
        touch(i + 1, bit);
      }
    }
    this.stationKindMaskCache.set(sectionId, cached);
    return cached;
  }

  /** Whether a station is currently drawn, and so hit-testable
   *  ({@link isMaskVisible} of its kind mask). */
  private isStationVisible(
    sectionId: number,
    stationIndex: number,
    visible: VisibleKinds,
  ): boolean {
    return SurveyGeometryIndex.isMaskVisible(this.stationKindMask(sectionId).get(stationIndex) ?? 3, visible);
  }

  /** Whether a station with this kind mask is currently drawn, and so
   *  hit-testable. Mirrors the circle shader exactly: visible while ANY kind
   *  that touches it is visible. A station with no legs at all keeps the
   *  both-off rule. */
  private static isMaskVisible(mask: number, visible: VisibleKinds): boolean {
    if (mask === 0) return true; // a Normal shot touches it
    if ((mask & 1) !== 0 && visible.jump) return true;
    if ((mask & 2) !== 0 && visible.reference) return true;
    return false;
  }

  /**
   * Nearest station (screen space) among the candidate sections, within
   * `maxDistance` px of (px, py). Returns null if none qualifies.
   *
   * Rank (`stationKindRank`: real, then jump, then reference) settles which of
   * several stations takes the pick, the way the circles are drawn (real-line
   * markers on top, `writeCircleDrawOrder`):
   *  - stations at the same distance (coincident markers, e.g. a jump or
   *    reference section tied in at a real station): the best rank;
   *  - with `stationDiscPx`, when the cursor is on the drawn disc of a station
   *    that outranks the nearest one: that station, since its marker covers
   *    the nearer one's there. The visible edge of a worse-ranked marker poking
   *    out from under it still picks the worse-ranked station.
   * Among stations of one rank the nearest wins, as before. The raised
   * section (`opts.raisedSectionId`) outranks every rank: it is drawn on top.
   */
  nearestStation(
    project: ProjectFn,
    px: number,
    py: number,
    sectionIds: Iterable<number>,
    maxDistance: number,
    opts: ResolveOptions,
  ): NearestStationHit | null {
    this.ensureBuilt();
    const disc = opts.stationDiscPx ?? 0;
    const sizeMulOf = opts.stationSizeMul ?? ((_sid: number, zeroDatum: boolean) => (zeroDatum ? 2 : 1));
    let best: NearestStationHit | null = null;
    let bestRank = 0;
    // The best-ranked (then nearest) station whose drawn disc is under the cursor.
    let covering: NearestStationHit | null = null;
    let coveringRank = 0;
    for (const sid of sectionIds) {
      const idxs = this.sectionStationIdxs.get(sid);
      if (!idxs) continue;
      const kinds = opts.visibleKinds;
      const masks = this.stationKindMask(sid);
      for (const g of idxs) {
        const stationIndex = this.data.stationIndices[g]!;
        const mask = masks.get(stationIndex) ?? 3;
        if (!SurveyGeometryIndex.isMaskVisible(mask, kinds)) continue;
        const lngLat = this.stationLngLat(g);
        const p = project(lngLat);
        const dist = Math.hypot(p.x - px, p.y - py);
        if (dist > maxDistance) continue;
        const rank = sid === opts.raisedSectionId ? RAISED_RANK : stationKindRank(mask);
        const nearer = !best || dist < best.distance || (dist === best.distance && rank < bestRank);
        const covers =
          disc > 0 &&
          dist <= disc * sizeMulOf(sid, (this.data.stationFlags[g]! & STATION_FLAG_IS_ZERO_DATUM) !== 0) &&
          (!covering || rank < coveringRank || (rank === coveringRank && dist < covering.distance));
        if (!nearer && !covers) continue;
        const hit: NearestStationHit = { ref: { section_id: sid, station_index: stationIndex }, distance: dist, lngLat };
        if (nearer) {
          best = hit;
          bestRank = rank;
        }
        if (covers) {
          covering = hit;
          coveringRank = rank;
        }
      }
    }
    return covering && coveringRank < bestRank ? covering : best;
  }

  /**
   * Nearest shot segment (screen space) among the candidate sections, within
   * `maxDistance` px of (px, py). Returns null if none qualifies.
   *
   * Rank settles overlaps the way the lines are drawn (Normal on top of Jump on
   * top of Reference, `writeLineDrawOrder`), as {@link nearestStation} does for
   * markers: equal distances go to the better rank, and with `shotHalfWidthPx`
   * a cursor on the drawn width of a better-ranked line picks it over a nearer,
   * worse-ranked one it covers. Among shots of one kind the nearest wins. The
   * raised section's shots outrank every kind, as they are drawn on top.
   */
  nearestShot(
    project: ProjectFn,
    px: number,
    py: number,
    sectionIds: Iterable<number>,
    maxDistance: number,
    opts: ResolveOptions,
  ): NearestShotHit | null {
    this.ensureBuilt();
    const half = opts.shotHalfWidthPx ?? 0;
    let best: NearestShotHit | null = null;
    let bestRank = 0;
    // The best-ranked (then nearest) shot whose drawn width is under the cursor.
    let covering: NearestShotHit | null = null;
    let coveringRank = 0;
    for (const sid of sectionIds) {
      const section = this.sectionById.get(sid);
      const lookup = this.stationLookup.get(sid);
      if (!section || !lookup) continue;
      const reach = half > 0 ? half * (opts.shotWidthMul?.(sid) ?? 1) : 0;
      for (let i = 0; i < section.shotCount; i++) {
        const shotGlobalIdx = section.shotOffset + i;
        const flags = this.data.shotFlags[shotGlobalIdx]!;
        if (!isKindVisible(flags, opts.visibleKinds)) continue;
        const fromGlobal = lookup.get(i);
        const toGlobal = lookup.get(i + 1);
        if (fromGlobal === undefined || toGlobal === undefined) continue;
        const from = this.stationLngLat(fromGlobal);
        const to = this.stationLngLat(toGlobal);
        const p1 = project(from);
        const p2 = project(to);
        const dist = distanceToLineSegment(px, py, p1.x, p1.y, p2.x, p2.y);
        if (dist > maxDistance) continue;
        const rank = sid === opts.raisedSectionId ? RAISED_RANK : shotKindRank(flags);
        const nearer = !best || dist < best.distance || (dist === best.distance && rank < bestRank);
        const covers =
          reach > 0 &&
          dist <= reach &&
          (!covering || rank < coveringRank || (rank === coveringRank && dist < covering.distance));
        if (!nearer && !covers) continue;
        const hit: NearestShotHit = {
          sectionId: sid,
          shotId: this.data.shotIds[shotGlobalIdx]!,
          distance: dist,
          t: projectParamOnSegment(px, py, p1.x, p1.y, p2.x, p2.y),
          from,
          to,
        };
        if (nearer) {
          best = hit;
          bestRank = rank;
        }
        if (covers) {
          covering = hit;
          coveringRank = rank;
        }
      }
    }
    return covering && coveringRank < bestRank ? covering : best;
  }

  /**
   * Sections whose geometry may intersect the screen-space box
   * `[minX, minY, maxX, maxY]` (already padded by the caller to the type's hit
   * reach). Replaces the old `queryRenderedFeatures` candidate-section prefilter
   * (#customRender): once the custom WebGL layer renders the survey, the stock
   * shots/stations layers are hidden (visibility:none) and return nothing from
   * `queryRenderedFeatures`, so candidate discovery must come from the SoA index
   * rather than rendered features.
   *
   * Each section's world-coord AABB is projected corner by corner; the screen
   * AABB of those four corners is a conservative SUPERSET of the section's
   * projected footprint (exact under the app's unrotated top-down projection,
   * a safe over-approximation under any affine map transform). A section is a
   * candidate iff that screen AABB overlaps the probe box. The result is a
   * superset - the per-item resolvers ({@link nearestStation} /
   * {@link nearestShot}) then trim to exact tolerance, so no in-tolerance hit
   * is dropped and none leaks past it.
   */
  sectionsIntersectingBox(
    project: ProjectFn,
    minX: number,
    minY: number,
    maxX: number,
    maxY: number,
    camera?: CameraSnapshot | null,
  ): Set<number> {
    this.ensureBounds();
    // Reproject the section AABBs only when the camera changed. Without a
    // snapshot (unit tests / legacy callers) reproject every call - same result,
    // just uncached. With a matching key, skip straight to the overlap test.
    if (!camera || camera.key !== this.cachedCameraKey) {
      this.projectBounds(project, camera?.axisAligned ?? false);
      this.cachedCameraKey = camera ? camera.key : null;
    }
    const out = new Set<number>();
    for (let i = 0; i < this.boundsCount; i++) {
      if (
        this.projMaxX[i]! >= minX &&
        this.projMinX[i]! <= maxX &&
        this.projMaxY[i]! >= minY &&
        this.projMinY[i]! <= maxY
      ) {
        out.add(this.boundsSid[i]!);
      }
    }
    return out;
  }

  /** Map coords of one station by ref, or null if unknown. Used by the
   *  interact overlay to place individual highlight features. */
  stationCoords(ref: StationRef): [number, number] | null {
    this.ensureBuilt();
    const g = this.stationLookup.get(ref.section_id)?.get(ref.station_index);
    return g === undefined ? null : this.stationLngLat(g);
  }

  /** Global station index by ref, or null. */
  stationGlobalIndex(ref: StationRef): number | null {
    this.ensureBuilt();
    const g = this.stationLookup.get(ref.section_id)?.get(ref.station_index);
    return g === undefined ? null : g;
  }

  /**
   * Endpoints + global index of one shot by (sectionId, shotId), or null.
   * Used by the interact overlay to draw individual shot highlights.
   */
  shotGeometry(
    sectionId: number,
    shotId: number,
  ): {
    from: [number, number];
    to: [number, number];
    globalIdx: number;
    isReference: boolean;
    /** Raw flags byte, so callers can resolve the exact kind (#27). */
    kindFlags: number;
  } | null {
    this.ensureBuilt();
    const section = this.sectionById.get(sectionId);
    const lookup = this.stationLookup.get(sectionId);
    if (!section || !lookup) return null;
    for (let i = 0; i < section.shotCount; i++) {
      const shotGlobalIdx = section.shotOffset + i;
      if (this.data.shotIds[shotGlobalIdx] !== shotId) continue;
      const fromGlobal = lookup.get(i);
      const toGlobal = lookup.get(i + 1);
      if (fromGlobal === undefined || toGlobal === undefined) return null;
      return {
        from: this.stationLngLat(fromGlobal),
        to: this.stationLngLat(toGlobal),
        globalIdx: shotGlobalIdx,
        isReference: isVirtualKind(shotKindFromFlags(this.data.shotFlags[shotGlobalIdx]!)),
        kindFlags: this.data.shotFlags[shotGlobalIdx]!,
      };
    }
    return null;
  }

  /**
   * Endpoints of every shot whose canonical feature id
   * (`encodeFeatureId(sectionId, shotId)`) is in `fids`. The measured along-line
   * path's shot tint (#26) resolves its segments through here.
   *
   * Membership is tested by RE-DERIVING each shot's fid the same way every
   * producer does (`colorResolvers` / distance + gas overlays / the interact
   * overlay), NOT by decoding the fid back to `(section, shot)`. Decoding is lossy
   * for shots: the scoped id folded into the fid is the GLOBAL shot id (already
   * `sectionId * FEATURE_ID_STRIDE + local`), so a `floor(fid / STRIDE)` split
   * mis-attributes every shot in section >= 1 to the wrong section and shot id,
   * dropping its tint. Re-deriving is exact regardless of the shot-id range.
   *
   * `visibleKinds` mirrors the per-kind toggles: legs of a hidden kind are
   * skipped, matching the interact overlay / measure-path filter.
   */
  measuredShotSegments(
    fids: ReadonlySet<number>,
    visibleKinds: VisibleKinds,
  ): Array<{ from: [number, number]; to: [number, number] }> {
    this.ensureBuilt();
    const out: Array<{ from: [number, number]; to: [number, number] }> = [];
    if (fids.size === 0) return out;
    for (const section of this.data.sections) {
      const lookup = this.stationLookup.get(section.id);
      if (!lookup) continue;
      for (let i = 0; i < section.shotCount; i++) {
        const gi = section.shotOffset + i;
        if (!fids.has(encodeFeatureId(section.id, this.data.shotIds[gi]!))) continue;
        if (!isKindVisible(this.data.shotFlags[gi]!, visibleKinds)) continue;
        const fromGlobal = lookup.get(i);
        const toGlobal = lookup.get(i + 1);
        if (fromGlobal === undefined || toGlobal === undefined) continue;
        out.push({ from: this.stationLngLat(fromGlobal), to: this.stationLngLat(toGlobal) });
      }
    }
    return out;
  }

  /**
   * Every station center and shot segment in MAP coords, with identities.
   *
   * The authoritative geometry enumeration for tests/tooling: with the custom
   * WebGL renderer there are no stock `survey-stations` / `survey-shots` layers
   * for `queryRenderedFeatures` to read, so an E2E test that needs on-screen
   * positions projects these instead (see MapCanvas's `__stalagmight_geometry__`
   * hook). Same source of truth the renderer and hit-testing use, so positions
   * agree with what is actually drawn.
   *
   * `visibleKinds` mirrors the per-kind toggles - shots of a hidden kind, and
   * stations touched only by hidden kinds, are omitted exactly as the layers
   * filter them.
   */
  enumerateGeometry(visibleKinds: VisibleKinds): {
    stations: Array<{ sectionId: number; stationIndex: number; lngLat: [number, number] }>;
    shots: Array<{
      sectionId: number;
      shotId: number;
      from: [number, number];
      to: [number, number];
    }>;
  } {
    this.ensureBuilt();
    const stations: Array<{ sectionId: number; stationIndex: number; lngLat: [number, number] }> = [];
    for (let g = 0; g < this.data.stationCount; g++) {
      const sectionId = this.data.stationSectionIds[g]!;
      const stationIndex = this.data.stationIndices[g]!;
      if (!this.isStationVisible(sectionId, stationIndex, visibleKinds)) continue;
      stations.push({ sectionId, stationIndex, lngLat: this.stationLngLat(g) });
    }
    const shots: Array<{
      sectionId: number;
      shotId: number;
      from: [number, number];
      to: [number, number];
    }> = [];
    for (const section of this.data.sections) {
      const lookup = this.stationLookup.get(section.id);
      if (!lookup) continue;
      for (let i = 0; i < section.shotCount; i++) {
        const gi = section.shotOffset + i;
        if (!isKindVisible(this.data.shotFlags[gi]!, visibleKinds)) continue;
        const fromGlobal = lookup.get(i);
        const toGlobal = lookup.get(i + 1);
        if (fromGlobal === undefined || toGlobal === undefined) continue;
        shots.push({
          sectionId: section.id,
          shotId: this.data.shotIds[gi]!,
          from: this.stationLngLat(fromGlobal),
          to: this.stationLngLat(toGlobal),
        });
      }
    }
    return { stations, shots };
  }

  /**
   * One section's stations (ordered by `station_index`) and shots, straight from
   * the SoA - map coords + the global indices the color resolvers need.
   *
   * This exists so the drag-preview capture never touches the lazy `Station[]`
   * proxy or the lazy `section.shots` getter. Doing `stations.filter(s => s.
   * station_ref.section_id === id)` per section materialized the WHOLE ~89k
   * station object graph once per section (quadratic, and on a 14k-section
   * project it pinned a core for minutes), and `shotGeometry()` per shot is a
   * linear scan over the section, so the pair was O(stations x sections) +
   * O(shots^2). Both are linear here.
   */
  sectionGeometry(sectionId: number): {
    stations: Array<{ stationIndex: number; lngLat: [number, number]; globalIdx: number }>;
    shots: Array<{
      /** Position in the section: joins station `index` to `index + 1`. NOT the
       *  array position - shots with a missing endpoint are skipped. */
      index: number;
      shotId: number;
      globalIdx: number;
      isReference: boolean;
      from: [number, number];
      to: [number, number];
    }>;
  } {
    this.ensureBuilt();
    const section = this.sectionById.get(sectionId);
    const lookup = this.stationLookup.get(sectionId);
    if (!section || !lookup) return { stations: [], shots: [] };

    const stations: Array<{ stationIndex: number; lngLat: [number, number]; globalIdx: number }> = [];
    for (const g of this.sectionStationIdxs.get(sectionId) ?? []) {
      stations.push({
        stationIndex: this.data.stationIndices[g]!,
        lngLat: this.stationLngLat(g),
        globalIdx: g,
      });
    }
    // The capture walks stations in station_index order (shot i joins station i
    // to i+1); SoA iteration order is global-index order, which normally matches
    // but is not guaranteed after edits.
    stations.sort((a, b) => a.stationIndex - b.stationIndex);

    const shots: Array<{
      index: number;
      shotId: number;
      globalIdx: number;
      isReference: boolean;
      from: [number, number];
      to: [number, number];
    }> = [];
    for (let i = 0; i < section.shotCount; i++) {
      const gi = section.shotOffset + i;
      const fromGlobal = lookup.get(i);
      const toGlobal = lookup.get(i + 1);
      if (fromGlobal === undefined || toGlobal === undefined) continue;
      shots.push({
        index: i,
        shotId: this.data.shotIds[gi]!,
        globalIdx: gi,
        isReference: isVirtualKind(shotKindFromFlags(this.data.shotFlags[gi]!)),
        from: this.stationLngLat(fromGlobal),
        to: this.stationLngLat(toGlobal),
      });
    }
    return { stations, shots };
  }

  /** Whether every shot touching the station is a reference shot (the
   *  station is filtered out while reference shots are hidden). Stations
   *  with no feature-producing shots at all also count as all-reference,
   *  mirroring the builder's `allShotsReference` property. */
  /** Whether the station is currently DRAWN under `visible`, i.e. hit-testable.
   *  The public form of the per-kind rule the resolvers use internally, so
   *  callers outside the index cannot drift from the circle shader (#27). */
  isStationDrawn(ref: StationRef, visible: VisibleKinds): boolean {
    this.ensureBuilt();
    return this.isStationVisible(ref.section_id, ref.station_index, visible);
  }

  isAllReferenceStation(ref: StationRef): boolean {
    this.ensureBuilt();
    return (this.stationKindMask(ref.section_id).get(ref.station_index) ?? 3) !== 0;
  }

  /** Section metadata by id (color lookup for overlay styling). */
  section(sectionId: number): SectionMetadata | undefined {
    this.ensureBuilt();
    return this.sectionById.get(sectionId);
  }

  /** Whether the station is a zero-datum station. */
  isZeroDatum(ref: StationRef): boolean {
    const g = this.stationGlobalIndex(ref);
    if (g === null) return false;
    return (this.data.stationFlags[g]! & STATION_FLAG_IS_ZERO_DATUM) !== 0;
  }
}
