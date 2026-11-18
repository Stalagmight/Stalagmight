/**
 * #357 item 9 (second half) - a line's colour is per-project user data.
 * Recolouring an already-decided line when the theme changes is explicitly
 * rejected on the issue: a survey must keep the colour it was created with.
 * `ImportDialog` restores `savedFormState.color` when reopening after a
 * placement cancel, and that restore path must win over the theme-aware
 * seed - a line a user already picked (or was seeded on a previous
 * render) must not be silently recoloured just because `resolvedTheme`
 * changed underneath it.
 *
 * `.tsx` so vitest runs it under jsdom.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, fireEvent } from '@testing-library/react';

// jsdom measures every box as 0 tall, so the real virtualizer renders no rows
// at all. Same stand-in the SpreadsheetGrid suites use: every row is visible.
vi.mock('@tanstack/react-virtual', () => ({
  useVirtualizer: ({ count }: { count: number }) => ({
    getVirtualItems: () =>
      Array.from({ length: count }, (_, index) => ({
        index,
        key: index,
        start: 33 + index * 33,
        size: 33,
      })),
    getTotalSize: () => count * 33,
    scrollToIndex: vi.fn(),
  }),
}));

vi.mock('../../../utils/telemetry', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../utils/telemetry')>()),
  trackDialogComplete: vi.fn(),
  setSource: vi.fn(),
}));

import '../../../i18n';
import ImportDialog from '../ImportDialog';
import { useUIStore } from '../../../store/uiStore';
import { useProjectStore } from '../../../store/projectStore';
import { useSpeleoStore } from '../../../store/speleoStore';
import { LINE_COLORS, LIGHT_LINE_COLORS } from '../../../utils/colors';
import type { ShotData } from '../../../types/project';
import type { ShotDraft } from '../../SingleSystemView/dataTableDrafts';

// DialogWrapper renders via a portal, so the input lives on document.body
// rather than under the RTL container.
function getColorInputValue(): string {
  const input = document.body.querySelector('input[type="color"]') as HTMLInputElement | null;
  if (!input) throw new Error('color input not found');
  return input.value;
}

function byTestId(id: string): HTMLElement {
  const el = document.body.querySelector(`[data-testid="${id}"]`);
  if (!el) throw new Error(`${id} not found`);
  return el as HTMLElement;
}

const draft = (key: number, data: Partial<ShotData>): ShotDraft => ({ key, anchor: 'end', data });

/** Open the dialog with a given set of grid rows, via the placement-cancel
 *  restore path (the only way to seed rows without driving the grid). */
function renderWithDrafts(drafts: ShotDraft[]) {
  useUIStore.setState({
    dialogData: {
      savedFormState: {
        name: 'Line 1',
        people: { surveyors: [], explorers: [] },
        date: '',
        color: LINE_COLORS[0]!,
        drafts,
        nextKey: drafts.length + 1,
      },
    },
    resolvedTheme: 'dark',
  });
  render(<ImportDialog />);
}

beforeEach(() => {
  useUIStore.setState({
    dialogData: {},
    resolvedTheme: 'dark',
    appMode: 'survey',
    geometryMode: 'measured',
  });
  useProjectStore.setState({ project: null, config: null });
  // useEditingBlocked() fails CLOSED on an unestablished project (#270), and the
  // store's default `projectLinkKind` is 'unknown' - so without this the grid
  // and the Create button render read-only.
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
});

describe('ImportDialog color seeding (#357 item 9)', () => {
  it('seeds a brand-new line from the theme-appropriate palette', () => {
    useUIStore.setState({ dialogData: {}, resolvedTheme: 'light' });
    render(<ImportDialog />);
    expect(getColorInputValue().toLowerCase()).toBe(LIGHT_LINE_COLORS[0]!.toLowerCase());
  });

  it('does NOT rewrite a restored color when the theme differs from the one it was seeded under', () => {
    // Simulates reopening the dialog (after a placement cancel) under a
    // different theme than the one active when this color was picked: a
    // dark-palette color, restored while `resolvedTheme` is now 'light'.
    const persistedColor = LINE_COLORS[0]!;
    useUIStore.setState({
      dialogData: {
        savedFormState: {
          name: '',
          people: { surveyors: [], explorers: [] },
          date: '',
          color: persistedColor,
          drafts: [],
          nextKey: 0,
        },
      },
      resolvedTheme: 'light',
    });
    render(<ImportDialog />);
    const value = getColorInputValue().toLowerCase();
    expect(value).toBe(persistedColor.toLowerCase());
    // The important negative: it must NOT have been reseeded from the light
    // canvas palette just because the theme is now light.
    expect(LIGHT_LINE_COLORS.map((c) => c.toLowerCase())).not.toContain(value);
  });
});

