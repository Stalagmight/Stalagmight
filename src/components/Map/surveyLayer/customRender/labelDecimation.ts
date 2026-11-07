/**
 * Zoom-band label decimation planning (#customRender labels).
 *
 * The stock shot- and station-label layers thin labels by a zoom-banded
 * `['%', ['get', 'index' | 'globalOrdinal'], N]` filter (SurveyLayer.ts), with
 * the station layer additionally always showing the first station of each
 * section (`isFirstInSection`). Section names and comments are NOT decimated.
 *
 * REVEAL-ZOOM REWORK (deliberate stock-parity break): the stock bands are
 * `5 -> 2 -> 1`, which are NON-NESTED - `{index % 5 == 0}` is NOT a subset of
 * `{index % 2 == 0}` (index 5, 15, 25 ... are kept at z15 but vanish at z16),
 * so the visible set is not monotone in zoom. That non-monotonicity is part of
 * the user's "labels disappear when I zoom in" complaint. We replace the bands
 * with the NESTED power-of-two chain `4 -> 2 -> 1`: `{o % 4 == 0}` ⊆
 * `{o % 2 == 0}` ⊆ `{all}`, so once a label passes decimation it never fails at
 * a higher zoom. 4 is the nested modulus closest to the stock 5 (density 0.25 vs
 * 0.2; the next nested step, 8, would be far too sparse at 0.125).
 *
 * Instead of a per-frame `mod(ordinal, N)` in the shader, decimation is folded
 * into the per-label REVEAL ZOOM (`labelReveal.ts`): {@link decimationAdmitZoom}
 * is the minimum zoom at which a label passes the nested filter, and the reveal
 * pass takes the max of it and the collision-clearance zoom. The shader then
 * only compares the live zoom against the baked `a_reveal_zoom` - one float, no
 * modulus, monotone by construction. See §5/§6 of the design.
 */
import { LABEL_KIND, type LabelKind } from './labelShaders';

/** Zoom at/above which the MID decimation band starts (`% 2`), matching the
 *  stock `[16, 18)` boundary. */
export const DECIM_BAND_MID_ZOOM = 16;
/** Zoom at/above which the FINE band starts (`% 1` = show all), matching the
 *  stock `>= 18` boundary. */
export const DECIM_BAND_FINE_ZOOM = 18;

/**
 * Decimation modulus N for shot / station labels at `zoom`, using the NESTED
 * chain (see the module header - a deliberate break from the non-monotone stock
 * `5 -> 2 -> 1`):
 *   - `[minzoom, 16)` -> 4 (every 4th; nested replacement for the stock 5)
 *   - `[16, 18)`      -> 2 (every 2nd)
 *   - `>= 18`         -> 1 (show all)
 * Below the kind's `minzoom` the kind is hidden (the `u_kind_visible` gate), so
 * the coarse band's modulus (4) is returned there but is moot.
 */
export function shotStationDecimationModulus(zoom: number): number {
  if (zoom < DECIM_BAND_MID_ZOOM) return 4;
  if (zoom < DECIM_BAND_FINE_ZOOM) return 2;
  return 1;
}

/** Whether a kind is decimated at all (only shot + station labels are). */
export function kindDecimates(kind: LabelKind): boolean {
  return kind === LABEL_KIND.SHOT_LABEL || kind === LABEL_KIND.STATION_LABEL;
}

/**
 * Per-kind decimation modulus at `zoom`. Shot + station labels use the nested
 * zoom bands; section names and comments never decimate (modulus 1 = show all).
 */
export function decimationModulusForKind(kind: LabelKind, zoom: number): number {
  return kindDecimates(kind) ? shotStationDecimationModulus(zoom) : 1;
}

/**
 * Nested-decimation TIER of an ordinal (coarser tier = smaller number = appears
 * at a lower zoom):
 *   - tier 0: `o % 4 == 0` -> present from the coarse band (the kind's minzoom)
 *   - tier 1: `o % 2 == 0` (but not `% 4`) -> appears at the mid band (z16)
 *   - tier 2: odd -> appears only at the fine band (z18)
 * The tier is monotone in zoom because the moduli are nested. Used both as the
 * decimation-admission floor ({@link decimationAdmitZoom}) and as the PRIMARY
 * zoom-independent collision-priority key (coarser tier placed first), so a
 * finer-tier label can never evict a coarser one it arrives after.
 */
export function decimationTier(ordinal: number): 0 | 1 | 2 {
  if (ordinal % 4 === 0) return 0;
  if (ordinal % 2 === 0) return 1;
  return 2;
}

/**
 * The minimum zoom at which a label passes decimation - the floor of its reveal
 * zoom (`labelReveal.ts` takes `max(admitZoom, collisionClearanceZoom)`). A
 * first-in-section station label and every non-decimating kind (section names,
 * comments) are admitted at the kind's `minzoom`; otherwise the nested tier sets
 * the band edge: tier 0 -> minzoom, tier 1 -> z16, tier 2 -> z18. Monotone by
 * construction: a higher tier only ever RAISES the floor, never lowers it, so
 * once admitted a label stays admitted at every higher zoom.
 */
export function decimationAdmitZoom(
  kind: LabelKind,
  ordinal: number,
  firstInSection: boolean,
  minzoom: number,
): number {
  if (firstInSection || !kindDecimates(kind)) return minzoom;
  switch (decimationTier(ordinal)) {
    case 0:
      return minzoom;
    case 1:
      return Math.max(minzoom, DECIM_BAND_MID_ZOOM);
    default:
      return Math.max(minzoom, DECIM_BAND_FINE_ZOOM);
  }
}

/**
 * Whether a label is thinned OUT by decimation at a given modulus - the CPU
 * predicate the reveal pass and the nested-monotonicity tests share. A
 * first-in-section station label always survives; a modulus <= 1 shows all.
 * (With the nested moduli this is monotone in zoom: as the modulus drops
 * 4 -> 2 -> 1 the kept set only grows.)
 */
export function isLabelDecimatedOut(ordinal: number, modulus: number, firstInSection: boolean): boolean {
  if (firstInSection) return false;
  if (modulus <= 1.5) return false;
  return ordinal % modulus !== 0;
}
