/**
 * Combine Sections: the dialog's defaults, the swap, the choices it offers
 * only on a conflict, and what it sends.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';

const closeDialog = vi.fn();
const setSelection = vi.fn();
const combineSections = vi.fn();

let dialogData: Record<string, unknown> = {};
const uiState = () => ({ closeDialog, setSelection, dialogData });
vi.mock('../../../store/uiStore', () => ({
  useUIStore: Object.assign(() => uiState(), { getState: () => uiState() }),
}));

type MockShot = { id: number; length: number; credit?: Record<string, unknown> };
const shots = (n: number): MockShot[] => Array.from({ length: n }, (_, i) => ({ id: i, length: 5 }));
const credited = (surveyors: string[], date: string, explorers: string[] = []) => ({
  surveyors: surveyors.map((name) => ({ name })),
  explorers: explorers.map((name) => ({ name })),
  date,
});
/** Main-A: Ann on all three shots, Bob on one; Main-B: Ann and Bob, one explorer. */
const creditedSections = () => [
  {
    id: 2,
    name: 'Main-A',
    color: '#111111',
    direction: 'in',
    shots: [
      { id: 21, length: 5, credit: credited(['Ann'], '2026-03-02') },
      { id: 22, length: 5, credit: credited(['Ann', 'Bob'], '2026-03-02') },
      { id: 23, length: 5, credit: credited(['Ann'], '2026-03-03') },
    ],
  },
  {
    id: 5,
    name: 'Main-B',
    color: '#222222',
    direction: 'out',
    shots: [
      { id: 51, length: 5, credit: credited(['Bob'], '2026-04-11', ['Cid']) },
      { id: 52, length: 5, credit: credited(['Ann'], '2026-04-10') },
    ],
  },
];
let sections: Array<{ id: number; name: string; color: string; direction: string; shots: MockShot[] }> = [];
vi.mock('../../../store/projectStore', () => ({
  useProjectStore: () => ({
    combineSections,
    stations: [],
    project: { sections },
  }),
}));

let blocked = false;
vi.mock('../../../hooks/useEditingBlocked', () => ({
  isEditingBlockedNow: () => blocked,
}));

vi.mock('../../../utils/telemetry', () => ({ trackDialogComplete: vi.fn() }));

vi.mock('../DialogWrapper', () => ({
  DialogWrapper: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
}));

import CombineSectionsDialog from '../CombineSectionsDialog';

function candidate(overrides: Record<string, unknown> = {}) {
  return {
    section_a: 2,
    section_b: 5,
    end_a: 'end',
    end_b: 'start',
    station_a: { section_id: 2, station_index: 3 },
    station_b: { section_id: 5, station_index: 0 },
    merged: false,
    reverses: false,
    blocked_reason: null,
    blocked_keeping_a: null,
    blocked_keeping_b: null,
    shot_count_a: 3,
    shot_count_b: 2,
    comment_a: null,
    comment_b: null,
    station_name_a: null,
    station_name_b: null,
    ...overrides,
  };
}

