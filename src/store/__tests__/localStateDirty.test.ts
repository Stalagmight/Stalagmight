import { beforeEach, describe, expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
}));

import { useUIStore } from '../uiStore';
import { useProjectStore } from '../projectStore';
import type { Project } from '../../types/project';

/** The backend's answers: `has_unsaved_changes` says `unsaved`. */
function backend(unsaved: boolean): void {
  vi.mocked(invoke).mockImplementation(async (cmd: string) => {
    if (cmd === 'has_unsaved_changes') return unsaved;
    return undefined;
  });
}

const askedForUnsaved = () =>
  vi.mocked(invoke).mock.calls.filter(([cmd]) => cmd === 'has_unsaved_changes').length;

function openProject(createdAt: string): void {
  useProjectStore.setState({
    project: { created_at: createdAt } as unknown as Project,
    isDirty: false,
  });
}

/**
 * A display toggle on a 0.6.0 file waits in memory for the upgrading save, so
 * the backend reports unsaved changes and the store's `isDirty` must follow,
 * or the close prompt lets the toggle go (review finding D-migration-upgrade-5).
 */
describe('unsaved changes after a display toggle', () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
    openProject('2026-01-01T00:00:00Z');
  });

  it('marks the project unsaved when the backend kept the toggle for the next save', async () => {
    backend(true);
    await useUIStore.getState().saveDisplaySetting('showGrid', true);
    expect(askedForUnsaved()).toBe(1);
    expect(useProjectStore.getState().isDirty).toBe(true);
  });

  it('leaves the flag alone when the toggle was written at once', async () => {
    backend(false);
    await useUIStore.getState().saveDisplaySetting('showGrid', true);
    expect(askedForUnsaved()).toBe(1);
    expect(useProjectStore.getState().isDirty).toBe(false);
  });

  it('never marks a project opened since the toggle', async () => {
    let answer: (value: boolean) => void = () => {};
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'has_unsaved_changes') return new Promise<boolean>((resolve) => { answer = resolve; });
      return undefined;
    });
    const pending = useUIStore.getState().saveDisplaySetting('showGrid', true);
    await vi.waitFor(() => expect(askedForUnsaved()).toBe(1));
    openProject('2026-02-02T00:00:00Z');
    answer(true);
    await pending;
    expect(useProjectStore.getState().isDirty).toBe(false);
  });

  it('asks nothing with no project open', async () => {
    backend(true);
    useProjectStore.setState({ project: null, isDirty: false });
    await useUIStore.getState().saveDisplaySetting('showGrid', true);
    expect(askedForUnsaved()).toBe(0);
  });
});
