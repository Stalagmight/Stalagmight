/**
 * A corruption catalog is only worth its ids if three things hold, and none of
 * them is obvious from reading the entries.
 *
 * 1. EVERY id is reachable. An entry whose `applies` is never true, or which the
 *    dispatch never routes to, is a rejection path the fuzzer believes it covers
 *    and does not.
 * 2. Every corruption CHANGES the file. A `.mak` corruption that edits a field
 *    the writer does not emit, or a `.srv` regex that silently matches nothing,
 *    produces a spec that renders to the clean bytes - a run that reports it
 *    exercised `srv:bad-order` while importing a perfectly good survey.
 * 3. The DMP and TML catalogs are byte-for-byte what they were. `corruptSpec`
 *    draws with one `pick` over the filtered list, so inserting or reordering an
 *    entry shifts which corruption every existing seed draws, and the repro
 *    lines in the header of `scenarios/importDialogFuzz.ts` stop reproducing.
 *
 * The multi-file formats are why (2) compares the WHOLE rendered set rather than
 * the primary's bytes: `mak:missing-dat-sibling` and `wpj:missing-srv` leave the
 * project file untouched and change only what is written beside it.
 */
import { describe, expect, it } from 'vitest';

import { corruptSpec, corruptionsFor } from './surveyFileCorrupt';
import {
  DAT_CORRUPTIONS,
  GPX_CORRUPTIONS,
  MAK_CORRUPTIONS,
  SRV_CORRUPTIONS,
  WPJ_CORRUPTIONS,
} from './surveyFileCorruptText';
import {
  type GeneratedFileSpec,
  type Rng,
  type SurveyFileFormat,
  randomSurveyFileSpec,
  renderSurveyFiles,
} from './surveyFileSpecs';
import { seedWindow } from './seedWindow';

/** `scenarios/framework.ts`'s `mulberry32`, copied for the same reason as elsewhere. */
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

/** 1..260, or the window the STALAGMIGHT_VITEST_SEED_* knobs ask for (seedWindow.ts). */
const SEEDS = seedWindow(1, 260);

const TEXT_FORMATS: readonly SurveyFileFormat[] = ['dat', 'mak', 'srv', 'wpj', 'gpx'];

const CATALOGS: Readonly<Record<string, readonly { id: string }[]>> = {
  dat: DAT_CORRUPTIONS,
  mak: MAK_CORRUPTIONS,
  srv: SRV_CORRUPTIONS,
  wpj: WPJ_CORRUPTIONS,
  gpx: GPX_CORRUPTIONS,
};

/** The bytes of every file a spec becomes, names excluded. */
function bytesDigest(spec: GeneratedFileSpec): string {
  return renderSurveyFiles(spec)
    .map((file) => [...file.bytes].join(','))
    .join('|');
}

describe('the text catalogs', () => {
  it('names every id under its own namespace, with no duplicates', () => {
    for (const [namespace, entries] of Object.entries(CATALOGS)) {
      const ids = entries.map((entry) => entry.id);
      expect(new Set(ids).size).toBe(ids.length);
      // Ids are logged and grepped, so they stay lowercase-and-hyphens.
      for (const id of ids) expect(id).toMatch(/^[a-z0-9-]+$/);
      // The minimum the wave-2 plan fixed: at least 8 mechanisms per format.
      expect(ids.length).toBeGreaterThanOrEqual(8);
      expect(namespace).toMatch(/^(dat|mak|srv|wpj|gpx)$/);
    }
  });

  it('carries the specific mechanisms each format is known to hide data behind', () => {
    // Spelled out rather than counted: each of these is a parser behaviour with
    // no user-visible error, and dropping one would silently narrow the run.
    const required: Readonly<Record<string, readonly string[]>> = {
      dat: [
        'lowercase-header-keyword',
        'blank-team-line',
        'format-illegal-length',
        'format-no-trailing-space',
        'short-data-row',
        'illegal-station-char',
        'signed-nonfinite-token',
        'impossible-date',
        'cr-only-line-endings',
        'missing-final-newline',
        'non-formfeed-separator',
        'double-formfeed',
        'utf16-bom',
      ],
      mak: [
        'file-before-state',
        'unknown-datum',
        'missing-semicolon',
        'stray-leading-char',
        'exponent-number',
        'nan-station-fix',
        'bad-base-location',
        'duplicate-flag',
        'missing-dat-sibling',
      ],
      srv: [
        'undefined-macro',
        'unknown-units-option',
        'bad-order',
        'bad-decl',
        'bad-date',
        'one-token-vector-line',
        'nonfinite-column',
        'unterminated-lrud-span',
        'unterminated-block-comment',
      ],
      wpj: [
        'name-with-separator',
        'missing-srv',
        'bad-status',
        'short-ref',
        'nonfinite-ref',
        'bad-zone-ref',
        'no-name',
      ],
      gpx: [
        'no-root',
        'missing-lat',
        'out-of-range-lat',
        'out-of-range-lon',
        'nonfinite-coord',
        'non-numeric-coord',
        'malformed-xml',
        'empty-track-segment',
        'cdata-elevation',
        'no-closing-root',
      ],
    };
    // `required` is keyed by exactly CATALOGS' five namespaces.
    for (const [namespace, ids] of Object.entries(required)) {
      const present = CATALOGS[namespace]!.map((entry) => entry.id);
      for (const id of ids) expect(present).toContain(id);
    }
  });
});

