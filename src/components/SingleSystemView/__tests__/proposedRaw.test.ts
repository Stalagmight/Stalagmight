/**
 * Behavior-parity test for the WarningResolution dedup refactor (issue #57).
 *
 * The on-map preview and the applied `update_shot` patch used to be produced by
 * two separate switch statements that mirrored each other option-for-option. The
 * refactor routes BOTH through a single pure source, `computeProposedRaw`. This
 * test pins the refactor to the PRE-refactor behavior: for the full option-id
 * table it transcribes the OLD apply-switch and OLD preview-switch verbatim and
 * asserts the NEW derivations (built from computeProposedRaw exactly as the
 * production preview/apply do) produce identical results.
 */
import { describe, it, expect } from 'vitest';
import { computeProposedRaw, type ManualKind } from '../WarningResolution';
import { normalizeAzimuth, averageAzimuth } from '../../../utils/azimuth';
import { horizontalLength } from '../../../utils/resolutionPreview';
import type { Shot } from '../../../types/project';
import { makeShot as shotOf } from '../../../test/factories/survey';

const LRUD_FIELDS = ['left', 'right', 'up', 'down'] as const;
type LrudField = (typeof LRUD_FIELDS)[number];

// Mirror of the production MIN_SIN_INCLINATION floor (kept local so the OLD
// transcription is self-contained); production hosts it in resolutionPreview.ts.
const MIN_SIN_INCLINATION = 0.05;

// --- Pure helpers transcribed verbatim from WarningResolution.tsx -----------
function recordedPitch(shot: Shot): number | null {
  if (shot.pitch_in != null && shot.pitch_out != null) return (shot.pitch_in + shot.pitch_out) / 2;
  return shot.pitch_in ?? shot.pitch_out ?? null;
}
// Signed implied inclination in the +down convention (matches the production
// helper and DetailPanel.computeInclination: positive = descending, depth
// increases), issue #57 finding #7, issue #296.
function impliedInclination(shot: Shot): number {
  if (!(shot.length > 0)) return 0;
  const ratio = (shot.depth_out - shot.depth_in) / shot.length;
  return (Math.asin(Math.max(-1, Math.min(1, ratio))) * 180) / Math.PI;
}

// --- A shot fixture builder --------------------------------------------------
function makeShot(overrides: Partial<Shot> = {}): Shot {
  return shotOf({
    azimuth_in: 100,
    azimuth_out: 110,
    length: 5,
    depth_out: 2,
    pitch_in: 30,
    pitch_out: 30,
    left: -1,
    right: 2,
    up: -3,
    ...overrides,
  });
}

function negativeLrudOf(shot: Shot): LrudField[] {
  return LRUD_FIELDS.filter((f) => (shot[f] ?? 0) < 0);
}

// ============================================================================
// OLD apply switch - transcribed verbatim from the pre-refactor apply().
// Returns { patch } on success, { invalid } on a rejected manual input,
// { failed: true } for an unhandled option id.
// ============================================================================
type OldApply =
  | { patch: Partial<Record<string, number | null>> }
  | { invalid: 'azimuth' | 'length' | 'pitch' | 'invalid' }
  | { failed: true };

