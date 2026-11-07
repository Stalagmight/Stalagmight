/**
 * #576 RV2-FE-G4: autosave is held while the open file's SpeleoDB link could not be
 * read, as it is for a file a newer version saved and for unread project settings.
 *
 * The backend refuses every save that would rewrite such a file (the rebuilt file
 * would lose the link row) - only a Save As to another file goes ahead. Autosave kept
 * trying: each attempt was refused, and each refusal raised the autosave-failed
 * banner next to the unreadable-link banner that already said why. `.tsx` for jsdom.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, renderHook } from '@testing-library/react';

import { useAutoSave } from '../useAutoSave';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import { useAutosaveHoldStore } from '../../utils/autosaveHold';
import type { CommandResult } from '../../types/commands';

const INTERVAL_S = 30;
const REFUSED: CommandResult = {
  success: false,
  error: 'SPELEODB_LINK_UNREADABLE: refused',
  warnings: [],
  section_ids: [],
};

let saveProject: ReturnType<typeof vi.fn>;

async function advance(seconds: number): Promise<void> {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(seconds * 1000);
  });
}

function edit(): void {
  act(() => {
    useProjectStore.setState({ isDirty: true });
  });
}

beforeEach(() => {
  vi.useFakeTimers();
  useAutosaveHoldStore.setState({ holds: 0 });
  saveProject = vi.fn(async () => REFUSED);
  useProjectStore.setState({
    project: { file_path: '/caves/CaveA.stalx', sections: [] } as never,
    config: { general: { auto_save: true, auto_save_interval_seconds: INTERVAL_S } } as never,
    isDirty: false,
    integritySaveBlocked: false,
    newerSchemaVersion: null,
    unreadableProjectSettings: [],
    speleoLinkUnreadable: true,
    saveProject,
  } as never);
  useUIStore.setState({ activeDialog: null, dialogData: {} });
});

afterEach(() => {
  vi.useRealTimers();
  useProjectStore.setState({ project: null, isDirty: false, speleoLinkUnreadable: false } as never);
});

describe('autosave while the SpeleoDB link is unreadable (RV2-FE-G4)', () => {
  it('does not arm or save, however long the project stays dirty', async () => {
    const hook = renderHook(() => useAutoSave());
    edit();
    expect(hook.result.current.status, 'autosave armed a save the backend refuses').toBe('idle');
    await advance(5 * INTERVAL_S);
    edit();
    await advance(5 * INTERVAL_S);
    expect(saveProject).not.toHaveBeenCalled();
  });

  it('drops a save armed before the link was found unreadable', async () => {
    useProjectStore.setState({ speleoLinkUnreadable: false });
    renderHook(() => useAutoSave());
    edit();
    act(() => {
      useProjectStore.setState({ speleoLinkUnreadable: true });
    });
    await advance(INTERVAL_S + 1);
    expect(saveProject).not.toHaveBeenCalled();
  });

  it('saves again once the flag clears (a Save As to another file)', async () => {
    saveProject.mockImplementation(async () => {
      useProjectStore.setState({ isDirty: false });
      return { ...REFUSED, success: true, error: null };
    });
    renderHook(() => useAutoSave());
    edit();
    act(() => {
      useProjectStore.setState({ speleoLinkUnreadable: false, isDirty: false });
    });
    edit();
    await advance(INTERVAL_S + 1);
    expect(saveProject).toHaveBeenCalledTimes(1);
  });
});
