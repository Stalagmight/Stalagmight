/**
 * Component test - [#576 PREPR-594-3-LOW] "Save & Quit" on the unsaved-changes prompt
 * over an edit that has not settled.
 *
 * The quit's guard waits for the queued edits, at most 5 s (`editsSettledForExit`), and
 * an edit that is still not settled then counts as unsaved work, so this prompt opens.
 * Save & Quit then saved at once - the save does not wait for edits - and the quit was
 * pre-confirmed, so the window closed while that edit was still queued: it never reached
 * the file. Now Save & Quit waits for the queued edits again before it saves; when they
 * still have not settled it neither saves nor quits, says so with a quiet toast, and puts
 * the prompt back. An edit queued while the save runs does not let it quit either.
 *
 * The real store (`updateShot` through the mutation queue, `saveProject`), the real
 * `useProject().handleSaveProject` and the real `editsSettledForExit`; the IPC is the
 * shared mock, the edit's and the save's replies are held by hand. All data is synthetic.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';

vi.mock('../../utils/ipc');

const performQuitMock = vi.hoisted(() => vi.fn());
const abandonUpdateRestartMock = vi.hoisted(() => vi.fn());
vi.mock('../../utils/quit', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../utils/quit')>()),
  performQuit: performQuitMock,
  abandonUpdateRestart: abandonUpdateRestartMock,
}));

import UnsavedChangesDialog from './UnsavedChangesDialog';
import { mutationsSettled } from '../../store/mutationQueue';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { useUIStore } from '../../store/uiStore';
import { TOAST_KEYS } from '../../utils/toastKeys';
import { deferred, type Deferred } from '../../test/factories/async';
import { emptyBinaryRefresh, invokeIpcMock } from '../../test/factories/ipc';
import { makeProject } from '../../test/factories/survey';

emptyBinaryRefresh();

const FILE = '/caves/CaveA.stalx';
const EDIT = 'update_shot';
const SAVE = 'save_project';
const showToast = vi.fn();
const sent = () => invokeIpcMock.mock.calls.map((c) => c[0] as string);

/** The prompt as `guardQuit` raises it for a quit whose edits did not settle in time. */
const QUIT_PROMPT = { pendingQuit: true };

/** The prompt as App renders it from the dialog slot. */
function Host() {
  const activeDialog = useUIStore((s) => s.activeDialog);
  return activeDialog === 'unsaved-changes' ? <UnsavedChangesDialog /> : null;
}

/** Replies the test holds open; settled in `afterEach` whatever happened. */
let heldEdit: Deferred<unknown> | null = null;
let heldSave: Deferred<unknown> | null = null;

function answer(command: string): Promise<unknown> {
  if (command === EDIT && heldEdit) return heldEdit.promise;
  if (command === SAVE && heldSave) return heldSave.promise;
  if (command === SAVE) return Promise.resolve({ saved_path: FILE, warnings: [] });
  if (command === 'check_project_integrity') return Promise.resolve({ messages: [], save_blocked: false });
  return Promise.resolve(null);
}

/**
 * A grid cell committed on blur: queued, its IPC not answered yet. Hands back the edit's
 * promise boxed, so awaiting this does not await the edit.
 */
async function startEdit(): Promise<{ done: Promise<unknown> }> {
  heldEdit = deferred<unknown>();
  const done = useProjectStore.getState().updateShot(1, 1, { length: 55 });
  await vi.advanceTimersByTimeAsync(0);
  expect(sent().filter((c) => c === EDIT)).toHaveLength(1);
  return { done };
}

/** Let fake time run in short steps until `p` settles; fails rather than hangs. */
async function settle(p: Promise<unknown>, stepMs = 10, steps = 100): Promise<void> {
  let done = false;
  void p.finally(() => {
    done = true;
  });
  for (let i = 0; i < steps && !done; i += 1) await vi.advanceTimersByTimeAsync(stepMs);
  expect(done).toBe(true);
}

/** Press Save & Quit and let the handler run, including a full 5 s wait. */
async function pressSaveAndQuit(): Promise<void> {
  fireEvent.click(screen.getByTestId('unsaved-changes-save-quit'));
  for (let i = 0; i < 60; i += 1) await vi.advanceTimersByTimeAsync(100);
}

