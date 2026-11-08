/**
 * Component test - [#576 W87a-NF2] "Save & Quit" on the unsaved-changes prompt,
 * pressed while a project switch is under way.
 *
 * The prompt closed itself, then saved, and quit only once the save succeeded. A save
 * requested during a switch is refused (`switchGate.ts`) and `handleSaveProject` stays
 * silent on that refusal by design, so the prompt was gone, the window stayed, and
 * nothing said why: the user was left in the app with the unsaved work and no prompt.
 * Now the refusal says so with the quiet notice an edit refused during a switch gets,
 * the window does not close, and the prompt is back as it was, so the same button
 * saves and quits once the switch has settled.
 *
 * The real store's `saveProject`, the real `useProject().handleSaveProject` and the
 * real switch gate; the IPC is the shared mock. All data is synthetic.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

vi.mock('../../utils/ipc');

const performQuitMock = vi.hoisted(() => vi.fn());
const abandonUpdateRestartMock = vi.hoisted(() => vi.fn());
vi.mock('../../utils/quit', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../utils/quit')>()),
  performQuit: performQuitMock,
  abandonUpdateRestart: abandonUpdateRestartMock,
}));

import UnsavedChangesDialog from './UnsavedChangesDialog';
import { noteSwitchRequested } from '../../store/switchGate';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { useUIStore } from '../../store/uiStore';
import { TOAST_KEYS } from '../../utils/toastKeys';
import { emptyBinaryRefresh, invokeIpcMock } from '../../test/factories/ipc';
import { makeProject } from '../../test/factories/survey';

emptyBinaryRefresh();

const FILE = '/caves/CaveA.stalx';
const showToast = vi.fn();
const sentCommands = () => invokeIpcMock.mock.calls.map((c) => c[0] as string);

/** The prompt as `guardQuit` raises it over a dialog it displaced. */
const QUIT_PROMPT = { pendingQuit: true, cancelReturnsTo: 'settings', cancelReturnsData: { tab: 'general' } };

/** The prompt as App renders it from the dialog slot. */
function Host() {
  const activeDialog = useUIStore((s) => s.activeDialog);
  return activeDialog === 'unsaved-changes' ? <UnsavedChangesDialog /> : null;
}

/** The switch the test holds under way; released in `afterEach` whatever happened. */
let releaseSwitch: (() => void) | null = null;

beforeEach(() => {
  invokeIpcMock.mockReset();
  invokeIpcMock.mockImplementation(async (command: string) => {
    if (command === 'save_project') return { saved_path: FILE, warnings: [] };
    if (command === 'check_project_integrity') return { messages: [], save_blocked: false };
    return {};
  });
  performQuitMock.mockReset();
  abandonUpdateRestartMock.mockReset();
  showToast.mockReset();
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
  useProjectStore.setState({
    project: makeProject({ created_at: '2026-03-03T10:00:00Z', file_path: FILE }),
    isDirty: true,
    integrityIssues: [],
  });
  useUIStore.setState({ activeDialog: 'unsaved-changes', dialogData: { ...QUIT_PROMPT }, showToast });
  releaseSwitch = noteSwitchRequested();
});

afterEach(() => {
  releaseSwitch?.();
  releaseSwitch = null;
});

describe('UnsavedChangesDialog - Save & Quit refused during a project switch (#576 W87a-NF2)', () => {
  it('says to wait for the project, does not quit, puts the prompt back, and quits on the next press once the switch settled', async () => {
    render(<Host />);

    await userEvent.click(screen.getByTestId('unsaved-changes-save-quit'));

    await waitFor(() =>
      expect(showToast).toHaveBeenCalledWith(
        'Wait for the project to finish opening.',
        TOAST_KEYS.EDIT_DURING_SWITCH,
        'info',
      ),
    );
    // Refused before any IPC: nothing saved, the window was not asked to close.
    expect(sentCommands()).not.toContain('save_project');
    expect(performQuitMock).not.toHaveBeenCalled();
    expect(abandonUpdateRestartMock).not.toHaveBeenCalled();
    // The prompt is back as it was, so the exit can be finished, or cancelled back to
    // the dialog it displaced.
    expect(useUIStore.getState().activeDialog).toBe('unsaved-changes');
    expect(useUIStore.getState().dialogData).toEqual(QUIT_PROMPT);
    expect(screen.getByTestId('unsaved-changes-save-quit')).toBeEnabled();

    releaseSwitch?.();
    releaseSwitch = null;
    await userEvent.click(screen.getByTestId('unsaved-changes-save-quit'));

    await waitFor(() => expect(performQuitMock).toHaveBeenCalledTimes(1));
    expect(sentCommands()).toContain('save_project');
  });

  it("the backend's own PROJECT_CHANGED gets a user-started verb's short line, and does not quit either", async () => {
    releaseSwitch?.();
    releaseSwitch = null;
    invokeIpcMock.mockImplementation(async (command: string) => {
      if (command === 'save_project') throw { code: 'PROJECT_CHANGED', message: 'another project is open' };
      if (command === 'check_project_integrity') return { messages: [], save_blocked: false };
      return {};
    });
    render(<Host />);

    await userEvent.click(screen.getByTestId('unsaved-changes-save-quit'));

    await waitFor(() => expect(showToast).toHaveBeenCalledWith(expect.any(String), TOAST_KEYS.PROJECT_CHANGED, 'warning'));
    expect(performQuitMock).not.toHaveBeenCalled();
    expect(useUIStore.getState().activeDialog).toBe('unsaved-changes');
    expect(useUIStore.getState().dialogData).toMatchObject({ pendingQuit: true });
  });

  it('Cancel on the prompt put back abandons the exit and returns to the dialog it displaced', async () => {
    render(<Host />);
    await userEvent.click(screen.getByTestId('unsaved-changes-save-quit'));
    await waitFor(() => expect(useUIStore.getState().activeDialog).toBe('unsaved-changes'));
    await waitFor(() => expect(screen.getByTestId('unsaved-changes-cancel')).toBeInTheDocument());

    await userEvent.click(screen.getByTestId('unsaved-changes-cancel'));

    expect(abandonUpdateRestartMock).toHaveBeenCalledTimes(1);
    expect(useUIStore.getState().activeDialog).toBe('settings');
    expect(useUIStore.getState().dialogData).toEqual({ tab: 'general' });
    expect(performQuitMock).not.toHaveBeenCalled();
  });

  it('control: with no switch under way, Save & Quit saves and quits', async () => {
    releaseSwitch?.();
    releaseSwitch = null;
    render(<Host />);

    await userEvent.click(screen.getByTestId('unsaved-changes-save-quit'));

    await waitFor(() => expect(performQuitMock).toHaveBeenCalledTimes(1));
    expect(sentCommands()).toContain('save_project');
    expect(showToast).not.toHaveBeenCalledWith(expect.any(String), TOAST_KEYS.EDIT_DURING_SWITCH, expect.anything());
  });
});
