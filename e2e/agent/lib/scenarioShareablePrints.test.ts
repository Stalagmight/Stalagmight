/**
 * #576 W51: what the scenarios print of a value they did not word themselves.
 *
 * A refusal quotes the thing it refused - a test id, a store action, a chord, a
 * scenario name - and that thing came from a trace, a recorded call or the
 * environment, which a forged copy or the user fills with any text. A metric
 * key prints in the runner's PASS line unfolded. Each case hands a synthetic
 * name (`qzCaveA`) in and checks that it does not come out.
 */
import * as fs from 'fs';
import * as path from 'path';
import { fileURLToPath } from 'url';
import { afterEach, describe, expect, it } from 'vitest';

import { shareableTestId } from '../../../scripts/lib/shareable-selector.mjs';

import type { ScenarioContext, ScenarioTarget } from '../scenarios/framework';
import { makeContext } from '../scenarios/framework';
import { runScenario } from '../scenarios/index';
import { clickControl, enabledFormats, isExportFormat, selectValue, setCheckbox } from '../scenarios/exportFuzzActions';
import { mapSkipCode } from '../scenarios/importDialogFuzz';
import { answerKeyboardFileDialog, runUiStep } from '../scenarios/importDialogBridge';
import { FileSeq, moveFromTraceStep } from '../scenarios/importDialogTrace';
import { runStep } from '../scenarios/undoRedoFuzzBridge';
import { seedLargeProject } from '../scenarios/seedLargeProject';
import { toolSwitchSpam } from '../scenarios/toolSwitchSpam';
import type { TraceStep } from './fuzzTrace';

const NAME = 'qzCaveA';
/** No call below reaches the page: each refuses in Node first. */
const NO_PAGE = {} as ScenarioContext;
const TARGET = { browser: {}, sessionDir: '/tmp/qzw51-session', exclusive: (fn) => fn() } as ScenarioTarget;

/** The message `run` throws, which must exist and must not carry `NAME`. */
async function refusal(run: () => unknown): Promise<string> {
  try {
    await run();
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    expect(message).not.toContain(NAME);
    return message;
  }
  throw new Error('expected a refusal');
}

describe('a refused value prints as its shape, not its text (#576 W51)', () => {
  const saved = process.env.STALAGMIGHT_FUZZ_EXPORT_FORMATS;
  afterEach(() => {
    if (saved === undefined) delete process.env.STALAGMIGHT_FUZZ_EXPORT_FORMATS;
    else process.env.STALAGMIGHT_FUZZ_EXPORT_FORMATS = saved;
  });

  it('an unknown scenario name', async () => {
    expect(await refusal(() => runScenario(`${NAME} north`, TARGET))).toMatch(/^unknown scenario a string of 13 character\(s\); known: /);
  });

  it('an export-format knob entry the scenario does not export', async () => {
    process.env.STALAGMIGHT_FUZZ_EXPORT_FORMATS = `csv,${NAME}`;
    expect(await refusal(() => enabledFormats())).toContain('names a string of 7 character(s)');
  });

  it("a test id outside the export fuzzer's allowlist keeps the source's words only", async () => {
    // `export` is a word of the app's source, so it stays; the rest is counted.
    expect(await refusal(() => clickControl(NO_PAGE, `export-${NAME}`))).toMatch(/^refusing to click "export-<7 characters>"/);
    await refusal(() => selectValue(NO_PAGE, NAME, 'csv'));
    await refusal(() => setCheckbox(NO_PAGE, NAME, true));
  });

  it("a move, a chord and a store action outside the import and undo fuzzers' sets", async () => {
    await refusal(() => runUiStep(NO_PAGE, { kind: 'ui', testid: NAME, label: 'replayed' }));
    await refusal(() => answerKeyboardFileDialog(NO_PAGE, `Ctrl+${NAME}`, null));
    const step = { kind: 'mutate', action: NAME, args: [], label: 'load' } as TraceStep;
    expect(await refusal(() => moveFromTraceStep(step, new FileSeq(), 'import-dialog fuzz'))).toContain(
      'refusing to replay mutate "<7 characters>"',
    );
    await refusal(() => runStep(NO_PAGE, { kind: 'mutate', action: NAME, args: [] } as unknown as Parameters<typeof runStep>[1]));
  });
});

