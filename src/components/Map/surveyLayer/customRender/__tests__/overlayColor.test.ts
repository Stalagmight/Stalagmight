import { describe, expect, it } from 'vitest';
import { emptySurveyData, type SurveyData } from '../../../../../types/survey-data';
import { DISTANCE_NEAR, DISTANCE_FAR } from '../../../../../utils/distanceGradient';
import { GAS_NEAR, GAS_FAR, BEYOND_GAS_RANGE } from '../../../../../utils/gasReach';
import { encodeFeatureId } from '../../../../../utils/ids';
import type { DisplayColorContext, OverlayColorInput } from '../../displayColor';
import { packCssColor, type Rgba8 } from '../colorPack';
import { makeOverlayColor, packRampColor, overlayInputFrom } from '../overlayColor';
import { makeColorResolvers } from '../colorResolvers';

/**
 * Analytical-overlay colors for the custom renderer (#34 distance, #38 gas).
 * The stock bulk layers color these with a data-driven feature-state ramp;
 * MapLibre evaluates that ramp per feature on the CPU and packs it with
 * `Math.floor(255 * channel)`. The custom layer resolves the SAME color at
 * build time, so these tests assert byte-identity against a faithful replica of
 * MapLibre's exact float path (the oracle below) - the strongest parity claim.
 */

// ─── MapLibre data-driven color oracle ─────────────────────────────────────
// Mirrors parseCssColor (hex/255), interpolate(['linear']) and packColor
// (floor(255*channel)) op-for-op. The endpoint short-circuits match `interpolate`
// evaluate returning outputs[0]/[last] directly at the stops.
const parseHex01 = (hex: string): number => parseInt(hex.padEnd(2, hex), 16) / 255;
function parse01(css: string): [number, number, number, number] {
  const h = css.slice(1);
  return [parseHex01(h.slice(0, 2)), parseHex01(h.slice(2, 4)), parseHex01(h.slice(4, 6)), 1];
}
const floorByte = (x: number): number => Math.min(Math.max(0, Math.floor(255 * x)), 255);
function mlRamp(value: number, max: number, near: string, far: string): Rgba8 {
  const safeMax = max > 0 ? max : 1;
  const N = parse01(near);
  const F = parse01(far);
  let out: number[];
  if (value <= 0) out = N;
  else if (value >= safeMax) out = F;
  else {
    const t = value / safeMax;
    out = N.map((c, i) => c + t * (F[i]! - c));
  }
  return out.map(floorByte) as unknown as Rgba8;
}

const NEAR = packCssColor(DISTANCE_NEAR);
const FAR = packCssColor(DISTANCE_FAR);
const GREY = packCssColor('#888888');

describe('packRampColor byte-parity with MapLibre data-driven interpolate', () => {
  it('unreachable (value < 0) renders the grey sentinel', () => {
    expect(packRampColor(-1, 100, NEAR, FAR, GREY)).toEqual(GREY);
    expect(packRampColor(-0.0001, 100, NEAR, FAR, GREY)).toEqual(GREY);
  });

  it('beyond-gas-range clamps to the ramp far color, never grey or the base color (#395)', () => {
    expect(packRampColor(BEYOND_GAS_RANGE, 100, NEAR, FAR, GREY)).toEqual(FAR);
  });

  it('endpoints are the exact near / far colors', () => {
    expect(packRampColor(0, 100, NEAR, FAR, GREY)).toEqual(NEAR);
    expect(packRampColor(100, 100, NEAR, FAR, GREY)).toEqual(FAR);
    // over max clamps to far (stock evaluate returns outputs[last]).
    expect(packRampColor(250, 100, NEAR, FAR, GREY)).toEqual(FAR);
  });

  it('a known midpoint matches the hand-computed ramp byte', () => {
    // t = 0.5 over gold(#f4d03f) -> purple(#7d3c98).
    expect(packRampColor(50, 100, NEAR, FAR, GREY)).toEqual([184, 134, 107, 255]);
  });

  it('matches the MapLibre oracle across values and scales (byte-exact)', () => {
    for (const max of [1, 100, 137, 5000.5]) {
      for (let v = 0; v <= max * 1.1; v += max / 59) {
        expect(packRampColor(v, max > 0 ? max : 1, NEAR, FAR, GREY)).toEqual(
          mlRamp(v, max, DISTANCE_NEAR, DISTANCE_FAR),
        );
      }
    }
  });

  it('degenerate max <= 0 guards to a strictly ascending scale (all reachable = far)', () => {
    // safeMax becomes 1; any value >= 1 is far, matching the stock guard.
    expect(packRampColor(5, 0, NEAR, FAR, GREY)).toEqual(FAR);
  });
});

describe('makeOverlayColor (mode -> hues + grey)', () => {
  it('distance mode uses the distance ramp hues', () => {
    const f = makeOverlayColor({ mode: 'distance', shots: new Map(), stations: new Map(), max: 100 }, '#888888');
    expect(f(50)).toEqual(mlRamp(50, 100, DISTANCE_NEAR, DISTANCE_FAR));
    expect(f(-1)).toEqual(GREY);
  });

  it('gas-reach mode uses the gas ramp hues', () => {
    const f = makeOverlayColor({ mode: 'gasReach', shots: new Map(), stations: new Map(), max: 80 }, '#888888');
    expect(f(40)).toEqual(mlRamp(40, 80, GAS_NEAR, GAS_FAR));
    expect(f(-1)).toEqual(GREY);
  });
});

