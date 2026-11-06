/**
 * The v3 import group, pinned where a fuzz run cannot pin it.
 *
 * Three properties matter and none of them is visible from a green fuzz run:
 *
 *   1. THE STREAM GATE. Every generator here must be `since: 'v3'`, and a v1 or
 *      v2 run must build the exact weighted name array it built before this
 *      group existed. That is what keeps every repro line in `undoRedoFuzz.ts`'s
 *      header replaying bit for bit, and a run that drew one extra name would
 *      still pass - it would just be a different run than the line describes.
 *   2. THE FILE CONTRACT. A generator that needs an input file must put a
 *      SESSION-RELATIVE path in `args[0]` and name an action in
 *      `FILE_ARG_ACTIONS`, or the replayer has no way to know argument zero is a
 *      path and will hand the app a `fuzz-files/...` string that resolves
 *      against the process cwd.
 *   3. `mustBeRejected` IS CONSERVATIVE. It fires an outright scenario failure,
 *      so a false positive on a legal payload would turn every affected run red.
 *      It must claim a refusal for the two shapes that genuinely cannot be
 *      deserialized, and for nothing else - in particular not for the OPTIONAL
 *      shot numbers, where `null` legally means "absent".
 *
 * The context is a stub rather than a live `ScenarioContext`: the generators
 * read only the PRNG surface and the digest, which is the whole reason they are
 * pure and the whole reason a trace can replay without them.
 */
import { describe, expect, it } from 'vitest';

import { FILE_ARG_ACTIONS } from './fuzzTrace';
import type { TargetingDigest } from './fuzzOracleCore';
import { mulberry32, type ScenarioContext } from '../scenarios/framework';
import {
  IMPORT_ACTION_GENERATORS,
  importGeneratorNames,
  MOTIF_KEYS,
  motifShots,
  mustBeRejected,
} from '../scenarios/undoRedoFuzzImportActions';
import {
  swarmProfileFor,
  weightedNamesFor,
  type FuzzAction,
} from '../scenarios/undoRedoFuzzActions';
import { seedWindow } from './seedWindow';

/** Enough of a `ScenarioContext` for a generator, and nothing more. */
function stubContext(seed: number): ScenarioContext {
  const rand = mulberry32(seed);
  const partial = {
    seed,
    artifactsDir: null,
    metrics: {},
    rng: () => rand(),
    randInt: (min: number, max: number) => min + Math.floor(rand() * (max - min + 1)),
    pick: <T>(items: readonly T[]): T => items[Math.floor(rand() * items.length)] as T,
    log: () => undefined,
    metric: () => undefined,
    setMetric: () => undefined,
  };
  return partial as unknown as ScenarioContext;
}

/** A digest with two populated sections, one merge, one datum and one closure. */
const DIGEST: TargetingDigest = {
  s: [
    [1, [1001, 1002, 1003]],
    [2, [2001, 2002]],
    [3, []],
  ],
  st: [
    [1, 0],
    [1, 1],
    [1, 2],
    [1, 3],
    [2, 0],
    [2, 1],
    [2, 2],
  ],
  m: [[1, 2, 2, 0]],
  z: [[1, 0]],
  c: [7],
};

/** Every action the group can build over a spread of seeds (1..`seeds`, or the
 *  window the STALAGMIGHT_VITEST_SEED_* knobs ask for). */
function drawAll(seeds: number): FuzzAction[] {
  const out: FuzzAction[] = [];
  for (const seed of seedWindow(1, seeds)) {
    for (const generator of IMPORT_ACTION_GENERATORS) {
      const action = generator.build(stubContext(seed * 7919 + 13), DIGEST);
      if (action) out.push(action);
    }
  }
  return out;
}

/** Run `fn` with `STALAGMIGHT_FUZZ_STREAM` set to `value` (or unset for null). */
function withStream<T>(value: string | null, fn: () => T): T {
  const before = process.env.STALAGMIGHT_FUZZ_STREAM;
  try {
    if (value === null) delete process.env.STALAGMIGHT_FUZZ_STREAM;
    else process.env.STALAGMIGHT_FUZZ_STREAM = value;
    return fn();
  } finally {
    if (before === undefined) delete process.env.STALAGMIGHT_FUZZ_STREAM;
    else process.env.STALAGMIGHT_FUZZ_STREAM = before;
  }
}

