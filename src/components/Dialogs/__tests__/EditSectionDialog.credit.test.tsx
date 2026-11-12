/**
 * Edit Section's survey credit: the union of the shots' people (partial pills
 * for someone on only some shots), the date or its per-shot range, a pending
 * edit that Save sends as ONE editShotCredit on the section - never per
 * keystroke - and dirty tracking that counts a name still being typed.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen } from '@testing-library/react';

vi.mock('@tanstack/react-virtual', () => ({
  useVirtualizer: ({ count }: { count: number }) => ({
    getVirtualItems: () =>
      Array.from({ length: count }, (_, index) => ({ index, key: index, start: 33 + index * 33, size: 33 })),
    getTotalSize: () => count * 33,
    scrollToIndex: vi.fn(),
    measureElement: vi.fn(),
  }),
}));
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => []) }));

import '../../../i18n';
import EditSectionDialog from '../EditSectionDialog';
import { useUIStore } from '../../../store/uiStore';
import { useProjectStore } from '../../../store/projectStore';
import { useSpeleoStore } from '../../../store/speleoStore';
import type { CommandResult, CreditEdit, CreditTargets, SurveyCredit } from '../../../types/ipc.generated';
import type { Project, Section, Shot, SectionUpdates } from '../../../types/project';

class NoopResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}

const ok = { success: true, warnings: [], error: null, section_ids: [1] } as CommandResult;
const editShotCredit = vi.fn(async (_t: CreditTargets, _e: CreditEdit) => ok);
const updateSection = vi.fn(async (_id: number, _u: SectionUpdates) => ok);

function shot(id: number, credit: SurveyCredit): Shot {
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
    credit,
  } as Shot;
}

const ann = { name: 'Ann Diver' };
const bo = { name: 'Bo Lead' };

function seed(dates: Array<string | null>) {
  const section = {
    id: 1,
    name: 'Main',
    color: '#336699',
    direction: 'in',
    quality: 1,
    next_shot_id: 104,
    shots: [
      shot(101, { surveyors: [ann, bo], explorers: [], date: dates[0] ?? null }),
      shot(102, { surveyors: [ann], explorers: [bo], date: dates[1] ?? null }),
      shot(103, { surveyors: [ann], explorers: [], date: dates[2] ?? null }),
    ],
  } as unknown as Section;
  useProjectStore.setState({
    project: { sections: [section], station_merges: [], zero_datums: [], loop_closures: [] } as unknown as Project,
    config: null,
    editShotCredit,
    updateSection,
    isShotFrozen: () => false,
  });
}

const tid = (id: string) => screen.getByTestId(id);
const qid = (id: string) => screen.queryByTestId(id);

function typePerson(field: 'surveyors' | 'explorers', name: string, enter = true) {
  const input = tid(`section-${field}-input`) as HTMLInputElement;
  fireEvent.change(input, { target: { value: name } });
  if (enter) fireEvent.keyDown(input, { key: 'Enter' });
}

beforeEach(() => {
  vi.stubGlobal('ResizeObserver', NoopResizeObserver);
  editShotCredit.mockClear();
  updateSection.mockClear();
  useUIStore.setState({ dialogData: { sectionId: 1 }, appMode: 'survey', geometryMode: 'measured', closeDialog: vi.fn() });
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
});

describe('Edit Section survey credit', () => {
  it('shows the union of the shots, with partial pills', () => {
    seed(['2026-03-02', '2026-03-02', '2026-03-02']);
    render(<EditSectionDialog />);
    expect(tid('section-surveyors-pill-0-label')).toHaveTextContent('Ann Diver');
    expect(tid('section-surveyors-pill-0').dataset.partial).toBeUndefined();
    expect(tid('section-surveyors-pill-1').dataset.partial).toBe('true');
    expect(tid('section-surveyors-pill-1-count')).toHaveTextContent('1/3');
    expect(tid('section-explorers-pill-0-count')).toHaveTextContent('1/3');
    expect((tid('section-date-input') as HTMLInputElement).value).toBe('2026-03-02');
  });

  it('applies adds, removes and promotes as ONE editShotCredit on Save', async () => {
    seed([null, null, null]);
    render(<EditSectionDialog />);
    typePerson('surveyors', 'Cy Cave');
    fireEvent.click(tid('section-surveyors-pill-0-remove')); // Ann off every shot
    fireEvent.click(tid('section-explorers-pill-0-promote')); // Bo on every shot
    expect(editShotCredit).not.toHaveBeenCalled();
    await act(async () => {
      fireEvent.click(tid('edit-section-save'));
    });
    expect(updateSection).toHaveBeenCalledWith(1, expect.not.objectContaining({ surveyor: expect.anything() }));
    expect(editShotCredit).toHaveBeenCalledTimes(1);
    expect(editShotCredit).toHaveBeenCalledWith(
      { type: 'section', section_id: 1 },
      { add_surveyors: [{ name: 'Cy Cave' }], remove_surveyors: [ann], add_explorers: [bo] },
    );
  });

  it('sends no credit edit when only the name changed', async () => {
    seed([null, null, null]);
    render(<EditSectionDialog />);
    fireEvent.change(tid('section-name-input'), { target: { value: 'Renamed' } });
    await act(async () => {
      fireEvent.click(tid('edit-section-save'));
    });
    expect(updateSection).toHaveBeenCalled();
    expect(editShotCredit).not.toHaveBeenCalled();
  });

  it('commits a name still being typed when Save is pressed', async () => {
    seed([null, null, null]);
    render(<EditSectionDialog />);
    const input = tid('section-explorers-input') as HTMLInputElement;
    input.focus();
    typePerson('explorers', 'Dee Deep', false);
    await act(async () => {
      fireEvent.click(tid('edit-section-save'));
    });
    expect(editShotCredit).toHaveBeenCalledWith({ type: 'section', section_id: 1 }, { add_explorers: [{ name: 'Dee Deep' }] });
  });

  it('asks before discarding a credit change or a name still being typed', () => {
    seed([null, null, null]);
    const { unmount } = render(<EditSectionDialog />);
    typePerson('surveyors', 'Half typ', false);
    fireEvent.click(tid('edit-section-dialog-close'));
    expect(qid('dirty-dialog-discard')).not.toBeNull();
    unmount();

    render(<EditSectionDialog />);
    fireEvent.click(tid('edit-section-dialog-close'));
    expect(qid('dirty-dialog-discard')).toBeNull();
  });

  it('shows a per-shot date range read-only until "Set for all shots"', async () => {
    seed(['2026-03-02', '2026-04-11', null]);
    render(<EditSectionDialog />);
    expect(tid('section-date-input-mixed')).toHaveTextContent('2026-03-02 - 2026-04-11, 1 shot undated');
    expect(qid('section-date-input')).toBeNull();
    fireEvent.click(tid('section-date-input-set-all'));
    fireEvent.change(tid('section-date-input'), { target: { value: '2026-05-01' } });
    // Back to per-shot dates is offered while the new date is pending.
    expect(tid('section-date-input-reset')).toBeInTheDocument();
    await act(async () => {
      fireEvent.click(tid('edit-section-save'));
    });
    expect(editShotCredit).toHaveBeenCalledWith({ type: 'section', section_id: 1 }, { set_date: '2026-05-01' });
  });
});
