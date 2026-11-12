/**
 * Edit Section's Save is ONE undo step, through the real store path (the real
 * `executeCommand`, with only the IPC mocked): an unchanged section sends no
 * metadata update (that was an invisible step a second Ctrl+Z "did nothing"
 * on), people alone send one `edit_shot_credit`, and a rename plus people go
 * together in one `execute_batch`.
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
import EditSectionDialog from '../EditSectionDialog';
import { useUIStore } from '../../../store/uiStore';
import { useProjectStore } from '../../../store/projectStore';
import { useSpeleoStore } from '../../../store/speleoStore';
import type { Project, Section, Shot } from '../../../types/project';

class NoopResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}

/** The commands that would each push an undo step. */
const MUTATIONS = new Set(['update_section', 'edit_shot_credit', 'execute_batch', 'update_shot', 'create_shot', 'delete_shot']);
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
    if (MUTATIONS.has(command)) return { success: true, warnings: [], error: null, section_ids: [1] };
    if (command === 'can_undo' || command === 'can_redo') return false;
    if (command === 'get_project_state') return null;
    if (command.includes('integrity')) return { messages: [] };
    if (command.startsWith('get_') || command.startsWith('validate')) return [];
    return null;
  });
  const section = {
    id: 1,
    name: 'Main',
    color: '#336699',
    direction: 'in',
    quality: 1,
    next_shot_id: 103,
    shots: [shot(101), shot(102)],
  } as unknown as Section;
  useProjectStore.setState({
    project: {
      created_at: 'one-step',
      sections: [section],
      station_merges: [],
      zero_datums: [],
      loop_closures: [],
    } as unknown as Project,
    config: null,
  });
  useUIStore.setState({ dialogData: { sectionId: 1 }, appMode: 'survey', geometryMode: 'measured', closeDialog: vi.fn() });
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
});

function addSurveyor(name: string) {
  const input = screen.getByTestId('section-surveyors-input');
  fireEvent.change(input, { target: { value: name } });
  fireEvent.keyDown(input, { key: 'Enter' });
}

async function save() {
  await act(async () => {
    fireEvent.click(screen.getByTestId('edit-section-save'));
  });
}

describe('Edit Section Save is one undo step', () => {
  it('sends only the credit edit when only people changed', async () => {
    render(<EditSectionDialog />);
    addSurveyor('Bo Lead');
    await save();
    expect(mutations()).toEqual([
      [
        'edit_shot_credit',
        { targets: { type: 'section', section_id: 1 }, edit: { add_surveyors: [{ name: 'Bo Lead' }] } },
      ],
    ]);
  });

  it('sends a rename and the people in ONE batch', async () => {
    render(<EditSectionDialog />);
    fireEvent.change(screen.getByTestId('section-name-input'), { target: { value: 'Renamed' } });
    addSurveyor('Bo Lead');
    await save();
    expect(mutations()).toEqual([
      [
        'execute_batch',
        {
          commands: [
            { type: 'update_section_metadata', section_id: 1, updates: { name: 'Renamed', color: '#336699', direction: 'in', quality: 1 } },
            { type: 'edit_shot_credit', targets: { type: 'section', section_id: 1 }, edit: { add_surveyors: [{ name: 'Bo Lead' }] } },
          ],
        },
      ],
    ]);
  });

  it('sends only the metadata update for a rename alone', async () => {
    render(<EditSectionDialog />);
    fireEvent.change(screen.getByTestId('section-name-input'), { target: { value: 'Renamed' } });
    await save();
    expect(mutations().map(([command]) => command)).toEqual(['update_section']);
  });

  it('sends nothing at all when nothing changed', async () => {
    render(<EditSectionDialog />);
    await save();
    expect(mutations()).toEqual([]);
  });
});
