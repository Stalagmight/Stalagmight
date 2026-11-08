/**
 * Component test - [#576 W87a-4] the quit prompt's local save, refused because a
 * project switch is under way.
 *
 * Quitting a disk-dirty SpeleoDB project with "Save locally" ticked saves the .stalx
 * first and quits only once that save succeeds. A save requested while a switch is
 * under way is refused (`switchGate.ts`), and `surfaceSaveFailure` stays silent on
 * that refusal on purpose (a switch is the user's own action, not a failed save). The
 * prompt then stayed open with no word of why: the user pressed a button and nothing
 * happened. It now says so with the quiet notice an edit refused during a switch
 * gets, and stays usable: once the switch has settled the same button saves and quits.
 *
 * The real store's `saveProject` and the real switch gate; the IPC is the shared
 * mock. All data is synthetic.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

vi.mock('../../utils/ipc');

const performQuitMock = vi.hoisted(() => vi.fn());
vi.mock('../../utils/quit', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../utils/quit')>()),
  performQuit: performQuitMock,
}));

import SpeleoQuitDialog from './SpeleoQuitDialog';
import { noteSwitchRequested } from '../../store/switchGate';
import { useSpeleoStore } from '../../store/speleoStore';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import { TOAST_KEYS } from '../../utils/toastKeys';
import { emptyBinaryRefresh, invokeIpcMock } from '../../test/factories/ipc';
import { makeSpeleoLink } from '../../test/factories/speleo';
import { makeProject } from '../../test/factories/survey';

emptyBinaryRefresh();

const FILE = '/caves/CaveA.stalx';
const showToast = vi.fn();
const sentCommands = () => invokeIpcMock.mock.calls.map((c) => c[0] as string);

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
  showToast.mockReset();
  useProjectStore.setState({
    project: makeProject({ created_at: '2026-03-03T10:00:00Z', file_path: FILE }),
    isDirty: true,
    integrityIssues: [],
  });
  useSpeleoStore.setState({
    link: makeSpeleoLink({ editState: 'editing', hasUnpushedEdits: true }),
    banner: null,
    busy: false,
  });
  useUIStore.setState({ activeDialog: 'speleodb-quit', dialogData: {}, showToast });
  releaseSwitch = noteSwitchRequested();
});

afterEach(() => {
  releaseSwitch?.();
  releaseSwitch = null;
});

describe('SpeleoQuitDialog - a local save refused during a project switch (#576 W87a-4)', () => {
  it('says to wait for the project, keeps the prompt usable, and quits on the next press once the switch settled', async () => {
    render(<SpeleoQuitDialog />);
    expect(screen.getByTestId('speleodb-quit-save-locally')).toBeChecked();

    await userEvent.click(screen.getByTestId('speleodb-quit-keep-lock'));

    await waitFor(() =>
      expect(showToast).toHaveBeenCalledWith(
        'Wait for the project to finish opening.',
        TOAST_KEYS.EDIT_DURING_SWITCH,
        'info',
      ),
    );
    // Refused before any IPC: nothing was saved, nothing quit, the prompt is still up.
    expect(sentCommands()).not.toContain('save_project');
    expect(performQuitMock).not.toHaveBeenCalled();
    expect(useUIStore.getState().activeDialog).toBe('speleodb-quit');
    // ...and usable: no verb is left marked in flight.
    expect(screen.getByTestId('speleodb-quit-keep-lock')).toBeEnabled();
    expect(screen.getByTestId('speleodb-quit-push-release')).toBeEnabled();

    releaseSwitch?.();
    releaseSwitch = null;
    await userEvent.click(screen.getByTestId('speleodb-quit-keep-lock'));

    await waitFor(() => expect(performQuitMock).toHaveBeenCalledTimes(1));
    expect(sentCommands()).toContain('save_project');
  });

  it('gives the same notice on the push-and-release path, before any upload', async () => {
    render(<SpeleoQuitDialog />);

    await userEvent.click(screen.getByTestId('speleodb-quit-push-release'));

    await waitFor(() =>
      expect(showToast).toHaveBeenCalledWith(
        'Wait for the project to finish opening.',
        TOAST_KEYS.EDIT_DURING_SWITCH,
        'info',
      ),
    );
    expect(sentCommands()).toEqual([]);
    expect(performQuitMock).not.toHaveBeenCalled();
    expect(screen.getByTestId('speleodb-quit-push-release')).toBeEnabled();
  });
});