describe('a trace format the export fuzzer does not export is refused (#576 W51)', () => {
  it('isExportFormat accepts the table and nothing else', () => {
    expect(['csv', 'tmlu', 'diveProfile'].every(isExportFormat)).toBe(true);
    expect([NAME, '', 3, null, undefined, ['csv']].some(isExportFormat)).toBe(false);
  });
});

describe('a map click refusal is a metric code (#576 W51)', () => {
  it("names each of mapClick's own refusals, and anything else as other", () => {
    expect(mapSkipCode(new Error('map not available'))).toBe('map-not-available');
    expect(
      mapSkipCode(new Error('map click failed: map point is outside the visible viewport: [-87.1, 20.2] projects to [1, 2]')),
    ).toBe('outside-viewport');
    expect(mapSkipCode(new Error('map click failed: map interaction hooks are unavailable'))).toBe('click-refused');
    expect(mapSkipCode(new Error('map interaction hooks are unavailable'))).toBe('no-interaction-hooks');
    expect(mapSkipCode(new Error('map placement handler did not process the click\n  dispatched to: x'))).toBe('not-processed');
    expect(mapSkipCode(new Error(`javascript error: page error at /tmp/w/${NAME} north.dmp`))).toBe('other');
    expect(mapSkipCode(NAME)).toBe('unknown');
  });
});

describe('the seed a scenario prints is a number (#576 W51)', () => {
  it('makeContext turns a forged seed into the default', () => {
    expect(makeContext(TARGET, 7).seed).toBe(7);
    expect(makeContext(TARGET, NAME as unknown as number).seed).toBe(1);
    expect(makeContext(TARGET, '5' as unknown as number).seed).toBe(5);
  });
});

describe('a store value of a union type prints in the test-id form (#576 W58 Q11)', () => {
  const UI_STORE = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../../../src/store/uiStore.ts');

  /** The string literals of a type alias or state field declared in src/store/uiStore.ts. */
  function literals(source: string, declaration: RegExp): string[] {
    const found = declaration.exec(source);
    expect(found, String(declaration)).not.toBeNull();
    return [...(found?.[1] ?? '').matchAll(/'([^']+)'/g)].map((m) => m[1] ?? '');
  }

  it("prints every real activeTool, appMode, view and importContext exactly as the app's source spells it", () => {
    const source = fs.readFileSync(UI_STORE, 'utf8');
    const real = [
      ...literals(source, /export type ToolType =([^;]+);/),
      ...literals(source, /export type AppMode =([^;]+);/),
      ...literals(source, /export type ViewType =([^;]+);/),
      ...literals(source, /\n\s+importContext: ([^;]+);/),
    ];
    expect(real).toEqual(expect.arrayContaining(['move_comment', 'export', 'single', 'existing-project']));
    expect(real.length).toBeGreaterThan(15);
    for (const value of real) expect(shareableTestId(value), value).toBe(value);
  });

  it("tool-switch-spam's refusal prints a forged activeTool by the source's words only", async () => {
    const ctx = {
      // hasProject, the navigation-tools setting, then the activeTool probe.
      eval: async (script: string) => (script.includes('setActiveTool') ? `move_${NAME}` : true),
      metric: () => undefined,
      callAction: async () => ({ ok: true }),
      log: () => undefined,
    } as unknown as ScenarioContext;
    const message = await refusal(() => toolSwitchSpam.run(ctx, { iterations: 1 } as never));
    expect(message).toBe('setActiveTool(select) left activeTool=move_<7 characters>');
  });

  it("seed-large-project's progress line prints a forged view by the source's words only", async () => {
    const lines: string[] = [];
    const ctx = {
      eval: async (script: string) => (script.includes('ui.view') ? { sections: 2, stations: 5, view: `${NAME} single` } : true),
      setMetric: () => undefined,
      log: (line: string) => lines.push(line),
    } as unknown as ScenarioContext;
    await seedLargeProject.run(ctx, { iterations: 1 } as never);
    expect(lines).toHaveLength(1);
    expect(lines[0]).not.toContain(NAME);
    expect(lines[0]).toMatch(/^loaded 2 sections \/ 5 stations, view=<\d+ characters>/);
  });
});