describe('the suggestion motif', () => {
  /**
   * The suggester's hard cutoffs, mirrored from `PlacementSuggestionConfig`
   * defaults (`crates/models/src/config/mod.rs`) and the guards in
   * `compute_sequence_shot_score`. Same mirror, and the same caveat, as the one
   * in `surveyFileSpecs.test.ts`: it catches the generator drifting, which is
   * the likely direction, and goes stale if the product retunes.
   */
  const MAX_AZIMUTH_ERROR = 25;
  const MAX_DEPTH_ERROR = 0.3;
  const LENGTH_FLOOR = 0.4;
  const LENGTH_FRACTION = 0.03;

  const number = (shot: Record<string, unknown>, key: string): number => {
    const value = shot[key];
    expect(typeof value, `${key} must be a number`).toBe('number');
    return value as number;
  };

  it('keeps a drifted probe inside every cutoff of its own planted section', () => {
    // Without this the probe is just `randomShotData` by another name: the
    // command returns an empty list and the scoring loop, the merge-offset
    // search and the crossing-clearance guard behind it never run.
    for (let key = 0; key < MOTIF_KEYS; key++) {
      const planted = motifShots(key, 0);
      for (const drift of [-2, -1, 0, 1, 2]) {
        const probe = motifShots(key, drift);
        expect(probe.length, `motif ${key} shot count`).toBe(planted.length);
        for (let i = 0; i < planted.length; i++) {
          const a = planted[i] as Record<string, unknown>;
          const b = probe[i] as Record<string, unknown>;
          const azimuthError = Math.abs(number(a, 'azimuth_in') - number(b, 'azimuth_in'));
          const lengthError = Math.abs(number(a, 'length') - number(b, 'length'));
          const maxLengthError = Math.max(LENGTH_FRACTION * number(a, 'length'), LENGTH_FLOOR);
          expect(azimuthError, `motif ${key} drift ${drift} shot ${i} azimuth`).toBeLessThan(MAX_AZIMUTH_ERROR);
          expect(lengthError, `motif ${key} drift ${drift} shot ${i} length`).toBeLessThan(maxLengthError);
          for (const field of ['depth_in', 'depth_out'] as const) {
            expect(
              Math.abs(number(a, field) - number(b, field)),
              `motif ${key} drift ${drift} shot ${i} ${field}`,
            ).toBeLessThan(MAX_DEPTH_ERROR);
          }
        }
      }
    }
  });

  it('is pure, so the planting and probing generators agree without shared state', () => {
    // Both halves rebuild the run from the same two integers. A generator that
    // remembered anything between calls would derive a different one on a
    // shrinker replay, which is why this is a property and not a convention.
    for (let key = 0; key < MOTIF_KEYS; key++) {
      for (const drift of [-2, 0, 2]) {
        expect(JSON.stringify(motifShots(key, drift))).toBe(JSON.stringify(motifShots(key, drift)));
      }
    }
  });

  it('emits shots the store action can send as they are', () => {
    for (let key = 0; key < MOTIF_KEYS; key++) {
      for (const shot of motifShots(key, 0)) {
        expect(shot.kind).toBe('Normal');
        for (const field of ['azimuth_in', 'azimuth_out', 'length', 'depth_in', 'depth_out'] as const) {
          const value = shot[field];
          expect(typeof value, `${field}`).toBe('number');
          expect(Number.isFinite(value as number), `${field} finite`).toBe(true);
        }
        expect(number(shot, 'azimuth_in')).toBeGreaterThanOrEqual(0);
        expect(number(shot, 'azimuth_in')).toBeLessThan(360);
        expect(number(shot, 'length')).toBeGreaterThan(0);
      }
    }
  });
});

