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

describe('SplitLineHereDialog', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    blocked = false;
    stations = [];
    mainColor = '#123456';
    otherLines = [];
    resolvedTheme = 'dark';
    splitLine.mockResolvedValue({ success: true });
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
    // The halves share the split station (plan 5.4): the dialog says so.
    expect(screen.getByTestId('split-here-shared-station-hint')).toHaveTextContent(
      'The two halves stay joined: the split station becomes one station on both lines'
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

  it('offers no comment or datum side: the halves share the station that holds them', async () => {
    stations = [station(2, { comment: 'restriction', is_zero_datum: true })];
    render(<SplitLineHereDialog />);
    expect(screen.queryByTestId('split-here-comment-side')).toBeNull();
    expect(screen.queryByTestId('split-here-datum-side')).toBeNull();
    fireEvent.click(screen.getByTestId('split-here-confirm'));
    await waitFor(() => expect(splitLine).toHaveBeenCalled());
    expect(splitLine.mock.calls[0]?.[2]).toMatchObject({ comment_side: 'a', datum_side: 'a' });
  });

  it('refuses to split in a read-only view', () => {
    blocked = true;
    render(<SplitLineHereDialog />);
    fireEvent.click(screen.getByTestId('split-here-confirm'));
    expect(splitLine).not.toHaveBeenCalled();
    expect(screen.getByTestId('split-here-error')).toHaveTextContent('Editing is disabled');
  });

  it('warns about no closure: nothing moves and nothing becomes a merge', () => {
    render(<SplitLineHereDialog />);
    expect(screen.queryByTestId('split-here-closure-warning')).toBeNull();
  });
});
