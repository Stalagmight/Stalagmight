/**
 * DRAFT-1 (#576): a draft "new shot" row keeps what the user typed when its
 * create_shot is refused.
 *
 * The auto-commit effect cleared the draft in a `.then` that never looked at
 * `CommandResult.success`, so a refused create_shot (any backend rejection, a
 * read-only gate, or GHOST-2's refusal while a move is carried) threw the row
 * away - the failure notice appeared and the four values typed into it were
 * gone. A failed draft now stays as typed; it is not re-sent on its own (the
 * failure path's refresh would otherwise retry it, and re-toast, on every
 * refresh), and editing the row sends it again.
 *
 * Full-component tier: real DataTable and grid (virtualizer mocked so jsdom
 * renders every row), real stores, `createShot` replaced on the project store.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';

vi.mock('@tanstack/react-virtual', async () =>
  (await import('../../../test/factories/virtualizer')).fakeVirtualizer({ headerOffset: 0 }),
);

vi.mock('../../../utils/ipc');

import DataTable from '../DataTable';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { useSpeleoStore } from '../../../store/speleoStore';
import { defaultConfig } from '../../../types/config';
import type { Config } from '../../../types/config';
import type { CommandResult } from '../../../types/commands';
import type { Section, Shot } from '../../../types/project';
import { invokeIpcMock } from '../../../test/factories/ipc';
import { NoopResizeObserver } from '../../../test/factories/dom';
import { makeSection, makeShot } from '../../../test/factories/survey';
import { actMicrotasks } from '../../../test/factories/act';

const shot = (id: number, length: number): Shot =>
  makeShot({ id, azimuth_in: 10, azimuth_out: 10, length, depth_out: 1 });

const section: Section = makeSection({ name: 'CaveA', shots: [shot(1, 10), shot(2, 20), shot(3, 30)] });

const result = (success: boolean): CommandResult => ({
  success,
  warnings: [],
  error: success ? null : 'refused',
  section_ids: [],
});

/** The end draft sits below the three shots. */
const DRAFT_ROW = 3;

function typeCell(row: number, colKey: string, value: string) {
  fireEvent.doubleClick(screen.getByTestId(`grid-cell-${row}-${colKey}`));
  const editor = screen.getByTestId('grid-cell-editor');
  fireEvent.change(editor, { target: { value } });
  fireEvent.keyDown(editor, { key: 'Enter' });
}

const cellText = (row: number, colKey: string) =>
  screen.getByTestId(`grid-cell-${row}-${colKey}`).textContent?.trim() ?? '';

async function fillDraft() {
  typeCell(DRAFT_ROW, 'length', '5');
  typeCell(DRAFT_ROW, '_azimuth_avg', '100');
  typeCell(DRAFT_ROW, 'depth_in', '1');
  typeCell(DRAFT_ROW, 'depth_out', '2');
  await actMicrotasks(3);
}

let createShot: ReturnType<typeof vi.fn>;

beforeEach(() => {
  vi.stubGlobal('ResizeObserver', NoopResizeObserver);
  invokeIpcMock.mockReset();
  invokeIpcMock.mockImplementation((command: string) =>
    Promise.resolve(command === 'get_config' ? structuredClone(defaultConfig) : undefined)
  );
  createShot = vi.fn();
  useProjectStore.setState({ config: structuredClone(defaultConfig) as Config, createShot });
  useUIStore.setState({ toasts: [], selection: null });
  // An open local project: editable, so the end draft row is shown.
  useSpeleoStore.setState({ projectLinkKind: 'local', link: null });
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('DataTable draft row when create_shot fails (DRAFT-1)', () => {
  it('keeps the typed values in the draft row after a refused create_shot', async () => {
    createShot.mockResolvedValue(result(false));
    render(<DataTable sections={[section]} />);

    await fillDraft();

    expect(createShot).toHaveBeenCalledTimes(1);
    expect(cellText(DRAFT_ROW, 'length')).toBe('5.0');
    expect(cellText(DRAFT_ROW, '_azimuth_avg')).toBe('100.0');
    expect(cellText(DRAFT_ROW, 'depth_in')).toBe('1.0');
    expect(cellText(DRAFT_ROW, 'depth_out')).toBe('2.0');
  });

  it('does not re-send the refused draft by itself, and re-sends it once the row is edited', async () => {
    createShot.mockResolvedValue(result(false));
    const { rerender } = render(<DataTable sections={[section]} />);
    await fillDraft();
    expect(createShot).toHaveBeenCalledTimes(1);

    // A refresh (a new sections array, as after the failure path's
    // refreshProjectState) must not retry it silently.
    rerender(<DataTable sections={[{ ...section, shots: [...section.shots] }]} />);
    await actMicrotasks(3);
    expect(createShot).toHaveBeenCalledTimes(1);

    // Editing the row is the retry; this time the backend accepts it.
    createShot.mockResolvedValue(result(true));
    typeCell(DRAFT_ROW, 'length', '6');
    await actMicrotasks(3);
    expect(createShot).toHaveBeenCalledTimes(2);
    expect(createShot.mock.calls[1]?.[1]).toMatchObject({ length: 6, depth_in: 1, depth_out: 2 });
    // Success clears the row back to a blank end draft, as before.
    expect(cellText(DRAFT_ROW, 'length')).toBe('');
  });

  it('still clears the draft row when create_shot succeeds', async () => {
    createShot.mockResolvedValue(result(true));
    render(<DataTable sections={[section]} />);

    await fillDraft();

    expect(createShot).toHaveBeenCalledTimes(1);
    expect(cellText(DRAFT_ROW, 'length')).toBe('');
    expect(cellText(DRAFT_ROW, 'depth_out')).toBe('');
  });
});