describe('the v3 import group', () => {
  it('is entirely since:v3, so no other stream can draw it', () => {
    for (const generator of IMPORT_ACTION_GENERATORS) {
      expect(generator.since, generator.name).toBe('v3');
    }
    expect(importGeneratorNames().length).toBe(IMPORT_ACTION_GENERATORS.length);
    expect(new Set(importGeneratorNames()).size).toBe(IMPORT_ACTION_GENERATORS.length);
  });

  it('leaves the v1 and v2 weighted name arrays untouched', () => {
    // ONE profile across all three streams: the swarm draw legitimately differs
    // under v3 (see the next test), and mixing the two effects would leave this
    // assertion unable to say which one moved the array.
    const profile = { name: 'balanced', multipliers: {} };
    const v1 = withStream('v1', () => weightedNamesFor(profile));
    const v2 = withStream(null, () => weightedNamesFor(profile));
    const v3 = withStream('v3', () => weightedNamesFor(profile));
    const names = new Set(importGeneratorNames());

    // Not one v3 name reaches a v1 or v2 run...
    expect(v1.filter((name) => names.has(name))).toEqual([]);
    expect(v2.filter((name) => names.has(name))).toEqual([]);
    // ...and v3 is v2 with those names APPENDED, so every earlier index - which
    // is what `ctx.pick` indexes - still holds exactly what it held.
    expect(v3.slice(0, v2.length)).toEqual(v2);
    expect(v3.length).toBeGreaterThan(v2.length);
    expect(v3.slice(v2.length).every((name) => names.has(name))).toBe(true);
    // Note what is NOT claimed: v1 is a SUBSEQUENCE of v2, not a prefix -
    // `removeLoopClosure` is a v1 generator declared after the v2 block, so v2
    // interleaves names ahead of it. That is exactly why v1 repro lines do not
    // replay under v2 and need the stream selector. The v3 group avoids the
    // same fate by being appended in one run at the very end.
    expect(v1.every((name) => v2.includes(name))).toBe(true);
  });

  it('adds import-heavy to the swarm pool for v3 only', () => {
    const seeds = Array.from({ length: 200 }, (_unused, index) => index + 1);
    const v2Profiles = withStream(null, () => new Set(seeds.map((seed) => swarmProfileFor(seed).name)));
    const v3Profiles = withStream('v3', () => new Set(seeds.map((seed) => swarmProfileFor(seed).name)));

    expect(v2Profiles.has('import-heavy')).toBe(false);
    expect(v3Profiles.has('import-heavy')).toBe(true);
    // The v2 pool is exactly what it was: the new profile is APPENDED under the
    // v3 gate, so `SWARM_PROFILES.length` - the divisor a seed's profile is
    // chosen with - is unchanged for v1 and v2.
    for (const name of v2Profiles) expect(v3Profiles.has(name)).toBe(true);
    expect(v2Profiles.size).toBe(7);
    expect(v3Profiles.size).toBe(8);
  });

  it('keeps imports a minority of a balanced v3 run', () => {
    const balanced = { name: 'balanced', multipliers: {} };
    const v2 = withStream(null, () => weightedNamesFor(balanced));
    const v3 = withStream('v3', () => weightedNamesFor(balanced));
    const share = (v3.length - v2.length) / v3.length;

    expect(share).toBeGreaterThan(0.15);
    expect(share).toBeLessThan(0.3);
  });

  it('gives every file-backed action a session-relative path in args[0]', () => {
    const actions = drawAll(24);
    const withFiles = actions.filter((action) => action.file !== undefined);

    expect(withFiles.length).toBeGreaterThan(20);
    for (const action of withFiles) {
      expect(FILE_ARG_ACTIONS.has(action.action), action.action).toBe(true);
      const rel = action.args[0];
      expect(typeof rel).toBe('string');
      expect(rel).toBe(action.file?.rel);
      const text = rel as string;
      expect(text.startsWith('/')).toBe(false);
      expect(/^[A-Za-z]:/.test(text)).toBe(false);
      expect(text.split(/[/\\]/).includes('..')).toBe(false);
      expect(text.startsWith('fuzz-files/')).toBe(true);
    }
  });

  it('never puts an absolute path in an args[0] a FILE_ARG_ACTION reads', () => {
    for (const action of drawAll(24)) {
      if (!FILE_ARG_ACTIONS.has(action.action)) continue;
      const rel = action.args[0];
      expect(typeof rel, action.action).toBe('string');
      expect((rel as string).startsWith('/')).toBe(false);
    }
  });

  it('bounds the shots one importSections call carries', () => {
    for (const action of drawAll(40)) {
      if (action.action !== 'importSections') continue;
      const sections = action.args[0] as { shots: unknown[] }[];
      const total = sections.reduce((sum, section) => sum + section.shots.length, 0);
      expect(total).toBeLessThanOrEqual(24);
    }
  });
});

