/**
 * [#576 D34] Autosave must not write the open project while the user is deciding
 * whether to leave it, nor while a discard is switching projects.
 *
 * Before: the debounce armed by the last edit kept running under the Unsaved
 * Changes prompt, and after "Discard Changes" it kept running while the user named
 * the new project (or the next project loaded) - so the edits the prompt said
 * would be lost were written into the old file.
 *
 * Pinned with the maintainer's decisions: D34-D2 "Pause while prompt open" and
 * D34-D1 "Keep edits, resume autosave" (backing out returns to the project with its
 * edits and a fresh interval). `.tsx` for jsdom (renderHook / render).
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { act, fireEvent, render, renderHook, screen } from '@testing-library/react';

const closeWindowMock = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ close: closeWindowMock }),
}));

import { useAutoSave } from '../useAutoSave';
import UnsavedChangesDialog from '../../components/Dialogs/UnsavedChangesDialog';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import { isAutosaveHeld, isLeavePromptOpen, useAutosaveHoldStore } from '../../utils/autosaveHold';
import { performQuit, resetQuitConfirmationForTests } from '../../utils/quit';
import type { CommandResult } from '../../types/commands';

const INTERVAL_S = 30;
const OK: CommandResult = { success: true, error: null, warnings: [], section_ids: [] };

let saveProject: ReturnType<typeof vi.fn>;

/** Seconds of (fake) wall clock, flushing the async save the timer starts. */
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

/** What App renders for the prompt, plus the autosave hook SingleSystemView runs. */
function Harness() {
  useAutoSave();
  const dialog = useUIStore((s) => s.activeDialog);
  return dialog === 'unsaved-changes' ? <UnsavedChangesDialog /> : null;
}

beforeEach(() => {
  vi.useFakeTimers();
  resetQuitConfirmationForTests();
  useAutosaveHoldStore.setState({ holds: 0 });
  saveProject = vi.fn(async () => {
    useProjectStore.setState({ isDirty: false });
    return OK;
  });
  useProjectStore.setState({
    project: { file_path: '/caves/CaveA.stalx', sections: [] } as never,
    config: { general: { auto_save: true, auto_save_interval_seconds: INTERVAL_S } } as never,
    isDirty: false,
    integritySaveBlocked: false,
    newerSchemaVersion: null,
    saveProject,
  } as never);
  useUIStore.setState({ activeDialog: null, dialogData: {} });
});

afterEach(() => {
  vi.useRealTimers();
  useUIStore.setState({ activeDialog: null, dialogData: {} });
  useProjectStore.setState({ project: null, isDirty: false } as never);
});

describe('autosave while a leave-project prompt is open (D34-D2)', () => {
  it('does not save while the Unsaved Changes prompt is open', async () => {
    const hook = renderHook(() => useAutoSave());
    edit();
    expect(hook.result.current.status).toBe('pending');

    act(() => {
      useUIStore.getState().openDialog('unsaved-changes', { pendingNewProject: true });
    });
    await advance(5 * 60);

    expect(saveProject).not.toHaveBeenCalled();
    // Not a countdown to a save that is not going to happen.
    expect(hook.result.current.status).toBe('idle');
  });

  it('Cancel re-arms a full interval for the edits that are still there', async () => {
    renderHook(() => useAutoSave());
    edit();
    await advance(20);
    act(() => {
      useUIStore.getState().openDialog('unsaved-changes', { pendingNewProject: true });
    });
    await advance(60);
    act(() => {
      useUIStore.getState().closeDialog();
    });

    await advance(INTERVAL_S - 1);
    expect(saveProject).not.toHaveBeenCalled();
    await advance(1);
    expect(saveProject).toHaveBeenCalledTimes(1);
    expect(saveProject).toHaveBeenCalledWith(undefined, true);
  });

  it('an edit that lands while the prompt is open does not arm the timer', async () => {
    renderHook(() => useAutoSave());
    act(() => {
      useUIStore.getState().openDialog('speleodb-close');
    });
    edit();
    await advance(5 * 60);
    expect(saveProject).not.toHaveBeenCalled();
  });

  it('counts every leave prompt, and the unwound-push confirmation only when it carries an exit', () => {
    for (const dialog of ['unsaved-changes', 'speleodb-close', 'speleodb-quit', 'new-project']) {
      expect(isLeavePromptOpen(dialog, {})).toBe(true);
    }
    expect(isLeavePromptOpen('speleodb-unwound-push', { pendingExit: { kind: 'quit' } })).toBe(true);
    expect(isLeavePromptOpen('speleodb-unwound-push', {})).toBe(false);
    expect(isLeavePromptOpen('export', {})).toBe(false);
    expect(isLeavePromptOpen(null, {})).toBe(false);
  });
});

