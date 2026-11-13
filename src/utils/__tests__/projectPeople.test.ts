/**
 * projectPeople: everyone the open project credits, without materializing a
 * binary-loaded section's lazy shots.
 */
import { describe, expect, it } from 'vitest';
import { noteLazySectionCredits, projectPeople, sectionPeople } from '../projectPeople';
import type { Section, SurveyCredit } from '../../types/ipc.generated';

const credit = (surveyors: string[], explorers: string[] = []): SurveyCredit => ({
  surveyors: surveyors.map((name) => ({ name })),
  explorers: explorers.map((name) => ({ name })),
  date: null,
});

function eager(credits: Array<SurveyCredit | undefined>): Section {
  return { shots: credits.map((c, i) => ({ id: i, ...(c ? { credit: c } : {}) })) } as unknown as Section;
}

function lazy(credits: SurveyCredit[]): { section: Section; materialized: () => boolean } {
  let touched = false;
  const section = {} as Section;
  Object.defineProperty(section, 'shots', {
    get() {
      touched = true;
      return [];
    },
    enumerable: true,
    configurable: true,
  });
  noteLazySectionCredits(section, () => credits);
  return { section, materialized: () => touched };
}

describe('projectPeople', () => {
  it('collects surveyors and explorers of every shot of every section once', () => {
    const a = eager([credit(['Ann', 'Bo']), undefined, credit(['ann'], ['Cy'])]);
    const b = eager([credit([], ['Bo', 'Dee'])]);
    expect(projectPeople({ sections: [a, b] }).map((p) => p.name)).toEqual(['Ann', 'Bo', 'Cy', 'Dee']);
  });

  it('reads a lazy section from its registered credits without materializing it', () => {
    const { section, materialized } = lazy([credit(['Eve'], ['Fay'])]);
    expect(sectionPeople(section).map((p) => p.name)).toEqual(['Eve', 'Fay']);
    expect(materialized()).toBe(false);
  });

  it('caches per section and per shots array', () => {
    const a = eager([credit(['Ann'])]);
    expect(sectionPeople(a)).toBe(sectionPeople(a));
  });

  it('is empty without a project', () => {
    expect(projectPeople(null)).toEqual([]);
    expect(projectPeople({} as never)).toEqual([]);
  });
});
