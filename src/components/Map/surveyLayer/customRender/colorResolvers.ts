/**
 * Shared per-feature color resolvers for the custom-render build + incremental
 * paths (#customRender). Factored out of the controller's `build()` so the
 * incremental section/station recompute (incremental.ts) resolves colors with
 * the EXACT same closures the full build uses - a prerequisite for the
 * byte-identical incremental guarantee (a moved section rebuilt incrementally
 * must be indistinguishable from a full rebuild of the same geometry).
 *
 * Pure (no GL, no DOM): reads `SurveyData` + a `DisplayColorContext`, returns
 * packed RGBA resolvers keyed by global shot / station index.
 */
import type { SurveyData, SectionMetadata } from '../../../../types/survey-data';
import { encodeFeatureId } from '../../../../utils/ids';
import {
  commentColorForKey,
  resolveShotDisplayColor,
  resolveShotEndpointColors,
  resolveStationDisplayColor,
  resolveStationStrokeColor,
  type DisplayColorContext,
} from '../displayColor';
import { GAS_FAR } from '../../../../utils/gasReach';
import { packCssColor, type Rgba8 } from './colorPack';
import { makeOverlayColor } from './overlayColor';

export interface ColorResolvers {
  /** Flat whole-segment shot color (section-color / severity mode). */
  colorForShot: (shotGlobalIdx: number, sectionId: number) => Rgba8;
  /** Per-endpoint `[from, to]` shot colors (depth-gradient mode). */
  colorForShotEndpoints: (shotGlobalIdx: number, sectionId: number) => readonly [Rgba8, Rgba8];
  fillForStation: (globalIdx: number) => Rgba8;
  strokeForStation: (globalIdx: number) => Rgba8;
  /** True in depth mode: the line builder should take the endpoint resolver so
   *  the GPU interpolates the shot's depth ramp (stock survey-shot-gradients). */
  useDepthEndpoints: boolean;
  /** Section id -> metadata, reused by the incremental diff (section color). */
  sectionById: Map<number, SectionMetadata>;
}

/**
 * Build the packed-color resolvers for one (surveyData, colorCtx) pair. A small
 * append cache parses each distinct CSS color once (the palette is tiny vs the
 * vertex count). The build path and the incremental recompute each call this so
 * their resolved bytes match exactly.
 */
export function makeColorResolvers(
  surveyData: SurveyData,
  colorCtx: DisplayColorContext,
): ColorResolvers {
  const sectionById = new Map<number, SectionMetadata>();
  for (const s of surveyData.sections) sectionById.set(s.id, s);
  const stationDefault = colorCtx.themeColors.stationDefault;

  const packCache = new Map<string, Rgba8>();
  const pack = (css: string): Rgba8 => {
    let v = packCache.get(css);
    if (!v) {
      v = packCssColor(css);
      packCache.set(css, v);
    }
    return v;
  };

  // Analytical overlay (distance / gas-reach): when active it OWNS the shot line
  // color and the station FILL (the stock overlay swaps line-color / circle-color
  // only), resolved per feature from the carried value maps through the shared
  // ramp. The station STROKE and the depth-gradient endpoints yield to it (stock
  // leaves circle-stroke-color untouched and hides the depth sub-layer). Every
  // feature's byte comes from the SAME ramp the stock feature-state paint uses.
  const overlay = colorCtx.overlay ?? null;
  const overlayColor = overlay ? makeOverlayColor(overlay, colorCtx.themeColors.grayLine) : null;

  // Move-comment palette tint (#52): a shot / station that owns a comment paints
  // in its deterministic palette color; everything else keeps its base color.
  // Mirrors the stock `coalesce(['get','commentColor'], base)`; the tint yields
  // to an active overlay (checked first) exactly as `shotsLineColor` does.
  const commentTint = colorCtx.commentTint === true;

  // Plan-mode idle tint (#395 revision): every shot/station paints the gas
  // ramp's flat far color while there is no origin (and hence no `overlay`) yet.
  // Checked after `overlay` (a real field wins outright) and before the comment
  // tint, mirroring the "an active overlay owns color outright" precedence.
  const planIdleColor = !overlay && colorCtx.planIdleTint === true ? pack(GAS_FAR) : null;

  const colorForShot = (shotGlobalIdx: number, sectionId: number): Rgba8 => {
    if (overlay && overlayColor) {
      const fid = encodeFeatureId(sectionId, surveyData.shotIds[shotGlobalIdx]!);
      return overlayColor(overlay.shots.get(fid) ?? -1);
    }
    if (planIdleColor) return planIdleColor;
    if (commentTint && surveyData.shotComments.get(shotGlobalIdx)) {
      return pack(commentColorForKey(`${sectionId}:s${surveyData.shotIds[shotGlobalIdx]!}`));
    }
    const sectionColor = sectionById.get(sectionId)?.color ?? stationDefault;
    return pack(resolveShotDisplayColor(surveyData, sectionColor, shotGlobalIdx, colorCtx));
  };
  const colorForShotEndpoints = (
    shotGlobalIdx: number,
    sectionId: number,
  ): readonly [Rgba8, Rgba8] => {
    // In overlay mode a shot is one flat ramp color (useDepthEndpoints is false),
    // but keep the endpoints coherent if a caller asks for them anyway.
    if (overlay && overlayColor) {
      const c = colorForShot(shotGlobalIdx, sectionId);
      return [c, c];
    }
    if (planIdleColor) return [planIdleColor, planIdleColor];
    const sectionColor = sectionById.get(sectionId)?.color ?? stationDefault;
    const [from, to] = resolveShotEndpointColors(surveyData, sectionColor, shotGlobalIdx, colorCtx);
    return [pack(from), pack(to)];
  };
  const fillForStation = (g: number): Rgba8 => {
    const sectionId = surveyData.stationSectionIds[g]!;
    if (overlay && overlayColor) {
      const fid = encodeFeatureId(sectionId, surveyData.stationIndices[g]!);
      return overlayColor(overlay.stations.get(fid) ?? -1);
    }
    if (planIdleColor) return planIdleColor;
    if (commentTint && surveyData.stationComments.get(g)) {
      return pack(commentColorForKey(`${sectionId}:st${surveyData.stationIndices[g]!}`));
    }
    const sectionColor = sectionById.get(sectionId)?.color ?? stationDefault;
    return pack(resolveStationDisplayColor(surveyData, sectionColor, g, colorCtx));
  };
  const strokeForStation = (g: number): Rgba8 => pack(resolveStationStrokeColor(surveyData.stationFlags[g]!, colorCtx));

  // Overlay mode draws flat per-shot ramp colors, and move-comment hides the
  // depth sub-layer too (applyOverlayPaint), so in both the depth-endpoint path
  // is off and shots draw the flat colorForShot (with the comment tint coalesced).
  const useDepthEndpoints =
    colorCtx.showDepthGradient && !colorCtx.showIssueSeverity && !overlay && !commentTint && !planIdleColor;

  return {
    colorForShot,
    colorForShotEndpoints,
    fillForStation,
    strokeForStation,
    useDepthEndpoints,
    sectionById,
  };
}
