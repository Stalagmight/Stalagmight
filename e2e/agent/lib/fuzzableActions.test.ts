/**
 * The fuzzable-action allowlist, from both directions.
 *
 * `runStep` dispatches by NAME (`store.getState()[req.action]`), and on a
 * replay that name comes out of a trace file. The allowlist is what stops a
 * crafted trace calling `loadProject` or `saveProject` on a path of its
 * choosing inside a privileged webview - and it is only useful if it also
 * COVERS everything the generators legitimately produce, or the first honest
 * repro of a newly added action dies at the boundary instead of reproducing.
 *
 * Reading the generator sources rather than importing them is deliberate:
 * `undoRedoFuzzActions.ts` and `undoPersistenceCoverage.ts` build their actions
 * inside closures that need a live `ScenarioContext`, so the names cannot be
 * enumerated by calling them. The regex is anchored to the `action: '...'`
 * literal both files use for every generator.
 */
import * as fs from 'fs';
import * as path from 'path';
import { fileURLToPath } from 'url';
import { describe, expect, it } from 'vitest';

import { FUZZABLE_ACTIONS, REPLAYABLE_MUTATIONS } from '../scenarios/undoRedoFuzzBridge';

const scenariosDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', 'scenarios');

function actionLiteralsIn(file: string): string[] {
  const source = fs.readFileSync(path.join(scenariosDir, file), 'utf8');
  return [...source.matchAll(/\baction: '([A-Za-z][A-Za-z0-9]*)'/g)].map((match) => match[1] ?? '');
}

describe('FUZZABLE_ACTIONS', () => {
  it('covers every action the random generators can produce', () => {
    const produced = actionLiteralsIn('undoRedoFuzzActions.ts');

    expect(produced.length).toBeGreaterThan(15);
    // Replayable, not merely dispatchable: a generator's action has to survive
    // the narrower gate the REPLAY path applies, or its own recorded trace
    // cannot be replayed.
    expect(produced.filter((action) => !REPLAYABLE_MUTATIONS.has(action))).toEqual([]);
  });

  it('covers every action the v3 import generators can produce', () => {
    // A SEPARATE file, and so a separate scan: the v3 group lives in
    // `undoRedoFuzzImportActions.ts`, and the first test above would keep
    // passing while every one of its actions died at the dispatch boundary.
    const produced = actionLiteralsIn('undoRedoFuzzImportActions.ts');

    expect(produced.length).toBeGreaterThan(8);
    expect(produced.filter((action) => !REPLAYABLE_MUTATIONS.has(action))).toEqual([]);
  });

  it('covers every action the persistence coverage plan can produce', () => {
    const produced = actionLiteralsIn('undoPersistenceCoverage.ts');

    expect(produced.length).toBeGreaterThan(10);
    expect(produced.filter((action) => !REPLAYABLE_MUTATIONS.has(action))).toEqual([]);
  });

  it('does NOT carry the store actions a trace must never reach', () => {
    // The three that make a hostile trace worth writing: open a project of its
    // choosing, write over a file of its choosing, or drop the one under test.
    for (const forbidden of ['loadProject', 'closeProject', 'setConfig', 'saveProjectAs']) {
      expect(FUZZABLE_ACTIONS.has(forbidden)).toBe(false);
    }
  });

  it('carries the non-mutation steps the scenarios drive', () => {
    for (const required of ['noop:observe', 'undo', 'redo', 'undoTo', 'redoTo', 'saveProject']) {
      expect(FUZZABLE_ACTIONS.has(required)).toBe(true);
    }
  });

  it('keeps the path-taking control actions OUT of the replayable set', () => {
    // `saveProject(path?)` takes a path. The scenarios pass their own temp path
    // and never a recorded one, so a trace naming it would be choosing where
    // this process writes a .stalx - which is why the replay path checks the
    // narrower set and the dispatch path checks the wider one.
    for (const controlOnly of ['saveProject', 'undo', 'redo', 'noop:observe']) {
      expect(FUZZABLE_ACTIONS.has(controlOnly)).toBe(true);
      expect(REPLAYABLE_MUTATIONS.has(controlOnly)).toBe(false);
    }
  });

  it('is a subset relationship, not two hand-maintained lists', () => {
    for (const action of REPLAYABLE_MUTATIONS) expect(FUZZABLE_ACTIONS.has(action)).toBe(true);
  });
});