function oldApplyPatch(
  optId: string,
  manualKind: ManualKind | undefined,
  shot: Shot,
  manual: Record<string, string>,
  negativeLrud: readonly LrudField[],
): OldApply {
  const num = (key: string): number => parseFloat(manual[key] ?? '');
  const parseAzimuth = (key: string): number | null => {
    const v = num(key);
    if (Number.isNaN(v) || v < -360 || v > 360) return null;
    return normalizeAzimuth(v);
  };
  const patch: Partial<Record<string, number | null>> = {};
  switch (optId) {
    case 'use-in':
      patch.azimuth_out = shot.azimuth_in;
      break;
    case 'use-out':
      patch.azimuth_in = shot.azimuth_out;
      break;
    case 'average': {
      const avg = averageAzimuth(shot.azimuth_in, shot.azimuth_out);
      patch.azimuth_in = avg;
      patch.azimuth_out = avg;
      break;
    }
    case 'normalize':
      patch.azimuth_in = normalizeAzimuth(shot.azimuth_in);
      patch.azimuth_out = normalizeAzimuth(shot.azimuth_out);
      break;
    case 'incl-from-depth': {
      const p = impliedInclination(shot);
      patch.pitch_in = p;
      patch.pitch_out = p;
      break;
    }
    case 'split-incl-length': {
      const recorded = recordedPitch(shot);
      if (recorded === null) return { invalid: 'invalid' };
      const dz = shot.depth_out - shot.depth_in;
      const mid = (recorded + impliedInclination(shot)) / 2;
      const sinMid = Math.sin((mid * Math.PI) / 180);
      if (Math.abs(sinMid) < MIN_SIN_INCLINATION) return { invalid: 'invalid' };
      patch.pitch_in = mid;
      patch.pitch_out = mid;
      patch.length = Math.abs(dz) / Math.abs(sinMid);
      break;
    }
    case 'length-from-depth': {
      // Vertical shot: also pins pitch to ±90° in the +down convention (deeper =>
      // +90) so it stays consistent (issue #57 finding #2, issue #296).
      const dzLfd = shot.depth_out - shot.depth_in;
      patch.length = Math.abs(dzLfd);
      patch.pitch_in = dzLfd > 0 ? 90 : -90;
      patch.pitch_out = dzLfd > 0 ? 90 : -90;
      break;
    }
    case 'length-from-incl': {
      const recorded = recordedPitch(shot);
      const sinP = recorded === null ? 0 : Math.sin((recorded * Math.PI) / 180);
      if (Math.abs(sinP) < MIN_SIN_INCLINATION) return { invalid: 'invalid' };
      patch.length = Math.abs(shot.depth_out - shot.depth_in) / Math.abs(sinP);
      break;
    }
    case 'lrud-abs':
      for (const f of negativeLrud) patch[f] = Math.abs(shot[f] ?? 0);
      break;
    case 'lrud-clear':
      for (const f of negativeLrud) patch[f] = null;
      break;
    case 'manual':
      if (manualKind === 'azimuth') {
        const n = parseAzimuth('azimuth');
        if (n === null) return { invalid: 'azimuth' };
        patch.azimuth_in = n;
        patch.azimuth_out = n;
      } else if (manualKind === 'azimuthInOut') {
        const ni = parseAzimuth('azimuthIn');
        const no = parseAzimuth('azimuthOut');
        if (ni === null || no === null) return { invalid: 'azimuth' };
        patch.azimuth_in = ni;
        patch.azimuth_out = no;
      } else if (manualKind === 'length') {
        const v = num('length');
        if (Number.isNaN(v) || v <= 0) return { invalid: 'length' };
        patch.length = v;
      } else if (manualKind === 'depth') {
        const vi = num('depthIn');
        const vo = num('depthOut');
        if (Number.isNaN(vi) || Number.isNaN(vo)) return { invalid: 'invalid' };
        patch.depth_in = vi;
        patch.depth_out = vo;
      } else if (manualKind === 'lrud') {
        for (const f of LRUD_FIELDS) {
          const raw = manual[f]?.trim() ?? '';
          patch[f] = raw === '' ? null : parseFloat(raw);
        }
        if (LRUD_FIELDS.some((f) => patch[f] !== null && Number.isNaN(patch[f] as number))) {
          return { invalid: 'invalid' };
        }
      }
      break;
    default:
      return { failed: true };
  }
  return { patch };
}

