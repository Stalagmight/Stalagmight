/**
 * Combine Lines: the dialog's defaults, the swap, the choices it offers
 * only on a conflict, and what it sends.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';

const closeDialog = vi.fn();
const setSelection = vi.fn();
const setHoveredStation = vi.fn();
const setHoveredLineId = vi.fn();
const combineLines = vi.fn();

let dialogData: Record<string, unknown> = {};
const uiState = () => ({ closeDialog, setSelection, setHoveredStation, setHoveredLineId, dialogData });
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
const creditedLines = () => [
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
let lines: Array<{ id: number; name: string; color: string; direction: string; shots: MockShot[] }> = [];
/** Lines 8 and 9, which the ties below reach, under the names the backend seeds. */
const withSeeded = () => [
  ...lines,
  ...[8, 9]
    .filter((id) => !lines.some((l) => l.id === id))
    .map((id) => ({ id, name: `Line ${id}`, color: '#999999', direction: 'in', shots: [] })),
];
let stations: Array<{ station_ref: { line_id: number; station_index: number }; name: string }> = [];
vi.mock('../../../store/projectStore', () => ({
  useProjectStore: () => ({
    combineLines,
    stations,
    project: { lines: withSeeded() },
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
import CombineLinesDialog from '../CombineLinesDialog';

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
    moving_line: null,
    kept_moves_because: null,
    moved_lines: [],
    shift_m: 0,
    torn_ties: [],
    azimuth_rotation_deg: null,
    join_offset_change_m: null,
    ...overrides,
  };
}

function candidate(overrides: Record<string, unknown> = {}) {
  return {
    line_a: 2,
    line_b: 5,
    end_a: 'end',
    end_b: 'start',
    station_a: { line_id: 2, station_index: 3 },
    station_b: { line_id: 5, station_index: 0 },
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

describe('CombineLinesDialog', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    blocked = false;
    combineLines.mockResolvedValue({ success: true });
    lines = creditedLines();
    stations = [];
    dialogData = { candidate: candidate() };
  });

  it('summarises the join and defaults the name and colour from the kept line', () => {
    stations = [{ station_ref: { line_id: 2, station_index: 3 }, name: 'A3' }];
    render(<CombineLinesDialog />);
    // The join station by its line and name, never its position.
    expect(screen.getByTestId('combine-summary')).toHaveTextContent(
      'Combining Main-A (3 shots) and Main-B (2 shots) at station Main-A / A3'
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
    render(<CombineLinesDialog />);
    fireEvent.click(screen.getByTestId('combine-confirm'));
    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
    expect(combineLines).toHaveBeenCalledWith(2, 5, {
      name: 'Main',
      color: '#111111',
      swap_lr: true,
      comment_pick: 'kept',
      name_pick: 'kept',
      join: { kept: 'end', other: 'start' },
    });
    expect(setSelection).toHaveBeenCalledWith({ type: 'line', lineId: 2 });
    expect(screen.queryByTestId('combine-plan-notes')).toBeNull();
  });

  it('says which stations the combine renames, and names the join by its new name', () => {
    stations = [{ station_ref: { line_id: 2, station_index: 3 }, name: '3' }];
    const range = (from: [string, string], to: [string, string], count: number) => ({
      count,
      unchanged: 0,
      first_from: from[0],
      last_from: from[1],
      first_to: to[0],
      last_to: to[1],
    });
    dialogData = {
      candidate: candidate({
        plan_keeping_a: quietPlan(2, {
          station_renames: {
            reason: 'collision',
            kept: range(['0', '3'], ['A0', 'A3'], 4),
            other: range(['1', '2'], ['B1', 'B2'], 2),
            join_name: 'A3',
          },
        }),
      }),
    };
    render(<CombineLinesDialog />);
    expect(screen.getByTestId('combine-rename-note')).toHaveTextContent(
      'Station names collide, so stations will be renamed: Main-A 0..3 \u2192 A0..A3, Main-B 1..2 \u2192 B1..B2'
    );
    expect(screen.getByTestId('combine-summary')).toHaveTextContent('at station Main-A / A3');
    // Keeping the other line renames nothing here: no note, the join as it is.
    fireEvent.click(screen.getByTestId('combine-swap-kept'));
    expect(screen.queryByTestId('combine-rename-note')).toBeNull();
    expect(screen.getByTestId('combine-summary')).toHaveTextContent('at station Main-B / 0');
  });

  it('says how many stations keep their custom names, so the range does not read as renaming them', () => {
    dialogData = {
      candidate: candidate({
        plan_keeping_a: quietPlan(2, {
          station_renames: {
            reason: 'collision',
            kept: { count: 9, unchanged: 2, first_from: '0', last_from: '10', first_to: 'A0', last_to: 'A10' },
            other: { count: 2, unchanged: 1, first_from: '1', last_from: '3', first_to: 'B1', last_to: 'B3' },
            suffixed: [{ from: 'Sump', to: 'Sump-a' }],
            join_name: 'A10',
          },
        }),
      }),
    };
    render(<CombineLinesDialog />);
    expect(screen.getByTestId('combine-rename-note')).toHaveTextContent(
      'Station names collide, so stations will be renamed: ' +
        'Main-A 0..10 \u2192 A0..A10 (9 renamed, 2 keep their names), ' +
        'Main-B 1..3 \u2192 B1..B3 (2 renamed, 1 keeps its name), Main-B Sump \u2192 Sump-a'
    );
  });

  it('lists the names made unique by a letter, counting past three', () => {
    const suffixed = ['Sump', 'Pit', 'Dome', 'Hall', 'Gate'].map((from) => ({ from, to: `${from}-a` }));
    dialogData = {
      candidate: candidate({
        plan_keeping_a: quietPlan(2, {
          station_renames: {
            reason: 'collision',
            other: { count: 1, unchanged: 0, first_from: '1', last_from: '1', first_to: 'A1', last_to: 'A1' },
            suffixed,
            join_name: 'Entrance',
          },
        }),
      }),
    };
    render(<CombineLinesDialog />);
    expect(screen.getByTestId('combine-rename-note')).toHaveTextContent(
      'Station names collide, so stations will be renamed: Main-B 1 \u2192 A1 (1 renamed), ' +
        'Main-B Sump \u2192 Sump-a, Pit \u2192 Pit-a, Dome \u2192 Dome-a and 2 more'
    );
  });

  it('says when the other line continues a lettered kept line', () => {
    dialogData = {
      candidate: candidate({
        plan_keeping_a: quietPlan(2, {
          station_renames: {
            reason: 'lettered',
            other: { count: 1, unchanged: 0, first_from: '7', last_from: '7', first_to: 'C7', last_to: 'C7' },
            join_name: 'B2',
          },
        }),
      }),
    };
    render(<CombineLinesDialog />);
    expect(screen.getByTestId('combine-rename-note')).toHaveTextContent(
      'Main-A is lettered by an earlier combine, so stations will be renamed: Main-B 7 \u2192 C7'
    );
  });

  it('swaps the kept line, and the untouched defaults follow it', async () => {
    render(<CombineLinesDialog />);
    fireEvent.click(screen.getByTestId('combine-swap-kept'));
    expect(screen.getByTestId('combine-summary')).toHaveTextContent('Combining Main-B (2 shots) and Main-A (3 shots) at station Main-B / 0');
    expect(screen.getByTestId('combine-color')).toHaveValue('#222222');
    expect(screen.getByTestId('combine-plan')).toHaveTextContent('Main-A will be added before Main-B.');
    expect(screen.getByTestId('combine-direction-note')).toHaveTextContent('(Out)');
    fireEvent.click(screen.getByTestId('combine-confirm'));
    await waitFor(() => expect(combineLines).toHaveBeenCalled());
    expect(combineLines.mock.calls[0]?.slice(0, 2)).toEqual([5, 2]);
  });

  it('says the other line is reversed and offers the wall swap', async () => {
    dialogData = { candidate: candidate({ reverses: true, end_b: 'end' }) };
    render(<CombineLinesDialog />);
    expect(screen.getByTestId('combine-plan')).toHaveTextContent('Main-B will be reversed and added after Main-A.');
    expect(screen.getByTestId('combine-reverse-notice')).toHaveTextContent("Swap Main-B's left and right walls");
    // The why sits in the folded Details.
    expect(screen.getByTestId('combine-details')).not.toHaveAttribute('open');
    expect(screen.getByTestId('combine-reverse-explain')).toHaveTextContent('Reversing Main-B');
    fireEvent.click(screen.getByTestId('combine-swap-lr'));
    fireEvent.click(screen.getByTestId('combine-confirm'));
    await waitFor(() => expect(combineLines).toHaveBeenCalled());
    expect(combineLines.mock.calls[0]?.[2]).toMatchObject({ swap_lr: false });
  });

  it('asks about the join comment when both stations have one, never about its name', async () => {
    // The joined station keeps the kept line's name (names are unique per
    // line), so even a candidate that still carried both names offers no choice.
    dialogData = {
      candidate: candidate({ comment_a: 'restriction', comment_b: 'sump', station_name_a: 'S1', station_name_b: 'B0' }),
    };
    render(<CombineLinesDialog />);
    expect(screen.getByTestId('combine-comment-pick-kept').parentElement).toHaveTextContent('restriction');
    expect(screen.getByTestId('combine-comment-pick-other').parentElement).toHaveTextContent('sump');
    expect(screen.queryByTestId('combine-name-pick')).toBeNull();
    fireEvent.click(screen.getByTestId('combine-comment-pick-both'));
    fireEvent.click(screen.getByTestId('combine-confirm'));
    await waitFor(() => expect(combineLines).toHaveBeenCalled());
    expect(combineLines.mock.calls[0]?.[2]).toMatchObject({ comment_pick: 'both', name_pick: 'kept' });
  });

  it('defaults the split halves to their base name, and the new split names too', () => {
    lines = creditedLines().map((s) => ({ ...s, name: s.id === 2 ? 'Main A' : 'Main B' }));
    render(<CombineLinesDialog />);
    expect(screen.getByTestId('combine-name')).toHaveValue('Main');
  });

  it("falls back to the kept line's name when another line holds the base name", () => {
    lines = [
      ...creditedLines().map((s) => ({ ...s, name: s.id === 2 ? 'Main A' : 'Main B' })),
      { id: 7, name: 'Main', color: '#333333', direction: 'in', shots: [] },
    ];
    render(<CombineLinesDialog />);
    expect(screen.getByTestId('combine-name')).toHaveValue('Main A');
    expect(screen.queryByTestId('combine-name-error')).toBeNull();
  });

  it('gives an unnamed kept line a fresh "Line N" instead of leaving it unnamed', () => {
    lines = creditedLines().map((s) => (s.id === 2 ? { ...s, name: null as unknown as string } : s));
    render(<CombineLinesDialog />);
    expect(screen.getByTestId('combine-name')).toHaveValue('Line 1');
    expect(screen.getByTestId('combine-summary')).toHaveTextContent('Combining Unnamed line (3 shots)');
  });

  it("refuses an empty name or another line's, but not either combined line's", async () => {
    render(<CombineLinesDialog />);
    const field = screen.getByTestId('combine-name');
    const confirm = screen.getByTestId('combine-confirm');

    fireEvent.change(field, { target: { value: ' ' } });
    expect(screen.getByTestId('combine-name-error')).toHaveTextContent('A line needs a name.');
    expect(field).toHaveAttribute('aria-invalid', 'true');
    expect(confirm).toBeDisabled();
    expect(confirm).toHaveAttribute('title', 'A line needs a name.');

    fireEvent.change(field, { target: { value: 'Line 9' } });
    expect(screen.getByTestId('combine-name-error')).toHaveTextContent('A line named "Line 9" already exists.');
    expect(confirm).toBeDisabled();
    fireEvent.click(confirm);
    expect(combineLines).not.toHaveBeenCalled();

    // The other line's name is free: it stops existing once combined.
    fireEvent.change(field, { target: { value: ' Main-B ' } });
    expect(screen.queryByTestId('combine-name-error')).toBeNull();
    expect(confirm).toBeEnabled();
    fireEvent.click(confirm);
    await waitFor(() => expect(combineLines).toHaveBeenCalled());
    expect(combineLines.mock.calls[0]?.[2]).toMatchObject({ name: 'Main-B' });
  });

  it('counts one shot as "1 shot" everywhere', () => {
    dialogData = { candidate: candidate({ shot_count_b: 1 }) };
    lines = [
      creditedLines()[0]!,
      { ...creditedLines()[1]!, shots: [{ id: 51, length: 5, credit: credited(['Bob'], '2026-04-11', ['Cid']) }] },
    ];
    render(<CombineLinesDialog />);
    expect(screen.getByTestId('combine-summary')).toHaveTextContent(
      'Combining Main-A (3 shots) and Main-B (1 shot) at station Main-A / 3'
    );
    expect(screen.getByTestId('combine-credit-surveyors')).toHaveTextContent('Ann (3 shots), Bob (2 shots)');
    expect(screen.getByTestId('combine-credit-explorers')).toHaveTextContent('Explorers: Cid (1 shot)');
  });

  it('summarises the combined credit from every shot of both lines', () => {
    render(<CombineLinesDialog />);
    expect(screen.getByTestId('combine-credit')).toHaveTextContent('(5 shots)');
    expect(screen.getByTestId('combine-credit-surveyors')).toHaveTextContent('Surveyors: Ann (4 shots), Bob (2 shots)');
    expect(screen.getByTestId('combine-credit-explorers')).toHaveTextContent('Explorers: Cid (1 shot)');
    expect(screen.getByTestId('combine-credit-dates')).toHaveTextContent('Surveyed: 2026-03-02 - 2026-04-11');
    expect(screen.getByTestId('combine-credit')).toHaveTextContent('Every shot keeps its own surveyors, explorers and date.');
  });

  it('says when neither line records any credit', () => {
    lines = [
      { id: 2, name: 'Main-A', color: '#111111', direction: 'in', shots: shots(3) },
      { id: 5, name: 'Main-B', color: '#222222', direction: 'out', shots: shots(2) },
    ];
    render(<CombineLinesDialog />);
    expect(screen.getByTestId('combine-credit-none')).toBeInTheDocument();
  });

  it('starts on the line that can be kept when the other one has no shot ids left', async () => {
    dialogData = { candidate: candidate({ blocked_keeping_a: 'Main-A has used up its shot numbers' }) };
    render(<CombineLinesDialog />);
    expect(screen.getByTestId('combine-kept-label')).toHaveTextContent('Keep Main-B');
    expect(screen.getByTestId('combine-confirm')).toBeEnabled();
    fireEvent.click(screen.getByTestId('combine-swap-kept'));
    expect(screen.getByTestId('combine-blocked')).toHaveTextContent('used up its shot numbers');
    expect(screen.getByTestId('combine-confirm')).toBeDisabled();
  });

  it('shows why a blocked combine cannot run and never sends it', () => {
    dialogData = { candidate: candidate({ blocked_reason: 'This is a branch' }) };
    render(<CombineLinesDialog />);
    expect(screen.getByTestId('combine-blocked')).toHaveTextContent('This is a branch');
    expect(screen.getByTestId('combine-confirm')).toBeDisabled();
  });

  it('says how far the join moves in depth when the two ends disagree, and only then', () => {
    dialogData = { candidate: candidate({ depth_difference_m: 0.3 }) };
    const { unmount } = render(<CombineLinesDialog />);
    expect(screen.getByTestId('combine-depth-note')).toHaveTextContent(
      'The two ends are recorded 0.30m apart in depth. The joined station takes the average, so each end moves 0.15m in depth.'
    );
    expect(screen.getByTestId('combine-confirm')).toBeEnabled();
    unmount();

    dialogData = { candidate: candidate() };
    render(<CombineLinesDialog />);
    expect(screen.queryByTestId('combine-depth-note')).toBeNull();
  });

  it('says a third line ending at the join unmerged is left as it is', () => {
    lines = [...creditedLines(), { id: 7, name: 'Side', color: '#333333', direction: 'in', shots: [] }];
    dialogData = { candidate: candidate({ loose_ends: [7] }) };
    render(<CombineLinesDialog />);
    expect(screen.queryByTestId('combine-partner-picker')).toBeNull();
    expect(screen.getByTestId('combine-loose-ends-note')).toHaveTextContent(
      'Side also ends here but is not merged to the join, so it stays as it is.'
    );
  });

  it('refuses to combine in a read-only view', () => {
    blocked = true;
    render(<CombineLinesDialog />);
    fireEvent.click(screen.getByTestId('combine-confirm'));
    expect(combineLines).not.toHaveBeenCalled();
    expect(screen.getByTestId('combine-error')).toHaveTextContent('Editing is disabled');
  });

  it('across a loop closure, says what moves and which ties become closures, per kept line', async () => {
    const tie = (a: [number, number], b: [number, number], kind: string, reason = 'loop') => ({
      station_a: { line_id: a[0], station_index: a[1] },
      station_b: { line_id: b[0], station_index: b[1] },
      kind,
      reason,
    });
    dialogData = {
      candidate: candidate({
        join_kind: 'closure',
        plan_keeping_a: quietPlan(2, {
          moving_line: 5,
          moved_lines: [5, 7],
          shift_m: 0.42,
          torn_ties: [tie([5, 2], [9, 1], 'merge')],
        }),
        // Keeping Main-B: a zero datum holds Main-A, so the kept side moves.
        plan_keeping_b: quietPlan(5, {
          moving_line: 5,
          kept_moves_because: 'zero_datum',
          moved_lines: [5],
          shift_m: 0.42,
          torn_ties: [tie([2, 0], [9, 0], 'snapped_closure'), tie([5, 2], [8, 0], 'merge', 'zero_datum')],
        }),
      }),
    };
    lines = [
      ...creditedLines(),
      { id: 7, name: 'Side', color: '#333333', direction: 'in', shots: shots(1) },
    ];
    render(<CombineLinesDialog />);
    expect(screen.getByTestId('combine-closure-join')).toHaveTextContent(
      'Main-A and Main-B meet here across a loop closure: the survey puts the two ends 0.42m apart horizontally.'
    );
    expect(screen.getByTestId('combine-note-move')).toHaveTextContent(
      'Main-B moves 0.42m horizontally so the two ends meet, closing the loop here.'
    );
    expect(screen.getByTestId('combine-note-move-with')).toHaveTextContent('Side is attached to it and moves with it.');
    expect(screen.getByTestId('combine-note-torn')).toHaveTextContent('This connection becomes a loop closure:');
    // Each end of a torn tie is a line pill and a station pill, the link
    // pills of the Details panel, read here by their accessible names.
    expect(screen.getAllByTestId(/^combine-torn-tie-/).map(tieText)).toEqual([
      'The merge of [line Main-B] [station 2] and [line Line 9] [station 1]',
    ]);
    expect(screen.queryByTestId('combine-note-kept-moves')).toBeNull();

    fireEvent.click(screen.getByTestId('combine-swap-kept'));
    expect(screen.getByTestId('combine-note-move')).toHaveTextContent('Main-B moves 0.42m');
    expect(screen.getByTestId('combine-note-kept-moves')).toHaveTextContent(
      'Main-B is the one that moves, because a zero datum holds Main-A in place.'
    );
    expect(screen.getAllByTestId(/^combine-torn-tie-/).map(tieText)).toEqual([
      'The snapped closure between [line Main-A] [station 0] and [line Line 9] [station 0]',
      'The merge of [line Main-B] [station 2] and [line Line 8] [station 0] . [line Line 8] is held by a zero datum and stays where it is.',
    ]);
    // Hovering a station pill rings it on the map; clicking selects it.
    fireEvent.mouseEnter(screen.getAllByTestId('combine-tie-station-5-2')[0]!);
    expect(setHoveredStation).toHaveBeenCalledWith({ line_id: 5, station_index: 2 });
    fireEvent.click(screen.getAllByTestId('combine-tie-station-5-2')[0]!);
    expect(setSelection).toHaveBeenCalledWith({ type: 'station', stationRef: { line_id: 5, station_index: 2 } });
    setSelection.mockClear();
    expect(screen.getByTestId('combine-torn-why')).toHaveTextContent("The loop's misclosure moves to the new closure");
    fireEvent.click(screen.getByTestId('combine-confirm'));
    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
    expect(combineLines.mock.calls[0]?.[2]).toMatchObject({ join: { kept: 'start', other: 'end' } });
  });

  it('lists the ties the combine removes across one shot, per kept line, before the confirm (#746)', async () => {
    const removed = (a: [number, number], b: [number, number], kind: string) => ({
      station_a: { line_id: a[0], station_index: a[1] },
      station_b: { line_id: b[0], station_index: b[1] },
      kind,
    });
    dialogData = {
      candidate: candidate({
        join_kind: 'closure',
        plan_keeping_a: quietPlan(2, {
          moving_line: 5,
          moved_lines: [5],
          shift_m: 0.4,
          closures_removed_across_one_shot: [removed([9, 1], [5, 0], 'merge')],
        }),
        plan_keeping_b: quietPlan(5, {
          moving_line: 5,
          kept_moves_because: 'adjustment_anchor',
          moved_lines: [5],
          shift_m: 0.4,
          closures_removed_across_one_shot: [
            removed([9, 1], [5, 0], 'snapped_closure'),
            removed([2, 1], [5, 0], 'loop_closure'),
          ],
        }),
      }),
    };
    render(<CombineLinesDialog />);
    // Said outside the folded Details, which stay closed: a user who never
    // opens them still learns before confirming that a connection goes.
    const details = screen.getByTestId('combine-details');
    expect(details).not.toHaveAttribute('open');
    const summary = screen.getByTestId('combine-removed-summary');
    expect(details).not.toContainElement(summary);
    expect(summary).toHaveTextContent(
      'Combining removes 1 connection that would join the two ends of one shot (see Details).'
    );
    // Not a torn tie: the plan names it only as removed.
    expect(screen.queryByTestId('combine-note-torn')).toBeNull();
    expect(screen.getByTestId('combine-note-removed')).toHaveTextContent(/^This connection is removed:/);
    expect(screen.getAllByTestId(/^combine-removed-tie-/).map(tieText)).toEqual([
      'The merge of [line Line 9] [station 1] and [line Main-B] [station 0] : as a loop ' +
        'closure it would join the two ends of one shot.',
    ]);
    expect(screen.getByTestId('combine-removed-why')).toHaveTextContent(
      'Such a closure would squeeze that shot to zero length, so the Closures Computed View would only skip it. ' +
        'Undo brings it back.'
    );

    fireEvent.click(screen.getByTestId('combine-swap-kept'));
    expect(screen.getByTestId('combine-removed-summary')).toHaveTextContent(
      'Combining removes 2 connections that would each join the two ends of one shot (see Details).'
    );
    expect(screen.getByTestId('combine-note-removed')).toHaveTextContent(/^These 2 connections are removed:/);
    expect(screen.getAllByTestId(/^combine-removed-tie-/).map(tieText)).toEqual([
      'The snapped closure between [line Line 9] [station 1] and [line Main-B] [station 0] : ' +
        'it would join the two ends of one shot.',
      'Loop closure [line Main-A] [station 1] to [line Main-B] [station 0] : it would join the ' +
        'two ends of one shot.',
    ]);
    expect(screen.getByTestId('combine-removed-why')).toHaveTextContent(
      'Such closures would squeeze their shot to zero length, so the Closures Computed View would only skip them.'
    );
    fireEvent.click(screen.getByTestId('combine-confirm'));
    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
  });

  it('says nothing about removed connections when the combine removes none (#746)', () => {
    dialogData = {
      candidate: candidate({
        join_kind: 'closure',
        plan_keeping_a: quietPlan(2, { moving_line: 5, moved_lines: [5], shift_m: 0.4 }),
      }),
    };
    render(<CombineLinesDialog />);
    expect(screen.getByTestId('combine-closure-join')).toBeInTheDocument();
    expect(screen.queryByTestId('combine-removed-summary')).toBeNull();
    expect(screen.queryByTestId('combine-note-removed')).toBeNull();
  });

  it('shows the removed ties even when nothing else changes', () => {
    dialogData = {
      candidate: candidate({
        plan_keeping_a: quietPlan(2, {
          closures_removed_across_one_shot: [
            {
              station_a: { line_id: 2, station_index: 2 },
              station_b: { line_id: 5, station_index: 0 },
              kind: 'loop_closure',
            },
          ],
        }),
      }),
    };
    render(<CombineLinesDialog />);
    expect(screen.getByTestId('combine-plan-notes')).toBeInTheDocument();
    expect(screen.getAllByTestId(/^combine-removed-tie-/)).toHaveLength(1);
    expect(screen.getByTestId('combine-removed-summary')).toBeInTheDocument();
  });

  it('lists the closures the combine replaces by one the view adjusts on the same two stations (#749)', () => {
    const at = (s: number, i: number) => ({ line_id: s, station_index: i });
    const replaced = (
      a: [number, number],
      b: [number, number],
      kind: string,
      by: [[number, number], [number, number]],
      byKind: string
    ) => ({
      station_a: at(...a),
      station_b: at(...b),
      kind,
      by_station_a: at(...by[0]),
      by_station_b: at(...by[1]),
      by_kind: byKind,
    });
    dialogData = {
      candidate: candidate({
        join_kind: 'closure',
        plan_keeping_a: quietPlan(2, {
          moving_line: 2,
          kept_moves_because: 'adjustment_anchor',
          moved_lines: [2],
          shift_m: 0.4,
          closures_replaced: [replaced([5, 2], [5, 0], 'legacy_closure', [[2, 1], [5, 2]], 'merge')],
        }),
        plan_keeping_b: quietPlan(5, {
          moving_line: 2,
          moved_lines: [2],
          shift_m: 0.4,
          closures_replaced: [
            replaced([5, 2], [5, 0], 'snapped_closure', [[2, 1], [5, 2]], 'snapped_closure'),
            replaced([5, 3], [2, 0], 'former_snap', [[5, 3], [2, 1]], 'loop_closure'),
          ],
        }),
      }),
    };
    render(<CombineLinesDialog />);
    // A replacement keeps the connection, so it is listed in Details only,
    // with no summary line outside the fold (that is for removals, #746).
    expect(screen.queryByTestId('combine-removed-summary')).toBeNull();
    expect(screen.queryByTestId('combine-note-removed')).toBeNull();
    expect(screen.getByTestId('combine-note-replaced')).toHaveTextContent(/^This closure is replaced:/);
    expect(screen.getAllByTestId(/^combine-replaced-tie-/).map(tieText)).toEqual([
      'Loop closure [line Main-B] [station 2] to [line Main-B] [station 0] , an older kind the ' +
        'Closures Computed View does not adjust. The merge of [line Main-A] [station 1] and ' +
        '[line Main-B] [station 2] becomes a loop closure on the same two stations and takes its place.',
    ]);
    expect(screen.getByTestId('combine-replaced-why')).toHaveTextContent(
      'Two loop closures on the same two stations would count one loop twice, so the combine keeps the one ' +
        'the Closures Computed View adjusts. Undo brings this one back.'
    );

    fireEvent.click(screen.getByTestId('combine-swap-kept'));
    expect(screen.getByTestId('combine-note-replaced')).toHaveTextContent(/^These 2 closures are replaced:/);
    expect(screen.getAllByTestId(/^combine-replaced-tie-/).map(tieText)).toEqual([
      'The snapped closure between [line Main-B] [station 2] and [line Main-B] [station 0] : its ' +
        'two stations will no longer share one depth. The snapped closure between [line Main-A] ' +
        '[station 1] and [line Main-B] [station 2] becomes an ordinary loop closure on the same two ' +
        'stations and takes its place.',
      'Loop closure [line Main-B] [station 3] to [line Main-A] [station 0] , left from a snap whose ' +
        'stations have since moved apart; the Closures Computed View does not adjust it. Loop closure ' +
        '[line Main-B] [station 3] to [line Main-A] [station 1] , already there, ties the same ' +
        'two stations and takes its place.',
    ]);
    expect(screen.getByTestId('combine-replaced-why')).toHaveTextContent('Undo brings these back.');
  });

  it('says nothing about replaced closures when the combine replaces none (#749)', () => {
    dialogData = {
      candidate: candidate({
        join_kind: 'closure',
        plan_keeping_a: quietPlan(2, { moving_line: 5, moved_lines: [5], shift_m: 0.4 }),
      }),
    };
    render(<CombineLinesDialog />);
    expect(screen.queryByTestId('combine-note-replaced')).toBeNull();
    expect(screen.queryByTestId('combine-replaced-why')).toBeNull();
  });

  it('shows the replaced closures even when nothing else changes (#749)', () => {
    dialogData = {
      candidate: candidate({
        plan_keeping_a: quietPlan(2, {
          closures_replaced: [
            {
              station_a: { line_id: 5, station_index: 2 },
              station_b: { line_id: 5, station_index: 0 },
              kind: 'legacy_closure',
              by_station_a: { line_id: 2, station_index: 1 },
              by_station_b: { line_id: 5, station_index: 2 },
              by_kind: 'loop_closure',
            },
          ],
        }),
      }),
    };
    render(<CombineLinesDialog />);
    expect(screen.getByTestId('combine-plan-notes')).toBeInTheDocument();
    expect(screen.getAllByTestId(/^combine-replaced-tie-/)).toHaveLength(1);
  });

  it('says the kept side moves when the other side holds the Closures Computed View in place', () => {
    dialogData = {
      candidate: candidate({
        join_kind: 'closure',
        plan_keeping_a: quietPlan(2, {
          moving_line: 2,
          kept_moves_because: 'adjustment_anchor',
          moved_lines: [2],
          shift_m: 0.4,
        }),
      }),
    };
    render(<CombineLinesDialog />);
    expect(screen.getByTestId('combine-note-move')).toHaveTextContent('Main-A moves 0.40m horizontally');
    expect(screen.getByTestId('combine-note-kept-moves')).toHaveTextContent(
      "Main-A is the one that moves: the Closures Computed View holds the survey at a station on Main-B's side"
    );
  });

  it('lists a side-by-side move under 1 mm only where it changes a tie (#740)', () => {
    const tie = {
      station_a: { line_id: 5, station_index: 2 },
      station_b: { line_id: 9, station_index: 1 },
      kind: 'merge',
      reason: 'loop',
    };
    dialogData = {
      candidate: candidate({
        // The plan names the side that moves by the 0.6 mm residual, what
        // moves with it and why, exactly as for a shift.
        plan_keeping_a: quietPlan(2, {
          moving_line: 5,
          moved_lines: [5, 7],
          shift_m: 0.0006,
          torn_ties: [tie],
        }),
        plan_keeping_b: quietPlan(5, {
          moving_line: 5,
          kept_moves_because: 'adjustment_anchor',
          moved_lines: [5],
          shift_m: 0.0006,
        }),
      }),
    };
    lines = [
      ...creditedLines(),
      { id: 7, name: 'Side', color: '#333333', direction: 'in', shots: shots(1) },
    ];
    render(<CombineLinesDialog />);
    // Keeping Main-A the plan tears a tie, whose reason is the move: the move
    // is listed beside it, to 0.1 mm, with what moves along.
    expect(screen.getByTestId('combine-note-move')).toHaveTextContent(
      'Main-B moves 0.0006m horizontally, the gap between the two ends side by side, so they meet exactly.'
    );
    expect(screen.getByTestId('combine-note-move-with')).toHaveTextContent('Side');
    expect(screen.getAllByTestId(/^combine-torn-tie-/)).toHaveLength(1);
    expect(screen.getByTestId('combine-torn-why')).toHaveTextContent("The loop's misclosure moves to the new closure");

    // Keeping Main-B nothing is torn: a move that small is not listed, and
    // nothing claims a side stays either.
    fireEvent.click(screen.getByTestId('combine-swap-kept'));
    for (const id of ['combine-note-move', 'combine-note-move-with', 'combine-note-kept-moves', 'combine-note-torn']) {
      expect(screen.queryByTestId(id)).toBeNull();
    }
  });

  /** A tie the plan tears, between `a` and `b`, for `reason`. */
  const tornTie = (kind: string, a: [number, number], b: [number, number], reason: string) => ({
    station_a: { line_id: a[0], station_index: a[1] },
    station_b: { line_id: b[0], station_index: b[1] },
    kind,
    reason,
  });

  it('on a shift, names the line a zero datum holds for a snap torn for it, and claims no loop (#756)', () => {
    // Zero datums hold both sides and Main-B moves across the loop closure:
    // its merge to Line 9 (D, its datum side) becomes a loop closure, and
    // so does its snap into D, because D's datum keeps D where it is. The
    // dialog names the end that does not move (D's, here first on the snap,
    // as the snap dialog stores it). Neither tears to open a loop the combine
    // closes, so no paragraph says the loop's misclosure moves to it.
    const shift = (kept: number, torn: unknown[]) =>
      quietPlan(kept, {
        moving_line: 5,
        kept_moves_because: kept === 5 ? 'zero_datum' : null,
        moved_lines: [5],
        shift_m: 11.5,
        torn_ties: torn,
      });
    dialogData = {
      candidate: candidate({
        join_kind: 'closure',
        plan_keeping_a: shift(2, [
          tornTie('merge', [5, 2], [9, 0], 'zero_datum'),
          tornTie('snapped_closure', [9, 1], [5, 1], 'zero_datum'),
        ]),
        // The snap alone torn for the datum.
        plan_keeping_b: shift(5, [tornTie('snapped_closure', [9, 1], [5, 1], 'zero_datum')]),
      }),
    };
    render(<CombineLinesDialog />);
    expect(screen.getByTestId('combine-note-move')).toHaveTextContent(
      'Main-B moves 11.50m horizontally so the two ends meet, closing the loop here.'
    );
    expect(screen.getAllByTestId(/^combine-torn-tie-/).map(tieText)).toEqual([
      'The merge of [line Main-B] [station 2] and [line Line 9] [station 0] . [line Line 9] is held by a zero datum and stays where it is.',
      'The snapped closure between [line Line 9] [station 1] and [line Main-B] [station 1] . [line Line 9] is held by a zero datum and stays where it is.',
    ]);
    expect(screen.queryByTestId('combine-torn-why')).toBeNull();
    fireEvent.click(screen.getByTestId('combine-swap-kept'));
    expect(screen.getAllByTestId(/^combine-torn-tie-/).map(tieText)).toEqual([
      'The snapped closure between [line Line 9] [station 1] and [line Main-B] [station 1] . [line Line 9] is held by a zero datum and stays where it is.',
    ]);
    expect(screen.queryByTestId('combine-torn-why')).toBeNull();
  });

  it('keeps the loop paragraph for a loop tie beside a snap torn for a zero datum (#756)', () => {
    // Main-B moves: its snap into D tears for D's datum, and its snap across
    // to Line 7 (merged to Main-A) closes a loop through the combined
    // line. Only the snap into D says D is held; the paragraph under the
    // list, about the loop, stays.
    dialogData = {
      candidate: candidate({
        join_kind: 'closure',
        plan_keeping_a: quietPlan(2, {
          moving_line: 5,
          moved_lines: [5],
          shift_m: 11.5,
          torn_ties: [
            tornTie('snapped_closure', [5, 1], [9, 1], 'zero_datum'),
            tornTie('snapped_closure', [5, 0], [7, 1], 'loop'),
          ],
        }),
      }),
    };
    lines = [
      ...creditedLines(),
      { id: 7, name: 'Side', color: '#333333', direction: 'in', shots: shots(1) },
    ];
    render(<CombineLinesDialog />);
    expect(screen.getAllByTestId(/^combine-torn-tie-/).map(tieText)).toEqual([
      'The snapped closure between [line Main-B] [station 1] and [line Line 9] [station 1] . [line Line 9] is held by a zero datum and stays where it is.',
      'The snapped closure between [line Main-B] [station 0] and [line Side] [station 1]',
    ]);
    expect(screen.getByTestId('combine-torn-why').textContent).toBe(
      "The loop's misclosure moves to the new closure, and the Closures Computed View adjusts it like any other loop."
    );
  });

  it('says why a snap the joined station pulls apart becomes a loop closure, and names a former snap (#736)', () => {
    // Keeping Main-A nothing moves, but the joined station keeps Main-A's
    // hand move: the snap at Main-B's join station ends up apart. That is its
    // own reason, not a loop the combine closes, so no paragraph says the
    // combined line would be tied to itself.
    dialogData = {
      candidate: candidate({
        plan_keeping_a: quietPlan(2, {
          join_offset_change_m: 0.5,
          torn_ties: [tornTie('snapped_closure', [5, 0], [8, 1], 'joined_station_moves')],
        }),
        // Keeping Main-B, which moves (a zero datum holds Main-A): the
        // joined station keeps Main-A's position, and a torn snap the move
        // brings back together across the two sides is named as the loop
        // closure left from a snap it is.
        plan_keeping_b: quietPlan(5, {
          moving_line: 5,
          kept_moves_because: 'zero_datum',
          moved_lines: [5],
          shift_m: 0.3,
          join_offset_change_m: 0.5,
          torn_ties: [
            tornTie('former_snapped_closure', [5, 1], [2, 1], 'loop'),
            tornTie('snapped_closure', [2, 3], [8, 1], 'joined_station_moves'),
          ],
        }),
      }),
    };
    render(<CombineLinesDialog />);
    expect(screen.getAllByTestId(/^combine-torn-tie-/).map(tieText)).toEqual([
      'The snapped closure between [line Main-B] [station 0] and [line Line 8] [station 1] . ' +
        "The joined station keeps [line Main-A] 's position as shown, so the two stations end up more " +
        'than 0.01m apart.',
    ]);
    expect(screen.queryByTestId('combine-torn-why')).toBeNull();

    fireEvent.click(screen.getByTestId('combine-swap-kept'));
    expect(screen.getAllByTestId(/^combine-torn-tie-/).map(tieText)).toEqual([
      'Loop closure [line Main-B] [station 1] to [line Main-A] [station 1] , left from a snap whose ' +
        'stations had moved apart: combining brings them back together, where it would snap again',
      'The snapped closure between [line Main-A] [station 3] and [line Line 8] [station 1] . ' +
        "The joined station keeps [line Main-A] 's position as shown, so the two stations end up more " +
        'than 0.01m apart.',
    ]);
    expect(screen.getByTestId('combine-torn-why').textContent).toBe(
      "The loop's misclosure moves to the new closure, and the Closures Computed View adjusts it like any other loop."
    );
  });

  it('names a former snap the combine removes across one shot or that takes a closure\'s place (#736)', () => {
    const at = (s: number, i: number) => ({ line_id: s, station_index: i });
    dialogData = {
      candidate: candidate({
        join_kind: 'closure',
        plan_keeping_a: quietPlan(2, {
          moving_line: 5,
          moved_lines: [5],
          shift_m: 0.4,
          closures_removed_across_one_shot: [
            { station_a: at(5, 1), station_b: at(2, 2), kind: 'former_snapped_closure' },
          ],
          closures_replaced: [
            {
              station_a: at(5, 2),
              station_b: at(2, 1),
              kind: 'legacy_closure',
              by_station_a: at(5, 2),
              by_station_b: at(2, 1),
              by_kind: 'former_snapped_closure',
            },
          ],
        }),
      }),
    };
    render(<CombineLinesDialog />);
    expect(screen.getAllByTestId(/^combine-removed-tie-/).map(tieText)).toEqual([
      'Loop closure [line Main-B] [station 1] to [line Main-A] [station 2] , left from a snap whose ' +
        'stations had moved apart: combining brings them back together, where it would join the two ends of ' +
        'one shot.',
    ]);
    expect(screen.getAllByTestId(/^combine-replaced-tie-/).map(tieText)).toEqual([
      'Loop closure [line Main-B] [station 2] to [line Main-A] [station 1] , an older kind the ' +
        'Closures Computed View does not adjust. Loop closure [line Main-B] [station 2] to [line ' +
        'Main-A] [station 1] , left from a snap whose stations had moved apart, becomes an ordinary loop ' +
        'closure on the same two stations and takes its place.',
    ]);
  });

  it('lists converted bearings, the kept quality and offset, and crossings', () => {
    lines = creditedLines().map((s) => ({ ...s, quality: s.id === 2 ? 1 : 2 }));
    dialogData = {
      candidate: candidate({
        passing_through: [5],
        repeated_shot_ids: 2,
        plan_keeping_a: quietPlan(2, { azimuth_rotation_deg: 5, join_offset_change_m: 0.25 }),
      }),
    };
    render(<CombineLinesDialog />);
    expect(screen.getByTestId('combine-note-rotation')).toHaveTextContent(
      "Main-B's bearings turn by +5.0° to read against Main-A's north reference"
    );
    expect(screen.getByTestId('combine-note-quality')).toHaveTextContent(
      "The combined line keeps Main-A's quality factor (1); Main-B's was 2."
    );
    expect(screen.getByTestId('combine-note-offset')).toHaveTextContent(
      "The joined station keeps Main-A's, so Main-B's end moves 0.25m."
    );
    // Shot uuids are project-unique on the station graph: nothing is renewed.
    expect(screen.queryByTestId('combine-note-repeated-ids')).toBeNull();
    expect(screen.getByTestId('combine-note-passing')).toHaveTextContent('Main-B passes through the join');
  });

  it('lists what moves with the joined station right after the hand-move note (#755)', () => {
    lines = [...creditedLines(), { id: 7, name: 'Side', color: '#333333', direction: 'in', shots: shots(1) }];
    const joined = (shift_m: number) => ({
      station: { line_id: 5, station_index: 0 },
      lines: [7],
      shift_m,
    });
    dialogData = {
      candidate: candidate({
        branches: [7],
        plan_keeping_a: quietPlan(2, {
          moving_line: 5,
          moved_lines: [5],
          join_offset_change_m: 0.3,
          moved_with_joined_station: joined(0.3),
        }),
        plan_keeping_b: quietPlan(5, { moved_with_joined_station: joined(0.0006) }),
      }),
    };
    render(<CombineLinesDialog />);
    expect(screen.getByTestId('combine-note-joined-moves')).toHaveTextContent(
      'Side is merged at Main-B / 0 and moves 0.30m with the joined station, so it stays merged to it.'
    );
    const order = Array.from(screen.getByTestId('combine-plan-notes').querySelectorAll('li')).map((li) =>
      li.getAttribute('data-testid')
    );
    expect(order.indexOf('combine-note-joined-moves')).toBe(order.indexOf('combine-note-offset') + 1);
    // Under the side-by-side tolerance, with no tie torn, it is not listed,
    // as a move of the side is not.
    fireEvent.click(screen.getByTestId('combine-swap-kept'));
    expect(screen.queryByTestId('combine-note-joined-moves')).toBeNull();
  });

  it('lists a joined move under 1 mm beside the tie it tears (#755)', () => {
    lines = [...creditedLines(), { id: 7, name: 'Side', color: '#333333', direction: 'in', shots: shots(1) }];
    dialogData = {
      candidate: candidate({
        branches: [7],
        plan_keeping_a: quietPlan(2, {
          moving_line: 5,
          moved_lines: [5],
          torn_ties: [
            {
              station_a: { line_id: 7, station_index: 1 },
              station_b: { line_id: 5, station_index: 1 },
              kind: 'snapped_closure',
              reason: 'loop',
            },
          ],
          moved_with_joined_station: { station: { line_id: 5, station_index: 0 }, lines: [7], shift_m: 0.0006 },
        }),
      }),
    };
    render(<CombineLinesDialog />);
    expect(screen.getByTestId('combine-note-joined-moves')).toHaveTextContent(
      'Side is merged at Main-B / 0 and moves 0.0006m with the joined station, so it stays merged to it.'
    );
    expect(screen.getByTestId('combine-note-torn')).toBeInTheDocument();
  });

  it('folds everything informational under Details, closed until opened', () => {
    dialogData = { candidate: candidate({ branches: [7], depth_difference_m: 0.3 }) };
    lines = [...creditedLines(), { id: 7, name: 'Side', color: '#333333', direction: 'in', shots: shots(1) }];
    render(<CombineLinesDialog />);
    const details = screen.getByTestId('combine-details');
    expect(details).not.toHaveAttribute('open');
    for (const id of ['combine-branches-note', 'combine-depth-note', 'combine-direction-note', 'combine-credit']) {
      expect(details).toContainElement(screen.getByTestId(id));
    }
    // What the user decides stays out of the fold.
    expect(details).not.toContainElement(screen.getByTestId('combine-name'));
    fireEvent.click(screen.getByTestId('combine-details-toggle'));
    expect(details).toHaveAttribute('open');
  });

  it('keeps the line the menu named first', () => {
    dialogData = { candidate: candidate(), keep: 5 };
    render(<CombineLinesDialog />);
    expect(screen.getByTestId('combine-kept-label')).toHaveTextContent('Keep Main-B');
  });

  it('says the bearings keep their values only when the backend leaves them alone', () => {
    dialogData = { candidate: candidate({ plan_keeping_a: quietPlan(2, { azimuth_rotation_deg: 0 }) }) };
    const { unmount } = render(<CombineLinesDialog />);
    expect(screen.getByTestId('combine-note-rotation')).toHaveTextContent('keep their values');
    unmount();
    dialogData = { candidate: candidate({ plan_keeping_a: quietPlan(2, { azimuth_rotation_deg: 0.04 }) }) };
    render(<CombineLinesDialog />);
    expect(screen.getByTestId('combine-note-rotation')).toHaveTextContent('turn by +0.040°');
  });
});
