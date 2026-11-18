/**
 * Edit Line's rename (SPEC 4.1): a line name is required and unique in the
 * project. The dialog says so under the name field and keeps Save disabled
 * while the name is empty or another line's (its own is fine), and a refusal
 * the backend still makes is shown at the field too. Through the real store
 * path, with only the IPC mocked.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen } from '@testing-library/react';

const invokeIpcMock = vi.hoisted(() => vi.fn());
vi.mock('../../../utils/ipc', () => ({ invokeIpc: invokeIpcMock, getErrorMessage: String }));
vi.mock('@tanstack/react-virtual', () => ({
  useVirtualizer: ({ count }: { count: number }) => ({
    getVirtualItems: () =>
      Array.from({ length: count }, (_, index) => ({ index, key: index, start: 33 + index * 33, size: 33 })),
    getTotalSize: () => count * 33,
    scrollToIndex: vi.fn(),
    measureElement: vi.fn(),
  }),
}));

import '../../../i18n';
import EditLineDialog from '../EditLineDialog';
import { useUIStore } from '../../../store/uiStore';
import { useProjectStore } from '../../../store/projectStore';
import { useSpeleoStore } from '../../../store/speleoStore';
import type { Project, Line, Shot } from '../../../types/project';

class NoopResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}

/** The commands that would each push an undo step. */
const MUTATIONS = new Set(['update_line', 'edit_shot_credit', 'execute_batch', 'update_shot', 'create_shot', 'delete_shot']);
const mutations = () =>
  invokeIpcMock.mock.calls.filter(([command]) => MUTATIONS.has(command as string)).map(([command, args]) => [command, args]);

function shot(id: number): Shot {
  return {
    id,
    azimuth_in: 90,
    azimuth_out: 90,
    length: 5,
    depth_in: 1,
    depth_out: 2,
    pitch_in: null,
    pitch_out: null,
    left: null,
    right: null,
    up: null,
    down: null,
    comment: null,
    kind: 'Normal',
    marker_index: null,
    temperature: null,
    credit: { surveyors: [{ name: 'Ann Diver' }], explorers: [], date: null },
  } as Shot;
}

beforeEach(() => {
  vi.stubGlobal('ResizeObserver', NoopResizeObserver);
  invokeIpcMock.mockReset();
  invokeIpcMock.mockImplementation(async (command: string) => {
    if (MUTATIONS.has(command)) return { success: true, warnings: [], error: null, line_ids: [1] };
    if (command === 'can_undo' || command === 'can_redo') return false;
    if (command === 'get_project_state') return null;
    if (command.includes('integrity')) return { messages: [] };
    if (command.startsWith('get_') || command.startsWith('validate')) return [];
    return null;
  });
  const line = {
    id: 1,
    name: 'Main',
    color: '#336699',
    direction: 'in',
    quality: 1,
    next_shot_id: 103,
    shots: [shot(101), shot(102)],
  } as unknown as Line;
  useProjectStore.setState({
    project: {
      created_at: 'rename',
      lines: [line, { ...line, id: 2, name: 'Side', shots: [] }],
      station_merges: [],
      zero_datums: [],
      loop_closures: [],
    } as unknown as Project,
    config: null,
  });
  useUIStore.setState({ dialogData: { lineId: 1 }, appMode: 'survey', geometryMode: 'measured', closeDialog: vi.fn() });
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
});

const nameInput = () => screen.getByTestId('line-name-input');
const saveButton = () => screen.getByTestId('edit-line-save');

async function save() {
  await act(async () => {
    fireEvent.click(saveButton());
  });
}

describe('Edit Line rename', () => {
  it("accepts the line's own name", () => {
    render(<EditLineDialog />);
    expect(nameInput()).toHaveValue('Main');
    expect(screen.queryByTestId('line-name-error')).toBeNull();
    expect(nameInput()).toHaveAttribute('aria-invalid', 'false');
    expect(saveButton()).toBeEnabled();
  });

  it('says a name is needed and disables Save while it is empty', async () => {
    render(<EditLineDialog />);
    fireEvent.change(nameInput(), { target: { value: '  ' } });
    expect(screen.getByTestId('line-name-error')).toHaveTextContent('A line needs a name.');
    expect(nameInput()).toHaveAttribute('aria-invalid', 'true');
    expect(saveButton()).toBeDisabled();
    expect(saveButton()).toHaveAttribute('title', 'A line needs a name.');
    await save();
    expect(mutations()).toEqual([]);
  });

  it("refuses another line's name, trimmed", async () => {
    render(<EditLineDialog />);
    fireEvent.change(nameInput(), { target: { value: ' Side ' } });
    expect(screen.getByTestId('line-name-error')).toHaveTextContent(
      'A line named "Side" already exists. Line names must be unique in the project.'
    );
    expect(saveButton()).toBeDisabled();
    await save();
    expect(mutations()).toEqual([]);

    fireEvent.change(nameInput(), { target: { value: ' Side passage ' } });
    expect(screen.queryByTestId('line-name-error')).toBeNull();
    await save();
    expect(mutations()).toEqual([
      ['update_line', { lineId: 1, updates: { name: 'Side passage', color: '#336699', direction: 'in', quality: 1 } }],
    ]);
  });

  it('shows a refusal the backend still makes at the name, until the name changes', async () => {
    const refusal = 'A line named "Upstream" already exists';
    invokeIpcMock.mockImplementation(async (command: string) => {
      if (command === 'update_line') return { success: false, warnings: [], error: refusal, line_ids: [] };
      if (command === 'can_undo' || command === 'can_redo') return false;
      if (command === 'get_project_state') return null;
      if (command.includes('integrity')) return { messages: [] };
      if (command.startsWith('get_') || command.startsWith('validate')) return [];
      return null;
    });
    render(<EditLineDialog />);
    fireEvent.change(nameInput(), { target: { value: 'Upstream' } });
    await save();
    expect(screen.getByTestId('line-name-error')).toHaveTextContent(refusal);
    expect(saveButton()).toBeDisabled();
    expect(useUIStore.getState().closeDialog).not.toHaveBeenCalled();

    fireEvent.change(nameInput(), { target: { value: 'Upstream 2' } });
    expect(screen.queryByTestId('line-name-error')).toBeNull();
    expect(saveButton()).toBeEnabled();
  });
});
