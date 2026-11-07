/**
 * Golden tests for the agent-session generators (generate-replay.mjs and
 * generate-repro-report.mjs). Both are pure JSONL-in -> text-out transformers
 * with no test until now, so a parser regression shipped silently. Each test
 * builds a minimal-but-representative actions.jsonl fixture in a temp session
 * dir, runs the generator as a child process (the only supported interface),
 * and asserts the load-bearing lines of the output - "contains", not a full
 * snapshot, so cosmetic template tweaks don't churn the test.
 */
import { describe, it, expect, beforeAll, afterAll } from 'vitest';
import { execFileSync } from 'child_process';
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import { testIdSelector } from './lib/test-id-selector.mjs';

const repoRoot = path.resolve(__dirname, '..');
const replayGen = path.join(repoRoot, 'scripts', 'generate-replay.mjs');
const reportGen = path.join(repoRoot, 'scripts', 'generate-repro-report.mjs');

let sessionDir: string;

const record = (obj: object): string => JSON.stringify(obj);

beforeAll(() => {
  sessionDir = fs.mkdtempSync(path.join(os.tmpdir(), 'stalagmight-gen-test-'));
  const ts = (s: number) => new Date(1751970000000 + s * 1000).toISOString();
  const lines = [
    record({
      type: 'session-start',
      v: 1,
      sessionId: 'golden-fixture',
      startedAt: ts(0),
      gitHead: 'abc1234',
      binaryPath: '/home/user/.cargo_build/debug/stalagmight',
      binaryMtime: 0,
      display: ':89',
      headed: false,
      profileDir: '/tmp/stalagmight-agent-x',
      freshProfile: true,
    }),
    record({
      type: 'action',
      v: 1,
      seq: 1,
      ts: ts(1),
      tool: 'seed',
      params: { mode: 'fixture', name: 'fractal_plant_iter3.stalx' },
      resolved: { path: '/repo/.agent-artifacts/sessions/golden-fixture/fixtures/fractal_plant_iter3.stalx' },
      outcome: { ok: true, durationMs: 120 },
    }),
    record({
      type: 'action',
      v: 1,
      seq: 2,
      ts: ts(2),
      tool: 'click',
      params: { testid: 'save-button' },
      resolved: { selector: '[data-testid="save-button"]' },
      outcome: { ok: true, durationMs: 40 },
    }),
    // A driver_script that timed out live must not be started again during replay.
    record({
      type: 'action',
      v: 1,
      seq: 3,
      ts: ts(3),
      tool: 'driver_script',
      params: { script: 'while (true) { await ipc("get_recent_logs_since", {afterSeq: 0}); }', timeoutMs: 5000 },
      outcome: { ok: false, durationMs: 5001, error: 'driver_script timed out after 5000ms' },
    }),
    record({
      type: 'action',
      v: 1,
      seq: 4,
      ts: ts(4),
      tool: 'run_scenario',
      params: { name: 'mapStorm', seed: 7, iterations: 3 },
      outcome: { ok: true, durationMs: 9000 },
    }),
    record({
      type: 'event',
      v: 1,
      seq: 4,
      ts: ts(5),
      event: 'finding',
      detail: { severity: 'error', kind: 'log-error', message: 'ERROR something broke' },
    }),
    record({
      type: 'action',
      v: 1,
      seq: 5,
      ts: ts(6),
      tool: 'click',
      params: { testid: 'C:\\Caves\\"quoted".stalx' },
      resolved: { selector: '[data-testid="C:\\Caves\\"quoted".stalx"]' },
      outcome: { ok: true, durationMs: 10 },
    }),
    record({
      type: 'action',
      v: 1,
      seq: 6,
      ts: ts(7),
      tool: 'map_click',
      params: { lngLat: [12.34, -45.6], button: 'right' },
      resolved: { lngLat: [12.34, -45.6], point: { x: 999, y: 777 } },
      outcome: { ok: true, durationMs: 20 },
    }),
    record({
      type: 'action',
      v: 1,
      seq: 7,
      ts: ts(8),
      tool: 'wait_for',
      params: { condition: 'app-ready', timeoutMs: 4321 },
      outcome: { ok: true, durationMs: 10 },
    }),
    record({
      type: 'action',
      v: 1,
      seq: 8,
      ts: ts(9),
      tool: 'wait_for',
      params: { condition: 'active-tool', tool: 'move_station', timeoutMs: 7654 },
      outcome: { ok: true, durationMs: 10 },
    }),
    record({
      type: 'action',
      v: 1,
      seq: 9,
      ts: ts(10),
      tool: 'wait_for',
      params: { condition: 'store-path', store: 'ui', path: 'activeTool', equals: 'move_station' },
      outcome: { ok: true, durationMs: 10 },
    }),
    record({
      type: 'action',
      v: 1,
      seq: 10,
      ts: ts(11),
      tool: 'hover',
      params: { testid: 'toolbar-undo-button' },
      outcome: { ok: true, durationMs: 10 },
    }),
    record({
      type: 'action',
      v: 1,
      seq: 11,
      ts: ts(12),
      tool: 'drag',
      params: { fromTestid: 'station-a', toTestid: 'station-b', durationMs: 250 },
      outcome: { ok: true, durationMs: 260 },
    }),
    record({
      type: 'action',
      v: 1,
      seq: 12,
      ts: ts(13),
      tool: 'scroll',
      params: { testid: 'warnings-content', deltaY: 400 },
      outcome: { ok: true, durationMs: 10 },
    }),
    record({
      type: 'action',
      v: 1,
      seq: 13,
      ts: ts(14),
      tool: 'select_option',
      params: { testid: 'select-number-format', value: 'en-US' },
      outcome: { ok: true, durationMs: 10 },
    }),
    record({
      type: 'action',
      v: 1,
      seq: 14,
      ts: ts(15),
      tool: 'file_dialog_answer',
      params: { triggerTestid: 'import-dmp-button', kind: 'open', path: 'C:\\Caves\\survey.dmp' },
      outcome: { ok: true, durationMs: 10 },
    }),
    record({
      type: 'action',
      v: 1,
      seq: 15,
      ts: ts(16),
      tool: 'store_state',
      params: { store: 'project', path: 'isDirty' },
      outcome: { ok: true, durationMs: 10 },
      result: { value: false },
    }),
    record({
      type: 'action',
      v: 1,
      seq: 16,
      ts: ts(17),
      tool: 'map_feature_click',
      params: { featureType: 'shot', sectionId: 7, shotId: 42, verifySelection: true },
      resolved: { point: { x: 888, y: 666 }, clearancePx: 12 },
      outcome: { ok: true, durationMs: 20 },
    }),
    // Tier 2 OS-level input: recorded like any action, never replayable. Two
    // different os_* tools, so the test pins the prefix arm rather than a
    // per-tool case.
    record({
      type: 'action',
      v: 1,
      seq: 17,
      ts: ts(18),
      tool: 'os_click',
      params: { testid: 'toolbar-save-button' },
      resolved: { point: { x: 1440, y: 220 }, frontmost_ok: true },
      outcome: { ok: true, durationMs: 180 },
    }),
    record({
      type: 'action',
      v: 1,
      seq: 18,
      ts: ts(19),
      tool: 'os_keys',
      params: { spec: 'cmd+s' },
      outcome: { ok: true, durationMs: 90 },
    }),
    record({ type: 'session-end', v: 1, ts: ts(20), reason: 'app_stop', actions: 18 }),
  ];
  fs.writeFileSync(path.join(sessionDir, 'actions.jsonl'), `${lines.join('\n')}\n`);
});

