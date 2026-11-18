/**
 * #335: the "Split Line Here" dialog.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';

const closeDialog = vi.fn();
const clearSelection = vi.fn();
const splitLine = vi.fn();

let dialogData: Record<string, unknown> = {};
let resolvedTheme = 'dark';
const uiState = () => ({ closeDialog, clearSelection, dialogData, resolvedTheme });
vi.mock('../../../store/uiStore', () => ({
  useUIStore: Object.assign(() => uiState(), { getState: () => uiState() }),
}));

const shot = (id: number) => ({ id, length: 10 });
let stations: Array<Record<string, unknown>> = [];
let mainColor = '#123456';
let otherLines: Array<Record<string, unknown>> = [];
vi.mock('../../../store/projectStore', () => ({
  useProjectStore: () => ({
    splitLine,
    config: null,
    stations,
    project: {
      lines: [
        { id: 3, name: 'Main', color: mainColor, shots: [shot(31), shot(32), shot(33), shot(34)] },
        ...otherLines,
      ],
    },
  }),
}));

let blocked = false;
vi.mock('../../../hooks/useEditingBlocked', () => ({
  isEditingBlockedNow: () => blocked,
}));

vi.mock('../../../utils/telemetry', () => ({ trackDialogComplete: vi.fn() }));

const previewSplitLine = vi.fn();
vi.mock('../../../utils/splitLine', () => ({
  previewSplitLine: (...args: unknown[]) => previewSplitLine(...args),
}));
const NO_MOVE = {
  closures_to_merges: 0,
  moved_side: null,
  moved_m: 0,
  removed_closures: [],
  removed_already_one_point: [],
};
const ref = (line_id: number, station_index: number) => ({ line_id, station_index });

vi.mock('../DialogWrapper', () => ({
  DialogWrapper: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
}));

import '../../../i18n';
import SplitLineHereDialog from '../SplitLineHereDialog';
import { LIGHT_LINE_COLORS, LINE_COLORS } from '../../../utils/colors';

const station = (index: number, extra: Record<string, unknown>) => ({
  station_ref: { line_id: 3, station_index: index },
  is_zero_datum: false,
  ...extra,
});

/**
 * The lines the closure cases' stations live on, by name, and a named station
 * on Main: a closure is listed by its stations' names with their lines'.
 */
function namedClosureLines() {
  otherLines = [1, 2, 5, 6].map((id) => ({
    id,
    name: { 1: 'Inlet', 2: 'Upper', 5: 'Side', 6: 'Sump' }[id],
    color: '#000000',
    shots: [],
  }));
  stations = [station(3, { name: 'M3' })];
}