// ============================================================================
// OLD preview switch - transcribed verbatim from the pre-refactor preview memo.
// Returns { propAz, propHoriz } or null (no ghost). Operates from currentAz /
// currentHoriz computed identically to production.
// ============================================================================
function oldPreview(
  optId: string,
  manualKind: ManualKind | undefined,
  shot: Shot,
  manual: Record<string, string>,
): { propAz: number; propHoriz: number } | null {
  const num = (key: string): number => parseFloat(manual[key] ?? '');
  const parseAzimuth = (key: string): number | null => {
    const v = num(key);
    if (Number.isNaN(v) || v < -360 || v > 360) return null;
    return normalizeAzimuth(v);
  };
  // Early guards from the preview memo (ignore/navigate/remove-datum handled by
  // the caller; lrud here).
  if (optId === 'lrud-abs' || optId === 'lrud-clear' || manualKind === 'lrud') return null;

  const depthChange = shot.depth_out - shot.depth_in;
  const currentHoriz = horizontalLength(shot.length, depthChange);
  const currentAz = averageAzimuth(shot.azimuth_in, shot.azimuth_out);
  let propAz = currentAz;
  let propHoriz = currentHoriz;
  switch (optId) {
    case 'use-in':
      propAz = shot.azimuth_in;
      break;
    case 'use-out':
      propAz = shot.azimuth_out;
      break;
    case 'average':
      propAz = averageAzimuth(shot.azimuth_in, shot.azimuth_out);
      break;
    case 'normalize':
      propAz = averageAzimuth(normalizeAzimuth(shot.azimuth_in), normalizeAzimuth(shot.azimuth_out));
      break;
    case 'manual':
      if (manualKind === 'azimuth') {
        const n = parseAzimuth('azimuth');
        if (n === null) return null;
        propAz = n;
      } else if (manualKind === 'azimuthInOut') {
        const ni = parseAzimuth('azimuthIn');
        const no = parseAzimuth('azimuthOut');
        if (ni === null || no === null) return null;
        propAz = averageAzimuth(ni, no);
      } else if (manualKind === 'length') {
        const v = num('length');
        if (Number.isNaN(v) || v <= 0) return null;
        propHoriz = horizontalLength(v, depthChange);
      } else if (manualKind === 'depth') {
        const vi = num('depthIn');
        const vo = num('depthOut');
        if (Number.isNaN(vi) || Number.isNaN(vo)) return null;
        propHoriz = horizontalLength(shot.length, vo - vi);
      } else {
        return null;
      }
      break;
    case 'length-from-depth':
      propHoriz = horizontalLength(Math.abs(depthChange), depthChange);
      break;
    case 'length-from-incl': {
      const recorded = recordedPitch(shot);
      const sinP = recorded === null ? 0 : Math.sin((recorded * Math.PI) / 180);
      if (Math.abs(sinP) < MIN_SIN_INCLINATION) return null;
      propHoriz = horizontalLength(Math.abs(depthChange) / Math.abs(sinP), depthChange);
      break;
    }
    case 'split-incl-length': {
      const recorded = recordedPitch(shot);
      if (recorded === null) return null;
      const mid = (recorded + impliedInclination(shot)) / 2;
      const sinMid = Math.sin((mid * Math.PI) / 180);
      if (Math.abs(sinMid) < MIN_SIN_INCLINATION) return null;
      propHoriz = horizontalLength(Math.abs(depthChange) / Math.abs(sinMid), depthChange);
      break;
    }
    default:
      return null;
  }
  return { propAz, propHoriz };
}

// ============================================================================
// NEW derivations - exactly as the production preview memo / apply() build them
// from computeProposedRaw.
// ============================================================================
function newApplyPatch(
  optId: string,
  manualKind: ManualKind | undefined,
  shot: Shot,
  manual: Record<string, string>,
  negativeLrud: readonly LrudField[],
): OldApply {
  const raw = computeProposedRaw(optId, manualKind, shot, manual, negativeLrud);
  if (!raw) return { failed: true };
  if (raw.invalid) return { invalid: raw.invalid };
  const patch: Partial<Record<string, number | null>> = {};
  if (optId === 'use-in') {
    patch.azimuth_out = shot.azimuth_in;
  } else if (optId === 'use-out') {
    patch.azimuth_in = shot.azimuth_out;
  } else {
    if (raw.azimuthIn !== undefined) patch.azimuth_in = raw.azimuthIn;
    if (raw.azimuthOut !== undefined) patch.azimuth_out = raw.azimuthOut;
  }
  if (raw.pitchIn !== undefined) patch.pitch_in = raw.pitchIn;
  if (raw.pitchOut !== undefined) patch.pitch_out = raw.pitchOut;
  if (raw.length !== undefined) patch.length = raw.length;
  if (raw.depthIn !== undefined) patch.depth_in = raw.depthIn;
  if (raw.depthOut !== undefined) patch.depth_out = raw.depthOut;
  if (raw.lrud) {
    for (const f of LRUD_FIELDS) {
      if (f in raw.lrud) patch[f] = raw.lrud[f] ?? null;
    }
  }
  return { patch };
}

function newPreview(
  optId: string,
  manualKind: ManualKind | undefined,
  shot: Shot,
  manual: Record<string, string>,
  negativeLrud: readonly LrudField[],
): { propAz: number; propHoriz: number } | null {
  // Early guards from the production preview memo.
  if (optId === 'lrud-abs' || optId === 'lrud-clear' || manualKind === 'lrud') return null;
  const depthChange = shot.depth_out - shot.depth_in;
  const currentHoriz = horizontalLength(shot.length, depthChange);
  const currentAz = averageAzimuth(shot.azimuth_in, shot.azimuth_out);

  const raw = computeProposedRaw(optId, manualKind, shot, manual, negativeLrud);
  if (!raw || raw.invalid) return null;
  // Pitch-only proposals (incl-from-depth) → no ghost.
  if (
    raw.azimuthIn === undefined &&
    raw.azimuthOut === undefined &&
    raw.length === undefined &&
    raw.depthIn === undefined &&
    raw.depthOut === undefined
  ) {
    return null;
  }
  let propAz = currentAz;
  if (raw.azimuthIn !== undefined || raw.azimuthOut !== undefined) {
    propAz = averageAzimuth(raw.azimuthIn ?? shot.azimuth_in, raw.azimuthOut ?? shot.azimuth_out);
  }
  let propHoriz = currentHoriz;
  if (raw.length !== undefined) {
    propHoriz = horizontalLength(raw.length, depthChange);
  } else if (raw.depthIn !== undefined && raw.depthOut !== undefined) {
    propHoriz = horizontalLength(shot.length, raw.depthOut - raw.depthIn);
  }
  return { propAz, propHoriz };
}