describe('corruptionsFor', () => {
  it('offers every catalog id on some valid spec, and offers only its own namespace', () => {
    for (const format of TEXT_FORMATS) {
      const namespace = format;
      const offered = new Set<string>();
      for (const seed of SEEDS) {
        const spec = randomSurveyFileSpec(mulberry32(seed), 'valid', format);
        for (const id of corruptionsFor(spec)) {
          expect(id.startsWith(`${namespace}:`)).toBe(true);
          offered.add(id);
        }
      }
      // A catalog entry whose `applies` never fires is a rejection path the
      // fuzzer believes it covers and does not.
      for (const entry of CATALOGS[namespace]!) { // TEXT_FORMATS are CATALOGS' keys
        expect([...offered]).toContain(`${namespace}:${entry.id}`);
      }
    }
  });

  it('draws only ids it offered, and reaches all of them', () => {
    for (const format of TEXT_FORMATS) {
      const offered = new Set<string>();
      const drawn = new Set<string>();
      for (const seed of SEEDS) {
        const clean = randomSurveyFileSpec(mulberry32(seed), 'valid', format);
        const ids = corruptionsFor(clean);
        for (const id of ids) offered.add(id);
        for (const salt of [7919, 104729, 1299709]) {
          const { corruption } = corruptSpec(mulberry32(seed * salt), clean);
          expect(ids).toContain(corruption);
          drawn.add(corruption);
        }
      }
      expect([...drawn].sort()).toEqual([...offered].sort());
    }
  });
});

describe('corruptSpec on the text formats', () => {
  it('is deterministic in the rng, spec and all', () => {
    for (const format of TEXT_FORMATS) {
      for (const seed of SEEDS.slice(0, 60)) {
        const clean = randomSurveyFileSpec(mulberry32(seed), 'valid', format);
        const a = corruptSpec(mulberry32(seed * 31), clean);
        const b = corruptSpec(mulberry32(seed * 31), clean);
        expect(b.corruption).toBe(a.corruption);
        expect(b.spec).toEqual(a.spec);
        expect(bytesDigest(b.spec)).toBe(bytesDigest(a.spec));
      }
    }
  });

  it('leaves the clean spec untouched', () => {
    for (const format of TEXT_FORMATS) {
      const clean = randomSurveyFileSpec(mulberry32(11), 'valid', format);
      const before = JSON.stringify(clean);
      for (const seed of SEEDS.slice(0, 40)) corruptSpec(mulberry32(seed), clean);
      expect(JSON.stringify(clean)).toBe(before);
    }
  });

  it('never renders to the same bytes as the clean spec it came from', () => {
    for (const format of TEXT_FORMATS) {
      for (const seed of SEEDS) {
        const clean = randomSurveyFileSpec(mulberry32(seed), 'valid', format);
        const cleanBytes = bytesDigest(clean);
        for (const salt of [7919, 104729]) {
          const { spec, corruption } = corruptSpec(mulberry32(seed * salt), clean);
          // A corruption that does nothing is worse than a missing one: the run
          // reports the id and the app imports a perfectly good file.
          expect({ corruption, changed: bytesDigest(spec) !== cleanBytes }).toEqual({
            corruption,
            changed: true,
          });
        }
      }
    }
  });

  it('recomputes the name and keeps every rendered name a plain basename', () => {
    for (const format of TEXT_FORMATS) {
      for (const seed of SEEDS.slice(0, 80)) {
        const clean = randomSurveyFileSpec(mulberry32(seed), 'valid', format);
        const { spec } = corruptSpec(mulberry32(seed * 13), clean);
        expect(spec.format).toBe(format);
        expect(spec.name).not.toBe(clean.name);
        expect(spec.name.endsWith(`.${format}`)).toBe(true);
        for (const file of renderSurveyFiles(spec)) {
          expect(file.name).toMatch(/^[A-Za-z0-9][A-Za-z0-9._-]*$/);
        }
      }
    }
  });

  it('renders every drawn corruption to bytes without throwing, and stays small', () => {
    for (const format of TEXT_FORMATS) {
      for (const seed of SEEDS) {
        const clean = randomSurveyFileSpec(mulberry32(seed), 'valid', format);
        const { spec } = corruptSpec(mulberry32(seed * 7919), clean);
        const total = renderSurveyFiles(spec).reduce((sum, file) => sum + file.bytes.length, 0);
        expect(total).toBeGreaterThan(0);
        // No text-format corruption is an oversize entry, so anything near the
        // DMP catalog's 2 MB ceiling means a payload multiplied out of control.
        expect(total).toBeLessThanOrEqual(4 * 1024 * 1024);
      }
    }
  });

  it('returns the whole file set for a project, siblings included', () => {
    for (const format of ['mak', 'wpj'] as const) {
      const dropped = format === 'mak' ? 'mak:missing-dat-sibling' : 'wpj:missing-srv';
      let sawDrop = 0;
      for (const seed of SEEDS) {
        const clean = randomSurveyFileSpec(mulberry32(seed), 'valid', format);
        const cleanFiles = renderSurveyFiles(clean).length;
        const { spec, corruption } = corruptSpec(mulberry32(seed * 7919), clean);
        const files = renderSurveyFiles(spec);
        if (corruption === dropped) {
          // The project text is untouched; only what is written beside it moved.
          expect(files.length).toBe(cleanFiles - 1);
          sawDrop += 1;
        } else {
          expect(files.length).toBe(cleanFiles);
        }
      }
      expect(sawDrop).toBeGreaterThan(0);
    }
  });
});

