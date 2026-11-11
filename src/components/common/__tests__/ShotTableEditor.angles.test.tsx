/**
 * Editing the displayed average must OFFSET both legs, not flatten them.
 *
 * The editor used to set azimuth_in and azimuth_out to the typed value outright.
 * That was invisible while the average was the only azimuth column on screen -
 * and it silently destroyed the reciprocal spread, which is the whole reason a
 * shot has two legs. Now that the eye menu can show the legs beside the average,
 * a flatten would be plainly wrong on screen, so this pins the data table's rule:
 * both legs move by (new mean - old mean).
 *
 * `.tsx` so vitest runs it under jsdom. The grid is virtualized and jsdom gives
 * the virtualizer no layout to measure, so without this mock it renders zero rows
 * and every cell query fails - the same shim the grid's own component tests use.
 */
import { describe, it, expect, vi, beforeAll } from 'vitest';

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
    scrollToIndex: () => {},
  }),
}));

import { render, screen, fireEvent } from '@testing-library/react';
import { ShotTableEditor } from '../ShotTableEditor';
import type { EditableShot } from '../editableShot';
import { useProjectStore } from '../../../store/projectStore';

const shot = (over: Partial<EditableShot> = {}): EditableShot => ({
  id: 1,
  length: 10,
  // Mean 100, legs 20 apart. A flatten would put both at the typed value; an
  // offset keeps them 20 apart around the new mean.
  azimuth_in: 90,
  azimuth_out: 110,
  depth_in: -1,
  depth_out: -2,
  pitch_in: 4,
  pitch_out: 6,
  left: null,
  right: null,
  up: null,
  down: null,
  comment: null,
  kind: 'Normal',
  ...over,
});

beforeAll(() => {
  // jsdom has no ResizeObserver; the editor measures its grid box with one.
  if (typeof ResizeObserver === 'undefined') {
    (globalThis as { ResizeObserver?: unknown }).ResizeObserver = class {
      observe() {}
      unobserve() {}
      disconnect() {}
    };
  }
});

/** Type a value into a cell and commit it with Enter. */
function editCell(testId: string, value: string) {
  const cell = screen.getByTestId(testId);
  fireEvent.doubleClick(cell);
  const editor = screen.getByTestId('shot-editor-cell-editor');
  fireEvent.change(editor, { target: { value } });
  fireEvent.keyDown(editor, { key: 'Enter' });
}

describe('ShotTableEditor averaged-angle edits', () => {
  it('offsets both azimuth legs by the delta, preserving their spread', () => {
    const onShotChange = vi.fn();
    render(
      <ShotTableEditor
        shots={[shot()]}
        onShotChange={onShotChange}
        onAddShot={vi.fn()}
        onDeleteShot={vi.fn()}
      />
    );
    editCell('shot-cell-0-_azimuth_avg', '200');

    const written = Object.fromEntries(
      onShotChange.mock.calls.map(([, field, value]) => [field, value])
    );
    // Mean was 100, so +100 to each leg - NOT both set to 200.
    expect(written.azimuth_in).toBeCloseTo(190);
    expect(written.azimuth_out).toBeCloseTo(210);
  });

  it('offsets both inclination legs the same way', () => {
    const onShotChange = vi.fn();
    render(
      <ShotTableEditor
        shots={[shot()]}
        onShotChange={onShotChange}
        onAddShot={vi.fn()}
        onDeleteShot={vi.fn()}
      />
    );
    // The inclination average ships hidden, so reveal it the way a user would.
    fireEvent.click(screen.getByTestId('shot-editor-columns-menu'));
    fireEvent.click(screen.getByTestId('shot-editor-col-inclination-avg'));
    editCell('shot-cell-0-_pitch_avg', '15');

    const written = Object.fromEntries(
      onShotChange.mock.calls.map(([, field, value]) => [field, value])
    );
    // Mean was 5, so +10 to each leg.
    expect(written.pitch_in).toBeCloseTo(14);
    expect(written.pitch_out).toBeCloseTo(16);
  });

  it('writes the typed value to BOTH legs while they are unset, not 0', () => {
    // A row added and not yet measured has NaN legs. Rotating by a delta against
    // NaN yields NaN, which normalizeAngle folds to 0 - recording due north as a
    // bearing nobody took, and one that then passes range validation and saves.
    const onShotChange = vi.fn();
    render(
      <ShotTableEditor
        shots={[shot({ azimuth_in: NaN, azimuth_out: NaN })]}
        onShotChange={onShotChange}
        onAddShot={vi.fn()}
        onDeleteShot={vi.fn()}
      />
    );
    editCell('shot-cell-0-_azimuth_avg', '45');
    const written = Object.fromEntries(
      onShotChange.mock.calls.map(([, field, value]) => [field, value])
    );
    expect(written.azimuth_in).toBeCloseTo(45);
    expect(written.azimuth_out).toBeCloseTo(45);
  });

  it('lands an azimuth typed into the trailing blank row on the legs', () => {
    // The blank row is materialized by onAddShot during the edit, so its index is
    // one past the `shots` this render was given. Falling through here wrote the
    // VIRTUAL key onto the shot and dropped the value - on the primary add path.
    const onShotChange = vi.fn();
    render(
      <ShotTableEditor
        shots={[]}
        onShotChange={onShotChange}
        onAddShot={vi.fn()}
        onDeleteShot={vi.fn()}
      />
    );
    editCell('shot-cell-0-_azimuth_avg', '30');
    const fields = onShotChange.mock.calls.map(([, field]) => field);
    expect(fields).toContain('azimuth_in');
    expect(fields).toContain('azimuth_out');
    expect(fields).not.toContain('_azimuth_avg');
  });

  it('opens with the legs and the inclination family hidden', () => {
    render(
      <ShotTableEditor
        shots={[shot()]}
        onShotChange={vi.fn()}
        onAddShot={vi.fn()}
        onDeleteShot={vi.fn()}
      />
    );
    expect(screen.getByTestId('shot-cell-0-_azimuth_avg')).toBeInTheDocument();
    for (const key of ['azimuth_in', 'azimuth_out', '_pitch_avg', 'pitch_in', 'pitch_out']) {
      expect(screen.queryByTestId(`shot-cell-0-${key}`)).toBeNull();
    }
  });
});