describe('SplitLineHereDialog', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    blocked = false;
    stations = [];
    mainColor = '#123456';
    otherLines = [];
    resolvedTheme = 'dark';
    splitLine.mockResolvedValue({ success: true });
    previewSplitLine.mockResolvedValue(NO_MOVE);
    dialogData = { lineId: 3, at: { type: 'station', station_index: 2 } };
  });

  it('defaults the names and colours from the line', () => {
    stations = [station(2, { name: '17' })];
    render(<SplitLineHereDialog />);
    expect(screen.getByTestId('split-here-name-a')).toHaveValue('Main A');
    expect(screen.getByTestId('split-here-name-b')).toHaveValue('Main B');
    expect(screen.getByTestId('split-here-color-a')).toHaveValue('#123456');
    // B is a new line: the colour a new line gets, not A's.
    expect(screen.getByTestId('split-here-color-b')).toHaveValue(LINE_COLORS[0]);
    // The station by its name, never its position.
    expect(screen.getByTestId('split-here-summary')).toHaveTextContent('Splitting Main at station 17');
    expect(screen.getByTestId('split-here-credit-note')).toHaveTextContent(
      'Each shot keeps its own surveyors, explorers and survey date.'
    );
    expect(screen.getByTestId('split-here-unconnected-hint')).toHaveTextContent(
      'The halves are not connected: merge the two cut stations if routes and gas planning should continue across them.'
    );
  });

  it('defaults to names no other line holds, and never the same two', () => {
    otherLines = [
      { id: 4, name: 'Main A', color: '#000000', shots: [] },
      { id: 5, name: 'Main B (2)', color: '#000000', shots: [] },
    ];
    render(<SplitLineHereDialog />);
    expect(screen.getByTestId('split-here-name-a')).toHaveValue('Main A (2)');
    expect(screen.getByTestId('split-here-name-b')).toHaveValue('Main B');
    expect(screen.queryByTestId('split-here-name-a-error')).toBeNull();
    expect(screen.getByTestId('split-here-confirm')).toBeEnabled();
  });

  it('says what is wrong with a half name and disables Split until it is fixed', () => {
    otherLines = [{ id: 4, name: 'Side', color: '#000000', shots: [] }];
    render(<SplitLineHereDialog />);
    const confirm = screen.getByTestId('split-here-confirm');

    fireEvent.change(screen.getByTestId('split-here-name-a'), { target: { value: '  ' } });
    expect(screen.getByTestId('split-here-name-a-error')).toHaveTextContent('A line needs a name.');
    expect(screen.getByTestId('split-here-name-a')).toHaveAttribute('aria-invalid', 'true');
    expect(confirm).toBeDisabled();
    expect(confirm).toHaveAttribute('title', 'A line needs a name.');

    fireEvent.change(screen.getByTestId('split-here-name-a'), { target: { value: 'Head' } });
    fireEvent.change(screen.getByTestId('split-here-name-b'), { target: { value: 'Side' } });
    expect(screen.getByTestId('split-here-name-b-error')).toHaveTextContent('A line named "Side" already exists.');
    expect(confirm).toBeDisabled();

    fireEvent.change(screen.getByTestId('split-here-name-b'), { target: { value: ' Head' } });
    expect(screen.getByTestId('split-here-name-b-error')).toHaveTextContent('The two lines need different names.');
    expect(confirm).toBeDisabled();

    // The split line gives up its name (half A keeps the line), so B may take it.
    fireEvent.change(screen.getByTestId('split-here-name-b'), { target: { value: 'Main' } });
    expect(screen.queryByTestId('split-here-name-b-error')).toBeNull();
    expect(confirm).toBeEnabled();
    expect(confirm).not.toHaveAttribute('title');
  });

  it("defaults B to the next palette colour no line uses, never A's", () => {
    // A's colour IS the first palette colour (stored uppercase), and another
    // line has the second: B takes the third.
    mainColor = LINE_COLORS[0]!.toUpperCase();
    otherLines = [{ id: 4, name: 'Other', color: LINE_COLORS[1], shots: [] }];
    render(<SplitLineHereDialog />);
    expect(screen.getByTestId('split-here-color-a')).toHaveValue(LINE_COLORS[0]);
    expect(screen.getByTestId('split-here-color-b')).toHaveValue(LINE_COLORS[2]);
  });

  it('draws B from the light palette on a light canvas', () => {
    resolvedTheme = 'light';
    render(<SplitLineHereDialog />);
    expect(screen.getByTestId('split-here-color-b')).toHaveValue(LIGHT_LINE_COLORS[0]);
  });

  it("still lets B take A's colour from the picker", async () => {
    render(<SplitLineHereDialog />);
    fireEvent.change(screen.getByTestId('split-here-color-b'), { target: { value: '#123456' } });
    fireEvent.click(screen.getByTestId('split-here-confirm'));
    await waitFor(() => expect(splitLine).toHaveBeenCalled());
    expect(splitLine.mock.calls[0]?.[2]).toMatchObject({ color_a: '#123456', color_b: '#123456' });
  });

  it('sends a station split and clears the selection', async () => {
    render(<SplitLineHereDialog />);
    fireEvent.click(screen.getByTestId('split-here-confirm'));
    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
    expect(splitLine).toHaveBeenCalledWith(3, { type: 'station', station_index: 2 }, {
      name_a: 'Main A',
      name_b: 'Main B',
      color_a: '#123456',
      color_b: LINE_COLORS[0],
      comment_side: 'a',
      datum_side: 'a',
    });
    expect(clearSelection).toHaveBeenCalled();
  });

  it('sends a shot split with its distance', async () => {
    dialogData = { lineId: 3, at: { type: 'shot', shot_id: 32, distance: 4 } };
    stations = [station(1, { name: 'A1' }), station(2, { name: 'A2' })];
    render(<SplitLineHereDialog />);
    // Shot 32 is the line's second: it runs from station 1 to station 2.
    expect(screen.getByTestId('split-here-summary')).toHaveTextContent('(40%) along shot A1 \u2192 A2');
    fireEvent.change(screen.getByTestId('split-here-name-b'), { target: { value: 'Tail' } });
    fireEvent.click(screen.getByTestId('split-here-confirm'));
    await waitFor(() => expect(splitLine).toHaveBeenCalled());
    expect(splitLine.mock.calls[0]?.[1]).toEqual({ type: 'shot', shot_id: 32, distance: 4 });
    expect(splitLine.mock.calls[0]?.[2]).toMatchObject({ name_b: 'Tail' });
  });

  it('hides the comment and datum choices on a plain station', () => {
    stations = [station(2, {})];
    render(<SplitLineHereDialog />);
    expect(screen.queryByTestId('split-here-comment-side')).toBeNull();
    expect(screen.queryByTestId('split-here-datum-side')).toBeNull();
  });

  it('offers and sends the comment and datum choices when the station has them', async () => {
    stations = [station(2, { comment: 'restriction', is_zero_datum: true })];
    render(<SplitLineHereDialog />);
    expect(screen.getByTestId('split-here-comment-side-both')).toBeInTheDocument();
    expect(screen.queryByTestId('split-here-datum-side-both')).toBeNull();
    fireEvent.click(screen.getByTestId('split-here-comment-side-both'));
    fireEvent.click(screen.getByTestId('split-here-datum-side-b'));
    fireEvent.click(screen.getByTestId('split-here-confirm'));
    await waitFor(() => expect(splitLine).toHaveBeenCalled());
    expect(splitLine.mock.calls[0]?.[2]).toMatchObject({ comment_side: 'both', datum_side: 'b' });
  });

  it('hides the choices for a mid-shot cut, which makes a fresh station', () => {
    stations = [station(1, { comment: 'x', is_zero_datum: true }), station(2, { comment: 'y' })];
    dialogData = { lineId: 3, at: { type: 'shot', shot_id: 32, distance: 5 } };
    render(<SplitLineHereDialog />);
    expect(screen.queryByTestId('split-here-comment-side')).toBeNull();
    expect(screen.queryByTestId('split-here-datum-side')).toBeNull();
  });

  it('refuses to split in a read-only view', () => {
    blocked = true;
    render(<SplitLineHereDialog />);
    fireEvent.click(screen.getByTestId('split-here-confirm'));
    expect(splitLine).not.toHaveBeenCalled();
    expect(screen.getByTestId('split-here-error')).toHaveTextContent('Editing is disabled');
  });

  it('warns, before the split, that a loop closure across the cut will move a half', async () => {
    previewSplitLine.mockResolvedValue({ ...NO_MOVE, closures_to_merges: 1, moved_side: 'b', moved_m: 0.5 });
    render(<SplitLineHereDialog />);
    const warning = await screen.findByTestId('split-here-closure-warning');
    expect(warning).toHaveTextContent(
      'Main B is tied to Main A only by a loop closure, so the split turns it into a merge and moves Main B'
    );
    expect(warning).toHaveTextContent('0.50');
    expect(previewSplitLine).toHaveBeenCalledWith(
      3,
      { type: 'station', station_index: 2 },
      expect.objectContaining({ datum_side: 'a' })
    );
    // Still the user's call: the split goes ahead.
    expect(screen.getByTestId('split-here-confirm')).toBeEnabled();
  });

  it('names the first half and counts the closures when it is the one that moves', async () => {
    previewSplitLine.mockResolvedValue({ ...NO_MOVE, closures_to_merges: 2, moved_side: 'a', moved_m: 1.25 });
    render(<SplitLineHereDialog />);
    expect(await screen.findByTestId('split-here-closure-warning')).toHaveTextContent(
      'Main A is tied to Main B only by 2 loop closures, so the split turns them into merges and moves Main A'
    );
  });

  it('keeps the one-closure wording when the split removes no closure', async () => {
    previewSplitLine.mockResolvedValue({ ...NO_MOVE, closures_to_merges: 1, moved_side: 'b', moved_m: 0.1 });
    render(<SplitLineHereDialog />);
    const warning = await screen.findByTestId('split-here-closure-warning');
    expect(screen.getByTestId('split-here-closure-move')).toHaveTextContent(
      'Main B is tied to Main A only by a loop closure, so the split turns it into a merge and moves Main B 0.10m to close it.'
    );
    expect(screen.queryAllByTestId(/^split-here-closure-removed-/)).toHaveLength(0);
    expect(warning).toBeInTheDocument();
  });

  it('names each closure the split removes by the stations it was created with (#738)', async () => {
    namedClosureLines();
    // Two closures tie Main B's end to the one point Main:1 / 1:0: one becomes the
    // merge, the other would tie that point to itself.
    previewSplitLine.mockResolvedValue({
      ...NO_MOVE,
      closures_to_merges: 1,
      moved_side: 'b',
      moved_m: 0.1,
      removed_closures: [{ closure_id: 2n, station_a: ref(1, 0), station_b: ref(3, 3) }],
    });
    render(<SplitLineHereDialog />);
    await screen.findByTestId('split-here-closure-warning');
    expect(screen.getByTestId('split-here-closure-move')).toHaveTextContent(
      'Main B is tied to Main A only by loop closures, so the split turns one of them into a merge and moves Main B 0.10m to close it.'
    );
    expect(screen.getAllByTestId(/^split-here-closure-removed-/)).toHaveLength(1);
    expect(screen.getByTestId('split-here-closure-removed-0')).toHaveTextContent(
      'Loop closure Inlet / 0 to Main / M3 is removed: its two stations become one point.'
    );
    expect(screen.getByTestId('split-here-confirm')).toBeEnabled();
  });

  it('lists every removed closure and counts the merges when there are several', async () => {
    namedClosureLines();
    previewSplitLine.mockResolvedValue({
      ...NO_MOVE,
      closures_to_merges: 2,
      moved_side: 'a',
      moved_m: 1.25,
      removed_closures: [
        { closure_id: 4n, station_a: ref(3, 3), station_b: ref(5, 0) },
        { closure_id: 6n, station_a: ref(6, 0), station_b: ref(3, 3) },
      ],
    });
    render(<SplitLineHereDialog />);
    await screen.findByTestId('split-here-closure-warning');
    expect(screen.getByTestId('split-here-closure-move')).toHaveTextContent(
      'Main A is tied to Main B only by loop closures, so the split turns 2 of them into merges and moves Main A'
    );
    expect(screen.getAllByTestId(/^split-here-closure-removed-/).map((p) => p.textContent)).toEqual([
      'Loop closure Main / M3 to Side / 0 is removed: its two stations become one point.',
      'Loop closure Sump / 0 to Main / M3 is removed: its two stations become one point.',
    ]);
  });

  it('lists a closure that already tied a point to itself apart, without changing the move wording', async () => {
    namedClosureLines();
    // An undo can restore such a closure from a history recorded before #738; the
    // split's sweep removes it too, but it tied nothing, so the halves are still
    // tied "only by a loop closure".
    previewSplitLine.mockResolvedValue({
      ...NO_MOVE,
      closures_to_merges: 1,
      moved_side: 'b',
      moved_m: 0.1,
      removed_already_one_point: [{ closure_id: 7n, station_a: ref(5, 1), station_b: ref(6, 0) }],
    });
    render(<SplitLineHereDialog />);
    await screen.findByTestId('split-here-closure-warning');
    expect(screen.getByTestId('split-here-closure-move')).toHaveTextContent(
      'Main B is tied to Main A only by a loop closure, so the split turns it into a merge and moves Main B 0.10m to close it.'
    );
    expect(screen.queryAllByTestId(/^split-here-closure-removed-/)).toHaveLength(0);
    expect(screen.getByTestId('split-here-closure-already-0')).toHaveTextContent(
      'Loop closure Side / 1 to Sump / 0 is removed: its two stations are already one point.'
    );
  });

  it('keeps one line per closure when a file repeats a closure id', async () => {
    namedClosureLines();
    previewSplitLine.mockResolvedValue({
      ...NO_MOVE,
      closures_to_merges: 1,
      moved_side: 'b',
      moved_m: 0.1,
      removed_closures: [
        { closure_id: 4n, station_a: ref(1, 0), station_b: ref(3, 3) },
        { closure_id: 4n, station_a: ref(2, 0), station_b: ref(3, 3) },
      ],
      removed_already_one_point: [{ closure_id: 4n, station_a: ref(5, 1), station_b: ref(6, 0) }],
    });
    render(<SplitLineHereDialog />);
    await screen.findByTestId('split-here-closure-warning');
    expect(screen.getAllByTestId(/^split-here-closure-removed-/).map((p) => p.textContent)).toEqual([
      'Loop closure Inlet / 0 to Main / M3 is removed: its two stations become one point.',
      'Loop closure Upper / 0 to Main / M3 is removed: its two stations become one point.',
    ]);
    expect(screen.getAllByTestId(/^split-here-closure-already-/)).toHaveLength(1);
  });

  it('says a closure is removed even when nothing moves', async () => {
    namedClosureLines();
    previewSplitLine.mockResolvedValue({
      ...NO_MOVE,
      closures_to_merges: 1,
      removed_closures: [{ closure_id: 2n, station_a: ref(1, 0), station_b: ref(3, 3) }],
    });
    render(<SplitLineHereDialog />);
    await screen.findByTestId('split-here-closure-warning');
    expect(screen.queryByTestId('split-here-closure-move')).toBeNull();
    expect(screen.getByTestId('split-here-closure-removed-0')).toHaveTextContent(
      'Loop closure Inlet / 0 to Main / M3 is removed: its two stations become one point.'
    );
  });

  it('shows no warning when nothing moves, or the preview is refused', async () => {
    render(<SplitLineHereDialog />);
    await waitFor(() => expect(previewSplitLine).toHaveBeenCalled());
    expect(screen.queryByTestId('split-here-closure-warning')).toBeNull();

    previewSplitLine.mockRejectedValue('Station 2 is the first or last station');
    render(<SplitLineHereDialog />);
    await waitFor(() => expect(previewSplitLine).toHaveBeenCalledTimes(2));
    expect(screen.queryByTestId('split-here-closure-warning')).toBeNull();
  });

  it('asks again when the zero datum changes halves, which decides the half that moves', async () => {
    stations = [station(2, { is_zero_datum: true })];
    render(<SplitLineHereDialog />);
    await waitFor(() => expect(previewSplitLine).toHaveBeenCalledTimes(1));
    fireEvent.click(screen.getByTestId('split-here-datum-side-b'));
    await waitFor(() =>
      expect(previewSplitLine).toHaveBeenLastCalledWith(
        3,
        { type: 'station', station_index: 2 },
        expect.objectContaining({ datum_side: 'b' })
      )
    );
  });
});