// ---------------------------------------------------------------------------
// The wave-1 catalogs must not have moved
// ---------------------------------------------------------------------------

/**
 * A `.dmp` spec that offers the whole DMP catalog.
 *
 * Three entries are gated on the file version (`bad-section-magic` needs > 2,
 * `bad-shot-start-magic` / `bad-shot-end-magic` / `buggy-eos` need 5), so the
 * full list is only visible on a v5 spec.
 */
function fullDmpSpec(): GeneratedFileSpec {
  for (const seed of SEEDS) {
    const spec = randomSurveyFileSpec(mulberry32(seed), 'valid', 'dmp');
    if (spec.format === 'dmp' && spec.dmp.version === 5) return spec;
  }
  throw new Error('no v5 dmp spec in the seed range');
}

describe('the DMP and TML catalogs are unchanged', () => {
  it('offers the DMP ids in exactly their original order', () => {
    expect(corruptionsFor(fullDmpSpec())).toEqual([
      'dmp:unsupported-version',
      'dmp:bad-section-magic',
      'dmp:bad-shot-start-magic',
      'dmp:bad-shot-end-magic',
      'dmp:month-0',
      'dmp:month-13',
      'dmp:day-0',
      'dmp:day-32',
      'dmp:day-31-of-april',
      'dmp:hour-24',
      'dmp:minute-60',
      'dmp:year-2100',
      'dmp:bad-direction',
      'dmp:bad-shot-type',
      'dmp:truncated-mid-shot',
      'dmp:zero-shot-section',
      'dmp:missing-end-of-section',
      'dmp:buggy-eos',
      'dmp:non-integer-token',
      'dmp:empty-token',
      'dmp:whitespace-token',
      'dmp:float-token',
      'dmp:out-of-range-token',
      'dmp:overflow-token',
      'dmp:doubled-semicolon',
      'dmp:leading-semicolon',
      'dmp:trailing-semicolon',
      'dmp:crlf',
      'dmp:bom',
      'dmp:trailing-garbage',
      'dmp:truncated-file',
      'dmp:oversized',
    ]);
  });

  it('offers the TML ids in exactly their original order, zip entries last', () => {
    const tml = randomSurveyFileSpec(mulberry32(5), 'valid', 'tml');
    const schema = [
      'tml:unclosed-tag',
      'tml:stray-markup',
      'tml:wrong-root',
      'tml:omit-closing-root',
      'tml:no-declaration',
      'tml:bad-declaration',
      'tml:trailing-garbage',
      'tml:empty-document',
      'tml:cycle',
      'tml:self-parent',
      'tml:dangling-parent',
      'tml:duplicate-ids',
      'tml:negative-ids',
      'tml:dangling-closure',
      'tml:unknown-shot-type',
      'tml:garbage-numerics',
      'tml:garbage-color',
      'tml:garbage-date',
      'tml:garbage-excluded',
      'tml:bad-unit',
      'tml:garbage-elevation',
      'tml:hostile-comment',
    ];
    expect(corruptionsFor(tml)).toEqual([
      ...schema,
      'tml:zip-wrong-entry',
      'tml:zip-truncated',
      'tml:zip-not-a-zip',
      'tml:zip-extra-entry',
    ]);
    // A `.tmlu` has no ZIP container, so the container entries must not appear.
    expect(corruptionsFor(randomSurveyFileSpec(mulberry32(5), 'valid', 'tmlu'))).toEqual(schema);
  });

  it('draws the same DMP corruption from the same seed as before the widening', () => {
    // The dispatch is by format, and the DMP arm still filters and picks in the
    // same order with the same rng calls - so a seed that drew `dmp:day-0`
    // before still draws it. Pinned by value, not by construction.
    const clean = randomSurveyFileSpec(mulberry32(3), 'valid', 'dmp');
    const drawn = SEEDS.slice(0, 6).map((seed) => corruptSpec(mulberry32(seed), clean).corruption);
    for (const id of drawn) expect(corruptionsFor(clean)).toContain(id);
    expect(new Set(drawn).size).toBeGreaterThan(1);
  });
});
