import { beforeEach, describe, expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
}));

import { useUIStore } from '../uiStore';
import { useProjectStore } from '../projectStore';
import type { Project } from '../../types/project';

const askedForUnsaved = () =>
  vi.mocked(invoke).mock.calls.filter(([cmd]) => cmd === 'has_unsaved_changes').length;

/**
 * A display toggle is view state. On a 0.6.0 file it waits for the next save,
 * which writes it with the conversion, but it never makes the project unsaved:
 * that would arm autosave, and autosave would convert the file 30 s later
 * although the user only looked at it (demo issue I-6). The backend agrees
 * (`has_unsaved_changes` leaves a display-only change out); here the backend
 * answers "unsaved" anyway, as the old contract did, to prove the store never
 * asks it and never sets `isDirty` for a toggle. Injection: call
 * `has_unsaved_changes` after `save_display_setting` and mark the project
 * dirty on `true` (the removed `localStateDirty.ts`), and both tests fail.
 */
describe('a display toggle never makes the project unsaved', () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
    vi.mocked(invoke).mockImplementation(async (cmd: string) => (cmd === 'has_unsaved_changes' ? true : undefined));
    useUIStore.setState({ appMode: 'survey', geometryMode: 'measured' });
    useProjectStore.setState({
      project: { created_at: '2026-01-01T00:00:00Z' } as unknown as Project,
      isDirty: false,
    });
  });

  it('writes the toggle and leaves isDirty alone', async () => {
    await useUIStore.getState().saveDisplaySetting('showGrid', true);
    expect(vi.mocked(invoke)).toHaveBeenCalledWith('save_display_setting', { key: 'showGrid', value: 'true' });
    expect(askedForUnsaved()).toBe(0);
    expect(useProjectStore.getState().isDirty).toBe(false);
  });

  it('does the same for the grid spacing and the geometry mode', async () => {
    useUIStore.getState().setGridSpacing(25);
    useUIStore.getState().setGeometryMode('closureAdjusted');
    await vi.waitFor(() =>
      expect(vi.mocked(invoke).mock.calls.filter(([cmd]) => cmd === 'save_display_setting').length).toBe(2),
    );
    await Promise.resolve();
    expect(askedForUnsaved()).toBe(0);
    expect(useProjectStore.getState().isDirty).toBe(false);
  });
});