// --- The option-id × input table -------------------------------------------
interface Case {
  name: string;
  optId: string;
  manualKind?: ManualKind;
  shot?: Partial<Shot>;
  manual?: Record<string, string>;
}

const cases: Case[] = [
  { name: 'use-in', optId: 'use-in' },
  { name: 'use-out', optId: 'use-out' },
  { name: 'average', optId: 'average' },
  { name: 'normalize (out-of-range azimuths)', optId: 'normalize', shot: { azimuth_in: 370, azimuth_out: -20 } },
  { name: 'incl-from-depth', optId: 'incl-from-depth' },
  { name: 'split-incl-length (valid)', optId: 'split-incl-length' },
  { name: 'split-incl-length (no pitch → invalid)', optId: 'split-incl-length', shot: { pitch_in: null, pitch_out: null } },
  { name: 'length-from-depth', optId: 'length-from-depth' },
  { name: 'length-from-incl (valid)', optId: 'length-from-incl' },
  { name: 'length-from-incl (no pitch → invalid)', optId: 'length-from-incl', shot: { pitch_in: null, pitch_out: null } },
  { name: 'length-from-incl (shallow pitch → invalid)', optId: 'length-from-incl', shot: { pitch_in: 1, pitch_out: 1, depth_out: 0.1 } },
  { name: 'lrud-abs (some negative)', optId: 'lrud-abs' },
  { name: 'lrud-abs (none negative → empty)', optId: 'lrud-abs', shot: { left: 1, right: 2, up: 3, down: 4 } },
  { name: 'lrud-clear (some negative)', optId: 'lrud-clear' },
  { name: 'lrud-clear (none negative → empty)', optId: 'lrud-clear', shot: { left: 1, right: 2, up: 3, down: 4 } },
  { name: 'manual azimuth (valid)', optId: 'manual', manualKind: 'azimuth', manual: { azimuth: '57' } },
  { name: 'manual azimuth (invalid)', optId: 'manual', manualKind: 'azimuth', manual: { azimuth: '999' } },
  { name: 'manual azimuth (empty → invalid)', optId: 'manual', manualKind: 'azimuth', manual: { azimuth: '' } },
  { name: 'manual azimuthInOut (valid)', optId: 'manual', manualKind: 'azimuthInOut', manual: { azimuthIn: '120', azimuthOut: '130' } },
  { name: 'manual azimuthInOut (one invalid)', optId: 'manual', manualKind: 'azimuthInOut', manual: { azimuthIn: '120', azimuthOut: '-999' } },
  { name: 'manual length (valid)', optId: 'manual', manualKind: 'length', manual: { length: '7.5' } },
  { name: 'manual length (zero → invalid)', optId: 'manual', manualKind: 'length', manual: { length: '0' } },
  { name: 'manual length (empty → invalid)', optId: 'manual', manualKind: 'length', manual: { length: '' } },
  { name: 'manual depth (valid)', optId: 'manual', manualKind: 'depth', manual: { depthIn: '1', depthOut: '4' } },
  { name: 'manual depth (impossible geometry, still derives a patch)', optId: 'manual', manualKind: 'depth', manual: { depthIn: '0', depthOut: '12' } },
  { name: 'manual depth (NaN → invalid)', optId: 'manual', manualKind: 'depth', manual: { depthIn: '', depthOut: '4' } },
  { name: 'manual lrud (all values)', optId: 'manual', manualKind: 'lrud', manual: { left: '1', right: '2', up: '3', down: '4' } },
  { name: 'manual lrud (some empty)', optId: 'manual', manualKind: 'lrud', manual: { left: '1', right: '', up: '', down: '4' } },
  { name: 'manual lrud (one NaN → invalid)', optId: 'manual', manualKind: 'lrud', manual: { left: 'abc', right: '2', up: '3', down: '4' } },
  { name: 'ignore (no data fix)', optId: 'ignore' },
  { name: 'navigate / go-to-station (no data fix)', optId: 'go-to-station' },
  { name: 'remove-datum (no data fix)', optId: 'remove-datum' },
  { name: 'delete-shot (no data fix)', optId: 'delete-shot' },
  { name: 'unknown id', optId: 'no-such-option' },
];