describe('mustBeRejected', () => {
  const shot = (over: Record<string, unknown> = {}): Record<string, unknown> => ({
    azimuth_in: 10,
    azimuth_out: 10,
    length: 1,
    depth_in: 0,
    depth_out: 0,
    pitch_in: null,
    pitch_out: null,
    left: null,
    right: null,
    up: null,
    down: null,
    comment: null,
    kind: 'Normal',
    marker_index: null,
    temperature: null,
    ...over,
  });

  const config = (over: Record<string, unknown> = {}): Record<string, unknown> => ({
    section_index: 0,
    name: 'x',
    surveyor: null,
    date: '2024-01-01',
    color: '#3388FF',
    direction: 'in',
    quality: 3,
    shots: [shot()],
    placement: { type: 'origin' },
    station_comments: [],
    reverse: false,
    ...over,
  });

  it('says nothing about actions that are not importSections', () => {
    expect(mustBeRejected('createSection', [config()])).toBeNull();
    expect(mustBeRejected('executeTmlImport', ['fuzz-files/a.tml', null])).toBeNull();
  });

  it('passes a legal payload, including a null date and an empty batch', () => {
    expect(mustBeRejected('importSections', [[config()], null, null, 'Auto', null])).toBeNull();
    expect(mustBeRejected('importSections', [[config({ date: null })], null, null, 'Auto', null])).toBeNull();
    expect(mustBeRejected('importSections', [[], null, null, 'Auto', null])).toBeNull();
    expect(mustBeRejected('importSections', [[config({ shots: [] })], null, null, 'Auto', null])).toBeNull();
  });

  it('refuses date:"" - issue #478, on the route that bypasses the wizard', () => {
    const reason = mustBeRejected('importSections', [[config({ date: '' })], null, null, 'Auto', null]);

    expect(reason).toContain('date');
    expect(reason).toContain('#478');
  });

  it('refuses a non-finite REQUIRED shot number, in every spelling', () => {
    for (const poison of [Number.NaN, Number.POSITIVE_INFINITY, Number.NEGATIVE_INFINITY, null]) {
      for (const field of ['length', 'azimuth_in', 'azimuth_out', 'depth_in', 'depth_out']) {
        const args = [[config({ shots: [shot({ [field]: poison })] })], null, null, 'Auto', null];
        expect(mustBeRejected('importSections', args), `${field}=${String(poison)}`).toContain(field);
      }
    }
  });

  it('does NOT claim a refusal for a non-finite OPTIONAL shot number', () => {
    // `Option<f64>` deserializes `null` as "absent", so the import SUCCEEDS with
    // the value silently dropped. The frontend guard refuses it because dropping
    // a measurement is data loss; the backend accepting it is not a defect, and
    // claiming otherwise here would fail every run that drew one.
    for (const field of ['pitch_in', 'pitch_out', 'left', 'right', 'up', 'down']) {
      const args = [[config({ shots: [shot({ [field]: Number.NaN })] })], null, null, 'Auto', null];
      expect(mustBeRejected('importSections', args), field).toBeNull();
    }
  });

  it('does NOT claim a refusal for a non-finite existing_shot_update', () => {
    // `execute_import_sections` DOES refuse one by name - but only when it
    // arrives non-finite, which nothing crossing JSON can. Every `ShotUpdates`
    // field is `Option<f64>` with `#[serde(default)]`, `JSON.stringify` spells
    // NaN `null`, and `null` reads back as `None` = "leave this field alone".
    // Claiming a refusal here failed a legal run (naharon seed 59 x60, v3).
    const updates = [{ section_id: 1, shot_id: 1001, updates: { length: Number.NaN } }];

    expect(mustBeRejected('importSections', [[config()], null, null, 'Auto', null, updates])).toBeNull();
  });

  it('has no opinion about anything the v1/v2 generators produce', () => {
    // The guarantee that matters for compatibility: this oracle was added with
    // v3, and it must be silent on every payload the older streams can build -
    // `randomSectionData` uses finite boundary values and always a real date.
    const v2Payloads = [
      [[config({ name: '', surveyor: '', quality: 1 })], null, null, 'AllJumps', null],
      [[config({ shots: [shot({ length: 0 }), shot({ length: 1e-9 }), shot({ length: -5 })] })], null, null, null, null],
      [[config({ station_comments: [[0, '']] })], null, null, 'AllReferences', null],
    ];
    for (const args of v2Payloads) expect(mustBeRejected('importSections', args)).toBeNull();
  });
});
