/**
 * #576 W58 Q10: fuzz-shrink prints a trace step's label only when it is made of
 * the words the scenarios write a label from (`stepLabels.ts`). Synthetic names
 * only (`qz...`, which no harness or app source contains).
 */
import { describe, expect, it } from 'vitest';

import { mulberry32, type ScenarioContext } from '../scenarios/framework';
import { CHECKABLE_TESTIDS, CLICKABLE_TESTIDS, SELECTABLE_TESTIDS, TYPEABLE_TESTIDS } from '../scenarios/importDialogBridge';
import { GENERATORS, buildAuxInterjection, buildInterjection, type GenEnv } from '../scenarios/importDialogGenerators';
import { moveTraceStep, sourceTraceStep } from '../scenarios/importDialogTrace';
import { ACTION_GENERATORS, type TargetingDigest } from '../scenarios/undoRedoFuzzActions';
import { IMPORT_ACTION_GENERATORS } from '../scenarios/undoRedoFuzzImportActions';
import { PATH_SHAPES, WRITING_FORMATS } from './exportOracles';
import { shortId } from './crc32';
import { describeSteps, type TraceStep } from './fuzzTrace';
import { harnessWords, shareableStepLabel, stepLabelShaped } from './stepLabels';

/** A deterministic ScenarioContext stub: only the draw surface is exercised. */
function fakeCtx(seed: number): ScenarioContext {
  const rand = mulberry32(seed);
  return {
    seed,
    rng: () => rand(),
    randInt: (min: number, max: number) => min + Math.floor(rand() * (max - min + 1)),
    pick: <T,>(items: readonly T[]) => items[Math.floor(rand() * items.length)] as T,
  } as unknown as ScenarioContext;
}

