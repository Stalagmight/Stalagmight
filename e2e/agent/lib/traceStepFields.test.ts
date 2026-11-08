/**
 * #576 W58 NF2: the tables `traceStepFields.ts` prints a trace step's trigger,
 * export format and oracle name from are the harness's own, so a value a
 * scenario records prints as itself and not as its length.
 */
import * as fs from 'fs';
import * as path from 'path';
import { fileURLToPath } from 'url';
import { describe, expect, it } from 'vitest';

import { mulberry32, type ScenarioContext } from '../scenarios/framework';
import { enabledFormats } from '../scenarios/exportFuzzActions';
import { CHECKABLE_TESTIDS, CLICKABLE_TESTIDS, SELECTABLE_TESTIDS, TYPEABLE_TESTIDS } from '../scenarios/importDialogBridge';
import { GENERATORS, buildAuxInterjection, buildInterjection, type FuzzMove, type GenEnv } from '../scenarios/importDialogGenerators';
import { moveTraceStep } from '../scenarios/importDialogTrace';
import { TRACE_EXPORT_FORMATS, TRACE_ORACLE_NAMES, TRACE_TRIGGERS } from './traceStepFields';

const AGENT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

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

/** Every trigger the import-dialog fuzzer's moves record in a `fileDialog` step, over a spread of screens and seeds. */
function recordedTriggers(): Set<string> {
  const triggers = new Set<string>();
  const ids = [...CLICKABLE_TESTIDS, ...TYPEABLE_TESTIDS, ...SELECTABLE_TESTIDS, ...CHECKABLE_TESTIDS];
  const screens = [...new Set(GENERATORS.flatMap((generator) => generator.screens))];
  // Every id live, then all but the toolbar button: a picker opens on the first of the two that is.
  for (const liveIds of [ids, ids.filter((id) => id !== 'toolbar-import-button')]) {
    const live = Object.fromEntries(liveIds.map((id) => [id, 2]));
    for (const screen of screens) {
      for (let seed = 1; seed <= 20; seed++) {
        const env = {
          ctx: fakeCtx(seed),
          digest: { screen, dialog: null, placement: false, ctx: 'existing-project', phase: null, hasProject: true, live },
          filesRel: 'fuzz-files',
          fileSeq: seed,
          corpus: {},
        } as unknown as GenEnv;
        const builds: (() => FuzzMove | null)[] = [
          ...GENERATORS.filter((generator) => generator.screens.includes(screen)).map((generator) => () => generator.build(env)),
          ...(['escape', 'route', 'undo', 'redo', 'save'] as const).map((kind) => () => buildInterjection(kind, env)),
          ...(['export', 'mode', 'close', 'openKey'] as const).map((kind) => () => buildAuxInterjection(kind, env)),
        ];
        for (const build of builds) {
          let move: FuzzMove | null = null;
          try {
            move = build();
          } catch {
            // A digest field the stub does not carry.
          }
          const step = move === null ? null : moveTraceStep(move);
          if (step?.kind === 'fileDialog') triggers.add(step.trigger);
        }
      }
    }
  }
  return triggers;
}

describe("each table is the harness's own (#576 W58 NF2)", () => {
  it('holds the triggers to the ones the import-dialog fuzzer records', () => {
    expect([...recordedTriggers()].sort()).toEqual([...TRACE_TRIGGERS].sort());
  });

  it('holds the export formats to the ones the export fuzzer draws from', () => {
    const saved = process.env.STALAGMIGHT_FUZZ_EXPORT_FORMATS;
    delete process.env.STALAGMIGHT_FUZZ_EXPORT_FORMATS;
    try {
      expect([...enabledFormats()].sort()).toEqual([...TRACE_EXPORT_FORMATS].sort());
    } finally {
      if (saved !== undefined) process.env.STALAGMIGHT_FUZZ_EXPORT_FORMATS = saved;
    }
  });

  it("holds the oracle names to export-roundtrip-fuzz's ORACLE_NAMES", () => {
    const source = fs.readFileSync(path.join(AGENT, 'scenarios', 'exportRoundtripFuzz.ts'), 'utf8');
    const table = /const ORACLE_NAMES: readonly OracleName\[\] = \[([^\]]*)\]/.exec(source)?.[1] ?? '';
    const names = [...table.matchAll(/'([^']+)'/g)].map((match) => match[1]);
    expect(names.length).toBeGreaterThan(5);
    expect(names.sort()).toEqual([...TRACE_ORACLE_NAMES].sort());
  });
});
