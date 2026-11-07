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
import { codeWords, shareableSelector, shareableTestId } from './lib/shareable-selector.mjs';

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

  // stdout is shareable: the session dir and --out are the caller's paths,
  // which can sit outside the checkout and name their own directories (#576 W51).
  it('names what it wrote on stdout by its placeholder, never its path', () => {
    const withOut = execFileSync(process.execPath, [replayGen, sessionDir, '--out', path.join(sessionDir, 'qz-out.spec.ts')], {
      encoding: 'utf8',
    });
    const inDir = execFileSync(process.execPath, [replayGen, sessionDir], { encoding: 'utf8' });
    expect(withOut.split('\n')[0]).toBe('wrote the --out file (18 actions)');
    expect(inDir.split('\n')[0]).toBe('wrote <sessionDir>/replay.spec.ts (18 actions)');
    expect(withOut + inDir).not.toContain(sessionDir);
    expect(fs.existsSync(path.join(sessionDir, 'replay.spec.ts'))).toBe(true);
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

describe('shareable test ids and selectors', () => {
  // The kept words are the app's source's: with no source to read, nothing is
  // known and every word is counted (#576 RV19b-P1).
  it('counts every word when the source cannot be read', () => {
    const none = codeWords(path.join(os.tmpdir(), 'stalagmight-no-such-checkout'));
    expect(none.size).toBe(0);
    expect(shareableTestId('save-button', none)).toBe('<11 characters>');
    expect(shareableSelector('[data-testid="save-button"]', none)).toBe('[<11 characters>="<11 characters>"]');
  });

  it('keeps the words a test id and a selector share with the source, and numbers', () => {
    const words = new Set(['section', 'row', 'span', 'button', 'role', 'data', 'testid']);
    expect(shareableTestId('section-row-12', words)).toBe('section-row-12');
    expect(shareableTestId('section-row-Qzcave-row', words)).toBe('section-row-<6 characters>-row');
    expect(shareableSelector('button[role=button] span', words)).toBe('button[role=button] span');
    expect(shareableSelector('[data-testid=section-row-qz_cave]', words)).toBe('[data-testid=section-row-<7 characters>]');
    // Text a selector matches is counted even when its words are known.
    expect(shareableSelector('span=section row', words)).toBe('span=<11 characters>');
    expect(shareableSelector("//span[normalize-space()='row']", words)).toContain("='<3 characters>'");
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
    // The chord's modifier, its typed key counted (#576 RV19b-P6).
    expect(md).toContain('`os_keys` ["cmd", 1 character] [OS-level]');
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

  // A screenshot is an image of the app: its text and corpus file names, which
  // nothing can fold. docs/agent-harness.md lists screenshots as not for
  // sharing, and the report says the same of every file it points at, bar the
  // findings' shareable JSON (#576 W33-NF5).
  it('labels the screenshots local triage only too, and tells the reader to attach only shareable files', () => {
    execFileSync(process.execPath, [reportGen, sessionDir], { encoding: 'utf8' });
    const md = fs.readFileSync(path.join(sessionDir, 'report.md'), 'utf8');
    const evidence = md.split('## Evidence')[1].split('## Notes')[0];
    const listed = evidence.split('\n').filter((line) => line.startsWith('- ') && !line.startsWith('- Findings:'));
    expect(listed.map((line) => line.split(':')[0])).toEqual(['- Action log', '- Screenshots', '- Driver log']);
    for (const line of listed) expect(line).toContain('(local triage only, not for sharing');
    const notes = md.split('## Notes')[1];
    expect(notes).not.toContain('the specific files you checked');
    expect(notes).toContain('only files docs/agent-harness.md lists as shareable');
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
        // A selector keeps its structure (#576 RV19b-P1), so markdown in it is
        // still neutralised rather than counted away.
        record({
          type: 'action', v: 1, seq: 13, ts: ts(13), tool: 'click', mutating: true,
          params: { selector: 'div\n## Injected heading\n<img src=x onerror=alert(1)>' },
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
      // The chosen value as its length (#576 W33-NF2).
      expect(md).toContain('`select_option` testid=select-number-format = string(5)');
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
      // The test id is no word of the app's source: only its length is left.
      expect(steps).toContain('`click` testid=<60 characters>');
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

  // A script, a project or fixture name, an option value and a map position
  // are the user's data too: a script can name a station, a select can pick a
  // section, a coordinate is a cave's location (#576 W33-NF2). And a name a
  // step prints - the command's too - is given as its shape unless it is a name
  // of identifier characters (#576 W49-NF1).
  describe('steps quote no script, project or fixture name, option value or map position', () => {
    let quotesDir: string;

    beforeAll(() => {
      quotesDir = fs.mkdtempSync(path.join(os.tmpdir(), 'stalagmight-gen-quotes-'));
      const ts = (s: number) => new Date(1751970000000 + s * 1000).toISOString();
      const action = (seq: number, tool: string, params: object) =>
        record({ type: 'action', v: 1, seq, ts: ts(seq), tool, mutating: true, params, outcome: { ok: true, durationMs: 1 } });
      const lines = [
        record({ type: 'session-start', v: 1, sessionId: 'quotes-fixture', startedAt: ts(0) }),
        action(1, 'eval', { script: "return renameStation('StationA1', 'StationZ9')" }),
        action(2, 'driver_script', { script: 'await page.fill("#name", "CaveA North")\n## forged heading' }),
        action(3, 'seed', { mode: 'basic', projectName: 'CaveA Survey' }),
        action(4, 'seed', { mode: 'fixture', name: 'CaveB-entrance.stalx' }),
        action(5, 'seed', { mode: 'basic' }),
        action(6, 'select_option', { testid: 'section-select', value: 'CaveC sump' }),
        action(7, 'map_click', { lngLat: [-87.4321, 20.6543] }),
        action(8, 'map_view', { set: { center: [-87.4321, 20.6543], zoom: 18, bearing: 0 } }),
        action(9, 'ipc', { command: 'save_project`\n## forged', args: {} }),
        action(10, 'store_action', { store: 'project', action: 'undo`|x', args: [] }),
        action(11, 'os_click', { testid: 'toolbar`\n## forged' }),
        action(12, 'os_keys', { spec: 'cmd+s\n## forged' }),
      ];
      fs.writeFileSync(path.join(quotesDir, 'actions.jsonl'), `${lines.join('\n')}\n`);
      execFileSync(process.execPath, [reportGen, quotesDir], { encoding: 'utf8' });
    });

    afterAll(() => fs.rmSync(quotesDir, { recursive: true, force: true }));

    const steps = (): string =>
      fs
        .readFileSync(path.join(quotesDir, 'report.md'), 'utf8')
        .split('## Steps to reproduce')[1]
        .split('Automated replay')[0];

    it('prints none of them', () => {
      const md = fs.readFileSync(path.join(quotesDir, 'report.md'), 'utf8');
      for (const value of ['StationA1', 'StationZ9', 'CaveA', 'CaveB', 'CaveC', '87.43', '20.65']) {
        expect(md).not.toContain(value);
      }
    });

    it('gives each its shape or its extension, one line per step', () => {
      expect(steps().trim().split('\n')).toEqual([
        '1. `eval` script string(46)',
        '2. `driver_script` script string(57)',
        '3. `seed` basic project named string(12)',
        '4. `seed` open fixture <file.stalx>',
        '5. `seed` basic project (default name)',
        '6. `select_option` testid=section-select = string(10)',
        '7. `map_click` lngLat=array(2) of number',
        '8. `map_view` set {center: array(2) of number, zoom: number, bearing: number}',
        '9. `ipc` string(23)({})',
        '10. `store_action` project.string(7)()',
        '11. `os_click` testid=<18 characters> [OS-level]',
        '12. `os_keys` ["cmd", 11 characters] [OS-level]',
      ]);
    });
  });

  // A `press_keys` step types every key that is not a WebDriver key name, one
  // character at a time: a run of single characters - or one longer string -
  // spells a name as surely as `type_text` does (#576 W39-NF2). Named keys stay.
  describe('press_keys steps list the named keys and count the typed ones', () => {
    let keysDir: string;

    beforeAll(() => {
      keysDir = fs.mkdtempSync(path.join(os.tmpdir(), 'stalagmight-gen-keys-'));
      const ts = (s: number) => new Date(1751970000000 + s * 1000).toISOString();
      const action = (seq: number, keys: unknown[]) =>
        record({
          type: 'action',
          v: 1,
          seq,
          ts: ts(seq),
          tool: 'press_keys',
          mutating: true,
          params: { keys },
          outcome: { ok: true, durationMs: 1 },
        });
      const lines = [
        record({ type: 'session-start', v: 1, sessionId: 'keys-fixture', startedAt: ts(0) }),
        action(1, ['Control', 'z']),
        action(2, ['C', 'a', 'v', 'e', 'A', 'Enter']),
        action(3, ['StationA1']),
        action(4, ['Shift', 'Tab', 'Escape', 'F2', 'ArrowDown']),
        action(5, ['S', ' ', 'A', '', '1']),
        action(6, ['Page_Up', '`\n## forged', 7]),
      ];
      fs.writeFileSync(path.join(keysDir, 'actions.jsonl'), `${lines.join('\n')}\n`);
      execFileSync(process.execPath, [reportGen, keysDir], { encoding: 'utf8' });
    });

    afterAll(() => fs.rmSync(keysDir, { recursive: true, force: true }));

    const steps = (): string =>
      fs
        .readFileSync(path.join(keysDir, 'report.md'), 'utf8')
        .split('## Steps to reproduce')[1]
        .split('Automated replay')[0];

    it('prints no typed text', () => {
      const md = fs.readFileSync(path.join(keysDir, 'report.md'), 'utf8');
      for (const value of ['CaveA', 'StationA1', '"C"', '"S"', '"1"', 'forged']) expect(md).not.toContain(value);
    });

    it('keeps each named key and gives each run of typed keys as a count of characters', () => {
      expect(steps().trim().split('\n')).toEqual([
        '1. `press_keys` ["Control", 1 character]',
        '2. `press_keys` [5 characters, "Enter"]',
        '3. `press_keys` [9 characters]',
        '4. `press_keys` ["Shift", "Tab", "Escape", "F2", "ArrowDown"]',
        '5. `press_keys` [3 characters, U+E007, 1 character]',
        '6. `press_keys` ["Page_Up", 12 characters]',
      ]);
    });
  });

  // A test id or a selector can name an element by the user's data: its text
  // (`span=<name>`, an xpath text()), an attribute that holds text, or a value
  // a test id was built from (#576 RV19b-P1). The report keeps their structure
  // and the words of the app's source, and gives the rest as its length.
  describe('steps keep a test id or selector by its structure, never the text it names', () => {
    let selDir: string;

    beforeAll(() => {
      selDir = fs.mkdtempSync(path.join(os.tmpdir(), 'stalagmight-gen-sel-'));
      const ts = (s: number) => new Date(1751970000000 + s * 1000).toISOString();
      const step = (seq: number, tool: string, params: object) =>
        record({ type: 'action', v: 1, seq, ts: ts(seq), tool, mutating: true, params, outcome: { ok: true, durationMs: 1 } });
      const lines = [
        record({ type: 'session-start', v: 1, sessionId: 'sel-fixture', startedAt: ts(0) }),
        step(1, 'click', { selector: 'span=Qzcave Nord Sump' }),
        step(2, 'click', { testid: 'publish-to-speleodb-dialog-collab-remove-qzbob@example.org' }),
        step(3, 'hover', { selector: "//td[text()='Qzstation A12']" }),
        step(4, 'drag', { fromSelector: '[aria-label="Qzcave Nord"]', toTestid: 'system-card-%2Fhome%2Fqz%2FQzcave.srv' }),
        step(5, 'scroll', { selector: '*=Qzcave', deltaY: 100 }),
        step(6, 'select_option', { selector: 'aria/Qzcave', value: 'x' }),
        step(7, 'type_text', { testid: 'qzcave-name-input', text: 'abc' }),
        step(8, 'file_dialog_answer', { kind: 'open', triggerTestid: 'qzbob@example.org', path: '/x/a.srv' }),
        step(9, 'os_click', { testid: 'save-button' }),
        step(10, 'click', { selector: '[data-testid="toolbar-undo-button"] > svg' }),
        step(11, 'click', { selector: '//*[@data-testid="section-row-12"]' }),
      ];
      fs.writeFileSync(path.join(selDir, 'actions.jsonl'), `${lines.join('\n')}\n`);
      execFileSync(process.execPath, [reportGen, selDir], { encoding: 'utf8' });
    });

    afterAll(() => fs.rmSync(selDir, { recursive: true, force: true }));

    const steps = (): string =>
      fs
        .readFileSync(path.join(selDir, 'report.md'), 'utf8')
        .split('## Steps to reproduce')[1]
        .split('Automated replay')[0];

    it('prints none of the text', () => {
      for (const value of ['Qz', 'qz', 'Nord', 'Sump', 'example.org', 'bob', '%2F']) expect(steps()).not.toContain(value);
    });

    it('keeps the structure and the code words, and gives the rest as its length', () => {
      expect(steps().trim().split('\n')).toEqual([
        '1. `click` selector=span=<16 characters>',
        '2. `click` testid=publish-to-speleodb-dialog-collab-remove-<17 characters>',
        "3. `hover` selector=//td[text()='<13 characters>']",
        '4. `drag` selector=[aria-label="<11 characters>"] -> testid=system-card-<25 characters>',
        '5. `scroll` selector=*=<6 characters> by (0,100)',
        '6. `select_option` selector=aria/<6 characters> = string(1)',
        '7. `type_text` string(3) into <6 characters>-name-input',
        '8. `file_dialog_answer` open dialog answered <file.srv> (trigger <17 characters>)',
        '9. `os_click` testid=save-button [OS-level]',
        '10. `click` selector=[data-testid="toolbar-undo-button"] &gt; svg',
        '11. `click` selector=//*[@data-testid="section-row-12"]',
      ]);
    });
  });

  // map_feature_click's ids are integers in the MCP schema, but the report reads
  // whatever actions.jsonl holds: a forged or hand-edited log must not reach
  // report.md raw (a newline in sectionId printed its own `## ...` heading).
  // An id that is not an integer is given as its shape (#576 W47-NF2).
  describe('map_feature_click steps print the ids only as integers', () => {
    let mfcDir: string;

    beforeAll(() => {
      mfcDir = fs.mkdtempSync(path.join(os.tmpdir(), 'stalagmight-gen-mfc-'));
      const ts = (s: number) => new Date(1751970000000 + s * 1000).toISOString();
      const step = (seq: number, params: object) =>
        record({ type: 'action', v: 1, seq, ts: ts(seq), tool: 'map_feature_click', mutating: true, params, outcome: { ok: true, durationMs: 1 } });
      const lines = [
        record({ type: 'session-start', v: 1, sessionId: 'mfc-fixture', startedAt: ts(0) }),
        step(1, { featureType: 'shot', sectionId: 2, shotId: 17 }),
        step(2, { featureType: 'station', sectionId: 3, stationIndex: 0, button: 'right' }),
        step(3, { featureType: 'station', sectionId: 'x\n## injected in section x', stationIndex: '`Qzcave`|<b>' }),
        step(4, { featureType: 'shot', sectionId: 1.5, shotId: { name: 'Qzstation A1' } }),
      ];
      fs.writeFileSync(path.join(mfcDir, 'actions.jsonl'), `${lines.join('\n')}\n`);
      execFileSync(process.execPath, [reportGen, mfcDir], { encoding: 'utf8' });
    });

    afterAll(() => fs.rmSync(mfcDir, { recursive: true, force: true }));

    const steps = (): string =>
      fs
        .readFileSync(path.join(mfcDir, 'report.md'), 'utf8')
        .split('## Steps to reproduce')[1]
        .split('Automated replay')[0];

    it('keeps integer ids and gives any other value as its shape', () => {
      expect(steps().trim().split('\n')).toEqual([
        '1. `map_feature_click` shot id=17 in section 2',
        '2. `map_feature_click` station index=0 in section 3 (right)',
        '3. `map_feature_click` station index=string(12) in section string(26)',
        '4. `map_feature_click` shot id={name: string(12)} in section number',
      ]);
      expect(steps()).not.toContain('injected');
      expect(steps()).not.toContain('Qz');
    });
  });

  // os_keys presses one chord per step, so a run of them spells text a key at a
  // time, as press_keys does (#576 RV19b-P6): the same rule, per chord.
  describe('os_keys steps keep the modifiers and named keys and count the typed ones', () => {
    let osDir: string;

    beforeAll(() => {
      osDir = fs.mkdtempSync(path.join(os.tmpdir(), 'stalagmight-gen-oskeys-'));
      const ts = (s: number) => new Date(1751970000000 + s * 1000).toISOString();
      const chord = (seq: number, spec: string) =>
        record({ type: 'action', v: 1, seq, ts: ts(seq), tool: 'os_keys', mutating: true, params: { spec }, outcome: { ok: true, durationMs: 1 } });
      const lines = [
        record({ type: 'session-start', v: 1, sessionId: 'oskeys-fixture', startedAt: ts(0) }),
        chord(1, 'q'),
        chord(2, 'z'),
        chord(3, 'cmd+s'),
        chord(4, 'Cmd+Shift+Z'),
        chord(5, 'escape'),
        chord(6, 'F5'),
        chord(7, 'shift+equal'),
        chord(8, 'ctrl+qz'),
      ];
      fs.writeFileSync(path.join(osDir, 'actions.jsonl'), `${lines.join('\n')}\n`);
      execFileSync(process.execPath, [reportGen, osDir], { encoding: 'utf8' });
    });

    afterAll(() => fs.rmSync(osDir, { recursive: true, force: true }));

    const steps = (): string =>
      fs
        .readFileSync(path.join(osDir, 'report.md'), 'utf8')
        .split('## Steps to reproduce')[1]
        .split('Automated replay')[0];

    it('keeps each modifier and named key and gives a typed key as a count of characters', () => {
      expect(steps().trim().split('\n')).toEqual([
        '1. `os_keys` [1 character] [OS-level]',
        '2. `os_keys` [1 character] [OS-level]',
        '3. `os_keys` ["cmd", 1 character] [OS-level]',
        '4. `os_keys` ["cmd", "shift", 1 character] [OS-level]',
        '5. `os_keys` ["escape"] [OS-level]',
        '6. `os_keys` ["f5"] [OS-level]',
        '7. `os_keys` ["shift", "equal"] [OS-level]',
        '8. `os_keys` ["ctrl", 2 characters] [OS-level]',
      ]);
    });
  });

  // Every value report.md reads back from actions.jsonl - a step's number,
  // coordinate, delta, button or dialog kind, a scenario's seed, a name, the
  // session's commit, display and id, a finding's message and directory - is
  // printed only in the form the harness writes it. A forged or hand-edited log
  // put a newline and a `## heading` there and started a markdown line of its
  // own (#576 W49-NF1, as map_feature_click's ids did in W47-NF2); now any other
  // value is given as its shape. A real session prints exactly as before.
  describe('values read back from the log print only in the form the harness writes them', () => {
    const write = (prefix: string, start: object, actions: object[], end: object, findings: object[] = []): string => {
      const dir = fs.mkdtempSync(path.join(os.tmpdir(), prefix));
      const ts = (s: number) => new Date(1751970000000 + s * 1000).toISOString();
      const lines = [
        record({ type: 'session-start', v: 1, startedAt: ts(0), ...start }),
        ...actions.map((a, i) => record({ type: 'action', v: 1, ts: ts(i + 1), outcome: { ok: true, durationMs: 1 }, ...a })),
        ...findings.map((detail, i) => record({ type: 'event', v: 1, seq: i, ts: ts(90), event: 'finding', detail })),
        record({ type: 'session-end', v: 1, ts: ts(99), reason: 'app_stop', ...end }),
      ];
      fs.writeFileSync(path.join(dir, 'actions.jsonl'), `${lines.join('\n')}\n`);
      return dir;
    };
    const F = '\n## forged';
    let forgedDir: string;
    let validDir: string;
    let stdout: string;

    beforeAll(() => {
      forgedDir = write(
        'stalagmight-gen-forged-',
        { sessionId: `qzsession${F}`, gitHead: `abc1234${F}`, display: `:0${F}` },
        [
          // No `mutating` field: a legacy record, so the os_* steps are steps too.
          { seq: 1, tool: 'click', params: { x: `10${F}`, y: 20 } },
          { seq: 2, tool: 'drag', mutating: true, params: { fromX: 1, fromY: `2${F}`, toX: 3, toY: 4, button: `middle${F}` } },
          { seq: 3, tool: 'scroll', mutating: true, params: { deltaX: `0${F}`, deltaY: 5 } },
          { seq: 4, tool: 'file_dialog_answer', mutating: true, params: { kind: `open${F}`, cancel: true, triggerTestid: 'import-button' } },
          { seq: 5, tool: 'run_scenario', params: { name: `mapStorm${F}`, seed: `7${F}`, iterations: `3${F}` } },
          { seq: 6, tool: 'os_click', params: { x: `1${F}`, y: 2 } },
          { seq: 7, tool: 'os_screenshot', params: { name: `shot${F}` } },
          { seq: `8${F}`, tool: 'click', mutating: true, params: { testid: 'save-button' } },
          { seq: 9, tool: `qz_tool${F}`, mutating: true, params: {} },
        ],
        { actions: `9${F}` },
        [{ severity: 'error', kind: 'unresponsive', message: `IPC did not return within 5s${F}`, dir: `0001${F}` }],
      );
      validDir = write(
        'stalagmight-gen-valid-',
        { sessionId: 'valid-fixture', gitHead: 'abc1234', display: ':89' },
        [
          { seq: 1, tool: 'click', params: { x: 10, y: 20 } },
          { seq: 2, tool: 'drag', mutating: true, params: { fromX: 1.5, fromY: 2, toX: 3, toY: 4, button: 'right' } },
          { seq: 3, tool: 'scroll', mutating: true, params: { deltaY: 400 } },
          { seq: 4, tool: 'file_dialog_answer', mutating: true, params: { kind: 'save', cancel: true, triggerTestid: 'import-button' } },
          { seq: 5, tool: 'run_scenario', params: { name: 'mapStorm', seed: 7, iterations: 3 } },
          { seq: 6, tool: 'os_click', params: { x: 100, y: 200 } },
          { seq: 7, tool: 'os_screenshot', params: { name: 'after-drag' } },
          { seq: 8, tool: 'ipc', mutating: true, params: { command: 'save_project', args: {} } },
          { seq: 9, tool: 'store_action', mutating: true, params: { store: 'project', action: 'undo', args: [] } },
        ],
        { actions: 9 },
        [{ severity: 'error', kind: 'unresponsive', message: 'IPC did not return within 5s; possible command-lock deadlock', dir: '0001-unresponsive' }],
      );
      execFileSync(process.execPath, [reportGen, forgedDir], { encoding: 'utf8' });
      stdout = execFileSync(process.execPath, [reportGen, validDir], { encoding: 'utf8' });
    });

    afterAll(() => {
      fs.rmSync(forgedDir, { recursive: true, force: true });
      fs.rmSync(validDir, { recursive: true, force: true });
    });

    const report = (dir: string): string => fs.readFileSync(path.join(dir, 'report.md'), 'utf8');
    const steps = (dir: string): string[] =>
      report(dir).split('## Steps to reproduce')[1].split('Automated replay')[0].trim().split('\n');

    it('lets no forged value start a markdown line, or print at all', () => {
      const md = report(forgedDir);
      expect(md.split('\n').filter((line) => line.startsWith('## forged'))).toEqual([]);
      expect(md).not.toContain('forged');
      expect(md).not.toContain('qz');
    });

    it('gives each forged value as its shape', () => {
      expect(steps(forgedDir)).toEqual([
        '1. `click` coords=(string(12),20)',
        '2. `drag` (1,string(11)) -> (3,4) (string(16))',
        '3. `scroll` window by (string(11),5)',
        '4. `file_dialog_answer` string(14) dialog cancelled (trigger import-button)',
        '5. `run_scenario` string(18) (seed=string(11), iterations=string(11))',
        '6. `os_click` screen string(11),2 [OS-level]',
        '7. `os_screenshot` string(14) [OS-level, real pixels]',
        'string(11). `click` testid=save-button',
        '9. `string(17)`',
      ]);
      const md = report(forgedDir);
      expect(md).toContain('# Agent-found issue in session string(19)');
      expect(md).toContain('- Commit: `string(17)`');
      expect(md).toContain('- Display: `string(12)`');
      expect(md).toContain('- Session: `string(19)`  actions: `string(11)`');
      expect(md).toContain("- **[error] unresponsive** message withheld: it can quote the app's text");
      expect(md).toContain('  evidence: `findings/string(14)/`');
    });

    it('prints a real session exactly as before', () => {
      expect(steps(validDir)).toEqual([
        '1. `click` coords=(10,20)',
        '2. `drag` (1.5,2) -> (3,4) (right)',
        '3. `scroll` window by (0,400)',
        '4. `file_dialog_answer` save dialog cancelled (trigger import-button)',
        '5. `run_scenario` mapStorm (seed=7, iterations=3)',
        '6. `os_click` screen 100,200 [OS-level]',
        '7. `os_screenshot` after-drag [OS-level, real pixels]',
        '8. `ipc` save_project({})',
        '9. `store_action` project.undo()',
      ]);
      const md = report(validDir);
      expect(md).toContain('# Agent-found issue in session valid-fixture');
      expect(md).toContain('- Commit: `abc1234`');
      expect(md).toContain('- Display: `:89`  headed: `false`');
      expect(md).toContain('- Session: `valid-fixture`  actions: `9`');
      expect(md).toContain('- **[error] unresponsive** IPC did not return within 5s; possible command-lock deadlock');
      expect(md).toContain('  evidence: `findings/0001-unresponsive/`');
    });

    // stdout is shareable too: the session dir is the caller's path, which can
    // sit outside the checkout and name their own directories (#576 W51).
    it('names the session dir on stdout and stderr by its placeholder, never its path', () => {
      expect(stdout).toBe('wrote <sessionDir>/report.md\n');
      const missing = path.join(validDir, 'qz-missing');
      let stderr = '';
      try {
        execFileSync(process.execPath, [reportGen, missing], { encoding: 'utf8', stdio: 'pipe' });
      } catch (e) {
        stderr = String((e as { stderr?: string }).stderr);
      }
      expect(stderr).toBe('no actions.jsonl in <sessionDir>\n');
    });
  });
});
