import { describe, expect, it } from 'vitest';

import { parseScenarioArgs, scenarioRunOptions } from './scenarioRunArgs';

// [#576 NF-E12-1] `pnpm agent:scenario <name> --trace FILE` restored the
// recording's environment but not its seed or iteration count: a 1-cycle
// undoPersistenceFuzz recording replayed 12 cycles (every iteration replays the
// whole trace), and a seed-16 undoRedoFuzz recording ran - and reported - seed 1.

const TRACE = { seed: 16, iterations: 1 };

describe('scenarioRunOptions', () => {
  it("a replay runs at the trace's own seed and iteration count", () => {
    const options = scenarioRunOptions(parseScenarioArgs(['undoPersistenceFuzz', '--trace', 't.json']), TRACE);
    console.log(`[NF-E12-1] replay of a seed-16, 1-cycle trace runs with: ${JSON.stringify(options)}`);
    expect(options).toEqual({ seed: 16, iterations: 1 });
  });

  it('an explicit --seed / --iterations still wins over the trace', () => {
    const args = parseScenarioArgs(['undoRedoFuzz', '--trace', 't.json', '--seed', '-3', '--iterations', '5']);
    expect(scenarioRunOptions(args, TRACE)).toEqual({ seed: -3, iterations: 5 });
    expect(scenarioRunOptions(parseScenarioArgs(['undoRedoFuzz', '--seed', '7']), TRACE)).toEqual({
      seed: 7,
      iterations: 1,
    });
  });

  it("without a trace only the flags given are passed (the scenario's defaults apply)", () => {
    expect(scenarioRunOptions(parseScenarioArgs(['mapStorm']), null)).toEqual({});
    expect(scenarioRunOptions(parseScenarioArgs(['mapStorm', '--iterations', '60']), null)).toEqual({ iterations: 60 });
  });

  it("refuses a trace whose numbers the flags would refuse, unless a flag replaces them", () => {
    expect(() => scenarioRunOptions({}, { seed: 1.5, iterations: 1 })).toThrow(/seed 1.5 is not an integer/);
    expect(() => scenarioRunOptions({}, { seed: 1, iterations: 0 })).toThrow(/iteration count 0 is not a positive integer/);
    expect(scenarioRunOptions({ iterations: 2 }, { seed: 1, iterations: 0 })).toEqual({ seed: 1, iterations: 2 });
  });
});

describe('parseScenarioArgs', () => {
  it('parses the flags and refuses malformed numbers', () => {
    expect(parseScenarioArgs(['fractalStress', '--seed', '7', '--iterations', '60', '--release'])).toEqual({
      name: 'fractalStress',
      seed: 7,
      iterations: 60,
      release: true,
    });
    expect(() => parseScenarioArgs(['fractalStress', '--iterations', '0'])).toThrow(/positive integer/);
    expect(() => parseScenarioArgs(['fractalStress', '--seed', '3.7'])).toThrow(/needs an integer/);
    expect(() => parseScenarioArgs([])).toThrow(/usage: pnpm agent:scenario/);
    expect(() => parseScenarioArgs(['fractalStress', '--bogus'])).toThrow(/unknown flag, a string of 7 character\(s\)/);
  });

  it('prints what the user typed, and what a trace carries, as its shape (#576 W51)', () => {
    const refusal = (run: () => unknown): string => {
      try {
        run();
      } catch (err) {
        return (err as Error).message;
      }
      return '';
    };
    const refusals = [
      refusal(() => parseScenarioArgs(['fractalStress', '/home/qzCaveA/x.tml'])),
      refusal(() => parseScenarioArgs(['fractalStress', '--seed', 'qzCaveA'])),
      refusal(() => parseScenarioArgs(['fractalStress', '--iterations', 'qzCaveA'])),
      refusal(() => scenarioRunOptions({}, { seed: 'qzCaveA' as unknown as number, iterations: 1 })),
      refusal(() => scenarioRunOptions({}, { seed: 1, iterations: ['qzCaveA'] as unknown as number })),
    ];
    for (const message of refusals) {
      expect(message).not.toBe('');
      expect(message).not.toContain('qzCaveA');
    }
    expect(refusal(() => parseScenarioArgs(['fractalStress', '--iterations', '0']))).toBe(
      'flag --iterations needs a positive integer, got 0',
    );
  });
});
