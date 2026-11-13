/**
 * The trailing blank row is an invitation, not a shot - and every path that can
 * turn it into one has to agree about when that happens.
 *
 * Three did not. Delete on its comment cell materialized a blank shot (the
 * comment column parses '' to a real null, so the clear looked like an edit); a
 * typed `j` on its kind cell made a shot whose only content was "Jump", which
 * then blocked Save, while the same column's click and Space deliberately do
 * nothing there; and the row menu counted it as a selectable row, so a gutter
 * drag that ended on it said "3 shots", inserted three and deleted two.
 *
 * `.tsx` for jsdom, with the virtualizer shimmed - the grid measures nothing
 * under jsdom and would render zero rows. Same shim as the sibling suites.
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

const shot = (over: Partial<EditableShot> = {}): EditableShot => ({
  id: 1,
  length: 10,
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
  if (typeof ResizeObserver === 'undefined') {
    (globalThis as { ResizeObserver?: unknown }).ResizeObserver = class {
      observe() {}
      unobserve() {}
      disconnect() {}
    };
  }
});

const scroller = () => document.querySelector('.sg-grid [tabindex="0"]')!;

/** Focus a cell the way a click does, then send a key to the grid's scroller. */
function pressOnCell(testId: string, key: string) {
  fireEvent.mouseDown(screen.getByTestId(testId));
  fireEvent.keyDown(scroller(), { key });
}

describe('ShotTableEditor typing on the trailing blank row', () => {
  const renderOne = (over: Record<string, unknown> = {}) => {
    const onShotChange = vi.fn();
    const onAddShot = vi.fn();
    render(
      <ShotTableEditor
        shots={[shot()]}
        onShotChange={onShotChange}
        onAddShot={onAddShot}
        onDeleteShot={vi.fn()}
        {...over}
      />
    );
    return { onShotChange, onAddShot };
  };

  it('ignores a kind character typed into it, as its click and Space do', () => {
    const { onShotChange, onAddShot } = renderOne();
    pressOnCell('shot-editor-cell-1-kind', 'j');
    expect(onAddShot).not.toHaveBeenCalled();
    expect(onShotChange).not.toHaveBeenCalled();
  });

  it('ignores `n` too - a Normal-only row is just as incomplete', () => {
    const { onShotChange, onAddShot } = renderOne();
    pressOnCell('shot-editor-cell-1-kind', 'n');
    expect(onAddShot).not.toHaveBeenCalled();
    expect(onShotChange).not.toHaveBeenCalled();
  });

  it('still sets the kind from a typed character on a real row', () => {
    const { onShotChange } = renderOne();
    pressOnCell('shot-editor-cell-0-kind', 'j');
    expect(onShotChange).toHaveBeenCalledWith(0, 'kind', 'Jump');
  });

  it('still materializes the row for a measurement typed into it', () => {
    const { onAddShot } = renderOne();
    fireEvent.doubleClick(screen.getByTestId('shot-editor-cell-1-length'));
    fireEvent.change(screen.getByTestId('shot-editor-cell-editor'), { target: { value: '12' } });
    fireEvent.keyDown(screen.getByTestId('shot-editor-cell-editor'), { key: 'Enter' });
    expect(onAddShot).toHaveBeenCalledTimes(1);
  });
});

describe('ShotTableEditor clearing the trailing blank row', () => {
  const renderOne = () => {
    const onShotChange = vi.fn();
    const onAddShot = vi.fn();
    render(
      <ShotTableEditor
        shots={[shot()]}
        onShotChange={onShotChange}
        onAddShot={onAddShot}
        onDeleteShot={vi.fn()}
      />
    );
    return { onShotChange, onAddShot };
  };

  it('adds no shot when Delete lands on its comment cell', () => {
    const { onShotChange, onAddShot } = renderOne();
    pressOnCell('shot-editor-cell-1-comment', 'Delete');
    expect(onAddShot).not.toHaveBeenCalled();
    expect(onShotChange).not.toHaveBeenCalled();
  });

  it('adds no shot when Delete lands on its kind cell', () => {
    const { onShotChange, onAddShot } = renderOne();
    pressOnCell('shot-editor-cell-1-kind', 'Delete');
    expect(onAddShot).not.toHaveBeenCalled();
    expect(onShotChange).not.toHaveBeenCalled();
  });

  it('still clears a filled cell on a real row', () => {
    const { onShotChange } = renderOne();
    pressOnCell('shot-editor-cell-0-depth_in', 'Delete');
    // Required, so it clears through the missing route: NaN, never 0.
    expect(onShotChange).toHaveBeenCalledWith(0, 'depth_in', NaN);
  });
});

describe('ShotTableEditor row menu over a selection that swept the blank row', () => {
  const renderTwo = () => {
    const onInsertShots = vi.fn();
    const onDeleteShots = vi.fn();
    render(
      <ShotTableEditor
        shots={[shot({ id: 1 }), shot({ id: 2 })]}
        onShotChange={vi.fn()}
        onAddShot={vi.fn()}
        onAddShots={vi.fn()}
        onInsertShots={onInsertShots}
        onDeleteShot={vi.fn()}
        onDeleteShots={onDeleteShots}
      />
    );
    // Drag the gutter across both shots AND the trailing blank.
    const gutters = document.querySelectorAll('.sg-gutter');
    fireEvent.mouseDown(gutters[0]!);
    fireEvent.mouseDown(gutters[2]!, { shiftKey: true });
    fireEvent.mouseUp(gutters[2]!, { shiftKey: true });
    fireEvent.contextMenu(screen.getByTestId('shot-editor-cell-1-length'));
    return { onInsertShots, onDeleteShots };
  };

  it('names the two real shots, not the three selected rows', () => {
    renderTwo();
    expect(screen.getByTestId('shot-editor-context-delete-rows').textContent).toContain('2 shots');
    expect(screen.getByTestId('shot-editor-context-insert-above').textContent).toContain('2 shots');
  });

  it('inserts the number it named', () => {
    const { onInsertShots } = renderTwo();
    fireEvent.click(screen.getByTestId('shot-editor-context-insert-above'));
    expect(onInsertShots).toHaveBeenCalledWith(0, 2, 'before');
  });

  it('deletes the number it named', () => {
    const { onDeleteShots } = renderTwo();
    fireEvent.click(screen.getByTestId('shot-editor-context-delete-rows'));
    expect(onDeleteShots).toHaveBeenCalledWith([1, 0]);
  });
});

/**
 * `tempId ?? id` drew two identities out of one numeric space. The MNemo wizard
 * hands the editor both kinds at once, so a fresh row's tempId could equal an
 * imported row's id - and the grid keys its missing-cell marks off this value,
 * so clearing a cell on one row blanked the same cell on its twin.
 */
describe('ShotTableEditor row identity', () => {
  it('keeps a tempId row and an id row with the same number apart', () => {
    render(
      <ShotTableEditor
        shots={[shot({ id: 5, length: 10 }), shot({ id: 5, tempId: 5, isNew: true, length: 20 })]}
        onShotChange={vi.fn()}
        onAddShot={vi.fn()}
        onDeleteShot={vi.fn()}
      />
    );
    pressOnCell('shot-editor-cell-0-length', 'Delete');

    const twin = screen.getByTestId('shot-editor-cell-1-length');
    expect(twin.textContent).toContain('20');
    expect(twin.className).not.toContain('sg-cell-missing');
    // The row the user actually cleared still reads as emptied.
    expect(screen.getByTestId('shot-editor-cell-0-length').textContent?.trim()).toBe('');
  });
});
