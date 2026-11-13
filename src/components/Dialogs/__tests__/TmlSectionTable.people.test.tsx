/**
 * The Ariane import preview shows each section's people read-only: the union
 * of the importable shots' credit as compact pills (dashed "2/3" for a person
 * on only some shots), the shots' date or date range, and the legacy section
 * text split into its roles only when no shot carries a credit.
 */
import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';

vi.mock('react-i18next', async (importOriginal) => ({
  ...(await importOriginal<typeof import('react-i18next')>()),
  useTranslation: () => ({ t: (k: string) => k }),
}));

import { TmlSectionTable } from '../TmlSectionTable';
import { tmlSectionCredit } from '../../../utils/tmlSectionCredit';
import type { TmlFile, TmlSection, TmlShot } from '../../../types/commands';
import type { SurveyCredit } from '../../../types/ipc.generated';

function shot(id: number, over: Partial<TmlShot> = {}): TmlShot {
  return {
    id: BigInt(id),
    from_id: BigInt(id - 1),
    shot_type: 'Real',
    azimuth: 90,
    length: 10,
    depth: 0,
    depth_in: 0,
    inclination: 0,
    left: 0,
    right: 0,
    up: 0,
    down: 0,
    comment: null,
    explorer: null,
    date: null,
    color: '#ff0000',
    excluded: false,
    latitude: 0,
    longitude: 0,
    closure_to_id: -1n,
    section: '',
    ...over,
  };
}

function section(name: string, shots: TmlShot[], over: Partial<TmlSection> = {}): TmlSection {
  return {
    name,
    shots,
    surveyor: null,
    date: null,
    color: '#ff0000',
    is_branch_split: false,
    start_ariane_id: 0n,
    end_ariane_id: BigInt(shots.length),
    start_depth: 0,
    ...over,
  };
}

const credit = (surveyors: string[], explorers: string[], date: string | null): SurveyCredit => ({
  surveyors: surveyors.map((name) => ({ name })),
  explorers: explorers.map((name) => ({ name })),
  date,
});

const credited = section('Main', [
  shot(1, { credit: credit(['Ann', 'Bo'], ['Cy'], '2026-03-02') }),
  shot(2, { credit: credit(['Ann'], ['Cy'], '2026-04-11') }),
  shot(3, { credit: credit(['Ann', 'Bo'], [], '2026-03-02'), excluded: true }),
]);

describe('tmlSectionCredit', () => {
  it('aggregates the importable shots: counts and the date range', () => {
    const result = tmlSectionCredit(credited, false);
    expect(result.surveyors.map((p) => [p.person.name, p.count, p.total])).toEqual([
      ['Ann', 2, 2],
      ['Bo', 1, 2],
    ]);
    expect(result.explorers.map((p) => [p.person.name, p.count])).toEqual([['Cy', 2]]);
    expect(result.date).toBe('2026-03-02 - 2026-04-11');
  });

  it('counts the excluded shots too when they are imported', () => {
    const result = tmlSectionCredit(credited, true);
    expect(result.surveyors.map((p) => [p.person.name, p.count, p.total])).toEqual([
      ['Ann', 3, 3],
      ['Bo', 2, 3],
    ]);
  });

  it('falls back to the legacy text, split into roles, when no shot is credited', () => {
    const legacy = section('Old', [shot(1), shot(2)], { surveyor: 'Eve / Fay; Gus', date: '2020-01-01' });
    const result = tmlSectionCredit(legacy, false);
    expect(result.explorers.map((p) => p.person.name)).toEqual(['Eve']);
    expect(result.surveyors.map((p) => [p.person.name, p.count, p.total])).toEqual([
      ['Fay', 2, 2],
      ['Gus', 2, 2],
    ]);
    expect(result.date).toBe('2020-01-01');
  });
});

describe('TmlSectionTable people columns', () => {
  const tmlFile = { sections: [credited, section('Empty', [shot(1)])] } as unknown as TmlFile;

  it('renders read-only pills, partial for a person on only some shots', () => {
    render(
      <TmlSectionTable
        tmlFile={tmlFile}
        includeExcluded={false}
        selected={new Set([0, 1])}
        onToggle={vi.fn()}
        onToggleAll={vi.fn()}
        reanchored={new Set()}
        disabled={false}
      />,
    );
    expect(screen.getByTestId('tml-section-0-surveyors-pill-0-label')).toHaveTextContent('Ann');
    expect(screen.getByTestId('tml-section-0-surveyors-pill-0')).not.toHaveAttribute('data-partial');
    expect(screen.getByTestId('tml-section-0-surveyors-pill-1')).toHaveAttribute('data-partial', 'true');
    expect(screen.getByTestId('tml-section-0-surveyors-pill-1-count')).toHaveTextContent('1/2');
    expect(screen.getByTestId('tml-section-0-explorers-pill-0-label')).toHaveTextContent('Cy');
    // Read-only: no remove buttons, no input.
    expect(screen.queryByTestId('tml-section-0-surveyors-pill-0-remove')).not.toBeInTheDocument();
    expect(screen.queryByTestId('tml-section-0-surveyors-input')).not.toBeInTheDocument();
    expect(screen.getByTestId('tml-section-0-date')).toHaveTextContent('2026-03-02 - 2026-04-11');
    // A section nobody is credited on says so.
    expect(screen.getByTestId('tml-section-1-surveyors-field')).toHaveTextContent('common.states.none');
    expect(screen.getByTestId('tml-section-1-date')).toHaveTextContent('dialogs.tmlImport.stats.none');
  });
});