describe('ImportDialog create gate', () => {
  const complete = { length: 10, azimuth_in: 90, azimuth_out: 90, depth_in: 0, depth_out: 1 };

  it('disables Create on a fresh dialog with nothing typed', () => {
    useUIStore.setState({ dialogData: {}, resolvedTheme: 'dark' });
    render(<ImportDialog />);
    expect(byTestId('import-line-create')).toBeDisabled();
    expect(byTestId('import-line-blocker')).toHaveTextContent(
      'Enter at least one shot to create a line.'
    );
  });

  it('disables Create while a row is partially filled, naming the row', () => {
    // Row 2 has a length but no depths or azimuth. The old dialog would have
    // created the line anyway and silently dropped nothing (length > 0), so
    // this is the regression that gate exists for.
    renderWithDrafts([draft(1, complete), draft(2, { length: 5 })]);
    expect(byTestId('import-line-create')).toBeDisabled();
    expect(byTestId('import-line-blocker')).toHaveTextContent(/Row 2 is missing required values/);
  });

  it('ignores a wholly blank row and enables Create', () => {
    renderWithDrafts([draft(1, complete), draft(2, {})]);
    expect(byTestId('import-line-create')).toBeEnabled();
    expect(byTestId('import-line-blocker')).toHaveTextContent('');
  });

  it('pluralizes when several rows are incomplete', () => {
    renderWithDrafts([draft(1, { length: 5 }), draft(2, complete), draft(3, { depth_in: 2 })]);
    expect(byTestId('import-line-blocker')).toHaveTextContent(
      /Rows 1, 3 are missing required values/
    );
  });

  /**
   * A row can hold keys and still be empty: clearing an optional cell writes
   * `comment: null` / `pitch_in: null` rather than deleting the field. Judging
   * the trailing row by key count painted its required cells red with no blocker
   * text to explain them.
   */
  it('marks nothing on a trailing row holding only cleared optionals', () => {
    renderWithDrafts([
      draft(1, complete),
      draft(2, { comment: null, pitch_in: null, pitch_out: null, kind: 'Normal' }),
    ]);
    expect(document.body.querySelectorAll('.sg-cell-missing')).toHaveLength(0);
    expect(byTestId('import-line-create')).toBeEnabled();
    expect(byTestId('import-line-blocker')).toHaveTextContent('');
  });
});

describe('ImportDialog J/R cell', () => {
  const complete = { length: 10, azimuth_in: 90, azimuth_out: 90, depth_in: 0, depth_out: 1 };

  const rowCount = () => document.body.querySelectorAll('[data-testid^="new-line-row-"]').length;
  const cycleKind = (rowIndex: number) => {
    const cell = document.body.querySelector(
      `[data-testid="new-line-cell-${rowIndex}-kind"] [data-testid="shot-kind-cell"]`
    );
    if (!cell) throw new Error(`kind cell for row ${rowIndex} not found`);
    fireEvent.click(cell);
  };

  /** Cycling the kind used to write `drafts` around the edit funnel, so it
   *  skipped the "keep a blank row at the end" invariant: the invitation row was
   *  consumed by the click and there was nowhere left to type the next shot. */
  it('leaves a fresh blank row below the row it tagged', () => {
    renderWithDrafts([draft(1, complete), draft(2, {})]);
    expect(rowCount()).toBe(2);
    cycleKind(1);
    expect(rowCount()).toBe(3);
    expect(byTestId('import-line-blocker')).toHaveTextContent(
      /Row 2 is missing required values/
    );
  });

  it('cycles the whole ring back to blank', () => {
    renderWithDrafts([draft(1, complete), draft(2, {})]);
    cycleKind(1); // Jump
    cycleKind(1); // Reference
    cycleKind(1); // back to Normal - the row holds nothing again
    expect(byTestId('import-line-create')).toBeEnabled();
    expect(byTestId('import-line-blocker')).toHaveTextContent('');
  });
});