describe('CombineSectionsDialog', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    blocked = false;
    combineSections.mockResolvedValue({ success: true });
    sections = creditedSections();
    dialogData = { candidate: candidate() };
  });

  it('summarises the join and defaults the name and colour from the kept section', () => {
    render(<CombineSectionsDialog />);
    expect(screen.getByTestId('combine-summary')).toHaveTextContent(
      'Combining Main-A (3 shots) and Main-B (2 shots) at station 2:3'
    );
    expect(screen.getByTestId('combine-name')).toHaveValue('Main');
    expect(screen.getByTestId('combine-color')).toHaveValue('#111111');
    expect(screen.getByTestId('combine-direction-note')).toHaveTextContent("keeps Main-A's survey direction (In)");
    expect(screen.getByTestId('combine-plan')).toHaveTextContent('Main-B will be added after Main-A.');
    expect(screen.queryByTestId('combine-reverse-notice')).toBeNull();
    expect(screen.queryByTestId('combine-comment-pick')).toBeNull();
    expect(screen.queryByTestId('combine-name-pick')).toBeNull();
  });

  it('sends the combine and selects the result', async () => {
    render(<CombineSectionsDialog />);
    fireEvent.click(screen.getByTestId('combine-confirm'));
    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
    expect(combineSections).toHaveBeenCalledWith(2, 5, {
      name: 'Main',
      color: '#111111',
      swap_lr: true,
      comment_pick: 'kept',
      name_pick: 'kept',
    });
    expect(setSelection).toHaveBeenCalledWith({ type: 'section', sectionId: 2 });
  });

  it('swaps the kept section, and the untouched defaults follow it', async () => {
    render(<CombineSectionsDialog />);
    fireEvent.click(screen.getByTestId('combine-swap-kept'));
    expect(screen.getByTestId('combine-summary')).toHaveTextContent('Combining Main-B (2 shots) and Main-A (3 shots) at station 5:0');
    expect(screen.getByTestId('combine-color')).toHaveValue('#222222');
    expect(screen.getByTestId('combine-plan')).toHaveTextContent('Main-A will be added before Main-B.');
    expect(screen.getByTestId('combine-direction-note')).toHaveTextContent('(Out)');
    fireEvent.click(screen.getByTestId('combine-confirm'));
    await waitFor(() => expect(combineSections).toHaveBeenCalled());
    expect(combineSections.mock.calls[0]?.slice(0, 2)).toEqual([5, 2]);
  });

  it('says the other section is reversed and offers the wall swap', async () => {
    dialogData = { candidate: candidate({ reverses: true, end_b: 'end' }) };
    render(<CombineSectionsDialog />);
    expect(screen.getByTestId('combine-plan')).toHaveTextContent('Main-B will be reversed and added after Main-A.');
    expect(screen.getByTestId('combine-reverse-notice')).toHaveTextContent('Reversing Main-B');
    fireEvent.click(screen.getByTestId('combine-swap-lr'));
    fireEvent.click(screen.getByTestId('combine-confirm'));
    await waitFor(() => expect(combineSections).toHaveBeenCalled());
    expect(combineSections.mock.calls[0]?.[2]).toMatchObject({ swap_lr: false });
  });

  it('asks about the join comment and name only when both stations have different ones', async () => {
    dialogData = {
      candidate: candidate({ comment_a: 'restriction', comment_b: 'sump', station_name_a: 'S1', station_name_b: 'B0' }),
    };
    render(<CombineSectionsDialog />);
    expect(screen.getByTestId('combine-comment-pick-kept').parentElement).toHaveTextContent('restriction');
    expect(screen.getByTestId('combine-comment-pick-other').parentElement).toHaveTextContent('sump');
    expect(screen.queryByTestId('combine-name-pick-both')).toBeNull();
    fireEvent.click(screen.getByTestId('combine-comment-pick-both'));
    fireEvent.click(screen.getByTestId('combine-name-pick-other'));
    fireEvent.click(screen.getByTestId('combine-confirm'));
    await waitFor(() => expect(combineSections).toHaveBeenCalled());
    expect(combineSections.mock.calls[0]?.[2]).toMatchObject({ comment_pick: 'both', name_pick: 'other' });
  });

  it('leaves an unnamed kept section unnamed instead of naming it "Section N"', async () => {
    sections = creditedSections().map((s) => (s.id === 2 ? { ...s, name: null as unknown as string } : s));
    render(<CombineSectionsDialog />);
    const field = screen.getByTestId('combine-name');
    expect(field).toHaveValue('');
    expect(field).toHaveAttribute('placeholder', expect.stringContaining('Section 2'));
    expect(screen.getByTestId('combine-confirm')).toBeEnabled();
    fireEvent.click(screen.getByTestId('combine-confirm'));
    await waitFor(() => expect(combineSections).toHaveBeenCalled());
    expect(combineSections.mock.calls[0]?.[2]).toMatchObject({ name: '' });
  });

  it('summarises the combined credit from every shot of both sections', () => {
    render(<CombineSectionsDialog />);
    expect(screen.getByTestId('combine-credit')).toHaveTextContent('(5 shots)');
    expect(screen.getByTestId('combine-credit-surveyors')).toHaveTextContent('Surveyors: Ann (4 shots), Bob (2 shots)');
    expect(screen.getByTestId('combine-credit-explorers')).toHaveTextContent('Explorers: Cid (1 shot)');
    expect(screen.getByTestId('combine-credit-dates')).toHaveTextContent('Surveyed: 2026-03-02 - 2026-04-11');
    expect(screen.getByTestId('combine-credit')).toHaveTextContent('Every shot keeps its own surveyors, explorers and date.');
  });

  it('says when neither section records any credit', () => {
    sections = [
      { id: 2, name: 'Main-A', color: '#111111', direction: 'in', shots: shots(3) },
      { id: 5, name: 'Main-B', color: '#222222', direction: 'out', shots: shots(2) },
    ];
    render(<CombineSectionsDialog />);
    expect(screen.getByTestId('combine-credit-none')).toBeInTheDocument();
  });

  it('starts on the section that can be kept when the other one has no shot ids left', async () => {
    dialogData = { candidate: candidate({ blocked_keeping_a: 'Main-A has used up its shot numbers' }) };
    render(<CombineSectionsDialog />);
    expect(screen.getByTestId('combine-kept-label')).toHaveTextContent('Keep Main-B');
    expect(screen.getByTestId('combine-confirm')).toBeEnabled();
    fireEvent.click(screen.getByTestId('combine-swap-kept'));
    expect(screen.getByTestId('combine-blocked')).toHaveTextContent('used up its shot numbers');
    expect(screen.getByTestId('combine-confirm')).toBeDisabled();
  });

  it('shows why a blocked combine cannot run and never sends it', () => {
    dialogData = { candidate: candidate({ blocked_reason: 'This is a branch' }) };
    render(<CombineSectionsDialog />);
    expect(screen.getByTestId('combine-blocked')).toHaveTextContent('This is a branch');
    expect(screen.getByTestId('combine-confirm')).toBeDisabled();
  });

  it('refuses to combine in a read-only view', () => {
    blocked = true;
    render(<CombineSectionsDialog />);
    fireEvent.click(screen.getByTestId('combine-confirm'));
    expect(combineSections).not.toHaveBeenCalled();
    expect(screen.getByTestId('combine-error')).toHaveTextContent('Editing is disabled');
  });
});