describe('computeProposedRaw parity: apply patch matches the old switch', () => {
  for (const c of cases) {
    it(c.name, () => {
      const shot = makeShot(c.shot);
      const manual = c.manual ?? {};
      const neg = negativeLrudOf(shot);
      const oldR = oldApplyPatch(c.optId, c.manualKind, shot, manual, neg);
      const newR = newApplyPatch(c.optId, c.manualKind, shot, manual, neg);
      expect(newR).toEqual(oldR);
    });
  }
});

describe('computeProposedRaw parity: preview (propAz/propHoriz) matches the old switch', () => {
  for (const c of cases) {
    it(c.name, () => {
      const shot = makeShot(c.shot);
      const manual = c.manual ?? {};
      const neg = negativeLrudOf(shot);
      const oldR = oldPreview(c.optId, c.manualKind, shot, manual);
      const newR = newPreview(c.optId, c.manualKind, shot, manual, neg);
      if (oldR === null) {
        expect(newR).toBeNull();
      } else {
        expect(newR).not.toBeNull();
        expect(newR!.propAz).toBeCloseTo(oldR.propAz, 9);
        expect(newR!.propHoriz).toBeCloseTo(oldR.propHoriz, 9);
      }
    });
  }
});

describe('strict numeric parsing rejects trailing garbage (#57 #11)', () => {
  const shot = makeShot();
  it('manual length "5abc" is rejected (parseFloat would have read 5)', () => {
    expect(computeProposedRaw('manual', 'length', shot, { length: '5abc' }, [])).toEqual({ invalid: 'length' });
  });
  it('a clean length still parses', () => {
    expect(computeProposedRaw('manual', 'length', shot, { length: '7.5' }, [])).toEqual({ length: 7.5 });
  });
  it('manual lrud "2x" is rejected', () => {
    expect(
      computeProposedRaw('manual', 'lrud', shot, { left: '2x', right: '', up: '', down: '' }, []),
    ).toEqual({ invalid: 'invalid' });
  });
  it('manual pitch "30deg" is rejected', () => {
    expect(computeProposedRaw('manual', 'pitch', shot, { pitch: '30deg' }, [])).toEqual({ invalid: 'pitch' });
  });
  it('manual pitch out of [-90,90] is rejected', () => {
    expect(computeProposedRaw('manual', 'pitch', shot, { pitch: '120' }, [])).toEqual({ invalid: 'pitch' });
  });
  it('an in-range pitch parses to both legs', () => {
    expect(computeProposedRaw('manual', 'pitch', shot, { pitch: '-45' }, [])).toEqual({ pitchIn: -45, pitchOut: -45 });
  });
});

describe('length-from-depth pins pitch to ±90 in the +down convention (#57 #2, #296)', () => {
  it('downward shot (depth increases) → length=|dz|, pitch +90', () => {
    const shot = makeShot({ length: 1, depth_in: 0, depth_out: 3 });
    expect(computeProposedRaw('length-from-depth', undefined, shot, {}, [])).toEqual({ length: 3, pitchIn: 90, pitchOut: 90 });
  });
  it('upward shot (depth decreases) → length=|dz|, pitch -90', () => {
    const shot = makeShot({ length: 1, depth_in: 3, depth_out: 0 });
    expect(computeProposedRaw('length-from-depth', undefined, shot, {}, [])).toEqual({ length: 3, pitchIn: -90, pitchOut: -90 });
  });
});

describe('pitch-only proposals (incl-from-depth) produce no ghost', () => {
  it('incl-from-depth derives a pitch-only ProposedRaw and the preview returns null', () => {
    const shot = makeShot();
    const raw = computeProposedRaw('incl-from-depth', undefined, shot, {}, negativeLrudOf(shot));
    expect(raw).not.toBeNull();
    expect(raw!.pitchIn).toBeDefined();
    expect(raw!.pitchOut).toBeDefined();
    expect(raw!.azimuthIn).toBeUndefined();
    expect(raw!.length).toBeUndefined();
    expect(raw!.depthIn).toBeUndefined();
    expect(newPreview('incl-from-depth', undefined, shot, {}, negativeLrudOf(shot))).toBeNull();
  });
});