describe('ImportDialog line name (required, unique in the project)', () => {
  const complete = { length: 10, azimuth_in: 90, azimuth_out: 90, depth_in: 0, depth_out: 1 };
  const withLines = (...names: string[]) =>
    useProjectStore.setState({
      project: { lines: names.map((name, i) => ({ id: i + 1, name, color: '#123456', shots: [] })) } as never,
    });
  const nameInput = () => byTestId('line-name-input') as HTMLInputElement;
  const queryTestId = (id: string) => document.body.querySelector(`[data-testid="${id}"]`);
  // The cases below stub store actions; the next case gets the real ones back.
  const { openDialog, startPlacementMode } = useUIStore.getState();
  const { createLine } = useProjectStore.getState();
  afterEach(() => {
    useUIStore.setState({ openDialog, startPlacementMode });
    useProjectStore.setState({ createLine });
  });

  it('starts on the first free "Line N"', () => {
    withLines('Line 1', 'Main', 'Line 3');
    render(<ImportDialog />);
    expect(nameInput()).toHaveValue('Line 2');
    expect(queryTestId('line-name-error')).toBeNull();
  });

  it('keeps the name a restored form had', () => {
    withLines('Line 1');
    renderWithDrafts([draft(1, complete)]);
    expect(nameInput()).toHaveValue('Line 1');
  });

  it('says a name is needed and disables Create while it is empty', () => {
    renderWithDrafts([draft(1, complete)]);
    fireEvent.change(nameInput(), { target: { value: '   ' } });
    expect(byTestId('line-name-error')).toHaveTextContent('A line needs a name.');
    expect(nameInput()).toHaveAttribute('aria-invalid', 'true');
    expect(byTestId('import-line-create')).toBeDisabled();
    expect(byTestId('import-line-create')).toHaveAttribute('title', 'A line needs a name.');
  });

  it("refuses another line's name, trimmed, and lets Create go once it is free", () => {
    withLines('Main', 'Side');
    renderWithDrafts([draft(1, complete)]);
    fireEvent.change(nameInput(), { target: { value: ' Main ' } });
    const taken = 'A line named "Main" already exists. Line names must be unique in the project.';
    expect(byTestId('line-name-error')).toHaveTextContent(taken);
    expect(byTestId('import-line-create')).toBeDisabled();
    expect(byTestId('import-line-create')).toHaveAttribute('title', taken);
    fireEvent.change(nameInput(), { target: { value: 'Main 2' } });
    expect(queryTestId('line-name-error')).toBeNull();
    expect(byTestId('import-line-create')).toBeEnabled();
  });

  it('sends the trimmed name', () => {
    let placed: { name: string | null } | null = null;
    useUIStore.setState({
      startPlacementMode: ((data: { name: string | null }) => {
        placed = data;
      }) as never,
    });
    renderWithDrafts([draft(1, complete)]);
    fireEvent.change(nameInput(), { target: { value: '  Upstream  ' } });
    fireEvent.click(byTestId('import-line-create'));
    expect(placed).toMatchObject({ name: 'Upstream' });
  });

  it('reopens the form with the refusal when the backend refuses the name after placement', async () => {
    const refusal = 'A line named "Line 1" already exists';
    let complete_: ((result: unknown) => Promise<void>) | undefined;
    const openDialog = vi.fn();
    useUIStore.setState({
      openDialog: openDialog as never,
      startPlacementMode: ((_data: unknown, _align: number, onComplete: (r: unknown) => Promise<void>) => {
        complete_ = onComplete;
      }) as never,
    });
    const createLine = vi.fn(async () => ({ success: false, error: refusal }));
    useProjectStore.setState({ createLine: createLine as never });
    renderWithDrafts([draft(1, complete)]);
    fireEvent.click(byTestId('import-line-create'));
    await complete_?.({ position: { x: 0, y: 0 }, snappedToStation: null });
    expect(createLine).toHaveBeenCalled();
    expect(openDialog).toHaveBeenCalledWith('import', {
      savedFormState: expect.objectContaining({ name: 'Line 1' }),
      submitError: refusal,
    });
  });

  it('shows a refusal it was reopened with at the name, until the name changes', () => {
    useUIStore.setState({
      dialogData: {
        savedFormState: {
          name: 'Line 1',
          people: { surveyors: [], explorers: [] },
          date: '',
          color: LINE_COLORS[0]!,
          drafts: [draft(1, complete)],
          nextKey: 2,
        },
        submitError: 'A line named "Line 1" already exists',
      },
    });
    render(<ImportDialog />);
    expect(byTestId('line-name-error')).toHaveTextContent('A line named "Line 1" already exists');
    expect(byTestId('import-line-create')).toBeDisabled();
    fireEvent.change(nameInput(), { target: { value: 'Line 9' } });
    expect(queryTestId('line-name-error')).toBeNull();
    expect(byTestId('import-line-create')).toBeEnabled();
  });
});
