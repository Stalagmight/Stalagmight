/**
 * #576 W58 Q1: shareable output prints a fixture's name only when the repo
 * tracks it; any other fixture prints as a tag. Synthetic names only (`qz...`):
 * no fixture file is created, a private name reaches the fold the way a knob or
 * a trace hands it over.
 */
import { afterEach, describe, expect, it } from 'vitest';

import { DEFAULT_FIXTURE } from './fixtures';
import { fuzzFixture } from './fuzzKnobs';
import { describeSteps, tracePrivateFixtures, type TraceStep } from './fuzzTrace';
import { foldPrivateFixtures, fixtureTag, isTrackedFixture, noteFixtureName, shareableFixtureName } from './privateFixtures';
import { scenarioFailOutput, shareableProgressLine, shareableTail } from './shareableFailure';

const TAG = /^<private-fixture#[0-9a-f]{5}>$/;

describe('a fixture name in shareable output (#576 W58 Q1)', () => {
  const saved = process.env.STALAGMIGHT_FUZZ_FIXTURE;
  afterEach(() => {
    if (saved === undefined) delete process.env.STALAGMIGHT_FUZZ_FIXTURE;
    else process.env.STALAGMIGHT_FUZZ_FIXTURE = saved;
  });

  it('prints a tracked fixture as itself and anything else as a tag', () => {
    expect(isTrackedFixture(DEFAULT_FIXTURE)).toBe(true);
    expect(shareableFixtureName(DEFAULT_FIXTURE)).toBe(DEFAULT_FIXTURE);
    expect(shareableFixtureName('qzCaveA.stalx')).toMatch(TAG);
    expect(shareableFixtureName('qzCaveA.stalx')).toBe(fixtureTag('qzCaveA.stalx'));
    expect(shareableFixtureName('qzCaveA.stalx')).not.toBe(shareableFixtureName('qzCaveB.stalx'));
    // Only a bare name directly under e2e/fixtures/ is tracked.
    expect(shareableFixtureName(`sub/${DEFAULT_FIXTURE}`)).toMatch(TAG);
    expect(shareableFixtureName(['qzCaveA.stalx'])).toMatch(TAG);
  });

  it('folds a noted private name wherever it shows up, not inside a longer name, and folds once', () => {
    noteFixtureName('qz Cave D.stalx');
    noteFixtureName(DEFAULT_FIXTURE);
    const text = `loadProject(qz Cave D.stalx) failed at <repo>/x/fixtures/qz Cave D.stalx; xqz Cave D.stalxy and ${DEFAULT_FIXTURE}`;
    const folded = foldPrivateFixtures(text);
    const tag = fixtureTag('qz Cave D.stalx');
    expect(folded).toBe(`loadProject(${tag}) failed at <repo>/x/fixtures/${tag}; xqz Cave D.stalxy and ${DEFAULT_FIXTURE}`);
    expect(foldPrivateFixtures(folded)).toBe(folded);
  });

  it("prints a private knob fixture as its tag on a scenario's FAIL block, progress line and a matrix tail", () => {
    process.env.STALAGMIGHT_FUZZ_FIXTURE = 'qzCaveE.stalx';
    const fixture = fuzzFixture();
    const tag = fixtureTag('qzCaveE.stalx');
    const failure = scenarioFailOutput(
      'undo-redo-fuzz',
      new Error(
        `E2 INTEGRITY: ${fixture} opened SAVE-BLOCKED\n` +
          `  reproduce: STALAGMIGHT_FUZZ_FIXTURE=${fixture} pnpm agent:scenario undo-redo-fuzz --seed 3`,
      ),
    );
    expect(failure).not.toContain('qzCaveE');
    expect(failure).toBe(
      `FAIL: undo-redo-fuzz threw:\nE2 INTEGRITY: ${tag} opened SAVE-BLOCKED\n` +
        `  reproduce: STALAGMIGHT_FUZZ_FIXTURE=${tag} pnpm agent:scenario undo-redo-fuzz --seed 3\n`,
    );
    expect(shareableProgressLine(`cycle 2 on ${fixture}: 4 moves`)).toBe(`cycle 2 on ${tag}: 4 moves`);
    expect(shareableTail([`fatal: Error: loadProject(${fixture}) failed: no such file`])).not.toContain('qzCaveE');
  });

  it('keeps a tracked fixture in a reproduce line as it is', () => {
    process.env.STALAGMIGHT_FUZZ_FIXTURE = DEFAULT_FIXTURE;
    const line = `  reproduce: STALAGMIGHT_FUZZ_FIXTURE=${fuzzFixture()} pnpm agent:scenario undo-redo-fuzz --seed 3`;
    expect(scenarioFailOutput('undo-redo-fuzz', new Error(`E2 INTEGRITY: x\n${line}`))).toContain(line);
  });

  it('reads the knob as a basename, as its doc comment says', () => {
    process.env.STALAGMIGHT_FUZZ_FIXTURE = 'qzdir/qzCaveF.stalx';
    expect(fuzzFixture()).toBe('qzCaveF.stalx');
    delete process.env.STALAGMIGHT_FUZZ_FIXTURE;
    expect(fuzzFixture()).toBe(DEFAULT_FIXTURE);
  });
});

describe("fuzz-shrink's reload step (#576 W58 NF1)", () => {
  it('prints a tracked fixture by name and a private one as its tag', () => {
    const out = describeSteps([
      { kind: 'reload', fixture: DEFAULT_FIXTURE },
      { kind: 'reload', fixture: 'qzCaveG.stalx' },
    ]);
    expect(out).not.toContain('qzCaveG');
    expect(out.split('\n')).toEqual([`  1. reload ${DEFAULT_FIXTURE}`, `  2. reload ${fixtureTag('qzCaveG.stalx')}`]);
  });

  it("names each private fixture a trace holds once, for the session's private log", () => {
    const steps: TraceStep[] = [
      { kind: 'save' },
      { kind: 'reload', fixture: DEFAULT_FIXTURE },
      { kind: 'reload', fixture: 'qzCaveG.stalx' },
      { kind: 'reload', fixture: 'qzCaveH.stalx' },
    ];
    expect(tracePrivateFixtures({ fixture: 'qzCaveH.stalx', steps })).toEqual([
      { fixture: 'qzCaveH.stalx', tag: fixtureTag('qzCaveH.stalx') },
      { fixture: 'qzCaveG.stalx', tag: fixtureTag('qzCaveG.stalx') },
    ]);
    expect(tracePrivateFixtures({ fixture: DEFAULT_FIXTURE, steps: [{ kind: 'reload', fixture: DEFAULT_FIXTURE }] })).toEqual([]);
  });
});
