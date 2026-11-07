/**
 * The import fuzzer's testid allowlists, from both directions.
 *
 * `runUiStep` reaches a control by TESTID, and on a replay that testid comes out
 * of a trace file, which `readTrace` checks only for its schema version and step
 * shapes. So the allowlist is what stops a crafted trace clicking a
 * project-delete confirm or a SpeleoDB push inside a privileged webview with the
 * `__E2E_TEST__` hooks live - and it is only useful if it also COVERS everything
 * the generators legitimately produce, or the first honest repro of a newly
 * added control dies at the boundary instead of reproducing.
 *
 * Mirrors `fuzzableActions.test.ts`, which pins the same two directions for the
 * undo fuzzer's store-action allowlist.
 */
import { describe, expect, it } from 'vitest';

import {
  ARMABLE_KEYS,
  CHECKABLE_PATTERNS,
  CHECKABLE_TESTIDS,
  CLICKABLE_PATTERNS,
  CLICKABLE_TESTIDS,
  isDispatchable,
  keyEventInit,
  PRESSABLE_KEYS,
  SELECTABLE_PATTERNS,
  SELECTABLE_TESTIDS,
  TYPEABLE_TESTIDS,
} from '../scenarios/importDialogBridge';
import {
  ADVANCE_MOVE,
  ESCAPE_PATH,
  IN_FLOW_SCREENS,
  NON_FLOW_SCREENS,
  SELECT_TESTIDS,
  TESTID_INVENTORY,
} from '../scenarios/importDialogModel';
import {
  CORPUS_DRAW_P,
  corpusDrawProbability,
  describeSource,
  drawFileSource,
  enabledFamilies,
  GENERATORS,
  GPS_HOSTILE,
  GPS_PASSABLE,
  gpsPassableProbability,
} from '../scenarios/importDialogGenerators';
import { corpusPathHash } from './fixtureCorpus';
import { restoreTraceEnv } from './fuzzTrace';

/**
 * Testids a generator can emit, as literals.
 *
 * Read out of the SOURCE rather than by calling the generators: every `build`
 * needs a live `ScenarioContext` and a digest of a running app, so the emitted
 * ids cannot be enumerated by invoking them. The two regexes are anchored to the
 * `click(...)` / `type(...)` / `select(...)` / `check(...)` call shapes the
 * catalog uses for every entry, plus the template literals the dynamic ones
 * build.
 */
const source = new URL('../scenarios/importDialogGenerators.ts', import.meta.url);

async function generatorSource(): Promise<string> {
  const fs = await import('fs');
  return fs.readFileSync(source, 'utf8');
}

/** `click('x', ...)` / `type('x', ...)` / `select('x', ...)` / `check('x', ...)`. */
function literalTargets(text: string, fn: string): string[] {
  const re = new RegExp(`\\b${fn}\\('([^']+)'`, 'g');
  return [...text.matchAll(re)].map((match) => match[1] as string);
}

/** Every `` `prefix-${...}` `` template the catalog drives, by call shape. */
function templateTargets(text: string, fn = 'click'): string[] {
  const re = new RegExp(`\\b${fn}\\(\`([^\`]+)\``, 'g');
  return [...text.matchAll(re)].map((match) => match[1] as string);
}

/** Turn a template into a concrete id a pattern can be tested against. */
function sampleFrom(template: string): string {
  return template
    .replace(/\$\{[^}]*\[1\][^}]*\}/g, 'length')
    .replace(/\$\{[^}]*\}/g, '7');
}

