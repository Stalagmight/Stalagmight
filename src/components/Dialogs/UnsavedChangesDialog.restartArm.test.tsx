/**
 * Component test - [#576 FUZZ-MERGE2-NF] an update restart riding on the unsaved-changes
 * prompt, and Save & Quit failing.
 *
 * An update restart runs the quit's guards; this prompt opening leaves the restart armed
 * on purpose, because its own Save & Quit re-issues the close and that close installs.
 * Cancel abandons the restart. But when Save & Quit failed and left no prompt to press
 * again - another dialog took the slot during a refusal, the save itself failed, a Save
 * As was cancelled - the exit was over and the arm stayed set: the next ordinary quit the
 * user made installed the update with no restart asked for. Every such path now abandons
 * the restart, on both sides. A refusal that puts the prompt back keeps it armed, so
 * Save & Quit there still restarts into the update.
 *
 * The REAL update-restart arm (`requestUpdateRestart`, `isUpdateRestartArmed`,
 * `abandonUpdateRestart`), the real store's `saveProject`, the real
 * `useProject().handleSaveProject` and the real switch gate; only `performQuit` is a
 * spy. The IPC is the shared mock. All data is synthetic.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';

vi.mock('../../utils/ipc');
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ close: vi.fn(async () => {}), destroy: vi.fn(async () => {}) }),
}));
const saveDialogMock = vi.hoisted(() => vi.fn(async (): Promise<string | null> => null));
vi.mock('@tauri-apps/plugin-dialog', () => ({
  open: vi.fn(async () => null),
  save: saveDialogMock,
  confirm: vi.fn(async () => false),
}));

const performQuitMock = vi.hoisted(() => vi.fn());
vi.mock('../../utils/quit', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../utils/quit')>()),
  performQuit: performQuitMock,
}));

import UnsavedChangesDialog from './UnsavedChangesDialog';
import { isUpdateRestartArmed, requestUpdateRestart, resetUpdateRestartForTests } from '../../utils/quit';
import { noteSwitchRequested } from '../../store/switchGate';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { useUIStore } from '../../store/uiStore';
import { emptyBinaryRefresh, invokeIpcMock } from '../../test/factories/ipc';
import { makeProject } from '../../test/factories/survey';

emptyBinaryRefresh();

const FILE = '/caves/CaveA.stalx';
const sent = () => invokeIpcMock.mock.calls.map((c) => c[0] as string);
const QUIT_PROMPT = { pendingQuit: true };

function Host() {
  const activeDialog = useUIStore((s) => s.activeDialog);
  return activeDialog === 'unsaved-changes' ? <UnsavedChangesDialog /> : null;
}

let saveReply: () => Promise<unknown> = async () => ({ saved_path: FILE, warnings: [] });
let releaseSwitch: (() => void) | null = null;

/** Let the handler's awaits run. */
async function flush(): Promise<void> {
  for (let i = 0; i < 20; i += 1) await new Promise((resolve) => setTimeout(resolve, 0));
}

beforeEach(async () => {
  invokeIpcMock.mockReset();
  invokeIpcMock.mockImplementation(async (command: string) => {
    if (command === 'save_project') return saveReply();
    if (command === 'check_project_integrity') return { messages: [], save_blocked: false };
    if (command === 'save_target_presence') return { present: false, existence_unknown: false };
    return {};
  });
  performQuitMock.mockReset();
  saveDialogMock.mockReset();
  saveDialogMock.mockResolvedValue(null);
  saveReply = async () => ({ saved_path: FILE, warnings: [] });
  resetUpdateRestartForTests();
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
  useProjectStore.setState({
    project: makeProject({ created_at: '2026-03-03T10:00:00Z', file_path: FILE }),
    isDirty: true,
    isClosing: false,
    integrityIssues: [],
  });
  // An update restart asked for: armed on both sides, its close request raised this prompt.
  await requestUpdateRestart();
  expect(isUpdateRestartArmed()).toBe(true);
  useUIStore.setState({ activeDialog: 'unsaved-changes', dialogData: { ...QUIT_PROMPT } });
});

afterEach(() => {
  releaseSwitch?.();
  releaseSwitch = null;
  resetUpdateRestartForTests();
});

describe('UnsavedChangesDialog - Save & Quit failing abandons an update restart (#576 FUZZ-MERGE2-NF)', () => {
  it('refused during a switch while another dialog took the slot: the restart is abandoned', async () => {
    releaseSwitch = noteSwitchRequested();
    render(<Host />);

    fireEvent.click(screen.getByTestId('unsaved-changes-save-quit'));
    useUIStore.getState().openDialog('settings');
    await flush();

    expect(useUIStore.getState().activeDialog).toBe('settings');
    expect(performQuitMock).not.toHaveBeenCalled();
    expect(isUpdateRestartArmed()).toBe(false);
    expect(sent()).toContain('unarm_app_update_restart');
  });

  it('a save that fails: the restart is abandoned', async () => {
    saveReply = async () => {
      throw { code: 'SAVE_FAILED', message: 'disk full' };
    };
    render(<Host />);

    fireEvent.click(screen.getByTestId('unsaved-changes-save-quit'));
    await flush();

    expect(sent()).toContain('save_project');
    expect(performQuitMock).not.toHaveBeenCalled();
    expect(isUpdateRestartArmed()).toBe(false);
    expect(sent()).toContain('unarm_app_update_restart');
  });

  it('a Save As cancelled (an untitled project): the restart is abandoned', async () => {
    useProjectStore.setState({ project: makeProject({ created_at: '2026-03-03T10:00:00Z', file_path: null }) });
    render(<Host />);

    fireEvent.click(screen.getByTestId('unsaved-changes-save-quit'));
    await flush();

    expect(saveDialogMock).toHaveBeenCalledTimes(1);
    expect(sent()).not.toContain('save_project');
    expect(performQuitMock).not.toHaveBeenCalled();
    expect(isUpdateRestartArmed()).toBe(false);
    expect(sent()).toContain('unarm_app_update_restart');
  });

  it('control: a refusal that puts the prompt back keeps the restart armed, and Save & Quit then quits into it', async () => {
    releaseSwitch = noteSwitchRequested();
    render(<Host />);

    fireEvent.click(screen.getByTestId('unsaved-changes-save-quit'));
    await flush();

    expect(useUIStore.getState().activeDialog).toBe('unsaved-changes');
    expect(isUpdateRestartArmed()).toBe(true);
    expect(sent()).not.toContain('unarm_app_update_restart');

    releaseSwitch();
    releaseSwitch = null;
    fireEvent.click(screen.getByTestId('unsaved-changes-save-quit'));
    await flush();

    expect(performQuitMock).toHaveBeenCalledTimes(1);
    expect(isUpdateRestartArmed()).toBe(true);
  });
});
