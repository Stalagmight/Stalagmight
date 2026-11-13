/**
 * Combine Sections: the dialog's defaults, the swap, the choices it offers
 * only on a conflict, and what it sends.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';

const closeDialog = vi.fn();
const setSelection = vi.fn();
const setHoveredStation = vi.fn();
const setHoveredSectionId = vi.fn();
const combineSections = vi.fn();

let dialogData: Record<string, unknown> = {};
const uiState = () => ({ closeDialog, setSelection, setHoveredStation, setHoveredSectionId, dialogData });
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

import '../../../i18n';
import CombineSectionsDialog from '../CombineSectionsDialog';

/** A plan that changes nothing besides the join. */
/** A torn-tie row as text, each link pill written as [its accessible name]. */
function tieText(li: HTMLElement): string {
  const copy = li.cloneNode(true) as HTMLElement;
  copy.querySelectorAll('button').forEach((b) => {
    b.replaceWith(` [${(b.getAttribute('aria-label') ?? '').replace(/^Select /, '')}] `);
  });
  return (copy.textContent ?? '').replace(/\s+/g, ' ').trim();
}

function quietPlan(kept: number, overrides: Record<string, unknown> = {}) {
  return {
    kept,
    moving_section: null,
    moved_sections: [],
    shift_m: 0,
    torn_ties: [],
    azimuth_rotation_deg: null,
    join_offset_change_m: null,
    ...overrides,
  };
}

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
    depth_difference_m: null,
    branches: [],
    loose_ends: [],
    join_kind: 'co_located',
    passing_through: [],
    repeated_shot_ids: 0,
    plan_keeping_a: quietPlan(2),
    plan_keeping_b: quietPlan(5),
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
      join: { kept: 'end', other: 'start' },
    });
    expect(setSelection).toHaveBeenCalledWith({ type: 'section', sectionId: 2 });
    expect(screen.queryByTestId('combine-plan-notes')).toBeNull();
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

  it('counts one shot as "1 shot" everywhere', () => {
    dialogData = { candidate: candidate({ shot_count_b: 1 }) };
    sections = [
      creditedSections()[0]!,
      { ...creditedSections()[1]!, shots: [{ id: 51, length: 5, credit: credited(['Bob'], '2026-04-11', ['Cid']) }] },
    ];
    render(<CombineSectionsDialog />);
    expect(screen.getByTestId('combine-summary')).toHaveTextContent(
      'Combining Main-A (3 shots) and Main-B (1 shot) at station 2:3'
    );
    expect(screen.getByTestId('combine-credit-surveyors')).toHaveTextContent('Ann (3 shots), Bob (2 shots)');
    expect(screen.getByTestId('combine-credit-explorers')).toHaveTextContent('Explorers: Cid (1 shot)');
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

  it('says how far the join moves in depth when the two ends disagree, and only then', () => {
    dialogData = { candidate: candidate({ depth_difference_m: 0.3 }) };
    const { unmount } = render(<CombineSectionsDialog />);
    expect(screen.getByTestId('combine-depth-note')).toHaveTextContent(
      'The two ends are recorded 0.30m apart in depth. The joined station takes the average, so each end moves 0.15m in depth.'
    );
    expect(screen.getByTestId('combine-confirm')).toBeEnabled();
    unmount();

    dialogData = { candidate: candidate() };
    render(<CombineSectionsDialog />);
    expect(screen.queryByTestId('combine-depth-note')).toBeNull();
  });

  it('says a third section ending at the join unmerged is left as it is', () => {
    sections = [...creditedSections(), { id: 7, name: 'Side', color: '#333333', direction: 'in', shots: [] }];
    dialogData = { candidate: candidate({ loose_ends: [7] }) };
    render(<CombineSectionsDialog />);
    expect(screen.queryByTestId('combine-partner-picker')).toBeNull();
    expect(screen.getByTestId('combine-loose-ends-note')).toHaveTextContent(
      'Side also ends here but is not merged to the join, so it stays as it is.'
    );
  });

  it('refuses to combine in a read-only view', () => {
    blocked = true;
    render(<CombineSectionsDialog />);
    fireEvent.click(screen.getByTestId('combine-confirm'));
    expect(combineSections).not.toHaveBeenCalled();
    expect(screen.getByTestId('combine-error')).toHaveTextContent('Editing is disabled');
  });

  it('across a loop closure, says what moves and which ties become closures, per kept section', async () => {
    const tie = (a: [number, number], b: [number, number], kind: string, reason = 'loop') => ({
      station_a: { section_id: a[0], station_index: a[1] },
      station_b: { section_id: b[0], station_index: b[1] },
      kind,
      reason,
    });
    dialogData = {
      candidate: candidate({
        join_kind: 'closure',
        plan_keeping_a: quietPlan(2, {
          moving_section: 5,
          moved_sections: [5, 7],
          shift_m: 0.42,
          torn_ties: [tie([5, 2], [9, 1], 'merge')],
        }),
        // Keeping Main-B: a zero datum holds Main-A, so the kept side moves.
        plan_keeping_b: quietPlan(5, {
          moving_section: 5,
          moved_sections: [5],
          shift_m: 0.42,
          torn_ties: [tie([2, 0], [9, 0], 'snapped_closure'), tie([5, 2], [8, 0], 'merge', 'zero_datum')],
        }),
      }),
    };
    sections = [
      ...creditedSections(),
      { id: 7, name: 'Side', color: '#333333', direction: 'in', shots: shots(1) },
    ];
    render(<CombineSectionsDialog />);
    expect(screen.getByTestId('combine-closure-join')).toHaveTextContent(
      'Main-A and Main-B meet here across a loop closure: the survey puts the two ends 0.42m apart horizontally.'
    );
    expect(screen.getByTestId('combine-note-move')).toHaveTextContent(
      'Main-B moves 0.42m horizontally so the two ends meet, closing the loop here.'
    );
    expect(screen.getByTestId('combine-note-move-with')).toHaveTextContent('Side is attached to it and moves with it.');
    expect(screen.getByTestId('combine-note-torn')).toHaveTextContent('This connection becomes a loop closure:');
    // Each end of a torn tie is a section pill and a station pill, the link
    // pills of the Details panel, read here by their accessible names.
    expect(screen.getAllByTestId(/^combine-torn-tie-/).map(tieText)).toEqual([
      'The merge of [section Main-B] [station 5:2] and [section Section 9] [station 9:1]',
    ]);
    expect(screen.queryByTestId('combine-note-kept-moves')).toBeNull();

    fireEvent.click(screen.getByTestId('combine-swap-kept'));
    expect(screen.getByTestId('combine-note-move')).toHaveTextContent('Main-B moves 0.42m');
    expect(screen.getByTestId('combine-note-kept-moves')).toHaveTextContent(
      'Main-B is the one that moves, because a zero datum holds Main-A in place.'
    );
    expect(screen.getAllByTestId(/^combine-torn-tie-/).map(tieText)).toEqual([
      'The snapped closure between [section Main-A] [station 2:0] and [section Section 9] [station 9:0]',
      'The merge of [section Main-B] [station 5:2] and [section Section 8] [station 8:0] [section Section 8] is held by a zero datum and stays where it is.',
    ]);
    // Hovering a station pill rings it on the map; clicking selects it.
    fireEvent.mouseEnter(screen.getAllByTestId('combine-tie-station-5-2')[0]!);
    expect(setHoveredStation).toHaveBeenCalledWith({ section_id: 5, station_index: 2 });
    fireEvent.click(screen.getAllByTestId('combine-tie-station-5-2')[0]!);
    expect(setSelection).toHaveBeenCalledWith({ type: 'station', stationRef: { section_id: 5, station_index: 2 } });
    setSelection.mockClear();
    expect(screen.getByTestId('combine-torn-why')).toHaveTextContent("The loop's misclosure moves to the new closure");
    fireEvent.click(screen.getByTestId('combine-confirm'));
    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
    expect(combineSections.mock.calls[0]?.[2]).toMatchObject({ join: { kept: 'start', other: 'end' } });
  });

  it('lists converted bearings, the kept quality and offset, renewed shot ids and crossings', () => {
    sections = creditedSections().map((s) => ({ ...s, quality: s.id === 2 ? 1 : 2 }));
    dialogData = {
      candidate: candidate({
        passing_through: [5],
        repeated_shot_ids: 2,
        plan_keeping_a: quietPlan(2, { azimuth_rotation_deg: 5, join_offset_change_m: 0.25 }),
      }),
    };
    render(<CombineSectionsDialog />);
    expect(screen.getByTestId('combine-note-rotation')).toHaveTextContent(
      "Main-B's bearings turn by +5.0° to read against Main-A's north reference"
    );
    expect(screen.getByTestId('combine-note-quality')).toHaveTextContent(
      "The combined section keeps Main-A's quality factor (1); Main-B's was 2."
    );
    expect(screen.getByTestId('combine-note-offset')).toHaveTextContent(
      "The joined station keeps Main-A's, so Main-B's end moves 0.25m."
    );
    expect(screen.getByTestId('combine-note-repeated-ids')).toHaveTextContent(
      "2 of Main-B's shots have the same identity as shots of Main-A, and get new ones."
    );
    expect(screen.getByTestId('combine-note-passing')).toHaveTextContent('Main-B passes through the join');
  });

  it('says the bearings keep their values only when the backend leaves them alone', () => {
    dialogData = { candidate: candidate({ plan_keeping_a: quietPlan(2, { azimuth_rotation_deg: 0 }) }) };
    const { unmount } = render(<CombineSectionsDialog />);
    expect(screen.getByTestId('combine-note-rotation')).toHaveTextContent('keep their values');
    unmount();
    dialogData = { candidate: candidate({ plan_keeping_a: quietPlan(2, { azimuth_rotation_deg: 0.04 }) }) };
    render(<CombineSectionsDialog />);
    expect(screen.getByTestId('combine-note-rotation')).toHaveTextContent('turn by +0.040°');
  });
});