beforeEach(() => {
  vi.useFakeTimers();
  invokeIpcMock.mockReset();
  invokeIpcMock.mockImplementation(answer);
  performQuitMock.mockReset();
  abandonUpdateRestartMock.mockReset();
  showToast.mockReset();
  heldEdit = null;
  heldSave = null;
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
  useProjectStore.setState({
    project: makeProject({ created_at: '2026-03-03T10:00:00Z', file_path: FILE }),
    isDirty: true,
    isClosing: false,
    integrityIssues: [],
  });
  useUIStore.setState({ activeDialog: 'unsaved-changes', dialogData: { ...QUIT_PROMPT }, showToast });
});

afterEach(async () => {
  heldEdit?.resolve({ remote_link: null });
  heldSave?.resolve({ saved_path: FILE, warnings: [] });
  heldEdit = null;
  heldSave = null;
  await settle(mutationsSettled());
  vi.useRealTimers();
});

describe('UnsavedChangesDialog - Save & Quit over an edit that has not settled (#576 PREPR-594-3-LOW)', () => {
  it('a stuck edit: neither saves nor quits, says so, puts the prompt back; quits once the edit has landed', async () => {
    const { done: editDone } = await startEdit();
    render(<Host />);

    await pressSaveAndQuit();

    expect(performQuitMock).not.toHaveBeenCalled();
    expect(sent()).not.toContain(SAVE);
    expect(showToast).toHaveBeenCalledWith(
      'An edit is still being applied. Try again in a moment.',
      TOAST_KEYS.EXIT_EDIT_PENDING,
      'info',
    );
    expect(useUIStore.getState().activeDialog).toBe('unsaved-changes');
    expect(useUIStore.getState().dialogData).toEqual(QUIT_PROMPT);
    // The prompt is up, so the exit is not abandoned: Cancel still does that.
    expect(abandonUpdateRestartMock).not.toHaveBeenCalled();

    // The edit lands; the same button now saves it and quits.
    heldEdit?.resolve({ remote_link: null });
    await settle(editDone);
    await pressSaveAndQuit();

    expect(sent().indexOf(SAVE)).toBeGreaterThan(sent().indexOf(EDIT));
    expect(performQuitMock).toHaveBeenCalledTimes(1);
  });

  it('an edit that lands during the wait is saved before the quit', async () => {
    const { done: editDone } = await startEdit();
    render(<Host />);

    fireEvent.click(screen.getByTestId('unsaved-changes-save-quit'));
    await vi.advanceTimersByTimeAsync(1000);
    // The save has not been sent while the edit is still on its way.
    expect(sent()).not.toContain(SAVE);
    heldEdit?.resolve({ remote_link: null });
    await settle(editDone);
    for (let i = 0; i < 20; i += 1) await vi.advanceTimersByTimeAsync(10);

    expect(performQuitMock).toHaveBeenCalledTimes(1);
    expect(sent().filter((c) => c === SAVE)).toHaveLength(1);
    expect(sent().indexOf(SAVE)).toBeGreaterThan(sent().indexOf(EDIT));
    expect(showToast).not.toHaveBeenCalledWith(expect.any(String), TOAST_KEYS.EXIT_EDIT_PENDING, expect.anything());
  });

  it('an edit queued while the save runs: does not quit, says so, puts the prompt back', async () => {
    heldSave = deferred<unknown>();
    render(<Host />);

    fireEvent.click(screen.getByTestId('unsaved-changes-save-quit'));
    await vi.advanceTimersByTimeAsync(0);
    expect(sent()).toContain(SAVE);
    const { done: editDone } = await startEdit();
    heldSave.resolve({ saved_path: FILE, warnings: [] });
    heldEdit?.resolve({ remote_link: null });
    await settle(editDone);
    for (let i = 0; i < 20; i += 1) await vi.advanceTimersByTimeAsync(10);

    expect(performQuitMock).not.toHaveBeenCalled();
    expect(showToast).toHaveBeenCalledWith(expect.any(String), TOAST_KEYS.EXIT_EDIT_PENDING, 'info');
    expect(useUIStore.getState().activeDialog).toBe('unsaved-changes');
    expect(useUIStore.getState().dialogData).toEqual(QUIT_PROMPT);
  });

  it('control: nothing in flight, Save & Quit saves and quits at once', async () => {
    render(<Host />);

    fireEvent.click(screen.getByTestId('unsaved-changes-save-quit'));
    for (let i = 0; i < 10; i += 1) await vi.advanceTimersByTimeAsync(10);

    expect(sent()).toContain(SAVE);
    expect(performQuitMock).toHaveBeenCalledTimes(1);
    expect(showToast).not.toHaveBeenCalledWith(expect.any(String), TOAST_KEYS.EXIT_EDIT_PENDING, expect.anything());
  });
});