/**
 * The warning column was showing nothing where the data table showed warnings:
 * it only ever ran the editor's own two threshold checks, never the project's
 * analysis. Passing a sectionId opts a host into that shared source.
 */
describe('ShotTableEditor warning column', () => {
  const warning = (over: Record<string, unknown> = {}) => ({
    sectionId: 7,
    shotId: 1,
    shotIndex: 0,
    code: 'SHORT_SHOT',
    message: 'Shot 7:1 is suspiciously short',
    severity: 'warning',
    isResolved: false,
    ...over,
  });

  const renderEditor = (props: Record<string, unknown> = {}) =>
    render(
      <ShotTableEditor
        shots={[shot()]}
        onShotChange={vi.fn()}
        onAddShot={vi.fn()}
        onDeleteShot={vi.fn()}
        {...props}
      />
    );

  it('shows the project warning for a shot once a sectionId is given', () => {
    useProjectStore.setState({ warnings: [warning()] as never });
    renderEditor({ sectionId: 7 });
    expect(screen.getByTestId('shot-warning-0').getAttribute('title')).toContain(
      'suspiciously short'
    );
  });

  it('shows nothing from the project without a sectionId to look up', () => {
    useProjectStore.setState({ warnings: [warning()] as never });
    renderEditor();
    expect(screen.queryByTestId('shot-warning-0')).toBeNull();
  });

  it('reds the icon for an error-severity warning', () => {
    useProjectStore.setState({ warnings: [warning({ severity: 'error' })] as never });
    renderEditor({ sectionId: 7 });
    expect(screen.getByTestId('shot-warning-0').className).toContain('text-error');
  });

  it('ignores a resolved warning, and one for another shot or section', () => {
    useProjectStore.setState({
      warnings: [
        warning({ isResolved: true }),
        warning({ shotId: 999 }),
        warning({ sectionId: 8 }),
      ] as never,
    });
    renderEditor({ sectionId: 7 });
    expect(screen.queryByTestId('shot-warning-0')).toBeNull();
  });

  it('does not repeat an azimuth discrepancy the project already reports', () => {
    useProjectStore.setState({
      warnings: [warning({ code: 'AZIMUTH_DEVIATION', message: 'backend azimuth text' })] as never,
    });
    // Legs 180 apart trip the editor's own azimuth check too.
    render(
      <ShotTableEditor
        shots={[shot({ azimuth_in: 10, azimuth_out: 190 })]}
        onShotChange={vi.fn()}
        onAddShot={vi.fn()}
        onDeleteShot={vi.fn()}
        sectionId={7}
      />
    );
    const title = screen.getByTestId('shot-warning-0').getAttribute('title') ?? '';
    expect(title).toContain('backend azimuth text');
    expect(title).not.toContain('Azimuth in/out differs');
  });

  it('does not repeat an inclination discrepancy the project already reports', () => {
    useProjectStore.setState({
      warnings: [warning({ code: 'PITCH_DEVIATION', message: 'backend inclination text' })] as never,
    });
    // Legs 30 apart trip the editor's own inclination check too.
    render(
      <ShotTableEditor
        shots={[shot({ pitch_in: -10, pitch_out: 20 })]}
        onShotChange={vi.fn()}
        onAddShot={vi.fn()}
        onDeleteShot={vi.fn()}
        sectionId={7}
      />
    );
    const title = screen.getByTestId('shot-warning-0').getAttribute('title') ?? '';
    expect(title).toContain('backend inclination text');
    expect(title).not.toContain('Pitch in/out differs');
  });

  it('still runs its own checks for a row the project has never seen', () => {
    useProjectStore.setState({ warnings: [] as never });
    render(
      <ShotTableEditor
        shots={[shot({ isNew: true, azimuth_in: 10, azimuth_out: 190 })]}
        onShotChange={vi.fn()}
        onAddShot={vi.fn()}
        onDeleteShot={vi.fn()}
        sectionId={7}
      />
    );
    expect(screen.getByTestId('shot-warning-0').getAttribute('title')).toContain(
      'Azimuth in/out differs'
    );
  });
});

