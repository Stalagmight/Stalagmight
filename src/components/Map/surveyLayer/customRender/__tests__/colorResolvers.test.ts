import { describe, expect, it } from 'vitest';
import { makeColorResolvers } from '../colorResolvers';
import { packCssColor } from '../colorPack';
import { commentColorForKey, type DisplayColorContext } from '../../displayColor';
import { emptySurveyData, type SurveyData } from '../../../../../types/survey-data';
import { GAS_FAR } from '../../../../../utils/gasReach';

/**
 * Move-comment palette tint (#52) parity for the custom-render color resolvers.
 * The stock bulk layers coalesce a per-feature `commentColor` PROPERTY over the
 * base color while `move_comment` is active; custom features carry no properties,
 * so `makeColorResolvers` must recompute the SAME deterministic palette color
 * from the survey's comment maps. These pure checks lock that equivalence.
 */

/** One section: shots (shotId 0,1), stations idx 0,1,2. A comment on shot 0 and
 *  on station idx 2 (global 2); everything else is comment-free. */
function fixtureWithComments(): SurveyData {
  const d = emptySurveyData();
  d.sections = [
    { id: 1, name: 'A', color: '#ff0000', direction: 'in' as never, quality: 0, next_shot_id: 3, shotOffset: 0, shotCount: 2 },
  ];
  d.shotCount = 2;
  d.shotIds = new Uint32Array([0, 1]);
  d.shotSectionIds = new Uint32Array([1, 1]);
  d.shotFlags = new Uint8Array([0, 0]);
  d.stationCount = 3;
  d.stationSectionIds = new Uint32Array([1, 1, 1]);
  d.stationIndices = new Uint32Array([0, 1, 2]);
  d.stationCoords = new Float64Array([0, 0, 0, 10, 0, -1, 10, 10, -2]);
  d.stationFlags = new Uint8Array([0, 0, 0]);
  // Shot global 0 (shotId 0) and station global 2 (idx 2) own a comment.
  d.shotComments = new Map([[0, 'shot note']]);
  d.stationComments = new Map([[2, 'station note']]);
  return d;
}

const THEME = {
  stationDefault: '#ffffff',
  zeroDatum: '#00ffff',
  strokeColor: '#000000',
  depthShallow: '#0000ff',
  depthDeep: '#ff0000',
  severityError: '#ff0000',
  severityWarning: '#ffff00',
  severityClear: '#00000000',
  grayLine: '#808080',
};

function ctx(over: Partial<DisplayColorContext> = {}): DisplayColorContext {
  return {
    showDepthGradient: false,
    showIssueSeverity: false,
    depthRange: { min: 0, max: 1 },
    themeColors: THEME as unknown as DisplayColorContext['themeColors'],
    ...over,
  } as DisplayColorContext;
}

describe('move-comment tint in the custom color resolvers', () => {
  it('tints only commented features when commentTint is active; base otherwise', () => {
    const d = fixtureWithComments();
    const base = packCssColor('#ff0000'); // section color, flat mode
    const shotTint = packCssColor(commentColorForKey('1:s0'));
    const stationTint = packCssColor(commentColorForKey('1:st2'));

    // Off: every feature keeps its base (section) color.
    const off = makeColorResolvers(d, ctx({ commentTint: false }));
    expect(off.colorForShot(0, 1)).toEqual(base);
    expect(off.fillForStation(2)).toEqual(base);

    // On: the commented shot / station resolve to their palette color; the
    // comment-free ones stay on base. Stroke is never tinted (stock coalesces
    // circle-color only, not circle-stroke-color).
    const on = makeColorResolvers(d, ctx({ commentTint: true }));
    expect(on.colorForShot(0, 1)).toEqual(shotTint);
    expect(on.colorForShot(1, 1)).toEqual(base);
    expect(on.fillForStation(2)).toEqual(stationTint);
    expect(on.fillForStation(1)).toEqual(base);
    expect(on.strokeForStation(2)).toEqual(off.strokeForStation(2));
  });

  it('disables depth endpoints while the comment tint is active (stock hides the depth sublayer)', () => {
    const d = fixtureWithComments();
    expect(makeColorResolvers(d, ctx({ showDepthGradient: true })).useDepthEndpoints).toBe(true);
    expect(
      makeColorResolvers(d, ctx({ showDepthGradient: true, commentTint: true })).useDepthEndpoints,
    ).toBe(false);
  });

  it('an active overlay owns color and wins over the comment tint', () => {
    const d = fixtureWithComments();
    const overlay = {
      mode: 'distance' as const,
      shots: new Map<number, number>(), // empty -> every shot unreachable (grey)
      stations: new Map<number, number>(),
      max: 100,
    };
    const grey = packCssColor(THEME.grayLine);
    const res = makeColorResolvers(d, ctx({ commentTint: true, overlay }));
    // The commented shot would be tinted, but the overlay resolves it (grey =
    // unreachable) instead: overlay is checked before the comment tint.
    expect(res.colorForShot(0, 1)).toEqual(grey);
    expect(res.fillForStation(2)).toEqual(grey);
  });
});

/**
 * Plan-mode idle tint (#395 revision): before a gas origin is chosen, every
 * feature paints the gas ramp's flat far color instead of section/depth color
 * (dimming is a SEPARATE step, applied via the custom-render dim buffers - not
 * this resolver).
 */
describe('plan-idle tint in the custom color resolvers', () => {
  it('paints every shot/station the far color when active; base otherwise', () => {
    const d = fixtureWithComments();
    const base = packCssColor('#ff0000');
    const far = packCssColor(GAS_FAR);

    const off = makeColorResolvers(d, ctx({ planIdleTint: false }));
    expect(off.colorForShot(0, 1)).toEqual(base);
    expect(off.fillForStation(1)).toEqual(base);

    const on = makeColorResolvers(d, ctx({ planIdleTint: true }));
    expect(on.colorForShot(0, 1)).toEqual(far);
    expect(on.colorForShot(1, 1)).toEqual(far);
    expect(on.fillForStation(0)).toEqual(far);
    expect(on.fillForStation(1)).toEqual(far);
    // Endpoints (depth-mode callers) stay a flat pair of the same far color.
    expect(on.colorForShotEndpoints(0, 1)).toEqual([far, far]);
    // Stroke is untouched (mirrors the gas/distance overlay: stock leaves
    // circle-stroke-color alone).
    expect(on.strokeForStation(0)).toEqual(off.strokeForStation(0));
  });

  it('disables depth endpoints while the idle tint is active', () => {
    const d = fixtureWithComments();
    expect(
      makeColorResolvers(d, ctx({ showDepthGradient: true, planIdleTint: true })).useDepthEndpoints,
    ).toBe(false);
  });

  it('a real overlay wins over the idle tint', () => {
    const d = fixtureWithComments();
    const overlay = {
      mode: 'gasReach' as const,
      shots: new Map<number, number>(), // empty -> every shot unreachable (grey)
      stations: new Map<number, number>(),
      max: 100,
    };
    const grey = packCssColor(THEME.grayLine);
    const res = makeColorResolvers(d, ctx({ planIdleTint: true, overlay }));
    // Station has no entry in the overlay map -> unreachable grey, NOT the idle
    // far tint: the overlay is checked first and owns color outright once active.
    expect(res.fillForStation(0)).toEqual(grey);
  });
});