describe('autosave after "Discard Changes" (D34-D1)', () => {
  it('Discard -> New Project: no save while the user names the new project; Cancel resumes', async () => {
    render(<Harness />);
    edit();
    act(() => {
      useUIStore.getState().openDialog('unsaved-changes', { pendingNewProject: true });
    });

    await act(async () => {
      fireEvent.click(screen.getByTestId('unsaved-changes-discard'));
    });
    expect(useUIStore.getState().activeDialog).toBe('new-project');
    await advance(5 * 60);
    expect(saveProject).not.toHaveBeenCalled();

    // Backing out of New Project: the project and its edits are still open.
    act(() => {
      useUIStore.getState().closeDialog();
    });
    expect(useProjectStore.getState().isDirty).toBe(true);
    await advance(INTERVAL_S);
    expect(saveProject).toHaveBeenCalledTimes(1);
  });

  it('Discard -> New Project -> Create: the edits are never written', async () => {
    render(<Harness />);
    edit();
    act(() => {
      useUIStore.getState().openDialog('unsaved-changes', { pendingNewProject: true });
    });
    await act(async () => {
      fireEvent.click(screen.getByTestId('unsaved-changes-discard'));
    });
    await advance(45);

    // createProject replaces the project (clean), then the dialog closes.
    act(() => {
      useProjectStore.setState({
        project: { file_path: '/caves/CaveB.stalx', sections: [] } as never,
        isDirty: false,
      });
      useUIStore.getState().closeDialog();
    });
    await advance(5 * 60);
    expect(saveProject).not.toHaveBeenCalled();
  });

  it('Discard -> Open: no save while the next project loads; a failed load resumes', async () => {
    let finishLoad!: (outcome: { success: boolean }) => void;
    const loadProject = vi.fn(
      () =>
        new Promise<{ success: boolean }>((resolve) => {
          finishLoad = resolve;
        }),
    );
    useProjectStore.setState({ loadProject } as never);
    render(<Harness />);
    edit();
    act(() => {
      useUIStore.getState().openDialog('unsaved-changes', { pendingOpenPath: '/caves/CaveB.stalx' });
    });

    await act(async () => {
      fireEvent.click(screen.getByTestId('unsaved-changes-discard'));
    });
    expect(loadProject).toHaveBeenCalledWith('/caves/CaveB.stalx');
    // The prompt is gone, but the load has not landed: still held.
    expect(useUIStore.getState().activeDialog).toBeNull();
    await advance(5 * 60);
    expect(saveProject).not.toHaveBeenCalled();
    expect(isAutosaveHeld()).toBe(true);

    // The load fails: CaveA is still open with its edits, so autosave picks up again.
    await act(async () => {
      finishLoad({ success: false });
    });
    expect(isAutosaveHeld()).toBe(false);
    await advance(INTERVAL_S);
    expect(saveProject).toHaveBeenCalledTimes(1);
  });

  it('Discard -> close: no save while the close runs', async () => {
    let finishClose!: (r: CommandResult) => void;
    const closeProject = vi.fn(
      () =>
        new Promise<CommandResult>((resolve) => {
          finishClose = resolve;
        }),
    );
    useProjectStore.setState({ closeProject } as never);
    render(<Harness />);
    edit();
    act(() => {
      useUIStore.getState().openDialog('unsaved-changes');
    });

    await act(async () => {
      fireEvent.click(screen.getByTestId('unsaved-changes-discard'));
    });
    await advance(5 * 60);
    expect(saveProject).not.toHaveBeenCalled();

    await act(async () => {
      useProjectStore.setState({ project: null, isDirty: false } as never);
      finishClose(OK);
    });
    await advance(5 * 60);
    expect(saveProject).not.toHaveBeenCalled();
  });

  it('Discard & Quit: held while the window goes; a refused close releases at once', async () => {
    closeWindowMock.mockReturnValue(new Promise(() => {}));
    renderHook(() => useAutoSave());
    edit();
    // The debounce is 5 s from firing when the user confirms the quit.
    await advance(INTERVAL_S - 5);

    act(() => {
      performQuit();
    });
    await advance(9);
    expect(saveProject).not.toHaveBeenCalled();
    expect(isAutosaveHeld()).toBe(true);

    closeWindowMock.mockReturnValue(Promise.reject(new Error('close denied')));
    act(() => {
      performQuit();
    });
    await advance(0);
    // The refused close dropped its own hold; the first one is still timing out.
    expect(useAutosaveHoldStore.getState().holds).toBe(1);
    await advance(1);
    expect(isAutosaveHeld()).toBe(false);
  });
});
