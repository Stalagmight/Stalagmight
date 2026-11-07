/**
 * Hit-testing against the SoA geometry index (#576 D3 split): the candidate
 * sections under a probe point / box, the nearest-station / nearest-shot
 * resolvers, and the ruler's nearest-station / nearest-shot-point queries.
 * SurveyLayer's hover, click and context-menu paths reach these through its own
 * hitCandidateSections / resolveStationAt / resolveShotAt.
 */
import type { Map as MaplibreMap } from 'maplibre-gl';
import type * as maplibregl from 'maplibre-gl';
import type { StationRef } from '../../../types/project';
import { hasLiveStyle } from '../mapUtils';
import type { SurveyGeometryIndex, VisibleKinds } from './geometryIndex';
import { padProbeBox, type ProbeGeometry } from './hitProbe';

/** The SurveyLayer state a hit-test reads, live at each probe. */
export interface SurveyHitTesterHost {
  geoIndex(): SurveyGeometryIndex | null;
  /** The USER stations / shots visibility toggles (not raw layer visibility). */
  stationsVisible(): boolean;
  shotsVisible(): boolean;
  /** Which virtual kinds are drawn (#27). */
  visibleKinds(): VisibleKinds;
  /** Station circle radius / shot half line width of the old hitbox layers. */
  stationHitReach(): number;
  shotHitReach(): number;
}

/** Cheap camera snapshot keying the geometry index's projected-AABB cache.
 *  Keyed by the EXACT center (plus zoom / bearing / pitch / canvas size), so
 *  the key changes on ANY camera move; axisAligned (no bearing/pitch) enables
 *  the 2-corner reprojection fast path. The center must stay full-precision:
 *  quantizing it to pixel bins let two centers in one bin differ by ~1px (Web
 *  Mercator amplifies the latitude error away from the equator), exceeding the
 *  probe's 0.5px margin, so a feature near the hit-radius boundary then missed
 *  hover/click until the camera crossed a bin. */
export function cameraSnapshot(map: MaplibreMap): { key: string; axisAligned: boolean } {
  const c = map.getCenter();
  const cv = map.getCanvas();
  const bearing = map.getBearing();
  const pitch = map.getPitch();
  const key = `${map.getZoom()}|${c.lng}|${c.lat}|${bearing}|${pitch}|${cv.width}|${cv.height}`;
  return { key, axisAligned: bearing === 0 && pitch === 0 };
}

export class SurveyHitTester {
  constructor(
    private readonly map: MaplibreMap,
    private readonly host: SurveyHitTesterHost,
  ) {}

  /**
   * Candidate sections under a point/box, from the SoA geometry index
   * (#customRender / #mem). Candidate discovery no longer depends on stock-layer
   * residency: the endgame renders the survey via the custom WebGL layer, which
   * hides the stock shots/stations layers (visibility:none), and MapLibre returns
   * nothing from hidden layers. So instead of `queryRenderedFeatures` we ask the
   * geometry index for sections whose world-space bounds, projected to screen,
   * intersect the probe box padded by the type's hit reach. That padded-box query
   * is a SUPERSET of the sections whose geometry can be within tolerance; the SoA
   * resolver (`resolveStationAt` / `resolveShotAt`) then trims it to the exact
   * pick. Per-type padding preserves the two old queries' semantics (shot vs
   * station reach differ); the pad is the FULL reach
   * because we test the true geometry, not a thin rendered mark. See
   * `hitProbe.ts` (`padProbeBox`) and `geometryIndex.ts`
   * (`sectionsIntersectingBox`), both unit tested.
   *
   * A type is gated by the USER visibility flags (`stationsVisible` /
   * `shotsVisible`, set by setStationsVisibility / setShotsVisibility): a
   * user-hidden type yields no candidates, exactly as the old query returned
   * nothing for a hidden layer, so clicks fall through to the hidden-feature
   * toast in `handleEmptyClick`. These flags are NOT raw layer visibility: the
   * custom renderer hiding the stock layers (visibility:none) leaves the survey
   * fully interactive, while a user "hide stations/shots" toggle still suppresses
   * hits.
   */
  hitCandidateSections(
    geometry: maplibregl.PointLike | [maplibregl.PointLike, maplibregl.PointLike],
  ): { stationSections: Set<number>; shotSections: Set<number> } {
    let stationSections = new Set<number>();
    let shotSections = new Set<number>();
    const geoIndex = this.host.geoIndex();
    if (!geoIndex || !hasLiveStyle(this.map)) return { stationSections, shotSections };
    const project = this.projector();
    // One camera snapshot for both probes: the geometry index reuses its
    // projected per-section AABBs across probes of the same camera, so a static
    // hover reprojects the 14k+ sections once, not once per probe (#customRender).
    const camera = cameraSnapshot(this.map);
    if (this.host.shotsVisible()) {
      const [[minX, minY], [maxX, maxY]] = padProbeBox(
        geometry as ProbeGeometry,
        this.host.shotHitReach(),
      );
      shotSections = geoIndex.sectionsIntersectingBox(project, minX, minY, maxX, maxY, camera);
    }
    if (this.host.stationsVisible()) {
      const [[minX, minY], [maxX, maxY]] = padProbeBox(
        geometry as ProbeGeometry,
        this.host.stationHitReach(),
      );
      stationSections = geoIndex.sectionsIntersectingBox(project, minX, minY, maxX, maxY, camera);
    }
    return { stationSections, shotSections };
  }

