/**
 * The Detail panel's survey credit: one shot edits that shot, several selected
 * shots edit all of them at once (union, partial pills, promote), a section
 * edits every shot of it - each change ONE editShotCredit - and read-only
 * modes show the pills without any way to change them.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { act, render, fireEvent, screen } from '@testing-library/react';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => []) }));

import '../../../i18n';
import DetailPanel from '../DetailPanel';
import { useUIStore } from '../../../store/uiStore';
import { useProjectStore } from '../../../store/projectStore';
import { useSpeleoStore } from '../../../store/speleoStore';
import type { Project, Section, Shot } from '../../../types/project';
import type { CommandResult, CreditEdit, CreditTargets, SurveyCredit } from '../../../types/ipc.generated';

const tid = (id: string) => screen.getByTestId(id);
const qid = (id: string) => screen.queryByTestId(id);

function shot(id: number, credit?: SurveyCredit): Shot {
  return {
    id,
    azimuth_in: 90,
    azimuth_out: 90,
    length: 5,
    depth_in: 1,
    depth_out: 2,
    pitch_in: null,
    pitch_out: null,
    left: null,
    right: null,
    up: null,
    down: null,
    comment: null,
    kind: 'Normal',
    marker_index: null,
    temperature: null,
    ...(credit ? { credit } : {}),
  } as Shot;
}

const ann = { name: 'Ann Diver' };
const bo = { name: 'Bo Lead', email: 'bo@caves.example' };

function project(): Project {
  const section = {
    id: 1,
    name: 'Main',
    color: '#336699',
    direction: 'in',
    quality: 1,
    next_shot_id: 104,
    shots: [
      shot(101, { surveyors: [ann], explorers: [], date: '2026-03-02' }),
      shot(102, { surveyors: [ann, bo], explorers: [], date: '2026-03-02' }),
      shot(103, { surveyors: [], explorers: [bo], date: '2026-04-11' }),
    ],
  } as unknown as Section;
  return { sections: [section], station_merges: [], zero_datums: [], loop_closures: [] } as unknown as Project;
}

const editShotCredit = vi.fn(async (_targets: CreditTargets, _edit: CreditEdit) => ({ success: true, warnings: [], error: null, section_ids: [1] }) as CommandResult);

function pillByLabel(field: 'surveyors' | 'explorers', label: string): HTMLElement {
  const pills = [...document.querySelectorAll<HTMLElement>(`[data-testid^="detail-credit-${field}-pill-"][data-testid$="-label"]`)];
  const hit = pills.find((el) => el.textContent === label);
  if (!hit) throw new Error(`no ${field} pill labelled ${label}`);
  return hit.closest('li') as HTMLElement;
}

function typePerson(field: 'surveyors' | 'explorers', name: string) {
  const input = tid(`detail-credit-${field}-input`) as HTMLInputElement;
  fireEvent.change(input, { target: { value: name } });
  fireEvent.keyDown(input, { key: 'Enter' });
}

beforeEach(() => {
  editShotCredit.mockClear();
  useProjectStore.setState({ project: project(), stations: [], warnings: [], stats: null, config: null, editShotCredit });
  useUIStore.setState({ detailPanelTab: 'details', appMode: 'survey', geometryMode: 'measured', selection: null, multiSelection: [] });
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
});

describe('Detail panel survey credit - one shot', () => {
  beforeEach(() => {
    useUIStore.getState().setSelection({ type: 'shot', sectionId: 1, shotId: 102 });
  });

  it('shows the shot people and date', () => {
    render(<DetailPanel />);
    expect(tid('detail-credit').dataset.scope).toBe('shot');
    expect(tid('detail-credit-people')).toContainElement(tid('detail-credit-surveyors'));
    expect(tid('detail-credit-surveyors-pill-0-label')).toHaveTextContent('Ann Diver');
    expect(tid('detail-credit-surveyors-pill-1-label')).toHaveTextContent('Bo Lead');
    expect(qid('detail-credit-surveyors-pill-0')?.dataset.partial).toBeUndefined();
    expect((tid('detail-credit-date') as HTMLInputElement).value).toBe('2026-03-02');
  });

  it('adds a surveyor to that shot only, as one edit, and shows it at once', () => {
    render(<DetailPanel />);
    typePerson('explorers', 'Cy Cave');
    expect(editShotCredit).toHaveBeenCalledTimes(1);
    expect(editShotCredit).toHaveBeenCalledWith({ type: 'shots', shots: [[1, 102]] }, { add_explorers: [{ name: 'Cy Cave' }] });
    expect(tid('detail-credit-explorers-pill-0-label')).toHaveTextContent('Cy Cave');
  });

  it('removes a person with the x', () => {
    render(<DetailPanel />);
    fireEvent.click(tid('detail-credit-surveyors-pill-1-remove'));
    expect(editShotCredit).toHaveBeenCalledWith(
      { type: 'shots', shots: [[1, 102]] },
      { remove_surveyors: [{ name: 'Bo Lead', email: 'bo@caves.example' }] },
    );
  });

  // A date input reports a value whenever its typed segments form a valid
  // date, so typing 2026-04-11 over 2026-03-02 passes through 2026-04-01.
  // Only the final date may become an undo step.
  it('commits the date once, on Enter, not at every intermediate parse', () => {
    render(<DetailPanel />);
    const date = tid('detail-credit-date') as HTMLInputElement;
    fireEvent.change(date, { target: { value: '2026-04-02' } });
    fireEvent.change(date, { target: { value: '2026-04-01' } });
    fireEvent.change(date, { target: { value: '2026-04-11' } });
    expect(editShotCredit).not.toHaveBeenCalled();
    fireEvent.keyDown(date, { key: 'Enter' });
    expect(editShotCredit).toHaveBeenCalledTimes(1);
    expect(editShotCredit).toHaveBeenCalledWith({ type: 'shots', shots: [[1, 102]] }, { set_date: '2026-04-11' });
  });

  it('commits the date once when the field is left', () => {
    render(<DetailPanel />);
    const date = tid('detail-credit-date') as HTMLInputElement;
    fireEvent.change(date, { target: { value: '2026-04-01' } });
    fireEvent.change(date, { target: { value: '2026-04-11' } });
    fireEvent.blur(date);
    fireEvent.blur(date);
    expect(editShotCredit).toHaveBeenCalledTimes(1);
    expect(editShotCredit).toHaveBeenCalledWith({ type: 'shots', shots: [[1, 102]] }, { set_date: '2026-04-11' });
  });

  it('Escape puts the committed date back without writing anything', () => {
    render(<DetailPanel />);
    const date = tid('detail-credit-date') as HTMLInputElement;
    fireEvent.change(date, { target: { value: '2026-04-11' } });
    fireEvent.keyDown(date, { key: 'Escape' });
    expect(date.value).toBe('2026-03-02');
    fireEvent.blur(date);
    expect(editShotCredit).not.toHaveBeenCalled();
  });

  it('never turns a half-typed date into "no date"', () => {
    render(<DetailPanel />);
    const date = tid('detail-credit-date') as HTMLInputElement;
    Object.defineProperty(date, 'validity', { value: { badInput: true }, configurable: true });
    fireEvent.change(date, { target: { value: '' } });
    fireEvent.blur(date);
    expect(editShotCredit).not.toHaveBeenCalled();
    expect(date.value).toBe('2026-03-02');
  });

  it('is read-only in the closure-adjusted view: no x, no input', () => {
    useUIStore.setState({ geometryMode: 'closureAdjusted' });
    render(<DetailPanel />);
    expect(tid('detail-credit-surveyors-pill-0-label')).toHaveTextContent('Ann Diver');
    expect(qid('detail-credit-surveyors-pill-0-remove')).toBeNull();
    expect(qid('detail-credit-surveyors-input')).toBeNull();
  });
});

describe('Detail panel survey credit - several shots', () => {
  beforeEach(() => {
    const ui = useUIStore.getState();
    ui.setSelection({ type: 'shot', sectionId: 1, shotId: 101 });
    ui.addToSelection({ type: 'shot', sectionId: 1, shotId: 102 });
    ui.addToSelection({ type: 'shot', sectionId: 1, shotId: 103 });
  });

  it('shows the union with partial pills and the date range', () => {
    render(<DetailPanel />);
    expect(tid('multi-shot-detail')).toBeInTheDocument();
    expect(tid('detail-credit').dataset.scope).toBe('shots');
    // Ann is on 2 of the 3 shots, Bo surveyed 1 of 3.
    expect(tid('detail-credit-surveyors-pill-0').dataset.partial).toBe('true');
    expect(tid('detail-credit-surveyors-pill-0-count')).toHaveTextContent('2/3');
    expect(tid('detail-credit-surveyors-pill-1-count')).toHaveTextContent('1/3');
    expect(tid('detail-credit-date-mixed')).toHaveTextContent('2026-03-02 - 2026-04-11, differs per shot');
  });

  it('adds a person to every selected shot in one edit', () => {
    render(<DetailPanel />);
    typePerson('surveyors', 'Cy Cave');
    expect(editShotCredit).toHaveBeenCalledTimes(1);
    expect(editShotCredit).toHaveBeenCalledWith(
      { type: 'shots', shots: [[1, 101], [1, 102], [1, 103]] },
      { add_surveyors: [{ name: 'Cy Cave' }] },
    );
    // On every shot now, so a full pill (the union keeps first-appearance order).
    const cy = pillByLabel('surveyors', 'Cy Cave');
    expect(cy.dataset.partial).toBeUndefined();
  });

  it('puts a person just added at the end, and keeps them there after the refresh', () => {
    render(<DetailPanel />);
    typePerson('surveyors', 'Cy Cave');
    const labels = () => screen.getAllByTestId(/^detail-credit-surveyors-pill-\d+-label$/).map((el) => el.textContent);
    expect(labels()).toEqual(['Ann Diver', 'Bo Lead', 'Cy Cave']);
    // The store refreshes with Cy on every shot: by first appearance Cy would
    // now come second.
    act(() => {
      const project = useProjectStore.getState().project!;
      const section = project.sections[0]!;
      useProjectStore.setState({
        project: {
          ...project,
          sections: [
            {
              ...section,
              shots: section.shots.map((s) => ({
                ...s,
                credit: { ...s.credit!, surveyors: [...s.credit!.surveyors, { name: 'Cy Cave' }] },
              })),
            },
          ],
        },
      });
    });
    expect(labels()).toEqual(['Ann Diver', 'Bo Lead', 'Cy Cave']);
  });

  it('promotes a partial pill to every selected shot', () => {
    render(<DetailPanel />);
    fireEvent.click(tid('detail-credit-surveyors-pill-0-promote'));
    expect(editShotCredit).toHaveBeenCalledWith(
      { type: 'shots', shots: [[1, 101], [1, 102], [1, 103]] },
      { add_surveyors: [{ name: 'Ann Diver' }] },
    );
    expect(tid('detail-credit-surveyors-pill-0').dataset.partial).toBeUndefined();
  });

  it('sets one date on every selected shot only after "Set for all shots"', () => {
    render(<DetailPanel />);
    expect(qid('detail-credit-date')).toBeNull();
    fireEvent.click(tid('detail-credit-date-set-all'));
    fireEvent.change(tid('detail-credit-date'), { target: { value: '2026-05-01' } });
    expect(editShotCredit).not.toHaveBeenCalled();
    fireEvent.keyDown(tid('detail-credit-date'), { key: 'Enter' });
    expect(editShotCredit).toHaveBeenCalledTimes(1);
    expect(editShotCredit).toHaveBeenLastCalledWith(
      { type: 'shots', shots: [[1, 101], [1, 102], [1, 103]] },
      { set_date: '2026-05-01' },
    );
  });
});

describe('Detail panel survey credit - a section', () => {
  it('edits every shot of the section through the section target', () => {
    useUIStore.getState().setSelection({ type: 'section', sectionId: 1 });
    render(<DetailPanel />);
    expect(tid('detail-credit').dataset.scope).toBe('section');
    fireEvent.click(tid('detail-credit-explorers-pill-0-remove'));
    expect(editShotCredit).toHaveBeenCalledWith(
      { type: 'section', section_id: 1 },
      { remove_explorers: [{ name: 'Bo Lead', email: 'bo@caves.example' }] },
    );
  });
});
