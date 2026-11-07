/**
 * #576 NF-W2RO-1 / RV3-FE-14: Project Settings and a backend refusal.
 *
 * NF-W2RO-1: a staged elevation the backend refuses as read-only (EDIT_DISABLED: the
 * write lock was lost, or the workspace switched to Plan/Export after the draft was
 * typed) toasted the raw coded string - on top of the friendly read-only notice the
 * store's edit path already shows. One notice, the friendly one.
 *
 * RV3-FE-14: while the stored thresholds are unread, an untouched Save re-sends them
 * so the user can keep the shown ones - but on a project whose metadata cannot be
 * changed at all (a newer-version file, a SpeleoDB project without the lock) the
 * backend refuses that, and the user who changed nothing got an error and a dialog
 * that would not close. There, an untouched Save sends nothing and closes, as it did
 * before the unread-settings lift.
 *
 * The real store's edit path with the IPC mocked, so the notice is the one the app
 * shows. `.tsx` for jsdom.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

vi.mock('../../../utils/ipc');

import i18n from '../../../i18n';
import { ProjectSettingsDialog as Dialog } from '../ProjectSettingsDialog';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { useSpeleoStore } from '../../../store/speleoStore';
import { defaultConfig } from '../../../types/config';
import { TOAST_KEYS } from '../../../utils/toastKeys';
import type { Project } from '../../../types/project';
import { invokeIpcMock } from '../../../test/factories/ipc';

/** The backend's edit-gate refusal (#576 C4): a rejection with its code. */
const LOCK_REFUSAL = {
  code: 'EDIT_DISABLED',
  message: 'Set entrance elevation is disabled: this SpeleoDB project is open read-only. Acquire the lock to edit.',
};

let closeDialog: ReturnType<typeof vi.fn>;

const toasts = () => useUIStore.getState().toasts.map((t) => [t.message, t.key]);

beforeEach(() => {
  vi.clearAllMocks();
  invokeIpcMock.mockImplementation(async (command: string) => {
    if (command === 'set_surface_elevation' || command === 'update_project_settings') {
      throw LOCK_REFUSAL;
    }
    return undefined;
  });
  closeDialog = vi.fn();
  useProjectStore.setState({
    project: {
      name: 'CaveB',
      location: null,
      created_at: '2026-01-01T00:00:00Z',
      validation: defaultConfig.validation,
      surface_elevation: null,
    } as unknown as Project,
    config: defaultConfig,
    newerSchemaVersion: null,
    unreadableProjectSettings: [],
    refreshProjectState: vi.fn().mockResolvedValue(undefined),
    checkProjectIntegrity: vi.fn().mockResolvedValue([]),
  });
  useUIStore.setState({ closeDialog, toasts: [], activeDialog: 'project-settings', geometryMode: 'measured', appMode: 'survey' });
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
});

describe('Project Settings and a read-only refusal (NF-W2RO-1)', () => {
  it('shows the friendly read-only notice once, not the coded EDIT_DISABLED string', async () => {
    render(<Dialog />);
    await userEvent.type(screen.getByTestId('project-settings-elevation'), '12');
    await userEvent.click(screen.getByTestId('project-settings-save'));

    expect(toasts().some(([message]) => String(message).startsWith('EDIT_DISABLED')), 'raw coded refusal toasted').toBe(
      false
    );
    expect(toasts()).toEqual([[i18n.t('editGate.readOnlyNotice'), TOAST_KEYS.EDIT_DISABLED]]);
    // Refused: the dialog stays open with the staged draft.
    expect(closeDialog).not.toHaveBeenCalled();
    expect(screen.getByTestId('project-settings-elevation')).toHaveProperty('value', '12');
  });
});

describe('an untouched Save on a project whose metadata is read-only (RV3-FE-14)', () => {
  it('sends nothing for the unread thresholds of a newer-version file, and closes', async () => {
    useProjectStore.setState({ newerSchemaVersion: 99, unreadableProjectSettings: ['validation'] });
    render(<Dialog />);
    await userEvent.click(screen.getByTestId('project-settings-save'));

    expect(invokeIpcMock).not.toHaveBeenCalledWith('update_project_settings', expect.anything(), expect.anything());
    expect(toasts()).toEqual([]);
    expect(closeDialog).toHaveBeenCalledTimes(1);
  });

  it('nor on a SpeleoDB project open without the lock', async () => {
    useProjectStore.setState({ unreadableProjectSettings: ['validation'] });
    useSpeleoStore.setState({ link: null, projectLinkKind: 'remote' } as never);
    render(<Dialog />);
    await userEvent.click(screen.getByTestId('project-settings-save'));

    expect(invokeIpcMock).not.toHaveBeenCalledWith('update_project_settings', expect.anything(), expect.anything());
    expect(toasts()).toEqual([]);
    expect(closeDialog).toHaveBeenCalledTimes(1);
  });
});