  /** Screen projection closure for the geometry resolver. */
  private projector(): (c: [number, number]) => { x: number; y: number } {
    return (c) => this.map.project(c);
  }

  /** Nearest visible station among candidate sections, within maxDistance px. */
  resolveStationAt(
    x: number,
    y: number,
    sections: Set<number>,
    maxDistance: number,
  ): ReturnType<SurveyGeometryIndex['nearestStation']> {
    const geoIndex = this.host.geoIndex();
    if (sections.size === 0 || !geoIndex) return null;
    return geoIndex.nearestStation(this.projector(), x, y, sections, maxDistance, {
      visibleKinds: this.host.visibleKinds(),
    });
  }

  /** Nearest visible shot among candidate sections, within maxDistance px. */
  resolveShotAt(
    x: number,
    y: number,
    sections: Set<number>,
    maxDistance: number,
  ): ReturnType<SurveyGeometryIndex['nearestShot']> {
    const geoIndex = this.host.geoIndex();
    if (sections.size === 0 || !geoIndex) return null;
    return geoIndex.nearestShot(this.projector(), x, y, sections, maxDistance, {
      visibleKinds: this.host.visibleKinds(),
    });
  }

  /**
   * Query the nearest station to a screen point. The padded visible-layer query
   * (MapLibre's spatial index) gates the candidate sections; the SoA resolver
   * picks the exact station with a circular `threshold` (the old post-check
   * for corner candidates is inherent in the resolver's maxDistance) (#mem).
   */
  queryNearestStation(
    screenX: number,
    screenY: number,
    threshold: number
  ): StationRef | null {
    const bbox: [[number, number], [number, number]] = [
      [screenX - threshold, screenY - threshold],
      [screenX + threshold, screenY + threshold],
    ];
    const { stationSections } = this.hitCandidateSections(bbox);
    const nearest = this.resolveStationAt(screenX, screenY, stationSections, threshold);
    return nearest ? nearest.ref : null;
  }

  /**
   * Nearest shot to a screen point, plus the fractional position `t` ∈ [0,1]
   * along that shot's line (#26 ruler snap-to-shot). The caller interpolates the
   * 3D coords at `t` from the active stations (the 2D shot geometry has no z).
   * Returns null when no shot is within `threshold` pixels (circular - the
   * old corner post-check is inherent in the resolver's maxDistance).
   *
   * The SoA resolver computes `t` and the snap point from the TRUE shot
   * endpoints (#mem), which also fixes the previously-documented edge case
   * where tile-clipped rendered geometry skewed `t` for long shots.
   */
  queryNearestShotPoint(
    screenX: number,
    screenY: number,
    threshold: number
  ): { section_id: number; shot_id: number; t: number; lng: number; lat: number } | null {
    const bbox: [[number, number], [number, number]] = [
      [screenX - threshold, screenY - threshold],
      [screenX + threshold, screenY + threshold],
    ];
    const { shotSections } = this.hitCandidateSections(bbox);
    const nearest = this.resolveShotAt(screenX, screenY, shotSections, threshold);
    if (!nearest) return null;

    const lng = nearest.from[0] + nearest.t * (nearest.to[0] - nearest.from[0]);
    const lat = nearest.from[1] + nearest.t * (nearest.to[1] - nearest.from[1]);

    return {
      section_id: nearest.sectionId,
      shot_id: nearest.shotId,
      t: nearest.t,
      lng,
      lat,
    };
  }
}
