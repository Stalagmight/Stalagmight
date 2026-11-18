// Grabbed-feature highlight for the custom drag ghost (#mem drag latency).
//
// When a move drag picks up a feature, the interact overlay used to draw that
// feature's hover/selection highlight and follow it with a per-frame async
// GeoJSON setData - which visibly lagged the synchronous custom ghost. Instead
// we emit the grabbed feature's highlight INTO the ghost buffers (a larger disc /
// wider capsule drawn on top), so it shares the ghost's u_offset_px and tracks
// the cursor exactly in tandem. This module holds the pure "which feature is
// highlighted + at what style" logic (mirroring refreshInteractOverlay's hover/
// selection precedence + anchored/immovable/visibility gating), reusing
// SurveyLayer's existing interact-overlay feature builders for geometry + color
// so the highlight is pixel-consistent with what the overlay drew.

import type { MovingEnd } from './dragPreviewSource';
import type { Rgba8 } from './customRender/colorPack';
import type {
  GhostHighlights,
  GhostShotHighlight,
  GhostStationHighlight,
} from './customRender/ghostTessellate';

// Interact-overlay station stroke widths (stationOverlayFeature) over the ghost's
// base stroke (2.5px): hover ring is thicker, selection ring matches the base.
const HOVER_STROKE_MUL = 4 / 2.5;
const SEL_STROKE_MUL = 2.5 / 2.5;

/** Everything the pure highlight build needs from SurveyLayer. Geometry + color
 *  come from the existing overlay-feature builders (dragOverlayOffset is 0 at
 *  drag start, so they return un-offset coords the ghost then offsets by uniform). */
export interface GhostHighlightHost {
  hoveredShotId: string | null;
  selectedShotId: string | null;
  hoveredStationId: string | null;
  selectedStationId: string | null;
  shotsVisible: boolean;
  stationsVisible: boolean;
  /** Ghost base sizes the highlight multiplies against (selection width/radius). */
  selShotWidth: number;
  hoverShotWidth: number;
  selRadius: number;
  hoverRadius: number;
  hoverStroke: Rgba8;
  selStroke: Rgba8;
  /** movingEnd for a moving shot, or undefined if the id is not in the moving set. */
  movingEndForShot: (id: string) => MovingEnd | undefined;
  isMovingStation: (id: string) => boolean;
  parseFid: (id: string) => number | null;
  /** Anchored check keyed by the feature's "lineId:scopedId" id, not its fid:
   *  a shot fid is not decodable back to a line id (#301). */
  isAnchored: (id: string) => boolean;
  isShotImmovable: (fid: number) => boolean;
  isStationImmovable: (fid: number) => boolean;
  /** Existing SurveyLayer.shotOverlayFeature / stationOverlayFeature. */
  shotFeature: (id: string, width: number) => GeoJSON.Feature | null;
  stationFeature: (id: string, style: 'hover' | 'selected') => GeoJSON.Feature | null;
  /** Resolve a feature's fill/line color from overlay-feature props (base display
   *  color, or the analytical ramp in distance/gas mode) - same as the ghost base. */
  colorFromProps: (props: Record<string, unknown>) => Rgba8;
}

function shotPrim(host: GhostHighlightHost, id: string, width: number, movingEnd: MovingEnd): GhostShotHighlight | null {
  const f = host.shotFeature(id, width);
  if (!f || f.geometry.type !== 'LineString') return null;
  const coords = f.geometry.coordinates as [number, number][];
  const props = (f.properties ?? {}) as Record<string, unknown>;
  return {
    a: coords[0]!,
    b: coords[coords.length - 1]!,
    movingEnd,
    color: host.colorFromProps(props),
    opacity: typeof props.opacity === 'number' ? props.opacity : 1,
    widthMul: width / host.selShotWidth,
  };
}

function stationPrim(host: GhostHighlightHost, id: string, style: 'hover' | 'selected'): GhostStationHighlight | null {
  const f = host.stationFeature(id, style);
  if (!f || f.geometry.type !== 'Point') return null;
  const props = (f.properties ?? {}) as Record<string, unknown>;
  const hover = style === 'hover';
  return {
    coord: f.geometry.coordinates as [number, number],
    movingEnd: 'uniform',
    fill: host.colorFromProps(props),
    stroke: hover ? host.hoverStroke : host.selStroke,
    opacity: typeof props.opacity === 'number' ? props.opacity : 1,
    sizeMul: (hover ? host.hoverRadius : host.selRadius) / host.selRadius,
    strokeMul: hover ? HOVER_STROKE_MUL : SEL_STROKE_MUL,
  };
}

/**
 * Highlight prims for the grabbed feature(s): the hovered/selected shot + station
 * that are in the moving set, gated exactly like refreshInteractOverlay (hover
 * wins over selection on the same feature; no highlight for anchored/immovable
 * features or a hidden kind). Resolved once at drag start.
 */
export function buildGhostHighlights(host: GhostHighlightHost): GhostHighlights {
  const shots: GhostShotHighlight[] = [];
  const stations: GhostStationHighlight[] = [];

  const hShot = host.hoveredShotId;
  const sShot = host.selectedShotId !== host.hoveredShotId ? host.selectedShotId : null;
  const hShotMe = hShot ? host.movingEndForShot(hShot) : undefined;
  if (host.shotsVisible && hShot && hShotMe) {
    const fid = host.parseFid(hShot);
    if (fid === null || (!host.isAnchored(hShot) && !host.isShotImmovable(fid))) {
      const p = shotPrim(host, hShot, host.hoverShotWidth, hShotMe);
      if (p) shots.push(p);
    }
  }
  const sShotMe = sShot ? host.movingEndForShot(sShot) : undefined;
  if (host.shotsVisible && sShot && sShotMe) {
    const fid = host.parseFid(sShot);
    if (fid === null || (!host.isAnchored(sShot) && !host.isShotImmovable(fid))) {
      const p = shotPrim(host, sShot, host.selShotWidth, sShotMe);
      if (p) shots.push(p);
    }
  }

  const hSt = host.hoveredStationId;
  const sSt = host.selectedStationId !== host.hoveredStationId ? host.selectedStationId : null;
  if (host.stationsVisible && hSt && host.isMovingStation(hSt)) {
    const fid = host.parseFid(hSt);
    if (fid === null || (!host.isAnchored(hSt) && !host.isStationImmovable(fid))) {
      const p = stationPrim(host, hSt, 'hover');
      if (p) stations.push(p);
    }
  }
  if (host.stationsVisible && sSt && host.isMovingStation(sSt)) {
    const fid = host.parseFid(sSt);
    if (fid === null || (!host.isAnchored(sSt) && !host.isStationImmovable(fid))) {
      const p = stationPrim(host, sSt, 'selected');
      if (p) stations.push(p);
    }
  }

  return { stations, shots };
}