/** Two sections, a merge, a datum and a closure: every undo-redo generator has a target. */
const DIGEST: TargetingDigest = {
  s: [
    [1, [100000, 100001, 100002]],
    [2, [200000, 200001]],
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
  m: [[1, 3, 2, 0]],
  z: [[1, 0]],
  c: [7],
};

/** Indexed test ids the import fuzzer's patterns admit, one or two of each. */
const INDEXED_IDS = [
  'alternative-0',
  'alternative-2',
  'zero-datum-section-card-1',
  'shot-editor-cell-0-length',
  'shot-editor-cell-3-azimuth_in',
  'shot-editor-gutter-0',
  'shot-editor-gutter-4',
  'gpx-placement-zero-datum-0',
  'gpx-placement-zero-datum-2',
];

/** Every label the scenarios' generators write over a spread of seeds, keyed to who wrote it. */
function writtenLabels(): Map<string, string> {
  const labels = new Map<string, string>();
  const add = (label: unknown, by: string): void => {
    if (typeof label === 'string' && !labels.has(label)) labels.set(label, by);
  };
  for (let seed = 1; seed <= 200; seed++) {
    for (const generator of [...ACTION_GENERATORS, ...IMPORT_ACTION_GENERATORS]) {
      add(generator.build(fakeCtx(seed * 7919 + generator.name.length), DIGEST)?.label, `undo-redo ${generator.name}`);
    }
  }
  const ids = [...CLICKABLE_TESTIDS, ...TYPEABLE_TESTIDS, ...SELECTABLE_TESTIDS, ...CHECKABLE_TESTIDS, ...INDEXED_IDS];
  const screens = [...new Set(GENERATORS.flatMap((generator) => generator.screens))];
  for (const state of [2, 1]) {
    const live = Object.fromEntries(ids.map((id) => [id, state]));
    for (const screen of screens) {
      for (let seed = 1; seed <= 40; seed++) {
        // Only the fields the generators read; a generator that needs more of
        // a live digest throws here and is skipped (the count below holds).
        const env = {
          ctx: fakeCtx(seed),
          digest: { screen, dialog: null, placement: false, ctx: 'existing-project', phase: null, hasProject: true, live },
          filesRel: 'fuzz-files',
          fileSeq: seed,
          corpus: {},
        } as unknown as GenEnv;
        for (const generator of GENERATORS.filter((candidate) => candidate.screens.includes(screen))) {
          try {
            const move = generator.build(env);
            if (move === null) continue;
            add(move.label, `import ${generator.name}`);
            // `TraceStep | null`: every kind these return carries a label.
            add((moveTraceStep(move) as { label?: unknown } | null)?.label, `import ${generator.name} step`);
            if ('source' in move) {
              add((sourceTraceStep(move.source) as { label?: unknown } | null)?.label, `import ${generator.name} file`);
            }
          } catch {
            // A digest field the stub does not carry.
          }
        }
        for (const kind of ['escape', 'route', 'undo', 'redo', 'save'] as const) add(buildInterjection(kind, env)?.label, kind);
        for (const kind of ['export', 'mode', 'close', 'openKey'] as const) add(buildAuxInterjection(kind, env)?.label, kind);
      }
      for (const testid of CLICKABLE_TESTIDS) add(`ESCAPE ${screen} via ${testid}`, 'importDialogFuzz escape');
      add(`FORCED advance on ${screen}`, 'importDialogFuzz forced');
    }
  }
  // exportRoundtripFuzz: an export's `<format> via <ExportRoute> at <PathShape>`, an oracle's name.
  for (const format of WRITING_FORMATS) {
    for (const route of ['modal', 'panel', 'store']) {
      for (const shape of PATH_SHAPES) add(`${format} via ${route} at ${shape}`, 'exportRoundtripFuzz export');
    }
  }
  for (const oracle of ['x3-differential', 'x4-byte-identity .tml', 'x4-byte-identity .tmlu', 'x5x6-roundtrip', 'x9b-pre-plan-leak']) {
    add(oracle, 'exportRoundtripFuzz oracle');
  }
  return labels;
}

describe("a fuzz trace step's label (#576 W58 Q10)", () => {
  it('keeps every label the scenarios write', () => {
    const labels = writtenLabels();
    expect(labels.size).toBeGreaterThan(1500);
    const writers = new Set([...labels.values()].filter((by) => by.startsWith('import ')).map((by) => by.split(' ')[1]));
    expect(writers.size).toBeGreaterThan(80);
    expect([...labels].filter(([label]) => !stepLabelShaped(label)).map(([label, by]) => `${by}: ${label}`)).toEqual([]);
  });

  it('refuses a word no source wrote, a control character, an over-long label and a non-string', () => {
    for (const word of ['qzCaveA', 'qzStationB', 'Höhleqz']) expect(harnessWords().has(word), word).toBe(false);
    for (const label of [
      'click qzCaveA',
      'name="qzStationB"',
      'route /home/qzuser/caves/qzCaveA.stalx',
      'set Höhleqz=true',
      'select all\nqz',
      'createShot(s=1,append)'.repeat(30),
      '',
      undefined,
      42,
    ]) {
      expect(stepLabelShaped(label), String(label)).toBe(false);
    }
    expect(shareableStepLabel('click qzCaveA')).toBe('a string of 13 character(s)');
    expect(shareableStepLabel(undefined)).toBe('undefined');
  });

  it("fuzz-shrink's minimal step list prints a forged label as its length and keeps a written one", () => {
    const steps: TraceStep[] = [
      { kind: 'mutate', action: 'createShot', args: [], label: 'createShot(s=1,after=100001)' },
      { kind: 'mutate', action: 'createShot', args: [], label: 'createShot(qzCaveA)' },
      { kind: 'ui', testid: 'continue-button', label: 'continue' },
      { kind: 'ui', testid: 'continue-button', label: 'qzCaveA continue' },
      { kind: 'input', testid: 'section-name-input', value: 'x', mode: 'type', label: 'name="qzStationB"' },
      { kind: 'fileDialog', trigger: 'toolbar-import-button', dialog: 'open', rel: null, label: 'cancel qzCaveA' },
      { kind: 'genFile', spec: {}, rel: 'fuzz-files/a.dmp', label: 'malformed dmp' },
      { kind: 'route', rel: 'fuzz-files/a.dmp', label: 'route /home/qzuser/qzCaveA.stalx' },
      { kind: 'mapClick', lngLat: [1, 2], label: 'place near qzCaveA' },
      { kind: 'export', format: 'csv', options: {}, rel: 'out/a.csv', label: 'csv via modal at qzCaveA' },
      { kind: 'oracle', oracle: 'x3-differential', inputs: {}, label: 'qzCaveA' },
    ];
    const out = describeSteps(steps);
    expect(out).not.toMatch(/qz/);
    expect(out).toContain('createShot(s=1,after=100001)');
    expect(out).toContain('click continue-button - continue');
    // Its path, which the repo does not track, as its tag (#576 W74-NF2).
    expect(out).toContain(`genFile <private-path#${shortId('fuzz-files/a.dmp')}> - malformed dmp`);
    expect(out).toContain('  2. a string of 19 character(s)');
    expect(out).toContain('type section-name-input - a string of 17 character(s)');
    expect(out).toContain('oracle x3-differential - a string of 7 character(s)');
  });
});
