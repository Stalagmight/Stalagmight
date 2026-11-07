import { beforeEach, afterEach, describe, expect, it, vi } from 'vitest';

import { useProjectStore } from '../projectStore';
import { useUIStore } from '../uiStore';
import { TOAST_KEYS } from '../../utils/toastKeys';
import { surfaceSaveFailure } from '../../utils/saveFeedback';
import { invokeIpcMock } from '../../test/factories/ipc';
import type { ValidationWarning } from '../../types/ipc/ValidationWarning';

vi.mock('../../utils/ipc');

/**
 * [#576 A3-D2, NF-AN2-1] A save that SUCCEEDS can still have gone without
 * something: the undo history it could not write (dropped from the file), the
 * display settings it could not carry, or a file it could not reopen afterwards.
 * The backend says so on `CommandResult.warnings`; before this, nothing on the
 * frontend read that list except `DATABASE_NOT_REOPENED`'s gate, so the user was
 * told only "saved".
 */
describe('saveProject surfaces what a successful save went without', () => {
  let showToast: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    vi.clearAllMocks();
    showToast = vi.fn();
    useUIStore.setState({ showToast, openDialog: vi.fn() } as never);
    useProjectStore.setState({
      project: { file_path: '/CaveA.stalx' } as never,
      isDirty: true,
      autosaveFailure: null,
      integrityIssues: [],
      integritySaveBlocked: false,
      refreshUndoState: vi.fn().mockResolvedValue(undefined),
      refreshProjectState: vi.fn().mockResolvedValue(undefined),
      checkProjectIntegrity: vi.fn().mockResolvedValue(undefined),
    } as never);
  });

  afterEach(() => {
    useProjectStore.setState({ project: null, isDirty: false, autosaveFailure: null } as never);
  });

  const saveReturns = (result: Record<string, unknown>) => {
    invokeIpcMock.mockImplementation(async (command: string) =>
      command === 'save_project' ? result : undefined,
    );
  };

  // Typed, so tsc holds `severity` to the wire's lower-case WarningSeverity.
  const warning = (code: string, message = `${code} message`): ValidationWarning => ({
    code,
    message,
    severity: 'warning',
  });

  it.each([
    ['UNDO_HISTORY_DROPPED', /undo history/i],
    ['DISPLAY_SETTINGS_NOT_SAVED', /display settings/i],
    ['DATABASE_NOT_REOPENED', /could not be reopened/i],
  ])('shows a warning toast for %s, on a manual save and an autosave', async (code, text) => {
    for (const isAutosave of [false, true]) {
      showToast.mockClear();
      useProjectStore.setState({ isDirty: true } as never);
      saveReturns({ success: true, saved_path: '/CaveA.stalx', warnings: [warning(code)] });

      await useProjectStore.getState().saveProject(undefined, isAutosave);

      expect(showToast).toHaveBeenCalledTimes(1);
      expect(showToast).toHaveBeenCalledWith(
        expect.stringMatching(text),
        `${TOAST_KEYS.SAVE_WARNING}:${code}`,
        'warning',
      );
      // Words, never the raw code.
      expect(showToast).not.toHaveBeenCalledWith(
        expect.stringContaining(code),
        expect.anything(),
        expect.anything(),
      );
      // Still a successful save.
      expect(useProjectStore.getState().isDirty).toBe(false);
    }
  });

  it('reports each warning once, under its own key', async () => {
    saveReturns({
      success: true,
      saved_path: '/CaveA.stalx',
      warnings: [warning('DISPLAY_SETTINGS_NOT_SAVED'), warning('UNDO_HISTORY_DROPPED')],
    });

    await useProjectStore.getState().saveProject();

    expect(showToast.mock.calls.map((call) => call[1])).toEqual([
      `${TOAST_KEYS.SAVE_WARNING}:DISPLAY_SETTINGS_NOT_SAVED`,
      `${TOAST_KEYS.SAVE_WARNING}:UNDO_HISTORY_DROPPED`,
    ]);
  });

  it('still reaches the user for a code this build has no words for', async () => {
    saveReturns({
      success: true,
      saved_path: '/CaveA.stalx',
      warnings: [warning('SOMETHING_NEW', 'The project was saved without X.')],
    });

    await useProjectStore.getState().saveProject();

    expect(showToast).toHaveBeenCalledTimes(1);
    expect(showToast).toHaveBeenCalledWith(
      expect.stringContaining('The project was saved without X.'),
      expect.any(String),
      'warning',
    );
  });

  it('tells a Save a Copy what the copy went without, too', async () => {
    saveReturns({
      success: true,
      unlinked_copy_path: '/copy.stalx',
      warnings: [warning('DISPLAY_SETTINGS_NOT_SAVED')],
    });

    await useProjectStore.getState().saveProject('/copy.stalx');

    const keys = showToast.mock.calls.map((call) => call[1]);
    expect(keys).toContain(TOAST_KEYS.SPELEODB_UNLINKED_COPY);
    expect(keys).toContain(`${TOAST_KEYS.SAVE_WARNING}:DISPLAY_SETTINGS_NOT_SAVED`);
  });

  it('says nothing extra for a clean save', async () => {
    saveReturns({ success: true, saved_path: '/CaveA.stalx', warnings: [] });

    await useProjectStore.getState().saveProject();

    expect(showToast).not.toHaveBeenCalled();
  });
});

/**
 * [#576 A3-D2] The SpeleoDB link and the undo-history markers now STOP a save they
 * cannot read, with codes of their own. Like the ignore list's refusal (#517),
 * Review & Repair cannot fix either, so the refusal must reach the error dialog
 * even on a project whose integrity check is also unhappy - with the backend's
 * sentence on screen.
 */
describe('a refused save over an unreadable side table', () => {
  it.each([
    'SPELEODB_LINK_UNREADABLE',
    'UNDO_MARKERS_UNREADABLE',
  ] as const)('%s opens the error dialog with the sentence, never Review & Repair', (code) => {
    const openDialog = vi.fn();
    const openIntegrityDialog = vi.fn();
    useProjectStore.setState({ integritySaveBlocked: true, openIntegrityDialog } as never);
    const sentence =
      "Can't save: this project's SpeleoDB link could not be read (x). Saving would rewrite " +
      'the file without it, so nothing was written - the file on disk is unchanged.';

    surfaceSaveFailure({ code, error: sentence }, openDialog, useProjectStore.getState());

    expect(openIntegrityDialog).not.toHaveBeenCalled();
    expect(openDialog).toHaveBeenCalledWith('error', expect.objectContaining({ message: sentence }));
    useProjectStore.setState({ integritySaveBlocked: false } as never);
  });
});
