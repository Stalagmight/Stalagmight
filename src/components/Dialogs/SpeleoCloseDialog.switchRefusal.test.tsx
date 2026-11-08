/**
 * Component test - [#576 W87a-NF1] the close prompt's local save, refused because a
 * project switch is under way.
 *
 * Closing (or opening another project over, or starting a new one over) a disk-dirty
 * SpeleoDB project with "Save locally" ticked saves the .stalx first and hands the
 * project back only once that save succeeds. A save requested while a switch is under
 * way is refused (`switchGate.ts`), and `surfaceSaveFailure` stays silent on that
 * refusal on purpose. The prompt then stayed open with no word of why. It now says so
 * with the quiet notice an edit refused during a switch gets, as its sibling
 * `SpeleoQuitDialog` does (W87a-4), and stays usable: once the switch has settled the
 * same button saves and closes.
 *
 * The real store's `saveProject` and the real switch gate; the IPC is the shared
 * mock. All data is synthetic.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

vi.mock('../../utils/ipc');

import SpeleoCloseDialog from './SpeleoCloseDialog';
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
  showToast.mockReset();
  useProjectStore.setState({
    project: makeProject({ created_at: '2026-03-03T10:00:00Z', file_path: FILE }),
    isDirty: true,
    isClosing: false,
    integrityIssues: [],
  });
  useSpeleoStore.setState({
    link: makeSpeleoLink({ editState: 'editing', hasUnpushedEdits: true }),
    banner: null,
    busy: false,
  });
  useUIStore.setState({ activeDialog: 'speleodb-close', dialogData: {}, showToast });
  releaseSwitch = noteSwitchRequested();
});

afterEach(() => {
  releaseSwitch?.();
  releaseSwitch = null;
});

describe('SpeleoCloseDialog - a local save refused during a project switch (#576 W87a-NF1)', () => {
  it('says to wait for the project, keeps the prompt usable, and closes on the next press once the switch settled', async () => {
    render(<SpeleoCloseDialog />);
    expect(screen.getByTestId('speleodb-close-save-locally')).toBeChecked();

    await userEvent.click(screen.getByTestId('speleodb-close-keep-lock'));

    await waitFor(() =>
      expect(showToast).toHaveBeenCalledWith(
        'Wait for the project to finish opening.',
        TOAST_KEYS.EDIT_DURING_SWITCH,
        'info',
      ),
    );
    // Refused before any IPC: nothing was saved, nothing handed back, the prompt is still up.
    expect(sentCommands()).not.toContain('save_project');
    expect(sentCommands()).not.toContain('close_project');
    expect(useUIStore.getState().activeDialog).toBe('speleodb-close');
    // ...and usable: no verb is left marked in flight.
    expect(screen.getByTestId('speleodb-close-keep-lock')).toBeEnabled();
    expect(screen.getByTestId('speleodb-close-push-release')).toBeEnabled();
    expect(screen.getByTestId('speleodb-close-cancel')).toBeEnabled();

    releaseSwitch?.();
    releaseSwitch = null;
    await userEvent.click(screen.getByTestId('speleodb-close-keep-lock'));

    await waitFor(() => expect(sentCommands()).toContain('close_project'));
    expect(sentCommands().indexOf('save_project')).toBeLessThan(sentCommands().indexOf('close_project'));
  });

  it('gives the same notice on the push-and-release path, before any upload', async () => {
    render(<SpeleoCloseDialog />);

    await userEvent.click(screen.getByTestId('speleodb-close-push-release'));

    await waitFor(() =>
      expect(showToast).toHaveBeenCalledWith(
        'Wait for the project to finish opening.',
        TOAST_KEYS.EDIT_DURING_SWITCH,
        'info',
      ),
    );
    expect(sentCommands()).toEqual([]);
    expect(useUIStore.getState().activeDialog).toBe('speleodb-close');
    expect(screen.getByTestId('speleodb-close-push-release')).toBeEnabled();
  });

  it("the backend's own PROJECT_CHANGED gets a user-started verb's short line, and nothing is handed back", async () => {
    releaseSwitch?.();
    releaseSwitch = null;
    invokeIpcMock.mockImplementation(async (command: string) => {
      if (command === 'save_project') throw { code: 'PROJECT_CHANGED', message: 'another project is open' };
      if (command === 'check_project_integrity') return { messages: [], save_blocked: false };
      return {};
    });
    render(<SpeleoCloseDialog />);

    await userEvent.click(screen.getByTestId('speleodb-close-keep-lock'));

    await waitFor(() => expect(showToast).toHaveBeenCalledWith(expect.any(String), TOAST_KEYS.PROJECT_CHANGED, 'warning'));
    expect(sentCommands()).not.toContain('close_project');
    expect(useUIStore.getState().activeDialog).toBe('speleodb-close');
  });
});
