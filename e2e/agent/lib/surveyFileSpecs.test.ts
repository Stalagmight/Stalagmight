/**
 * What a generator has to be worth trusting is narrower than "it produces
 * files": a seed must reproduce the same file set, a `valid` spec must actually
 * survive the parser it was aimed at, and a corruption must be the one its id
 * claims. Those three are what these pin.
 *
 * The section-count check is the load-bearing one. `split_into_sections` cuts on
 * a `3` byte followed by a run of zeros, so a shot record that happens to be
 * mostly zeros silently splits a section in half - a file that still "parses",
 * with a different survey in it. The invariant documented at the top of
 * `surveyFileSpecs.ts` (never draw `length` or `markerIndex` as zero) is what
 * prevents it, and this checks the consequence over enough seeds that the
 * argument does not have to be taken on trust.
 */
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import { describe, expect, it } from 'vitest';

import { corruptSpec, corruptionsFor } from './surveyFileCorrupt';
import {
  type GeneratedFileSpec,
  type Rng,
  HOSTILE_COMMENTS,
  materializeSurveyFile,
  randomDmpSpec,
  randomSurveyFileSpec,
  randomTmlSpec,
  renderSurveyFile,
  suggestiveDmpSpec,
} from './surveyFileSpecs';
import { dmpFileBytes, endOfSectionBytes, escapeXmlText, writeTmluXml } from './surveyFileWriters';
import { seedWindow } from './seedWindow';

/**
 * A local copy of `scenarios/framework.ts`'s `mulberry32`.
 *
 * Copied rather than imported: `framework.ts` pulls in the WebdriverIO session
 * surface and, through `fixtures.ts`, `node:sqlite` - none of which a pure
 * generator test has any use for. The property under test is "the same stream
 * produces the same spec", which any deterministic PRNG demonstrates.
 */
