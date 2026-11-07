/**
 * Component test (#207) - the Create-on-SpeleoDB Publish dialog.
 *
 * Covers the submit gating (name + country + valid coords), the `speleodb_publish`
 * payload, and the load-guard: a failed reopen must NOT claim success or close.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor, fireEvent } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

vi.mock('../../utils/ipc');
// The real reader's shape: a crash inside the dialog (RV6a-6) must reach the toast as text.
stubGetErrorMessage((e, f) => (e instanceof Error ? e.message : (f ?? String(e))));

// jsdom does not implement scrollIntoView; the Select component calls it on open.
Element.prototype.scrollIntoView = vi.fn();

/** Pick `value` from a portal-based `Select` (src/components/common/Select.tsx):
 *  it isn't a native `<select>`, so this clicks the trigger, then the option
 *  rendered at `data-testid="${testId}-option-${value}"`. */
async function pickOption(testId: string, value: string) {
  await userEvent.click(screen.getByTestId(testId));
  await userEvent.click(screen.getByTestId(`${testId}-option-${encodeURIComponent(value)}`));
}

import PublishToSpeleoDbDialog from './PublishToSpeleoDbDialog';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { useUIStore } from '../../store/uiStore';
import { TOAST_KEYS } from '../../utils/toastKeys';
import i18n from '../../i18n';
import { invokeIpcMock, stubGetErrorMessage } from '../../test/factories/ipc';
import type { ValidationWarning } from '../../types/ipc/ValidationWarning';

/** `speleodb_publish`'s answer as the wire carries it (`SpeleoPublishReport`, #576
 *  RV6a-6): the outcome and what its saves could not keep (nothing here). */
const published = () => ({
  result: { project_id: 'p1', endpoint: 'https://x', local_path: '/cache/p1/project.stalx' },
  save_warnings: [],
});

let loadProjectMock: ReturnType<typeof vi.fn>;
let closeDialogMock: ReturnType<typeof vi.fn>;
let showToastMock: ReturnType<typeof vi.fn>;

beforeEach(() => {
  invokeIpcMock.mockReset();
  loadProjectMock = vi.fn(async () => ({ success: true }));
  closeDialogMock = vi.fn();
  showToastMock = vi.fn();
  useProjectStore.setState({
    project: { name: 'My Cave', zero_datums: [] } as never,
    loadProject: loadProjectMock,
  });
  useSpeleoStore.setState({
    refreshLinkStatus: vi.fn(async () => {}),
    clearSession: vi.fn(),
  });
  useUIStore.setState({ closeDialog: closeDialogMock, showToast: showToastMock });
});

