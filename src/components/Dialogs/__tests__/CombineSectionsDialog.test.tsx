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
    kept_moves_because: null,
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
    expect(screen.getByTestId('combine-reverse-notice')).toHaveTextContent("Swap Main-B's left and right walls");
    // The why sits in the folded Details.
    expect(screen.getByTestId('combine-details')).not.toHaveAttribute('open');
    expect(screen.getByTestId('combine-reverse-explain')).toHaveTextContent('Reversing Main-B');
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
          kept_moves_because: 'zero_datum',
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

  it('lists the ties the combine removes across one shot, per kept section, before the confirm (#746)', async () => {
    const removed = (a: [number, number], b: [number, number], kind: string) => ({
      station_a: { section_id: a[0], station_index: a[1] },
      station_b: { section_id: b[0], station_index: b[1] },
      kind,
    });
    dialogData = {
      candidate: candidate({
        join_kind: 'closure',
        plan_keeping_a: quietPlan(2, {
          moving_section: 5,
          moved_sections: [5],
          shift_m: 0.4,
          closures_removed_across_one_shot: [removed([9, 1], [5, 0], 'merge')],
        }),
        plan_keeping_b: quietPlan(5, {
          moving_section: 5,
          kept_moves_because: 'adjustment_anchor',
          moved_sections: [5],
          shift_m: 0.4,
          closures_removed_across_one_shot: [
            removed([9, 1], [5, 0], 'snapped_closure'),
            removed([2, 1], [5, 0], 'loop_closure'),
          ],
        }),
      }),
    };
    render(<CombineSectionsDialog />);
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
      'The merge of [section Section 9] [station 9:1] and [section Main-B] [station 5:0] : as a loop ' +
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
      'The snapped closure between [section Section 9] [station 9:1] and [section Main-B] [station 5:0] : ' +
        'as an ordinary closure it would join the two ends of one shot.',
      'Loop closure [section Main-A] [station 2:1] to [section Main-B] [station 5:0] : it would join the ' +
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
        plan_keeping_a: quietPlan(2, { moving_section: 5, moved_sections: [5], shift_m: 0.4 }),
      }),
    };
    render(<CombineSectionsDialog />);
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
              station_a: { section_id: 2, station_index: 2 },
              station_b: { section_id: 5, station_index: 0 },
              kind: 'loop_closure',
            },
          ],
        }),
      }),
    };
    render(<CombineSectionsDialog />);
    expect(screen.getByTestId('combine-plan-notes')).toBeInTheDocument();
    expect(screen.getAllByTestId(/^combine-removed-tie-/)).toHaveLength(1);
    expect(screen.getByTestId('combine-removed-summary')).toBeInTheDocument();
  });

  it('lists the closures the combine replaces by one the view adjusts on the same two stations (#749)', () => {
    const at = (s: number, i: number) => ({ section_id: s, station_index: i });
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
          moving_section: 2,
          kept_moves_because: 'adjustment_anchor',
          moved_sections: [2],
          shift_m: 0.4,
          closures_replaced: [replaced([5, 2], [5, 0], 'legacy_closure', [[2, 1], [5, 2]], 'merge')],
        }),
        plan_keeping_b: quietPlan(5, {
          moving_section: 2,
          moved_sections: [2],
          shift_m: 0.4,
          closures_replaced: [
            replaced([5, 2], [5, 0], 'snapped_closure', [[2, 1], [5, 2]], 'snapped_closure'),
            replaced([5, 3], [2, 0], 'former_snap', [[5, 3], [2, 1]], 'loop_closure'),
          ],
        }),
      }),
    };
    render(<CombineSectionsDialog />);
    // A replacement keeps the connection, so it is listed in Details only,
    // with no summary line outside the fold (that is for removals, #746).
    expect(screen.queryByTestId('combine-removed-summary')).toBeNull();
    expect(screen.queryByTestId('combine-note-removed')).toBeNull();
    expect(screen.getByTestId('combine-note-replaced')).toHaveTextContent(/^This closure is replaced:/);
    expect(screen.getAllByTestId(/^combine-replaced-tie-/).map(tieText)).toEqual([
      'Loop closure [section Main-B] [station 5:2] to [section Main-B] [station 5:0] , an older kind the ' +
        'Closures Computed View does not adjust. The merge of [section Main-A] [station 2:1] and ' +
        '[section Main-B] [station 5:2] becomes a loop closure on the same two stations and takes its place.',
    ]);
    expect(screen.getByTestId('combine-replaced-why')).toHaveTextContent(
      'Two loop closures on the same two stations would count one loop twice, so the combine keeps the one ' +
        'the Closures Computed View adjusts. Undo brings this one back.'
    );

    fireEvent.click(screen.getByTestId('combine-swap-kept'));
    expect(screen.getByTestId('combine-note-replaced')).toHaveTextContent(/^These 2 closures are replaced:/);
    expect(screen.getAllByTestId(/^combine-replaced-tie-/).map(tieText)).toEqual([
      'The snapped closure between [section Main-B] [station 5:2] and [section Main-B] [station 5:0] : its ' +
        'two stations will no longer share one depth. The snapped closure between [section Main-A] ' +
        '[station 2:1] and [section Main-B] [station 5:2] becomes an ordinary loop closure on the same two ' +
        'stations and takes its place.',
      'Loop closure [section Main-B] [station 5:3] to [section Main-A] [station 2:0] , left from a snap whose ' +
        'stations have since moved apart; the Closures Computed View does not adjust it. Loop closure ' +
        '[section Main-B] [station 5:3] to [section Main-A] [station 2:1] , already there, ties the same ' +
        'two stations and takes its place.',
    ]);
    expect(screen.getByTestId('combine-replaced-why')).toHaveTextContent('Undo brings these back.');
  });

  it('says nothing about replaced closures when the combine replaces none (#749)', () => {
    dialogData = {
      candidate: candidate({
        join_kind: 'closure',
        plan_keeping_a: quietPlan(2, { moving_section: 5, moved_sections: [5], shift_m: 0.4 }),
      }),
    };
    render(<CombineSectionsDialog />);
    expect(screen.queryByTestId('combine-note-replaced')).toBeNull();
    expect(screen.queryByTestId('combine-replaced-why')).toBeNull();
  });

  it('shows the replaced closures even when nothing else changes (#749)', () => {
    dialogData = {
      candidate: candidate({
        plan_keeping_a: quietPlan(2, {
          closures_replaced: [
            {
              station_a: { section_id: 5, station_index: 2 },
              station_b: { section_id: 5, station_index: 0 },
              kind: 'legacy_closure',
              by_station_a: { section_id: 2, station_index: 1 },
              by_station_b: { section_id: 5, station_index: 2 },
              by_kind: 'loop_closure',
            },
          ],
        }),
      }),
    };
    render(<CombineSectionsDialog />);
    expect(screen.getByTestId('combine-plan-notes')).toBeInTheDocument();
    expect(screen.getAllByTestId(/^combine-replaced-tie-/)).toHaveLength(1);
  });

  it('says the kept side moves when the other side holds the Closures Computed View in place', () => {
    dialogData = {
      candidate: candidate({
        join_kind: 'closure',
        plan_keeping_a: quietPlan(2, {
          moving_section: 2,
          kept_moves_because: 'adjustment_anchor',
          moved_sections: [2],
          shift_m: 0.4,
        }),
      }),
    };
    render(<CombineSectionsDialog />);
    expect(screen.getByTestId('combine-note-move')).toHaveTextContent('Main-A moves 0.40m horizontally');
    expect(screen.getByTestId('combine-note-kept-moves')).toHaveTextContent(
      "Main-A is the one that moves: the Closures Computed View holds the survey at a station on Main-B's side"
    );
  });

  it('lists a side-by-side move under 1 mm only where it changes a tie (#740)', () => {
    const tie = {
      station_a: { section_id: 5, station_index: 2 },
      station_b: { section_id: 9, station_index: 1 },
      kind: 'merge',
      reason: 'loop',
    };
    dialogData = {
      candidate: candidate({
        // The plan names the side that moves by the 0.6 mm residual, what
        // moves with it and why, exactly as for a shift.
        plan_keeping_a: quietPlan(2, {
          moving_section: 5,
          moved_sections: [5, 7],
          shift_m: 0.0006,
          torn_ties: [tie],
        }),
        plan_keeping_b: quietPlan(5, {
          moving_section: 5,
          kept_moves_because: 'adjustment_anchor',
          moved_sections: [5],
          shift_m: 0.0006,
        }),
      }),
    };
    sections = [
      ...creditedSections(),
      { id: 7, name: 'Side', color: '#333333', direction: 'in', shots: shots(1) },
    ];
    render(<CombineSectionsDialog />);
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

  it('lists what moves with the joined station right after the hand-move note (#755)', () => {
    sections = [...creditedSections(), { id: 7, name: 'Side', color: '#333333', direction: 'in', shots: shots(1) }];
    const joined = (shift_m: number) => ({
      station: { section_id: 5, station_index: 0 },
      sections: [7],
      shift_m,
    });
    dialogData = {
      candidate: candidate({
        branches: [7],
        plan_keeping_a: quietPlan(2, {
          moving_section: 5,
          moved_sections: [5],
          join_offset_change_m: 0.3,
          moved_with_joined_station: joined(0.3),
        }),
        plan_keeping_b: quietPlan(5, { moved_with_joined_station: joined(0.0006) }),
      }),
    };
    render(<CombineSectionsDialog />);
    expect(screen.getByTestId('combine-note-joined-moves')).toHaveTextContent(
      'Side is merged at 5:0 and moves 0.30m with the joined station, so it stays merged to it.'
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
    sections = [...creditedSections(), { id: 7, name: 'Side', color: '#333333', direction: 'in', shots: shots(1) }];
    dialogData = {
      candidate: candidate({
        branches: [7],
        plan_keeping_a: quietPlan(2, {
          moving_section: 5,
          moved_sections: [5],
          torn_ties: [
            {
              station_a: { section_id: 7, station_index: 1 },
              station_b: { section_id: 5, station_index: 1 },
              kind: 'snapped_closure',
              reason: 'loop',
            },
          ],
          moved_with_joined_station: { station: { section_id: 5, station_index: 0 }, sections: [7], shift_m: 0.0006 },
        }),
      }),
    };
    render(<CombineSectionsDialog />);
    expect(screen.getByTestId('combine-note-joined-moves')).toHaveTextContent(
      'Side is merged at 5:0 and moves 0.0006m with the joined station, so it stays merged to it.'
    );
    expect(screen.getByTestId('combine-note-torn')).toBeInTheDocument();
  });

  it('folds everything informational under Details, closed until opened', () => {
    dialogData = { candidate: candidate({ branches: [7], depth_difference_m: 0.3 }) };
    sections = [...creditedSections(), { id: 7, name: 'Side', color: '#333333', direction: 'in', shots: shots(1) }];
    render(<CombineSectionsDialog />);
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

  it('keeps the section the menu named first', () => {
    dialogData = { candidate: candidate(), keep: 5 };
    render(<CombineSectionsDialog />);
    expect(screen.getByTestId('combine-kept-label')).toHaveTextContent('Keep Main-B');
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