afterAll(() => {
  fs.rmSync(sessionDir, { recursive: true, force: true });
});

describe('generate-replay.mjs', () => {
  it('emits executable steps for every mutating action, bounded and tolerant for recorded failures', () => {
    const out = path.join(sessionDir, 'replay.spec.ts');
    execFileSync(process.execPath, [replayGen, sessionDir, '--out', out], { encoding: 'utf8' });
    const spec = fs.readFileSync(out, 'utf8');

    // seed -> a fresh copy of the tracked fixture, never the prunable recorded artifact path.
    expect(spec).toContain(
      `'loadProject', [materializeReplayFixture("fractal_plant_iter3.stalx")]`,
    );
    expect(spec).toContain('afterEach(() => cleanupReplayFixtures());');
    expect(spec).not.toContain('/repo/.agent-artifacts/sessions/golden-fixture');
    // click -> the resolved selector, clicked.
    expect(spec).toContain(`replayClick(browser, {"selector":"[data-testid=\\"save-button\\"]"})`);
    // Unsafe legacy resolved selectors are rebuilt from the recorded test ID.
    expect(spec).toContain(JSON.stringify(testIdSelector('C:\\Caves\\"quoted".stalx')).slice(1, -1));
    // Map clicks replay geography, not the pixels from the recording viewport.
    expect(spec).toContain(`await mapClick(browser, [12.34,-45.6], "right");`);
    expect(spec).not.toContain('999, 777');
    // Semantic feature clicks resolve the current geometry again during replay.
    expect(spec).toContain(
      `await mapFeatureClick(browser, {"featureType":"shot","sectionId":7,"shotId":42,"verifySelection":true});`,
    );
    expect(spec).not.toContain('888, 666');
    // Semantic waits remain semantic and keep their recorded arguments.
    expect(spec).toContain(`waitForReplay(browser, {"condition":"app-ready","timeoutMs":4321})`);
    expect(spec).toContain(`waitForReplay(browser, {"condition":"active-tool","tool":"move_station","timeoutMs":7654})`);
    expect(spec).toContain(`waitForReplay(browser, {"condition":"store-path","store":"ui","path":"activeTool","equals":"move_station"})`);
    expect(spec).not.toContain('pause(browser, 300); // wait_for');
    // First-class controls and native-dialog answers are replayable actions.
    expect(spec).toContain(`hoverElement(browser, {"testid":"toolbar-undo-button"})`);
    expect(spec).toContain(`dragPointer(browser, {"fromTestid":"station-a","toTestid":"station-b","durationMs":250})`);
    expect(spec).toContain(`scrollTarget(browser, {"testid":"warnings-content","deltaY":400})`);
    expect(spec).toContain(`selectOption(browser, {"testid":"select-number-format","value":"en-US"})`);
    expect(spec).toContain(
      `answerFileDialog(browser, "import-dmp-button", "open", "C:\\\\Caves\\\\survey.dmp")`,
    );
    expect(spec).toContain('driver_script recorded as FAILED; skipped');
    expect(spec).not.toContain('driverScript(browser,');
    // run_scenario -> deterministic replay with the recorded seed + iterations.
    expect(spec).toContain(`await runScenarioReplay(browser, "mapStorm", 7, 3);`);
    // Observation-only tools stay comments.
    expect(spec).toContain('store_state');
    expect(spec).not.toContain('await storeState');
  });

  it('comments out every os_* step as OS-level, with its params, instead of replaying it', () => {
    const out = path.join(sessionDir, 'replay-os.spec.ts');
    execFileSync(process.execPath, [replayGen, sessionDir, '--out', out], { encoding: 'utf8' });
    const spec = fs.readFileSync(out, 'utf8');

    expect(spec).toContain(
      '// [seq 17] os_click {"testid":"toolbar-save-button"} (OS-level; not replayable)',
    );
    expect(spec).toContain('// [seq 18] os_keys {"spec":"cmd+s"} (OS-level; not replayable)');
    // Not the generic observation arm, and not an executable click: trusted input
    // needs TCC grants, a frontmost desktop and pixel determinism.
    expect(spec).not.toContain('os_click (observation; not replayed)');
    expect(spec).not.toContain('os_keys (observation; not replayed)');
    expect(spec).not.toContain('toolbar-save-button"]');
    expect(spec).not.toContain('browser.keys(["cmd+s"])');
  });
});