describe('import fuzzer testid allowlists', () => {
  it('allows every literal testid the generators can click', async () => {
    const text = await generatorSource();
    const clicked = literalTargets(text, 'click').filter((id) => !id.startsWith('key:'));
    expect(clicked.length).toBeGreaterThan(25);
    const rejected = clicked.filter((id) => !isDispatchable({ kind: 'ui', testid: id, label: 'x' }));
    expect(rejected).toEqual([]);
  });

  it('allows every dynamic testid the generators can click', async () => {
    const text = await generatorSource();
    const templates = templateTargets(text);
    expect(templates.length).toBeGreaterThan(4);
    const rejected = templates
      .map(sampleFrom)
      .filter((id) => !isDispatchable({ kind: 'ui', testid: id, label: 'x' }));
    expect(rejected).toEqual([]);
  });

  it('allows every key a generator can press', async () => {
    const text = await generatorSource();
    const keys = literalTargets(text, 'click').filter((id) => id.startsWith('key:'));
    expect(keys.length).toBeGreaterThan(0);
    for (const key of keys) {
      expect(isDispatchable({ kind: 'ui', testid: key, label: 'x' })).toBe(true);
    }
  });

  it('allows every testid the generators can type into, select or check', async () => {
    const text = await generatorSource();
    for (const [fn, mode] of [
      ['type', 'type'],
      ['select', 'select'],
      ['check', 'check'],
    ] as const) {
      const targets = literalTargets(text, fn);
      expect(targets.length).toBeGreaterThan(2);
      const rejected = targets.filter(
        (id) => !isDispatchable({ kind: 'input', testid: id, value: 'v', mode, label: 'x' }),
      );
      expect(rejected).toEqual([]);
    }
    // `check` also drives `section-checkbox-N`, built as a template literal.
    expect(
      isDispatchable({ kind: 'input', testid: 'section-checkbox-12', value: 'true', mode: 'check', label: 'x' }),
    ).toBe(true);
    // Selects reached through a variable rather than a literal (the wizard's
    // from-existing / manual families) still have to be allowed.
    for (const id of SELECT_TESTIDS) {
      expect(isDispatchable({ kind: 'input', testid: id, value: '0', mode: 'select', label: 'x' })).toBe(true);
    }
  });

  it('allows every testid the model can force as an advance or an escape', () => {
    const forced = [
      ...Object.values(ADVANCE_MOVE).flatMap((route) => [...(route ?? [])]),
      ...Object.values(ESCAPE_PATH).flatMap((route) => [...route]),
    ].filter((id): id is string => typeof id === 'string');
    expect(forced.length).toBeGreaterThan(30);
    // A `*` entry is a PREFIX the shell expands against the digest, so what has
    // to be dispatchable is the EXPANDED id - which means the bridge needs a
    // matching pattern for it, not a literal.
    const rejected = forced
      .map((id) => (id.endsWith('*') ? `${id.slice(0, -1)}7` : id))
      .filter((id) => !isDispatchable({ kind: 'ui', testid: id, label: 'x' }));
    expect(rejected).toEqual([]);
  });

  it('keeps every advance-route prefix inside the observable inventory', () => {
    // The other half: the shell expands a `*` route entry out of `digest.live`,
    // so the digest has to be reporting those ids in the first place. A prefix
    // the inventory does not carry would expand to nothing, every time, and the
    // screen would silently have no advancing move at all.
    const prefixes = Object.values(ADVANCE_MOVE)
      .flatMap((route) => [...(route ?? [])])
      .filter((id) => id.endsWith('*'));
    expect(prefixes.length).toBeGreaterThan(0);
    const inventoryPrefixes = TESTID_INVENTORY.filter((entry) => entry.endsWith('*'));
    for (const prefix of prefixes) {
      expect(inventoryPrefixes, `${prefix} must be observable`).toContain(prefix);
    }
  });

  it('allows every dynamic testid the generators can select', async () => {
    const text = await generatorSource();
    const templates = templateTargets(text, 'select');
    expect(templates.length).toBeGreaterThan(0);
    const rejected = templates
      .map(sampleFrom)
      .filter((id) => !isDispatchable({ kind: 'input', testid: id, value: 'v', mode: 'select', label: 'x' }));
    expect(rejected).toEqual([]);
  });

  it('allows the two dynamic checkbox families', () => {
    // Both are built into a local before the call, so the source scan above
    // cannot see them; they are named here instead of being missed.
    for (const id of ['section-checkbox-12', 'walls-survey-7']) {
      expect(
        isDispatchable({ kind: 'input', testid: id, value: 'true', mode: 'check', label: 'x' }),
        `${id} must be checkable`,
      ).toBe(true);
    }
  });

  it('does NOT carry the destructive controls a trace must never reach', () => {
    // The ones that make a hostile trace worth writing: destroy the project,
    // publish it somewhere, or write it to a path of the trace's choosing.
    const forbidden = [
      'delete-project-confirm',
      'delete-project-button',
      'confirm-delete-button',
      'speleodb-publish',
      'publish-to-speleodb-confirm',
      'speleodb-push',
      'speleodb-force-edit-confirm',
      'save-as-button',
      'project-save-path-browse-button',
      'bulk-delete-confirm',
      'unsaved-changes-discard',
      'debug-trigger-panic',
    ];
    for (const id of forbidden) {
      expect(CLICKABLE_TESTIDS.has(id), `${id} must not be clickable`).toBe(false);
      expect(TYPEABLE_TESTIDS.has(id), `${id} must not be typeable`).toBe(false);
      expect(SELECTABLE_TESTIDS.has(id), `${id} must not be selectable`).toBe(false);
      expect(CHECKABLE_TESTIDS.has(id), `${id} must not be checkable`).toBe(false);
      expect(isDispatchable({ kind: 'ui', testid: id, label: 'x' }), `${id} must not dispatch`).toBe(false);
    }
  });

  it('keeps every pattern anchored and bounded', () => {
    for (const pattern of [...CLICKABLE_PATTERNS, ...CHECKABLE_PATTERNS, ...SELECTABLE_PATTERNS]) {
      const body = pattern.source;
      expect(body.startsWith('^'), `${body} must be anchored at the start`).toBe(true);
      expect(body.endsWith('$'), `${body} must be anchored at the end`).toBe(true);
      // An unbounded quantifier is how a pattern widens into an id it was never
      // meant to reach, so the digit and key runs carry explicit ceilings.
      expect(/[+*]/.test(body.replace(/\\[+*]/g, '')), `${body} must not use + or *`).toBe(false);
    }
  });

  it('refuses a key nobody registered', () => {
    expect(PRESSABLE_KEYS.has('Escape')).toBe(true);
    for (const key of ['Enter', 'Control+o', 'F5', 'Delete', 'ctrl+e']) {
      expect(isDispatchable({ kind: 'ui', testid: `key:${key}`, label: 'x' })).toBe(false);
    }
  });

  it('keeps the ARMABLE chords out of the pressable set', () => {
    // A bare press of Ctrl+O raises a native picker with no armed answer, which
    // falls through to the real GTK chooser and hangs the webview. The two
    // vocabularies must not be interchangeable even by accident.
    for (const chord of ARMABLE_KEYS) {
      expect(PRESSABLE_KEYS.has(chord), `${chord} must not be pressable`).toBe(false);
      expect(isDispatchable({ kind: 'ui', testid: `key:${chord}`, label: 'x' })).toBe(false);
    }
  });

  it('spells a chord the way the app itself reads one back', () => {
    /**
     * A MIRROR of `parseKeyEvent` + `normalizeKeyName` (`src/utils/shortcuts.ts`),
     * not an import of them: that module's first line pulls in `../i18n`, whose
     * `import.meta.env` does not typecheck under `tsconfig.agent.json`. The
     * mirror is three lines and is what the pseudo-testid's whole meaning rests
     * on - a chord this fuzzer presses has to fold back into the shortcut name
     * the app registered, or the keystroke is silently inert.
     */
    const spell = (init: ReturnType<typeof keyEventInit>): string => {
      const parts: string[] = [];
      if (init.ctrl) parts.push('Ctrl');
      if (init.alt) parts.push('Alt');
      if (init.shift) parts.push('Shift');
      parts.push(/^[a-zA-Z]$/.test(init.key) ? init.key.toUpperCase() : init.key);
      return parts.join('+');
    };
    for (const chord of [...PRESSABLE_KEYS, ...ARMABLE_KEYS]) {
      expect(spell(keyEventInit(chord)), `${chord} must round-trip`).toBe(chord);
    }
    // A real browser reports the LOWERCASE letter for an unshifted press, and
    // sending the uppercase one would make the event look like Shift+E.
    expect(keyEventInit('Ctrl+E').key).toBe('e');
    expect(keyEventInit('Escape').key).toBe('Escape');
  });

  it('has an inventory that covers every allowlisted literal', () => {
    // The other direction of the same contract: a control the bridge would
    // dispatch but the DIGEST never reports is one the generators can only ever
    // reach blind, so every literal in an allowlist must be observable.
    const inventory = new Set(TESTID_INVENTORY);
    const prefixes = TESTID_INVENTORY.filter((entry) => entry.endsWith('*')).map((entry) => entry.slice(0, -1));
    const covered = (id: string): boolean =>
      inventory.has(id) || prefixes.some((prefix) => id.startsWith(prefix));
    const uncovered = [
      ...CLICKABLE_TESTIDS,
      ...TYPEABLE_TESTIDS,
      ...SELECTABLE_TESTIDS,
      ...CHECKABLE_TESTIDS,
    ].filter((id) => !covered(id));
    expect(uncovered).toEqual([]);
  });

  it('has a generator for every literal the bridge will click', async () => {
    // The third direction of the contract, and the one that was missing.
    // Allowlisted-and-observable is not enough: a control no generator ever
    // names is one the fuzzer cannot reach, so it is dead surface that reads as
    // covered. `shot-editor-context-clear`, `shot-editor-expand` and
    // `dirty-dialog-keep-editing` all sat in both lists for a whole wave and
    // were never once clicked.
    //
    // Scanned as "does the CODE mention this id at all", not as a `click('x')`
    // call shape: a third of the catalog picks its target out of a local array
    // (`orphan:choose`, `blocked:choose`, the two gpx footer primaries) and a
    // call-shape scan reports every one of those as unreachable. Comments are
    // stripped first, so a name that survives only in prose does not count.
    const text = (await generatorSource())
      .replace(/\/\*[\s\S]*?\*\//g, ' ')
      .replace(/(^|[^:])\/\/[^\n]*/g, '$1 ');
    const named = new Set([
      ...[...text.matchAll(/'([a-z0-9-]{3,60})'/g)].map((match) => match[1] as string),
      ...templateTargets(text).map(sampleFrom),
      ...Object.values(ADVANCE_MOVE).flatMap((route) => [...(route ?? [])]),
      ...Object.values(ESCAPE_PATH).flatMap((route) => [...route]),
    ]);
    const unreachable = [...CLICKABLE_TESTIDS].filter((id) => !named.has(id));
    // No exceptions. Every clickable id is either drawn by a generator or named
    // by the model as an advance or an escape; `telemetry-opt-in-no` qualifies
    // through `first-run`'s entry in both tables.
    expect(unreachable.sort()).toEqual([]);
  });

  it('has a generator catalog whose screens are all escapable', () => {
    const screens = new Set(GENERATORS.flatMap((generator) => [...generator.screens]));
    expect(screens.size).toBeGreaterThan(8);
    for (const screen of screens) {
      expect(ESCAPE_PATH[screen], `no escape path for ${screen}`).toBeDefined();
    }
  });

  it('has a generator for every screen a flow can be on', () => {
    // The other half of the escapability contract: a screen with an escape path
    // and no generators is one the walk can only ever leave, so the controls on
    // it are never exercised at all.
    const screens = new Set(GENERATORS.flatMap((generator) => [...generator.screens]));
    const uncovered = [...IN_FLOW_SCREENS].filter((screen) => !screens.has(screen));
    // The exceptions are named rather than tolerated. `wiz-init` and `wiz-blank`
    // are transient wizard phases that render no control of their own, and
    // `first-run` is the telemetry modal, whose only choice the scenario makes
    // once at startup - so progress forcing is the only thing that ever acts on
    // any of the three. (`wiz-no-suggest` is gone: the wizard no longer renders
    // the frame it classified, #576 NF-W8D4-1.)
    expect(uncovered.sort()).toEqual(['first-run', 'wiz-blank', 'wiz-init']);
    for (const screen of NON_FLOW_SCREENS) {
      expect(screens.has(screen), `${screen} needs a generator too`).toBe(true);
    }
  });

  it('never names a corpus file in anything a run publishes', () => {
    // A survey filename can name a cave, a landowner or an unreleased project.
    // `describeSource` is what reaches the move history, the metric keys and the
    // failure payload, so it carries the id and never the path.
    const relPath = 'some-cave-system/2019-expedition/final.tml';
    const described = describeSource({
      origin: 'corpus',
      corpusRel: relPath,
      id: 'abc12',
      hash: corpusPathHash(relPath),
      dirRel: 'fuzz-files/corpus-3',
      rel: 'fuzz-files/corpus-3/final.tml',
    });
    expect(described).toBe('corpus #abc12');
    for (const segment of relPath.split('/')) expect(described).not.toContain(segment);
    // The resolution key is not published either: it is a lookup field on the
    // trace step, and nothing renders it.
    expect(described).not.toContain(corpusPathHash(relPath));
  });

  it('reads the format axis at call time and falls back to every family', () => {
    const before = process.env.STALAGMIGHT_FUZZ_IMPORT_FORMAT;
    try {
      delete process.env.STALAGMIGHT_FUZZ_IMPORT_FORMAT;
      expect([...enabledFamilies()].sort()).toEqual(['compass', 'dmp', 'gpx', 'tml', 'walls']);
      process.env.STALAGMIGHT_FUZZ_IMPORT_FORMAT = 'walls, GPX ,nonsense';
      expect([...enabledFamilies()].sort()).toEqual(['gpx', 'walls']);
      // An axis that names nothing this build knows is a typo, and silently
      // fuzzing zero formats would be worse than fuzzing all of them.
      process.env.STALAGMIGHT_FUZZ_IMPORT_FORMAT = 'nonsense';
      expect([...enabledFamilies()].sort()).toEqual(['compass', 'dmp', 'gpx', 'tml', 'walls']);
    } finally {
      if (before === undefined) delete process.env.STALAGMIGHT_FUZZ_IMPORT_FORMAT;
      else process.env.STALAGMIGHT_FUZZ_IMPORT_FORMAT = before;
    }
  });

  it('keeps the zero-datum GPS draw passable at least half the time', () => {
    // The floor is the whole point. `zero-datum-config-confirm` is
    // `disabled={!gpsCoords}`, so a run whose GPS corpus never parses cannot
    // complete the zero-datum screen at all - which is what the 130-cell corpus
    // campaign measured (395 steps on the screen, zero `zdc:confirm`).
    const before = process.env.STALAGMIGHT_FUZZ_IMPORT_GPS_PASSABLE_P;
    try {
      delete process.env.STALAGMIGHT_FUZZ_IMPORT_GPS_PASSABLE_P;
      expect(gpsPassableProbability()).toBeGreaterThanOrEqual(0.5);
      for (const [raw, want] of [
        ['0', 0.5],
        ['0.25', 0.5],
        ['1', 1],
        ['4', 1],
        ['nonsense', 0.6],
        ['0.75', 0.75],
      ] as const) {
        process.env.STALAGMIGHT_FUZZ_IMPORT_GPS_PASSABLE_P = raw;
        expect(gpsPassableProbability(), `raw=${raw}`).toBe(want);
      }
    } finally {
      if (before === undefined) delete process.env.STALAGMIGHT_FUZZ_IMPORT_GPS_PASSABLE_P;
      else process.env.STALAGMIGHT_FUZZ_IMPORT_GPS_PASSABLE_P = before;
    }
    // Both corpora stay non-empty: the passable half is an addition to the
    // hostile one, not a replacement for it.
    expect(GPS_PASSABLE.length).toBeGreaterThan(4);
    expect(GPS_HOSTILE.length).toBeGreaterThan(4);
  });

  it('reads the corpus draw share at call time and clamps it', () => {
    // 1.0 is the value that matters: it is how a campaign says "import nothing
    // but real files". It stays a SHARE and not a switch, so a run with no
    // corpus configured still falls through to a generated file.
    const before = process.env.STALAGMIGHT_FUZZ_CORPUS_DRAW_P;
    try {
      delete process.env.STALAGMIGHT_FUZZ_CORPUS_DRAW_P;
      expect(corpusDrawProbability()).toBe(CORPUS_DRAW_P);
      for (const [raw, want] of [
        ['1', 1],
        ['0', 0],
        ['0.4', 0.4],
        ['nonsense', CORPUS_DRAW_P],
        ['2', CORPUS_DRAW_P],
        ['-1', CORPUS_DRAW_P],
      ] as const) {
        process.env.STALAGMIGHT_FUZZ_CORPUS_DRAW_P = raw;
        expect(corpusDrawProbability(), `raw=${raw}`).toBe(want);
      }
    } finally {
      if (before === undefined) delete process.env.STALAGMIGHT_FUZZ_CORPUS_DRAW_P;
      else process.env.STALAGMIGHT_FUZZ_CORPUS_DRAW_P = before;
    }
  });

  it('carries the two new shares in a trace header but never the corpus root', () => {
    // The share reproduces; the directory does not. `STALAGMIGHT_FUZZ_CORPUS_DIR`
    // names a tree this process would walk and copy out of, so a trace does not
    // get to choose it - which is the v3 privacy rule, and it has to keep
    // holding now that a sibling knob with the same prefix exists.
    const restored = restoreTraceEnv({
      STALAGMIGHT_FUZZ_CORPUS_DRAW_P: '1',
      STALAGMIGHT_FUZZ_IMPORT_NO_PROJECT_P: '0',
      STALAGMIGHT_FUZZ_CORPUS_DIR: '/somebody/elses/surveys',
    });
    try {
      expect(restored.applied.map(([key]) => key).sort()).toEqual([
        'STALAGMIGHT_FUZZ_CORPUS_DRAW_P',
        'STALAGMIGHT_FUZZ_IMPORT_NO_PROJECT_P',
      ]);
      expect(restored.refused).toEqual(['STALAGMIGHT_FUZZ_CORPUS_DIR']);
      expect(corpusDrawProbability()).toBe(1);
    } finally {
      delete process.env.STALAGMIGHT_FUZZ_CORPUS_DRAW_P;
      delete process.env.STALAGMIGHT_FUZZ_IMPORT_NO_PROJECT_P;
    }
  });

  it('registers redactions at EVERY corpus materialization site', async () => {
    // The recording path registered them and the REPLAY path did not, so
    // replaying a corpus trace put real basenames - which the app echoes back in
    // its own messages - into `fail()` output, the trace and the printed detail.
    // A structural check rather than a behavioural one because the helper lives
    // inside the scenario's closure and has no seam: what has to hold is that no
    // call site can materialize without registering, which is a property of the
    // SOURCE.
    const fs = await import('fs');
    const scenario = fs.readFileSync(new URL('../scenarios/importDialogFuzz.ts', import.meta.url), 'utf8');
    const lines = scenario.split('\n');
    const sites = lines
      .map((line, index) => ({ line, index }))
      .filter((entry) => /\bmaterializeCorpusEntry\(/.test(entry.line) && !entry.line.trimStart().startsWith('*'));
    // Both known sites: the generator draw and the replayed `corpusFile` step.
    expect(sites.length).toBeGreaterThanOrEqual(2);
    for (const site of sites) {
      // The registration follows within the same block - it needs the
      // `copied.names` the call returns, so it cannot precede it. The window is
      // generous because the generator site explains its skip reasons first.
      const window = lines.slice(site.index, site.index + 22).join('\n');
      expect(
        window,
        `materializeCorpusEntry at line ${site.index + 1} must register redactions`,
      ).toContain('registerCorpusRedactions(');
    }
  });

  it('spends a winning corpus draw on a format the corpus can actually serve', () => {
    // The bug: the format was picked FIRST and the corpus only consulted after,
    // so an enabled format the corpus has nothing for fell through to a
    // generated spec even at a draw share of 1. Measured on a `FORMAT=dmp,tml`
    // run over 11 `.dmp` and one `.tml`: `tmlu` is enabled, the corpus has none,
    // and a third of the draws took the synthetic path silently.
    const before = {
      format: process.env.STALAGMIGHT_FUZZ_IMPORT_FORMAT,
      share: process.env.STALAGMIGHT_FUZZ_CORPUS_DRAW_P,
    };
    try {
      // `tml` enables BOTH `tml` and `tmlu`; the corpus below has only `tml`.
      process.env.STALAGMIGHT_FUZZ_IMPORT_FORMAT = 'dmp,tml';
      process.env.STALAGMIGHT_FUZZ_CORPUS_DRAW_P = '1';
      const corpus = {
        dmp: [{ relPath: 'a/one.dmp', id: 'aaaaa', pathHash: 'h1', format: 'dmp' }],
        tml: [{ relPath: 'a/two.tml', id: 'bbbbb', pathHash: 'h2', format: 'tml' }],
        tmlu: [],
      } as unknown as Parameters<typeof drawFileSource>[0]['corpus'];

      let rngCalls = 0;
      const env = {
        ctx: { rng: () => ((rngCalls += 1) * 0.37) % 1, randInt: () => 0 },
        digest: {},
        filesRel: 'fuzz-files',
        fileSeq: 1,
        corpus,
      } as unknown as Parameters<typeof drawFileSource>[0];

      // Every draw at share 1 must be a corpus draw, and never a `tmlu` one.
      for (let i = 0; i < 60; i += 1) {
        const source = drawFileSource(env);
        expect(source.origin, `draw ${i}`).toBe('corpus');
        if (source.origin === 'corpus') {
          expect(['a/one.dmp', 'a/two.tml']).toContain(source.corpusRel);
        }
      }
    } finally {
      if (before.format === undefined) delete process.env.STALAGMIGHT_FUZZ_IMPORT_FORMAT;
      else process.env.STALAGMIGHT_FUZZ_IMPORT_FORMAT = before.format;
      if (before.share === undefined) delete process.env.STALAGMIGHT_FUZZ_CORPUS_DRAW_P;
      else process.env.STALAGMIGHT_FUZZ_CORPUS_DRAW_P = before.share;
    }
  });

  it('falls through to a generated file when the corpus serves NO enabled format', () => {
    // Narrower, not failed - and the shell counts it. The draw must still
    // produce a usable file rather than throwing on an empty intersection.
    const before = {
      format: process.env.STALAGMIGHT_FUZZ_IMPORT_FORMAT,
      share: process.env.STALAGMIGHT_FUZZ_CORPUS_DRAW_P,
    };
    try {
      process.env.STALAGMIGHT_FUZZ_IMPORT_FORMAT = 'gpx';
      process.env.STALAGMIGHT_FUZZ_CORPUS_DRAW_P = '1';
      const env = {
        ctx: { rng: () => 0.5, randInt: () => 0 },
        digest: {},
        filesRel: 'fuzz-files',
        fileSeq: 2,
        corpus: { gpx: [] },
      } as unknown as Parameters<typeof drawFileSource>[0];
      const source = drawFileSource(env);
      expect(source.origin).toBe('gen');
    } finally {
      if (before.format === undefined) delete process.env.STALAGMIGHT_FUZZ_IMPORT_FORMAT;
      else process.env.STALAGMIGHT_FUZZ_IMPORT_FORMAT = before.format;
      if (before.share === undefined) delete process.env.STALAGMIGHT_FUZZ_CORPUS_DRAW_P;
      else process.env.STALAGMIGHT_FUZZ_CORPUS_DRAW_P = before.share;
    }
  });

  it('has a generator catalog with only positive weights', () => {
    for (const generator of GENERATORS) {
      expect(generator.weight, `${generator.name} weight`).toBeGreaterThan(0);
      expect(generator.screens.length, `${generator.name} screens`).toBeGreaterThan(0);
    }
    const names = GENERATORS.map((generator) => generator.name);
    expect(new Set(names).size, 'generator names must be unique').toBe(names.length);
  });
});
