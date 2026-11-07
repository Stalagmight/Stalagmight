/**
 * #576 D39(f): a value typed into a new data-table row while that row is being
 * saved as a shot reaches the shot.
 *
 * The draft auto-commit took its create_shot payload the moment the row's
 * required fields were present and, when the create resolved, removed the draft
 * - with whatever the user had typed into it in the meantime (Left, just after
 * the Depth Out that completed the row). The cells stay editable while the create
 * is in flight, and on a large survey that round trip is long. Decided ("Forward
 * as follow-up edit"): the fields that changed since the payload are applied to
 * the created shot with one update_shot once the create lands; the created shot
 * is the one `ShotCreated.shot_id` names.
 *
 * Full-component tier, as dataTableDraftSaveFailure.test.tsx: the real DataTable
 * and grid, `createShot` / `updateShot` replaced on the project store.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen } from '@testing-library/react';

vi.mock('@tanstack/react-virtual', async () =>
  (await import('../../../test/factories/virtualizer')).fakeVirtualizer({ headerOffset: 0 }),
);

vi.mock('../../../utils/ipc');

import DataTable from '../DataTable';
import { AZIMUTH_AVG_KEY } from '../dataTableEdits';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { useSpeleoStore } from '../../../store/speleoStore';
import { defaultConfig } from '../../../types/config';
import type { Config } from '../../../types/config';
import { accepted, type CommandOutcome } from '../../../store/commandResult';
import type { ShotCreated } from '../../../types/ipc/ShotCreated';
import type { Section, Shot } from '../../../types/project';
import { invokeIpcMock } from '../../../test/factories/ipc';
import { NoopResizeObserver } from '../../../test/factories/dom';
import { makeSection, makeShot } from '../../../test/factories/survey';
import { actMicrotasks } from '../../../test/factories/act';

const shot = (id: number, length: number): Shot =>
  makeShot({ id, azimuth_in: 10, azimuth_out: 10, length, depth_out: 1 });

const section: Section = makeSection({ name: 'CaveA', shots: [shot(1, 10), shot(2, 20), shot(3, 30)] });

const created = (shotId: number): CommandOutcome<ShotCreated> =>
  accepted<ShotCreated>({ shot_id: shotId, warnings: [], remote_link: null });

/** The end draft sits below the three shots. */
const DRAFT_ROW = 3;

function typeCell(row: number, colKey: string, value: string) {
  fireEvent.doubleClick(screen.getByTestId(`grid-cell-${row}-${colKey}`));
  const editor = screen.getByTestId('grid-cell-editor');
  fireEvent.change(editor, { target: { value } });
  fireEvent.keyDown(editor, { key: 'Enter' });
}

/** Fill the draft's four required fields; the last one completes it and starts the create. */
async function completeDraft() {
  typeCell(DRAFT_ROW, 'length', '5');
  typeCell(DRAFT_ROW, AZIMUTH_AVG_KEY, '100');
  typeCell(DRAFT_ROW, 'depth_in', '1');
  typeCell(DRAFT_ROW, 'depth_out', '2');
  await actMicrotasks(5);
}

let createShot: ReturnType<typeof vi.fn>;
let updateShot: ReturnType<typeof vi.fn>;
let resolveCreate: (result: CommandOutcome<ShotCreated>) => void;

beforeEach(() => {
  vi.stubGlobal('ResizeObserver', NoopResizeObserver);
  invokeIpcMock.mockReset();
  invokeIpcMock.mockImplementation((command: string) =>
    Promise.resolve(command === 'get_config' ? structuredClone(defaultConfig) : undefined)
  );
  // A create that is still in flight until the test resolves it.
  createShot = vi.fn(
    () =>
      new Promise<CommandOutcome<ShotCreated>>((resolve) => {
        resolveCreate = resolve;
      })
  );
  updateShot = vi.fn(() => Promise.resolve(created(0)));
  useProjectStore.setState({ config: structuredClone(defaultConfig) as Config, createShot, updateShot });
  useUIStore.setState({ toasts: [], selection: null });
  useSpeleoStore.setState({ projectLinkKind: 'local', link: null });
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('DataTable draft row edited while its create is in flight (D39(f))', () => {
  it('applies a value typed during the create to the created shot with one update', async () => {
    render(<DataTable sections={[section]} />);
    await completeDraft();
    expect(createShot).toHaveBeenCalledTimes(1);
    expect(createShot.mock.calls[0]?.[1]).toMatchObject({ length: 5, left: null });

    // Left, typed while create_shot is still on the wire.
    typeCell(DRAFT_ROW, 'left', '2.5');
    await act(async () => resolveCreate(created(4)));
    await actMicrotasks(5);

    expect(updateShot, 'the late Left value never reached the created shot').toHaveBeenCalledTimes(1);
    expect(updateShot).toHaveBeenCalledWith(1, 4, { left: 2.5 });
  });

  it('sends only what changed since the create was sent - a corrected length and a new comment', async () => {
    render(<DataTable sections={[section]} />);
    await completeDraft();
    typeCell(DRAFT_ROW, 'length', '6');
    typeCell(DRAFT_ROW, 'comment', 'sump');
    await act(async () => resolveCreate(created(4)));
    await actMicrotasks(5);

    expect(updateShot).toHaveBeenCalledWith(1, 4, { length: 6, comment: 'sump' });
  });

  it('sends no follow-up when nothing was typed during the create', async () => {
    render(<DataTable sections={[section]} />);
    await completeDraft();
    await act(async () => resolveCreate(created(4)));
    await actMicrotasks(5);

    expect(updateShot).not.toHaveBeenCalled();
  });
});

/**
 * #576 RV3-FE-4 / NF-W3UI-5: the late-edit forwarding matched the draft by key alone,
 * and draft keys restarted on every section change - so the next section's end draft
 * carried the same key as the one whose create was still on the wire. A value typed
 * there was sent to the shot just created in the OTHER section, and the new section's
 * draft was wiped when the create landed.
 */
describe('DataTable section switch while a draft create is in flight (RV3-FE-4)', () => {
  const sectionB: Section = { ...section, id: 2, name: 'CaveB', shots: [shot(1, 11), shot(2, 22)], next_shot_id: 3 };
  /** Section B's end draft sits below its two shots. */
  const B_DRAFT_ROW = 2;

  it("keeps the other section's draft out of the shot created in the first", async () => {
    render(<DataTable sections={[section, sectionB]} />);
    await completeDraft();
    expect(createShot).toHaveBeenCalledTimes(1);
    expect(createShot.mock.calls[0]?.[0]).toBe(1);

    // The user switches to section B while A's create_shot is on the wire, and types a
    // length into B's end draft.
    await act(async () => {
      useUIStore.setState({ selection: { type: 'section', sectionId: 2 } });
    });
    await actMicrotasks(5);
    typeCell(B_DRAFT_ROW, 'length', '7');
    await actMicrotasks(5);
    await act(async () => resolveCreate(created(4)));
    await actMicrotasks(5);

    expect(updateShot, "B's draft values were written into the shot created in A").not.toHaveBeenCalled();
    expect(screen.getByTestId(`grid-cell-${B_DRAFT_ROW}-length`).textContent, "B's draft was wiped").toContain('7');
  });
});