function mulberry32(seed: number): Rng {
  let a = seed >>> 0;
  return () => {
    a |= 0;
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/** `split_into_sections`, reduced to the count. */
function countSections(bytes: readonly number[], version: number): number {
  const patterns = [endOfSectionBytes(version, 'normal')];
  if (version === 5) patterns.push(endOfSectionBytes(version, 'buggy'));
  const length = patterns[0]!.length; // built with one entry just above
  if (length === 0 || bytes.length < length) return 0;
  let found = 0;
  for (let index = 0; index + length <= bytes.length; index++) {
    const window = bytes.slice(index, index + length);
    if (patterns.some((pattern) => pattern.every((value, offset) => value === window[offset]))) {
      found++;
      index += length - 1;
    }
  }
  return found;
}

/** 1..120, or the window the STALAGMIGHT_VITEST_SEED_* knobs ask for (seedWindow.ts). */
const SEEDS = seedWindow(1, 120);

describe('randomDmpSpec (valid)', () => {
  it('never lets a shot record terminate its own section', () => {
    for (const seed of SEEDS) {
      const spec = randomDmpSpec(mulberry32(seed), 'valid');
      expect(countSections(dmpFileBytes(spec), spec.version)).toBe(spec.sections.length);
    }
  });

  it('stays inside every range parse_section validates', () => {
    for (const seed of SEEDS) {
      const spec = randomDmpSpec(mulberry32(seed), 'valid');
      expect([2, 3, 4, 5]).toContain(spec.version);
      for (const section of spec.sections) {
        expect(section.year).toBeGreaterThanOrEqual(0);
        expect(section.year).toBeLessThanOrEqual(99);
        expect(section.month).toBeGreaterThanOrEqual(1);
        expect(section.month).toBeLessThanOrEqual(12);
        // Never 29..31: those exist in some months and not others, and a spec
        // that parses in March but not February is not a `valid` spec.
        expect(section.day).toBeGreaterThanOrEqual(1);
        expect(section.day).toBeLessThanOrEqual(28);
        expect(section.hour).toBeLessThanOrEqual(23);
        expect(section.minute).toBeLessThanOrEqual(59);
        expect([0, 1]).toContain(section.direction);
        expect(section.shots.length).toBeGreaterThan(0);
        for (const shot of section.shots) {
          expect([0, 1, 2]).toContain(shot.shotType);
          expect(shot.length).not.toBe(0);
          expect(shot.markerIndex).not.toBe(0);
        }
      }
    }
  });

  it('reaches the duplicate-content shape a re-import has to notice', () => {
    const clones = SEEDS.map((seed) => randomDmpSpec(mulberry32(seed), 'valid')).filter((spec) =>
      spec.sections.some((section) => section.sameContentAs !== undefined),
    );
    expect(clones.length).toBeGreaterThan(0);
  });
});

/**
 * The suggester's per-shot rule, mirrored.
 *
 * A MIRROR of `compute_sequence_shot_score` + `max_sequence_length_error`
 * (`crates/models/src/placement_suggest.rs`) against `PlacementSuggestionConfig`
 * defaults, not an import of them - the Rust is not reachable from vitest, and
 * the property under test is exactly "the generated overlap stays inside those
 * numbers". If the product ever retunes them this mirror goes stale, and the
 * failure mode is the honest one: this test keeps passing while the scenario
 * stops finding suggestions, which the campaign's `screen:wiz-suggest` counter
 * is what actually watches - and `suggesterDrift`, which the scenario now emits
 * when suggestion-bearing files were imported and the wizard never reached
 * `wiz-suggest`. The mirror is here to catch the generator drifting, which is
 * the far likelier direction.
 *
 * These are the DEFAULTS. `PlacementSuggestionConfig` is serde-backed and
 * user-configurable, so a project that retuned it would score differently; the
 * calibration holds because a harness session runs on a fresh profile and never
 * writes those keys.
 */
const PS = {
  azimuthSigma: 18,
  lengthSigma: 0.8,
  depthSigma: 0.35,
  azimuthWeight: 0.35,
  lengthWeight: 0.25,
  depthInWeight: 0.2,
  depthOutWeight: 0.2,
  minShotScore: 0.15,
  minAggregateScore: 0.45,
  maxReferenceShots: 5,
  maxAzimuthErrorFull: 25,
  maxDepthError: 0.3,
  seqLengthBase: 0.03,
  seqLengthStep: 0.02,
  seqLengthMax: 0.11,
  lengthFloor: 0.4,
} as const;

/** What the suggester actually compares: azimuth, length and the two depths. */
interface MatchShot {
  azimuth: number;
  length: number;
  depthIn: number;
  depthOut: number;
}

/** `DmpShotSpec` in its pre-scaled integers -> the parsed values, as `parse_shot` scales them. */
function toMatchShot(shot: { headingIn: number; headingOut: number; length: number; depthIn: number; depthOut: number }): MatchShot {
  // The generator writes `headingIn === headingOut`, so `average_azimuth_of`
  // reduces to the heading itself.
  return {
    azimuth: shot.headingIn / 10,
    length: shot.length / 100,
    depthIn: shot.depthIn / 100,
    depthOut: shot.depthOut / 100,
  };
}

function reversedShot(shot: MatchShot): MatchShot {
  return {
    azimuth: (shot.azimuth + 180) % 360,
    length: shot.length,
    depthIn: shot.depthOut,
    depthOut: shot.depthIn,
  };
}

function angularDifference(a: number, b: number): number {
  const diff = ((a - b) % 360 + 360) % 360;
  return diff > 180 ? 360 - diff : diff;
}

function gaussian(error: number, sigma: number): number {
  return Math.exp(-((error / sigma) ** 2));
}

function sequenceShotScore(importShot: MatchShot, existing: MatchShot, matchedCount: number): number {
  const azError = angularDifference(importShot.azimuth, existing.azimuth);
  const lenError = Math.abs(importShot.length - existing.length);
  const depthInError = Math.abs(importShot.depthIn - existing.depthIn);
  const depthOutError = Math.abs(importShot.depthOut - existing.depthOut);
  const fraction = Math.min(
    PS.seqLengthMax,
    PS.seqLengthBase + PS.seqLengthStep * Math.max(0, matchedCount - 1),
  );
  const maxLenError = Math.max(fraction * existing.length, PS.lengthFloor);
  if (
    azError > PS.maxAzimuthErrorFull ||
    depthInError > PS.maxDepthError ||
    depthOutError > PS.maxDepthError ||
    lenError > maxLenError
  ) {
    return 0;
  }
  return (
    PS.azimuthWeight * gaussian(azError, PS.azimuthSigma) +
    PS.lengthWeight * gaussian(lenError, PS.lengthSigma) +
    PS.depthInWeight * gaussian(depthInError, PS.depthSigma) +
    PS.depthOutWeight * gaussian(depthOutError, PS.depthSigma)
  );
}

/** `oriented_existing_shots`: forward from `start`, or backwards-and-flipped to it. */
function orientedTarget(target: readonly MatchShot[], start: number, reverse: boolean): MatchShot[] {
  return reverse
    ? target.slice(0, start + 1).reverse().map(reversedShot)
    : target.slice(start);
}

/**
 * The best in-order run this leaf makes against `target`, as
 * (matched shots, aggregate score). A straight walk rather than the product's
 * DP: the generated overlap is a contiguous copy, so the DP's gap handling has
 * nothing to find that this does not.
 */
function bestRun(leaf: readonly MatchShot[], target: readonly MatchShot[]): { matched: number; score: number } {
  let best = { matched: 0, score: 0 };
  for (let start = 0; start < target.length; start++) {
    for (const reverse of [false, true]) {
      const oriented = orientedTarget(target, start, reverse);
      let sum = 0;
      let count = 0;
      for (let i = 0; i < leaf.length && i < oriented.length; i++) {
        const score = sequenceShotScore(leaf[i]!, oriented[i]!, PS.maxReferenceShots);
        if (score < PS.minShotScore) break;
        sum += score;
        count += 1;
        const mean = sum / count;
        if (count > best.matched || (count === best.matched && mean > best.score)) {
          best = { matched: count, score: mean };
        }
      }
    }
  }
  return best;
}

describe('suggestiveDmpSpec', () => {
  it('never lets a shot record terminate its own section', () => {
    for (const seed of SEEDS) {
      const spec = suggestiveDmpSpec(mulberry32(seed));
      expect(countSections(dmpFileBytes(spec), spec.version)).toBe(spec.sections.length);
    }
  });

  it('stays inside every range parse_section validates', () => {
    for (const seed of SEEDS) {
      const spec = suggestiveDmpSpec(mulberry32(seed));
      expect([3, 4, 5]).toContain(spec.version);
      expect(spec.sections.length).toBeGreaterThanOrEqual(2);
      for (const section of spec.sections) {
        expect(section.year).toBeGreaterThanOrEqual(0);
        expect(section.year).toBeLessThanOrEqual(99);
        expect(section.month).toBeGreaterThanOrEqual(1);
        expect(section.month).toBeLessThanOrEqual(12);
        expect(section.day).toBeGreaterThanOrEqual(1);
        expect(section.day).toBeLessThanOrEqual(28);
        expect(section.hour).toBeLessThanOrEqual(23);
        expect(section.minute).toBeLessThanOrEqual(59);
        expect(section.direction).toBe(0);
        for (const shot of section.shots) {
          // Standard only: it is the one type the wizard counts when it sizes
          // the merge-offset list.
          expect(shot.shotType).toBe(2);
          expect(shot.length).not.toBe(0);
          expect(shot.markerIndex).not.toBe(0);
          expect(shot.headingIn).toBeGreaterThanOrEqual(0);
          expect(shot.headingIn).toBeLessThan(3600);
          // The horizontal projection has to be real, or the placed geometry
          // disagrees with the numbers the match was made on.
          expect(Math.abs(shot.depthOut - shot.depthIn)).toBeLessThan(shot.length);
        }
      }
    }
  });

  it('gives every replay section a run the suggester would accept', () => {
    for (const seed of SEEDS) {
      const spec = suggestiveDmpSpec(mulberry32(seed));
      const anchor = spec.sections[0]!.shots.map(toMatchShot);
      for (let index = 1; index < spec.sections.length; index++) {
        const shots = spec.sections[index]!.shots.map(toMatchShot);
        const maxRef = Math.min(PS.maxReferenceShots, shots.length);
        // The two leaves `suggest_placements` builds: the first N in natural
        // order (station 0) and the last N reversed (station N).
        const fromZero = shots.slice(0, maxRef);
        const fromN = shots.slice(shots.length - maxRef).reverse().map(reversedShot);
        const best = [bestRun(fromZero, anchor), bestRun(fromN, anchor)].sort(
          (a, b) => b.matched - a.matched || b.score - a.score,
        )[0]!;
        expect(best.matched, `seed ${seed} section ${index}: matched shots`).toBeGreaterThanOrEqual(2);
        expect(best.score, `seed ${seed} section ${index}: aggregate`).toBeGreaterThan(PS.minAggregateScore);
      }
    }
  });

  it('repeats the motif inside the anchor, so a replay has more than one home', () => {
    // The alternatives list is filtered to suggestions sharing
    // `importSectionIndex`, so a replay section has to match its anchor in TWO
    // places or the list is always empty and its tie-break never runs.
    for (const seed of SEEDS) {
      const spec = suggestiveDmpSpec(mulberry32(seed));
      const anchor = spec.sections[0]!.shots.map(toMatchShot);
      const replay = spec.sections[1]!.shots.map(toMatchShot);
      const leaf = replay.slice(0, Math.min(PS.maxReferenceShots, replay.length));
      let homes = 0;
      for (let start = 0; start < anchor.length; start++) {
        const oriented = orientedTarget(anchor, start, false);
        let count = 0;
        for (let i = 0; i < leaf.length && i < oriented.length; i++) {
          if (sequenceShotScore(leaf[i]!, oriented[i]!, PS.maxReferenceShots) < PS.minShotScore) break;
          count += 1;
        }
        if (count >= 2) homes += 1;
      }
      expect(homes, `seed ${seed}: distinct anchor runs the replay fits`).toBeGreaterThanOrEqual(2);
    }
  });

  it('is deterministic for a seed', () => {
    for (const seed of SEEDS.slice(0, 20)) {
      expect(JSON.stringify(suggestiveDmpSpec(mulberry32(seed)))).toBe(
        JSON.stringify(suggestiveDmpSpec(mulberry32(seed))),
      );
    }
  });

  it('lets two DIFFERENT files match each other often enough to matter', () => {
    // The orphan surface needs it. An orphan component is a set of EXISTING
    // sections with no datum and no newly-imported member, so this fuzzer can
    // only make one by placing a segment manually in one flow and importing
    // something that MATCHES it in a later flow. With every file drawing a
    // fresh random motif that later import never matched, and
    // `screen:orphan-placement` stayed at zero for 214 campaign cells.
    const specs = SEEDS.map((seed) => suggestiveDmpSpec(mulberry32(seed)));
    let pairs = 0;
    let matched = 0;
    for (let i = 0; i < specs.length; i += 1) {
      for (let j = i + 1; j < specs.length; j += 1) {
        pairs += 1;
        const anchor = specs[i]!.sections[0]!.shots.map(toMatchShot);
        const leaf = specs[j]!.sections[1]!.shots.map(toMatchShot).slice(0, PS.maxReferenceShots);
        const best = bestRun(leaf, anchor);
        if (best.matched >= 2 && best.score > PS.minAggregateScore) matched += 1;
      }
    }
    // Four shapes, drawn uniformly, so a random pair shares one about a quarter
    // of the time. Asserted as a BAND: too low and the orphan surface goes dark
    // again, too high and every generated file is the same survey.
    const rate = matched / pairs;
    expect(rate, `cross-file match rate (${matched}/${pairs})`).toBeGreaterThan(0.1);
    expect(rate, `cross-file match rate (${matched}/${pairs})`).toBeLessThan(0.6);
  });
});

describe('randomTmlSpec (valid)', () => {
  it('builds a rooted tree with unique non-negative ids', () => {
    for (const seed of SEEDS) {
      const spec = randomTmlSpec(mulberry32(seed), 'valid');
      const ids = spec.shots.map((shot) => Number(shot.id));
      expect(new Set(ids).size).toBe(ids.length);
      expect(ids.every((id) => id >= 0)).toBe(true);
      const first = spec.shots[0];
      if (first === undefined) throw new Error(`seed ${seed}: the TML spec has no shots`);
      expect(first.type).toBe('START');
      expect(Number(first.fromId)).toBe(-1);
      for (let index = 1; index < spec.shots.length; index++) {
        expect(Number(spec.shots[index]!.fromId)).toBeLessThan(index);
        expect(Number(spec.shots[index]!.fromId)).toBeGreaterThanOrEqual(0);
      }
    }
  });

  it('emits a document with no raw angle bracket outside a tag', () => {
    for (const seed of SEEDS.slice(0, 40)) {
      const xml = writeTmluXml(randomTmlSpec(mulberry32(seed), 'valid'));
      // Every `<` must open a tag: text is escaped, so a `<` not followed by a
      // name or a slash means something leaked through unescaped.
      expect(/<(?![/?A-Za-z])/.test(xml)).toBe(false);
      expect(xml.match(/<Data>/g) ?? xml.match(/<Data\/>/g)).toHaveLength(1);
    }
  });
});

describe('determinism', () => {
  it('reproduces the same spec and the same bytes from the same seed', () => {
    for (const seed of SEEDS.slice(0, 20)) {
      for (const profile of ['valid', 'malformed'] as const) {
        const a = randomSurveyFileSpec(mulberry32(seed), profile);
        const b = randomSurveyFileSpec(mulberry32(seed), profile);
        expect(b).toEqual(a);
        expect(b.name).toBe(a.name);
        expect([...renderSurveyFile(b)]).toEqual([...renderSurveyFile(a)]);
      }
    }
  });

  it('gives two identical specs the same content-addressed name', () => {
    const first = randomSurveyFileSpec(mulberry32(3), 'valid', 'dmp');
    const second = randomSurveyFileSpec(mulberry32(3), 'valid', 'dmp');
    // This is the mechanism behind "the user imported the same file twice":
    // no separate sameFileAs flag, just an emission that cannot differ.
    expect(second.name).toBe(first.name);
  });
});

describe('corruptSpec', () => {
  it('is deterministic in the rng, spec and all', () => {
    for (const seed of SEEDS.slice(0, 30)) {
      const clean = randomSurveyFileSpec(mulberry32(seed), 'valid');
      const a = corruptSpec(mulberry32(seed * 31), clean);
      const b = corruptSpec(mulberry32(seed * 31), clean);
      expect(b.corruption).toBe(a.corruption);
      expect(b.spec).toEqual(a.spec);
    }
  });

  it('leaves the clean spec untouched', () => {
    const clean = randomSurveyFileSpec(mulberry32(11), 'valid', 'dmp');
    const before = JSON.stringify(clean);
    corruptSpec(mulberry32(4), clean);
    expect(JSON.stringify(clean)).toBe(before);
  });

  it('only draws corruptions that apply to the target', () => {
    const tmlu = randomSurveyFileSpec(mulberry32(5), 'valid', 'tmlu');
    expect(corruptionsFor(tmlu).some((id) => id.startsWith('tml:zip-'))).toBe(false);
    const tml = randomSurveyFileSpec(mulberry32(5), 'valid', 'tml');
    expect(corruptionsFor(tml)).toContain('tml:zip-wrong-entry');

    for (const seed of SEEDS.slice(0, 60)) {
      const spec = randomSurveyFileSpec(mulberry32(seed), 'valid', 'dmp');
      // The declared format is what came back: asking for one and being handed
      // another would make every assertion below vacuous.
      if (spec.format !== 'dmp') throw new Error(`asked for dmp, got ${spec.format}`);
      const ids = corruptionsFor(spec);
      if (spec.dmp.version < 5) {
        expect(ids).not.toContain('dmp:bad-shot-start-magic');
        expect(ids).not.toContain('dmp:buggy-eos');
      }
      if (spec.dmp.version === 2) expect(ids).not.toContain('dmp:bad-section-magic');
      // Whatever the version, the id drawn is one the catalog offered.
      expect(ids).toContain(corruptSpec(mulberry32(seed + 7), spec).corruption);
    }
  });

  it('renders every catalog entry to bytes without throwing, and bounds the big one', () => {
    const seen = new Set<string>();
    for (const seed of Array.from({ length: 600 }, (_unused, index) => index + 1)) {
      const clean = randomSurveyFileSpec(mulberry32(seed), 'valid');
      const { spec, corruption } = corruptSpec(mulberry32(seed * 7919), clean);
      const bytes = renderSurveyFile(spec);
      seen.add(corruption);
      expect(bytes.length).toBeLessThanOrEqual(4 * 1024 * 1024);
      if (corruption === 'dmp:oversized') expect(bytes.length).toBeGreaterThan(512 * 1024);
    }
    // Enough of the catalog is reachable that a run of a few hundred iterations
    // is really covering rejection paths, not re-drawing three of them.
    expect(seen.size).toBeGreaterThan(30);
  });
});

describe('HOSTILE_COMMENTS', () => {
  it('escapes to text with no markup left in it', () => {
    for (const comment of HOSTILE_COMMENTS) {
      const escaped = escapeXmlText(comment);
      expect(escaped).not.toMatch(/[<>]/);
      // Every surviving `&` is the start of one of the five entities.
      expect(escaped.replace(/&(amp|lt|gt|quot|apos);/g, '')).not.toContain('&');
    }
  });

  it('carries the shapes the catalog documents', () => {
    expect(HOSTILE_COMMENTS).toContain("=cmd|' /C calc'!A0");
    expect(HOSTILE_COMMENTS.some((text) => text.startsWith('\t'))).toBe(true);
    expect(HOSTILE_COMMENTS.some((text) => text.startsWith('\r'))).toBe(true);
    expect(HOSTILE_COMMENTS.some((text) => text.includes('\u200b'))).toBe(true);
    expect(HOSTILE_COMMENTS.some((text) => text.includes('\u202e'))).toBe(true);
    expect(HOSTILE_COMMENTS.some((text) => text.length >= 8 * 1024)).toBe(true);
    // A lone surrogate would not survive TextEncoder, so the corpus carries the
    // replacement character instead and says so.
    expect(HOSTILE_COMMENTS.some((text) => text.includes('\ufffd'))).toBe(true);
    for (const text of HOSTILE_COMMENTS) {
      expect(new TextDecoder().decode(new TextEncoder().encode(text))).toBe(text);
    }
  });
});

describe('materializeSurveyFile', () => {
  it('writes the rendered bytes under the spec name', () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'stalagmight-fuzz-files-'));
    try {
      const spec = randomSurveyFileSpec(mulberry32(2), 'valid', 'dmp');
      const nested = path.join(dir, 'fuzz-files');
      const written = materializeSurveyFile(nested, spec);
      expect(written.path).toBe(path.join(nested, spec.name));
      expect(fs.readFileSync(written.path)).toEqual(Buffer.from(written.bytes));
      expect(spec.name.endsWith('.dmp')).toBe(true);
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });

  it('refuses a name that is not a plain basename', () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'stalagmight-fuzz-files-'));
    try {
      const spec = randomSurveyFileSpec(mulberry32(2), 'valid', 'dmp');
      for (const name of ['../escape.dmp', 'sub/dir.dmp', '', '.hidden.dmp']) {
        const hostile = { ...spec, name } as GeneratedFileSpec;
        expect(() => materializeSurveyFile(dir, hostile)).toThrow(/plain basename/);
      }
      expect(fs.readdirSync(dir)).toEqual([]);
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });
});