describe('PublishToSpeleoDbDialog', () => {
  it('gates Publish on a chosen country and publishes with the right params', async () => {
    invokeIpcMock.mockResolvedValue(published());
    render(<PublishToSpeleoDbDialog />);

    // Name prefilled from the project; country still unchosen -> Publish disabled.
    expect(screen.getByTestId('publish-to-speleodb-dialog-name')).toHaveValue('My Cave');
    expect(screen.getByTestId('publish-to-speleodb-dialog-submit')).toBeDisabled();

    await pickOption('publish-to-speleodb-dialog-country', 'US');
    const submit = screen.getByTestId('publish-to-speleodb-dialog-submit');
    // Country chosen but description still empty -> stays disabled (server requires it).
    expect(submit).toBeDisabled();
    expect(screen.getByTestId('publish-to-speleodb-dialog-description-error')).toBeInTheDocument();

    fireEvent.change(screen.getByTestId('publish-to-speleodb-dialog-description'), { target: { value: 'Main line survey' } });
    expect(screen.queryByTestId('publish-to-speleodb-dialog-description-error')).toBeNull();
    await waitFor(() => expect(submit).toBeEnabled());
    await userEvent.click(submit);

    await waitFor(() => {
      const call = invokeIpcMock.mock.calls.find((c) => c[0] === 'speleodb_publish');
      expect(call).toBeTruthy();
      expect(call![1]).toEqual({
        params: {
          name: 'My Cave',
          description: 'Main line survey',
          country: 'US',
          visibility: 'PRIVATE',
          latitude: null,
          longitude: null,
        },
      });
    });
    // Reloaded the now-remote project and closed on success.
    await waitFor(() => expect(loadProjectMock).toHaveBeenCalledWith('/cache/p1/project.stalx'));
    await waitFor(() => expect(closeDialogMock).toHaveBeenCalled());
  });

  it("shows what publish's saves could not keep, like Done and Push (#576 NF-W4M-2)", async () => {
    invokeIpcMock.mockResolvedValue({
      result: { project_id: 'p1', endpoint: 'https://x', local_path: '/cache/p1/project.stalx' },
      save_warnings: [
        { code: 'DISPLAY_SETTINGS_NOT_SAVED', message: 'display settings not kept', severity: 'warning' } satisfies ValidationWarning,
      ],
    });
    render(<PublishToSpeleoDbDialog />);
    await pickOption('publish-to-speleodb-dialog-country', 'US');
    fireEvent.change(screen.getByTestId('publish-to-speleodb-dialog-description'), { target: { value: 'desc' } });
    await userEvent.click(screen.getByTestId('publish-to-speleodb-dialog-submit'));

    await waitFor(() => expect(loadProjectMock).toHaveBeenCalledWith('/cache/p1/project.stalx'));
    const keys = showToastMock.mock.calls.map((c) => c[1]);
    expect(keys).toContain(`${TOAST_KEYS.SAVE_WARNING}:DISPLAY_SETTINGS_NOT_SAVED`);
    await waitFor(() => expect(closeDialogMock).toHaveBeenCalled());
  });

  it('refuses an answer that is not the wire shape instead of loading from it (#576 RV6a-6)', async () => {
    // The bare outcome, without its envelope: what the stale doubles used to return,
    // and what a regression in the command would send. It must fail loudly, not
    // be read as a publish that lost nothing.
    invokeIpcMock.mockResolvedValue({ project_id: 'p1', endpoint: 'https://x', local_path: '/cache/p1/project.stalx' });
    render(<PublishToSpeleoDbDialog />);
    await pickOption('publish-to-speleodb-dialog-country', 'US');
    fireEvent.change(screen.getByTestId('publish-to-speleodb-dialog-description'), { target: { value: 'desc' } });
    await userEvent.click(screen.getByTestId('publish-to-speleodb-dialog-submit'));

    await waitFor(() => expect(showToastMock).toHaveBeenCalled());
    const kinds = showToastMock.mock.calls.map((c) => [c[1], c[2]]);
    console.log(`[RV6a-6] bare answer: loadProject calls=${loadProjectMock.mock.calls.length} toasts=${JSON.stringify(kinds)}`);
    expect(loadProjectMock).not.toHaveBeenCalled();
    expect(kinds).toEqual([[TOAST_KEYS.SPELEODB_PUBLISH, 'error']]);
    expect(closeDialogMock).not.toHaveBeenCalled();
  });

  it('a project a newer Stalagmight saved reads as the newer-version line, not "Not saved" (#576 NF-W9BE-1-D1)', async () => {
    // Publish saves the project into the SpeleoDB cache first, and the backend refuses
    // that save with NEWER_FILE_READ_ONLY. Decided: the toast reads like Start editing's
    // and Update's refusal of such a project, not like a refused save.
    invokeIpcMock.mockRejectedValue({
      code: 'NEWER_FILE_READ_ONLY',
      message: 'Saving is disabled: this project was saved by a newer version of Stalagmight (file format 99)',
    });
    render(<PublishToSpeleoDbDialog />);
    await pickOption('publish-to-speleodb-dialog-country', 'US');
    fireEvent.change(screen.getByTestId('publish-to-speleodb-dialog-description'), { target: { value: 'desc' } });
    await userEvent.click(screen.getByTestId('publish-to-speleodb-dialog-submit'));

    await waitFor(() => expect(showToastMock).toHaveBeenCalled());
    const [message, key, kind] = showToastMock.mock.calls[0] ?? [];
    console.log(`[NF-W9BE-1-D1] publish toast: ${String(message)}`);
    const noPeriod = (s: string) => s.replace(/\.\s*$/, '');
    expect(message).toBe(`${i18n.t('speleodb.publish.failed')}: ${noPeriod(i18n.t('banners.newerFile.lead'))}`);
    expect(message).not.toContain(noPeriod(i18n.t('banners.newerFile.saveRefused')));
    expect([key, kind]).toEqual([TOAST_KEYS.SPELEODB_PUBLISH, 'error']);
    expect(closeDialogMock).not.toHaveBeenCalled();
  });

  it('does NOT close or claim success when the reopen fails', async () => {
    invokeIpcMock.mockResolvedValue(published());
    loadProjectMock.mockResolvedValue({ success: false });
    render(<PublishToSpeleoDbDialog />);

    await pickOption('publish-to-speleodb-dialog-country', 'US');
    fireEvent.change(screen.getByTestId('publish-to-speleodb-dialog-description'), { target: { value: 'desc' } });
    await userEvent.click(screen.getByTestId('publish-to-speleodb-dialog-submit'));

    await waitFor(() => expect(loadProjectMock).toHaveBeenCalled());
    expect(closeDialogMock).not.toHaveBeenCalled();
  });

  it('blocks submit on an out-of-range coordinate pair', async () => {
    render(<PublishToSpeleoDbDialog />);
    await pickOption('publish-to-speleodb-dialog-country', 'US');
    await userEvent.type(screen.getByTestId('publish-to-speleodb-dialog-latitude'), '999');
    await userEvent.type(screen.getByTestId('publish-to-speleodb-dialog-longitude'), '0');
    expect(screen.getByTestId('publish-to-speleodb-dialog-submit')).toBeDisabled();
    expect(screen.getByTestId('publish-to-speleodb-dialog-coords-error')).toBeInTheDocument();
  });

  it('prefills country and coordinates from the first georeferenced datum (#217)', async () => {
    // A Yucatan cenote datum should offline-resolve the required country to MX.
    useProjectStore.setState({
      project: {
        name: 'Cenote',
        zero_datums: [{ gps: { latitude: 20.21, longitude: -87.46 } }],
      } as never,
      loadProject: loadProjectMock,
    });
    render(<PublishToSpeleoDbDialog />);
    expect(screen.getByTestId('publish-to-speleodb-dialog-country')).toHaveTextContent('Mexico');
    expect(screen.getByTestId('publish-to-speleodb-dialog-latitude')).toHaveValue(20.21);
    expect(screen.getByTestId('publish-to-speleodb-dialog-longitude')).toHaveValue(-87.46);
    // Country + coords + name are prefilled, but a description is still required, so
    // Publish stays disabled until one is entered.
    expect(screen.getByTestId('publish-to-speleodb-dialog-submit')).toBeDisabled();
    fireEvent.change(screen.getByTestId('publish-to-speleodb-dialog-description'), { target: { value: 'Cenote line' } });
    await waitFor(() =>
      expect(screen.getByTestId('publish-to-speleodb-dialog-submit')).toBeEnabled(),
    );
  });

  it('leaves country empty when the datum resolves to no country (open ocean)', () => {
    useProjectStore.setState({
      project: {
        name: 'Blue Hole',
        zero_datums: [{ gps: { latitude: 30.0, longitude: -40.0 } }],
      } as never,
      loadProject: loadProjectMock,
    });
    render(<PublishToSpeleoDbDialog />);
    expect(screen.getByTestId('publish-to-speleodb-dialog-country')).toHaveTextContent('Select a country');
  });

  it('autocompletes a collaborator, then grants it after the project is published', async () => {
    invokeIpcMock.mockImplementation((cmd: string) => {
      if (cmd === 'speleodb_search_users') {
        return Promise.resolve([{ email: 'alice@x.com', label: 'Alice Diver <alice@x.com>' }]);
      }
      if (cmd === 'speleodb_publish') {
        return Promise.resolve(published());
      }
      // speleodb_add_collaborator -> the (unused-here) refreshed roster.
      return Promise.resolve({ users: [], teams: [] });
    });
    render(<PublishToSpeleoDbDialog />);
    await pickOption('publish-to-speleodb-dialog-country', 'US');
    fireEvent.change(screen.getByTestId('publish-to-speleodb-dialog-description'), { target: { value: 'desc' } });

    // Pick a suggestion from the debounced typeahead, choose a level, and queue it.
    await userEvent.type(screen.getByTestId('publish-to-speleodb-dialog-collab-email'), 'ali');
    await userEvent.click(await screen.findByTestId('publish-to-speleodb-dialog-collab-suggestion-alice@x.com'));
    expect(screen.getByTestId('publish-to-speleodb-dialog-collab-email')).toHaveValue('alice@x.com');
    await pickOption('publish-to-speleodb-dialog-collab-add-level', 'READ_AND_WRITE');
    await userEvent.click(screen.getByTestId('publish-to-speleodb-dialog-collab-add'));
    expect(screen.getByTestId('publish-to-speleodb-dialog-collab-alice@x.com')).toBeInTheDocument();

    await userEvent.click(screen.getByTestId('publish-to-speleodb-dialog-submit'));

    // The grant lands only after the create, with the queued email + level.
    await waitFor(() => {
      const call = invokeIpcMock.mock.calls.find((c) => c[0] === 'speleodb_add_collaborator');
      expect(call).toBeTruthy();
      expect(call![1]).toEqual({ email: 'alice@x.com', permission: 'READ_AND_WRITE' });
    });
    await waitFor(() => expect(closeDialogMock).toHaveBeenCalled());
  });

  it('still closes but warns when a queued collaborator cannot be granted', async () => {
    invokeIpcMock.mockImplementation((cmd: string) => {
      if (cmd === 'speleodb_publish') {
        return Promise.resolve(published());
      }
      if (cmd === 'speleodb_add_collaborator') return Promise.reject(new Error('403'));
      return Promise.resolve({ users: [], teams: [] });
    });
    render(<PublishToSpeleoDbDialog />);
    await pickOption('publish-to-speleodb-dialog-country', 'US');
    fireEvent.change(screen.getByTestId('publish-to-speleodb-dialog-description'), { target: { value: 'desc' } });
    await userEvent.type(screen.getByTestId('publish-to-speleodb-dialog-collab-email'), 'bob@x.com');
    await userEvent.click(screen.getByTestId('publish-to-speleodb-dialog-collab-add'));
    await userEvent.click(screen.getByTestId('publish-to-speleodb-dialog-submit'));

    await waitFor(() => expect(closeDialogMock).toHaveBeenCalled());
    // The failure surfaced as the partial-warning toast, not the plain success one.
    const toasts = showToastMock.mock.calls.map((c) => String(c[0]));
    expect(toasts.some((m) => m.includes('bob@x.com'))).toBe(true);
    expect(toasts).not.toContain('Project published to SpeleoDB.');
  });

  it('grants the FINAL queued level after changing a collaborator row in place', async () => {
    invokeIpcMock.mockImplementation((cmd: string) => {
      if (cmd === 'speleodb_publish') {
        return Promise.resolve(published());
      }
      // speleodb_search_users (unused - added by typing) + add_collaborator roster.
      return Promise.resolve({ users: [], teams: [] });
    });
    render(<PublishToSpeleoDbDialog />);
    await pickOption('publish-to-speleodb-dialog-country', 'US');
    fireEvent.change(screen.getByTestId('publish-to-speleodb-dialog-description'), { target: { value: 'desc' } });

    // Queue carol at the default level, then change the QUEUED row's level.
    fireEvent.change(screen.getByTestId('publish-to-speleodb-dialog-collab-email'), {
      target: { value: 'carol@x.com' },
    });
    await userEvent.click(screen.getByTestId('publish-to-speleodb-dialog-collab-add'));
    await pickOption('publish-to-speleodb-dialog-collab-level-carol@x.com', 'ADMIN');

    await userEvent.click(screen.getByTestId('publish-to-speleodb-dialog-submit'));

    await waitFor(() => {
      const call = invokeIpcMock.mock.calls.find((c) => c[0] === 'speleodb_add_collaborator');
      expect(call).toBeTruthy();
      expect(call![1]).toEqual({ email: 'carol@x.com', permission: 'ADMIN' });
    });
  });

  it('removes a queued collaborator so it is never granted', async () => {
    invokeIpcMock.mockImplementation((cmd: string) => {
      if (cmd === 'speleodb_publish') {
        return Promise.resolve(published());
      }
      return Promise.resolve({ users: [], teams: [] });
    });
    render(<PublishToSpeleoDbDialog />);
    await pickOption('publish-to-speleodb-dialog-country', 'US');
    fireEvent.change(screen.getByTestId('publish-to-speleodb-dialog-description'), { target: { value: 'desc' } });

    fireEvent.change(screen.getByTestId('publish-to-speleodb-dialog-collab-email'), {
      target: { value: 'dave@x.com' },
    });
    await userEvent.click(screen.getByTestId('publish-to-speleodb-dialog-collab-add'));
    expect(screen.getByTestId('publish-to-speleodb-dialog-collab-dave@x.com')).toBeInTheDocument();

    await userEvent.click(screen.getByTestId('publish-to-speleodb-dialog-collab-remove-dave@x.com'));
    expect(screen.queryByTestId('publish-to-speleodb-dialog-collab-dave@x.com')).toBeNull();

    await userEvent.click(screen.getByTestId('publish-to-speleodb-dialog-submit'));

    await waitFor(() => expect(closeDialogMock).toHaveBeenCalled());
    // Nothing queued -> no grant call, and the plain success toast (not the partial one).
    expect(invokeIpcMock.mock.calls.find((c) => c[0] === 'speleodb_add_collaborator')).toBeUndefined();
    const toasts = showToastMock.mock.calls.map((c) => String(c[0]));
    expect(toasts).toContain('Project published to SpeleoDB.');
  });
});