/**
 * A shot with exactly one measured azimuth leg is valid: the completeness rules
 * allow it and Save accepts it. The average column computed a circular mean
 * against NaN, which is NaN, so that real bearing rendered as an empty cell -
 * indistinguishable from having no azimuth at all.
 */
describe('ShotTableEditor one-legged azimuth display', () => {
  const renderOne = (over: Partial<EditableShot>) =>
    render(
      <ShotTableEditor
        shots={[shot(over)]}
        onShotChange={vi.fn()}
        onAddShot={vi.fn()}
        onDeleteShot={vi.fn()}
      />
    );

  it('shows the measured leg as the average when the other is unset', () => {
    renderOne({ azimuth_in: 42, azimuth_out: NaN });
    expect(screen.getByTestId('shot-cell-0-_azimuth_avg').textContent).toContain('42');
  });

  it('shows it for the out leg too', () => {
    renderOne({ azimuth_in: NaN, azimuth_out: 137 });
    expect(screen.getByTestId('shot-cell-0-_azimuth_avg').textContent).toContain('137');
  });

  it('still renders blank when neither leg was measured', () => {
    // Blank is right here - and must NOT become a confident 0.0.
    renderOne({ azimuth_in: NaN, azimuth_out: NaN });
    expect(screen.getByTestId('shot-cell-0-_azimuth_avg').textContent?.trim()).toBe('');
  });

  it('still averages when both legs are measured', () => {
    renderOne({ azimuth_in: 90, azimuth_out: 110 });
    expect(screen.getByTestId('shot-cell-0-_azimuth_avg').textContent).toContain('100');
  });
});

/**
 * `onAddShots?.(n) ?? onAddShot()` ran BOTH callbacks: a void return is
 * `undefined`, which `??` reads as "nothing happened". With only the phantom
 * row present, one requested row became two - and in hosts allocating ids from
 * captured state the extra row could duplicate a temp key.
 */
describe('ShotTableEditor insert into an empty table', () => {
  const insertBelow = () => {
    fireEvent.contextMenu(screen.getByTestId('shot-cell-0-length'));
    fireEvent.click(screen.getByTestId('shot-editor-context-insert-below'));
  };

  it('uses the bulk callback alone, without also adding a single row', () => {
    const onAddShots = vi.fn();
    const onAddShot = vi.fn();
    render(
      <ShotTableEditor
        shots={[]}
        onShotChange={vi.fn()}
        onAddShot={onAddShot}
        onAddShots={onAddShots}
        onInsertShots={vi.fn()}
        onDeleteShot={vi.fn()}
      />
    );
    insertBelow();
    expect(onAddShots).toHaveBeenCalledTimes(1);
    expect(onAddShots).toHaveBeenCalledWith(1);
    expect(onAddShot).not.toHaveBeenCalled();
  });

  it('still falls back to the single add when there is no bulk callback', () => {
    const onAddShot = vi.fn();
    render(
      <ShotTableEditor
        shots={[]}
        onShotChange={vi.fn()}
        onAddShot={onAddShot}
        onInsertShots={vi.fn()}
        onDeleteShot={vi.fn()}
      />
    );
    insertBelow();
    expect(onAddShot).toHaveBeenCalledTimes(1);
  });
});
