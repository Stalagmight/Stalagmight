/**
 * projectPeople: everyone the open project credits, without materializing a
 * binary-loaded line's lazy shots.
 */
import { describe, expect, it } from 'vitest';
import { noteLazyLineCredits, projectPeople, linePeople } from '../projectPeople';
import type { Line, SurveyCredit } from '../../types/ipc.generated';

const credit = (surveyors: string[], explorers: string[] = []): SurveyCredit => ({
  surveyors: surveyors.map((name) => ({ name })),
  explorers: explorers.map((name) => ({ name })),
  date: null,
});

function eager(credits: Array<SurveyCredit | undefined>): Line {
  return { shots: credits.map((c, i) => ({ id: i, ...(c ? { credit: c } : {}) })) } as unknown as Line;
}

function lazy(credits: SurveyCredit[]): { line: Line; materialized: () => boolean } {
  let touched = false;
  const line = {} as Line;
  Object.defineProperty(line, 'shots', {
    get() {
      touched = true;
      return [];
    },
    enumerable: true,
    configurable: true,
  });
  noteLazyLineCredits(line, () => credits);
  return { line, materialized: () => touched };
}

describe('projectPeople', () => {
  it('collects surveyors and explorers of every shot of every line once', () => {
    const a = eager([credit(['Ann', 'Bo']), undefined, credit(['ann'], ['Cy'])]);
    const b = eager([credit([], ['Bo', 'Dee'])]);
    expect(projectPeople({ lines: [a, b] }).map((p) => p.name)).toEqual(['Ann', 'Bo', 'Cy', 'Dee']);
  });

  it('reads a lazy line from its registered credits without materializing it', () => {
    const { line, materialized } = lazy([credit(['Eve'], ['Fay'])]);
    expect(linePeople(line).map((p) => p.name)).toEqual(['Eve', 'Fay']);
    expect(materialized()).toBe(false);
  });

  it('caches per line and per shots array', () => {
    const a = eager([credit(['Ann'])]);
    expect(linePeople(a)).toBe(linePeople(a));
  });

  it('is empty without a project', () => {
    expect(projectPeople(null)).toEqual([]);
    expect(projectPeople({} as never)).toEqual([]);
  });
});