describe('testIdSelector', () => {
  it('targets exact IDs containing Windows separators, quotes, and control characters', () => {
    expect(testIdSelector('C:\\Caves\\survey.stalx')).toBe(
      '[data-testid="C:\\\\Caves\\\\survey.stalx"]',
    );
    expect(testIdSelector('path-"quoted"')).toBe('[data-testid="path-\\"quoted\\""]');
    expect(testIdSelector('line\nbreak')).toBe('[data-testid="line\\A break"]');
  });
});

describe('generate-repro-report.mjs', () => {
  it('produces a report.md with the mutating steps and the recorded finding, scrubbed of $HOME', () => {
    execFileSync(process.execPath, [reportGen, sessionDir, '--title', 'Golden fixture'], {
      encoding: 'utf8',
    });
    const md = fs.readFileSync(path.join(sessionDir, 'report.md'), 'utf8');

    expect(md).toContain('Golden fixture');
    // The mutating actions appear as repro steps; observations do not lead.
    expect(md).toContain('seed');
    expect(md).toContain('run_scenario');
    expect(md).toContain('mapStorm');
    // The oracle finding is surfaced.
    expect(md).toContain('log-error');
    // Public-paste hygiene: no raw $HOME-bearing absolute path survives.
    const home = process.env.HOME ?? '';
    if (home) expect(md).not.toContain(`${home}/`);
  });

  // OS-level steps mutate the app, so a repro that omits them is wrong. They
  // must appear AND carry their detail: a bare "os_keys" tells a reader nothing
  // about which chord reproduced the bug.
  it('includes os_* steps with their detail, marked OS-level', () => {
    execFileSync(process.execPath, [reportGen, sessionDir], { encoding: 'utf8' });
    const md = fs.readFileSync(path.join(sessionDir, 'report.md'), 'utf8');

    expect(md).toContain('os_click');
    expect(md).toContain('testid=toolbar-save-button');
    expect(md).toContain('os_keys');
    expect(md).toContain('cmd+s');
    expect(md).toContain('[OS-level]');
  });

  // docs/agent-harness.md marks actions.jsonl and tauri-driver.log "not for
  // sharing": they hold the full text the report withholds. The Evidence list
  // still points at them for whoever triages the run, and says so (#576 RV18-W23-7).
  it('labels the action log and the driver log as local triage only, not for sharing', () => {
    execFileSync(process.execPath, [reportGen, sessionDir], { encoding: 'utf8' });
    const evidence = fs
      .readFileSync(path.join(sessionDir, 'report.md'), 'utf8')
      .split('## Evidence')[1]
      .split('## Notes')[0];

    expect(evidence).toContain('- Action log: `actions.jsonl` (local triage only, not for sharing:');
    expect(evidence).toContain('- Driver log: `tauri-driver.log` (local triage only, not for sharing)');
  });

  // The bug this replaced: the step list came from a hand-maintained name list
  // that nobody updated as tools were added, so a repro whose decisive step was
  // a drag or a native-dialog answer rendered WITHOUT that step. The recorder
  // now stamps `mutating` per action, and the report must believe it - both
  // ways round, which is what makes the name list unable to rot again.
  describe('mutation comes from the action record, not a name list', () => {
    let v2Dir: string;

    beforeAll(() => {
      v2Dir = fs.mkdtempSync(path.join(os.tmpdir(), 'stalagmight-gen-v2-'));
      const ts = (s: number) => new Date(1751970000000 + s * 1000).toISOString();
      const lines = [
        record({ type: 'session-start', v: 1, sessionId: 'v2-fixture', startedAt: ts(0), gitHead: 'def5678' }),
        // Mutating, and absent from the frozen legacy list: these are exactly
        // the tools the old report dropped.
        record({
          type: 'action', v: 1, seq: 1, ts: ts(1), tool: 'drag', mutating: true,
          params: { fromTestid: 'station-a', toTestid: 'station-b' },
          outcome: { ok: true, durationMs: 10 },
        }),
        record({
          type: 'action', v: 1, seq: 2, ts: ts(2), tool: 'file_dialog_answer', mutating: true,
          params: { triggerTestid: 'import-button', kind: 'open', path: '/caves/survey.dmp' },
          outcome: { ok: true, durationMs: 10 },
        }),
        record({
          type: 'action', v: 1, seq: 3, ts: ts(3), tool: 'scroll', mutating: true,
          params: { testid: 'warnings-content', deltaY: 400 },
          outcome: { ok: true, durationMs: 10 },
        }),
        record({
          type: 'action', v: 1, seq: 4, ts: ts(4), tool: 'select_option', mutating: true,
          params: { testid: 'select-number-format', value: 'en-US' },
          outcome: { ok: true, durationMs: 10 },
        }),
        record({
          type: 'action', v: 1, seq: 5, ts: ts(5), tool: 'map_feature_click', mutating: true,
          params: { featureType: 'shot', sectionId: 2, shotId: 17 },
          outcome: { ok: true, durationMs: 10 },
        }),
        // A mutation that THREW is still the step that reproduces the bug.
        record({
          type: 'action', v: 1, seq: 6, ts: ts(6), tool: 'hover', mutating: true,
          params: { testid: 'toolbar-undo-button' },
          outcome: { ok: false, durationMs: 10, error: 'element not interactable' },
        }),
        // In the legacy list, but recorded as NOT mutating: a bare map_view is
        // a read. The record must win over the list.
        record({
          type: 'action', v: 1, seq: 7, ts: ts(7), tool: 'map_view', mutating: false,
          params: {}, outcome: { ok: true, durationMs: 10 },
        }),
        // The os_ family no longer rides a prefix rule, so each member's
        // classification has to be pinned individually: consent is a step,
        // capturing pixels is not.
        record({
          type: 'action', v: 1, seq: 8, ts: ts(8), tool: 'os_grant', mutating: true,
          params: {}, outcome: { ok: true, durationMs: 10 },
        }),
        record({
          type: 'action', v: 1, seq: 9, ts: ts(9), tool: 'os_screenshot', mutating: false,
          params: { name: 'after-drag' }, outcome: { ok: true, durationMs: 10 },
        }),
        // A survey filename names a real cave, and reports get pasted into
        // public issues - so this one sits under $HOME, where the prefix
        // scrubber would still leave the basename exposed.
        record({
          type: 'action', v: 1, seq: 10, ts: ts(10), tool: 'file_dialog_answer', mutating: true,
          params: {
            triggerTestid: 'open-button',
            kind: 'open',
            path: `${process.env.HOME ?? '/home/someone'}/Documents/Secret Sump Cave.stalx`,
          },
          outcome: { ok: true, durationMs: 10 },
        }),
        // The redacted extension is itself caller-controlled: path.extname
        // returns everything after the last dot, newlines included.
        record({
          type: 'action', v: 1, seq: 12, ts: ts(12), tool: 'file_dialog_answer', mutating: true,
          params: {
            triggerTestid: 'save-button',
            kind: 'save',
            path: '/tmp/cave.stalx\n## forged heading\n<img src=x>',
          },
          outcome: { ok: true, durationMs: 10 },
        }),
        // Params are bare strings: a newline or a backtick would break out of
        // the markdown list item in a report headed for a public issue.
        record({
          type: 'action', v: 1, seq: 11, ts: ts(11), tool: 'click', mutating: true,
          params: { testid: 'evil\n## Injected heading\n<img src=x onerror=alert(1)> `code`' },
          outcome: { ok: true, durationMs: 10 },
        }),
      ];
      fs.writeFileSync(path.join(v2Dir, 'actions.jsonl'), `${lines.join('\n')}\n`);
    });

    afterAll(() => fs.rmSync(v2Dir, { recursive: true, force: true }));

    it('lists every recorded mutation, with detail, including ones no name list knew about', () => {
      execFileSync(process.execPath, [reportGen, v2Dir], { encoding: 'utf8' });
      const md = fs.readFileSync(path.join(v2Dir, 'report.md'), 'utf8');

      // Present AND described - a bare tool name is not a reproducible step.
      expect(md).toContain('`drag` testid=station-a -> testid=station-b');
      expect(md).toContain('open dialog answered <file.dmp>');
      expect(md).toContain('`scroll` testid=warnings-content by (0,400)');
      expect(md).toContain('`select_option` testid=select-number-format = "en-US"');
      expect(md).toContain('`map_feature_click` shot id=17 in section 2');
      // A failed mutation stays, flagged.
      expect(md).toContain('`hover` testid=toolbar-undo-button **(failed)**');
    });

    it('drops an action the record says is not mutating, even though the legacy list names it', () => {
      execFileSync(process.execPath, [reportGen, v2Dir], { encoding: 'utf8' });
      const steps = reproSteps();

      expect(steps).not.toContain('map_view');
    });

    // The os_ prefix used to decide this for the whole family. It doesn't now,
    // so each member needs its own pin - os_grant is the consent every later
    // os_* step depends on, os_screenshot changes nothing.
    it('keeps os_grant as a step and drops os_screenshot, per member not per prefix', () => {
      execFileSync(process.execPath, [reportGen, v2Dir], { encoding: 'utf8' });
      const steps = reproSteps();

      expect(steps).toContain('os_grant');
      expect(steps).not.toContain('os_screenshot');
    });

    // Reports are pasted into public issues, and in this project a survey
    // filename names a real cave. `scrub` only rewrites the $HOME prefix, so
    // the basename has to be dropped at the point of rendering.
    it('redacts a chosen file to its extension, leaking neither the name nor the directory', () => {
      execFileSync(process.execPath, [reportGen, v2Dir], { encoding: 'utf8' });
      const md = fs.readFileSync(path.join(v2Dir, 'report.md'), 'utf8');

      expect(md).toContain('open dialog answered <file.stalx>');
      expect(md).not.toContain('Secret Sump Cave');
      expect(md).not.toContain('Documents');
    });

    // A step is one markdown list item. A param that escapes it can forge
    // headings, hide later steps behind a <details>, or smuggle HTML.
    it('neutralizes markdown and newlines in caller-supplied params', () => {
      execFileSync(process.execPath, [reportGen, v2Dir], { encoding: 'utf8' });
      const steps = reproSteps();

      expect(steps).not.toContain('\n## Injected heading');
      expect(steps).not.toContain('<img');
      expect(steps).toContain('&lt;img');
    });

    // path.extname hands back everything after the last dot, so the redacted
    // placeholder is caller-controlled too - it must be whitelisted, not just
    // escaped, or the sanitizer is bypassed by the very field that replaced
    // the leaking one.
    it('drops an extension that is not a plain extension, rather than rendering it', () => {
      execFileSync(process.execPath, [reportGen, v2Dir], { encoding: 'utf8' });
      const steps = reproSteps();

      expect(steps).toContain('save dialog answered <file>');
      expect(steps).not.toContain('forged heading');
    });

    function reproSteps(): string {
      return fs
        .readFileSync(path.join(v2Dir, 'report.md'), 'utf8')
        .split('## Steps to reproduce')[1]
        .split('Automated replay')[0];
    }
  });

  // An ipc / store_action call's arguments and typed text are the project's own
  // data - station and section names - and the report is pasted into public
  // issues, which keep only codes, counts and code locations (#576 RV18-W23-3).
  describe('steps give argument shapes, never their values', () => {
    let argsDir: string;

    beforeAll(() => {
      argsDir = fs.mkdtempSync(path.join(os.tmpdir(), 'stalagmight-gen-args-'));
      const ts = (s: number) => new Date(1751970000000 + s * 1000).toISOString();
      const lines = [
        record({ type: 'session-start', v: 1, sessionId: 'args-fixture', startedAt: ts(0) }),
        record({
          type: 'action', v: 1, seq: 1, ts: ts(1), tool: 'ipc', mutating: true,
          params: { command: 'rename_station', args: { oldName: 'StationA1', newName: 'StationZ9' } },
          outcome: { ok: false, durationMs: 1, error: 'station not found' },
        }),
        record({
          type: 'action', v: 1, seq: 2, ts: ts(2), tool: 'store_action', mutating: true,
          params: { store: 'project', action: 'renameSection', args: ['CaveA North', 3, false] },
          outcome: { ok: true, durationMs: 1 },
        }),
        record({
          type: 'action', v: 1, seq: 3, ts: ts(3), tool: 'type_text', mutating: true,
          params: { text: 'CaveA entrance', testid: 'station-name-input' },
          outcome: { ok: true, durationMs: 1 },
        }),
        // Nested values, a coordinate, and keys that are themselves names: a
        // map keyed by stations would otherwise print them as its keys.
        record({
          type: 'action', v: 1, seq: 4, ts: ts(4), tool: 'ipc', mutating: true,
          params: {
            command: 'update_section',
            args: {
              sectionId: 7,
              patch: { name: 'CaveA', visible: true, depths: [12.5, 31.25], anchor: { lat: 20.5, lng: -87.25 } },
              colors: { StationA1: 'tealA1', 'StationB2 junction': 'amberB2' },
              tags: [],
            },
          },
          outcome: { ok: true, durationMs: 1 },
        }),
      ];
      fs.writeFileSync(path.join(argsDir, 'actions.jsonl'), `${lines.join('\n')}\n`);
      execFileSync(process.execPath, [reportGen, argsDir], { encoding: 'utf8' });
    });

    afterAll(() => fs.rmSync(argsDir, { recursive: true, force: true }));

    const steps = (): string =>
      fs
        .readFileSync(path.join(argsDir, 'report.md'), 'utf8')
        .split('## Steps to reproduce')[1]
        .split('Automated replay')[0];

    it('prints no argument value or typed text', () => {
      const md = fs.readFileSync(path.join(argsDir, 'report.md'), 'utf8');
      for (const value of ['StationA1', 'StationZ9', 'StationB2', 'CaveA', '12.5', '31.25', '20.5', '-87.25', 'tealA1', 'amberB2']) {
        expect(md).not.toContain(value);
      }
    });

    it('keeps the tool, the command or action, the failed marker and each argument shape', () => {
      const md = steps();
      expect(md).toContain('1. `ipc` rename_station({oldName: string(9), newName: string(9)}) **(failed)**');
      expect(md).toContain('2. `store_action` project.renameSection(string(11), number, false)');
      expect(md).toContain('3. `type_text` string(14) into station-name-input');
      expect(md).toContain(
        '4. `ipc` update_section({sectionId: number, patch: {name: string(5), visible: true, ' +
          'depths: array(2) of number, anchor: object(2 keys)}, colors: {2 key(s) withheld}, tags: array(0)})',
      );
    });
  });
});