describe('overlayInputFrom (SurveyLayer state -> overlay input)', () => {
  const dist = { shots: new Map([[1, 2]]), stations: new Map([[3, 4]]), max: 9 };
  const gas = { shots: new Map([[5, 6]]), stations: new Map([[7, 8]]), max: 11 };

  it('returns distance when distance mode is on (and wins over gas)', () => {
    expect(overlayInputFrom(true, dist, false, gas)).toEqual({ mode: 'distance', ...dist });
    expect(overlayInputFrom(true, dist, true, gas)?.mode).toBe('distance');
  });

  it('returns gas when only gas mode is on', () => {
    expect(overlayInputFrom(false, dist, true, gas)).toEqual({ mode: 'gasReach', ...gas });
  });

  it('returns null when neither mode is on, or the state is missing', () => {
    expect(overlayInputFrom(false, dist, false, gas)).toBeNull();
    expect(overlayInputFrom(true, null, false, null)).toBeNull();
  });
});

// ─── Resolver integration (build path) ─────────────────────────────────────

const THEME = {
  stationDefault: '#ffffff',
  zeroDatum: '#00ffff',
  strokeColor: '#123456',
  grayLine: '#888888',
  depthShallow: '#0000ff',
  depthDeep: '#ff0000',
  severityError: '#ff0000',
  severityWarning: '#ffff00',
  severityClear: '#00000000',
} as unknown as DisplayColorContext['themeColors'];

/** One section (id 1): stations 0,1,2; shots (shotId 0,1). */
function survey(): SurveyData {
  const d = emptySurveyData();
  d.sections = [
    { id: 1, name: null, color: '#0a0b0c',
      direction: 'in' as never, quality: 0, next_shot_id: 3, shotOffset: 0, shotCount: 2 },
  ];
  d.shotCount = 2;
  d.shotIds = new Uint32Array([0, 1]);
  d.shotSectionIds = new Uint32Array([1, 1]);
  d.shotFlags = new Uint8Array([0, 0]);
  d.shotDepthIn = new Float32Array([0, -1]);
  d.shotDepthOut = new Float32Array([-1, -2]);
  d.stationCount = 3;
  d.stationSectionIds = new Uint32Array([1, 1, 1]);
  d.stationIndices = new Uint32Array([0, 1, 2]);
  d.stationCoords = new Float64Array([0, 0, 0, 10, 0, -1, 10, 10, -2]);
  d.stationFlags = new Uint8Array([0, 0, 0]);
  return d;
}

function ctx(overlay: OverlayColorInput | null, over: Partial<DisplayColorContext> = {}): DisplayColorContext {
  return {
    showDepthGradient: false,
    showIssueSeverity: false,
    depthRange: { min: 0, max: 50 },
    themeColors: THEME,
    overlay,
    ...over,
  };
}

describe('makeColorResolvers with an active overlay', () => {
  it('colors shots / stations by their carried value, unreachable = grey', () => {
    const d = survey();
    // shot global 0 -> featureId(1,0); station global 1 -> featureId(1,1).
    const overlay: OverlayColorInput = {
      mode: 'distance',
      shots: new Map([[encodeFeatureId(1, 0), 50]]), // shotId 1 absent -> grey
      stations: new Map([[encodeFeatureId(1, 0), 0], [encodeFeatureId(1, 1), 100]]),
      max: 100,
    };
    const r = makeColorResolvers(d, ctx(overlay));
    expect(r.colorForShot(0, 1)).toEqual([184, 134, 107, 255]); // dist 50 -> midpoint
    expect(r.colorForShot(1, 1)).toEqual(GREY); // absent -> unreachable
    expect(r.fillForStation(0)).toEqual(NEAR); // dist 0 -> near
    expect(r.fillForStation(1)).toEqual(FAR); // dist 100 -> far
    expect(r.fillForStation(2)).toEqual(GREY); // absent -> grey
  });

  it('leaves the station stroke as the theme stroke (stock swaps fill only)', () => {
    const d = survey();
    const overlay: OverlayColorInput = {
      mode: 'distance', shots: new Map(), stations: new Map(), max: 100,
    };
    const r = makeColorResolvers(d, ctx(overlay));
    expect(r.strokeForStation(0)).toEqual(packCssColor('#123456'));
  });

  it('forces flat per-shot color (no depth endpoints) even under depth mode', () => {
    const d = survey();
    const overlay: OverlayColorInput = {
      mode: 'distance', shots: new Map([[encodeFeatureId(1, 0), 50]]), stations: new Map(), max: 100,
    };
    const r = makeColorResolvers(d, ctx(overlay, { showDepthGradient: true }));
    expect(r.useDepthEndpoints).toBe(false);
    expect(r.colorForShotEndpoints(0, 1)).toEqual([[184, 134, 107, 255], [184, 134, 107, 255]]);
  });

  it('is identical to the plain build when no overlay is set', () => {
    const d = survey();
    const plain = makeColorResolvers(d, ctx(null));
    // Section color #0a0b0c for the shot; #0a0b0c for the (non-zero-datum) station.
    expect(plain.colorForShot(0, 1)).toEqual(packCssColor('#0a0b0c'));
    expect(plain.fillForStation(2)).toEqual(packCssColor('#0a0b0c'));
  });
});
