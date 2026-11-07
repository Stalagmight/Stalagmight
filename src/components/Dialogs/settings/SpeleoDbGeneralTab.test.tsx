/**
 * Component test (#206) - the General tab of the SpeleoDB project dialog and the
 * metadata hook behind it.
 *
 * Covers the logic the pure Rust builder tests can't: the FE that decides WHAT goes
 * in the PATCH (only changed fields), the both-or-neither + range coordinate gating,
 * and the read-only permission gate.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

vi.mock('../../../utils/ipc');
stubGetErrorMessage(errorMessageOrString);

import { SpeleoDbGeneralTab } from './SpeleoDbGeneralTab';
import { useSpeleoProjectMetadata } from '../../../hooks/useSpeleoProjectMetadata';
import { useSpeleoStore } from '../../../store/speleoStore';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import type { Permission, SpeleoProjectMetadata } from '../../../types/ipc.generated';
import { invokeIpcMock, stubGetErrorMessage, errorMessageOrString } from '../../../test/factories/ipc';

const META: SpeleoProjectMetadata = {
  name: 'Cave A',
  description: 'desc',
  country: 'US',
  visibility: 'PRIVATE',
  latitude: null,
  longitude: null,
  exclude_geojson: false,
};

/** Stands in for SpeleoDbProjectDialog: it owns the hook, the body renders the form
 *  and the footer owns Save, so both read one diff. */
function Harness() {
  const metadata = useSpeleoProjectMetadata();
  return (
    <>
      <SpeleoDbGeneralTab metadata={metadata} />
      {metadata.canEdit && (
        <button
          type="button"
          disabled={!metadata.canSave}
          onClick={() => void metadata.handleSave()}
          data-testid="speleodb-settings-save"
        >
          save
        </button>
      )}
    </>
  );
}

const seedRemote = (permission: Permission) =>
  useSpeleoStore.setState({
    link: {
      permission,
      editState: 'read-only',
      lockHolder: null,
      lockedByOther: false,
      hasUnpushedEdits: false,
      unwound: false,
      forceEditing: false,
      forceEditAllowed: false,
      undoMode: 'full',
      remoteState: 'up_to_date',
      noAccess: false,
      serverError: null,
    },
  });

beforeEach(() => {
  invokeIpcMock.mockReset();
  useSpeleoStore.setState({ link: null, refreshLinkStatus: vi.fn(async () => {}) });
  useProjectStore.setState({ project: { name: 'Cave A' } as never });
  useUIStore.setState({ showToast: vi.fn() });
});

describe('SpeleoDbGeneralTab', () => {
  it('loads metadata on mount and PATCHes ONLY the changed field', async () => {
    invokeIpcMock.mockImplementation((cmd: string) => {
      if (cmd === 'speleodb_get_project_metadata') return Promise.resolve(META);
      if (cmd === 'speleodb_patch_project') return Promise.resolve({ ...META, name: 'Cave B' });
      return Promise.resolve(null);
    });
    seedRemote('READ_AND_WRITE');
    render(<Harness />);

    const nameInput = await screen.findByTestId('speleodb-settings-name');
    expect(nameInput).toHaveValue('Cave A');

    await userEvent.clear(nameInput);
    await userEvent.type(nameInput, 'Cave B');
    await userEvent.click(screen.getByTestId('speleodb-settings-save'));

    await waitFor(() => {
      const call = invokeIpcMock.mock.calls.find((c) => c[0] === 'speleodb_patch_project');
      expect(call).toBeTruthy();
      // Only `name` changed - every other field is null (unchanged), and the
      // untouched location pin is NOT edited.
      expect(call![1]).toEqual({
        patch: {
          name: 'Cave B',
          description: null,
          country: null,
          visibility: null,
          coordinates_edited: false,
          latitude: null,
          longitude: null,
        },
        expectedProject: null,
      });
    });
  });

  it('disables Save when only one coordinate is entered (both-or-neither)', async () => {
    invokeIpcMock.mockResolvedValue(META);
    seedRemote('READ_AND_WRITE');
    render(<Harness />);

    const lat = await screen.findByTestId('speleodb-settings-latitude');
    await userEvent.type(lat, '45');
    expect(screen.getByTestId('speleodb-settings-save')).toBeDisabled();
    expect(screen.getByTestId('speleodb-settings-coords-error')).toBeInTheDocument();
  });

  it('disables Save for an out-of-range coordinate pair', async () => {
    invokeIpcMock.mockResolvedValue(META);
    seedRemote('READ_AND_WRITE');
    render(<Harness />);

    const lat = await screen.findByTestId('speleodb-settings-latitude');
    const lon = screen.getByTestId('speleodb-settings-longitude');
    await userEvent.type(lat, '999'); // out of [-90, 90]
    await userEvent.type(lon, '0');
    expect(screen.getByTestId('speleodb-settings-save')).toBeDisabled();
    expect(screen.getByTestId('speleodb-settings-coords-error')).toBeInTheDocument();
  });

  it('is read-only (badge, disabled fields, no Save) without write permission', async () => {
    invokeIpcMock.mockResolvedValue(META);
    seedRemote('READ_ONLY');
    render(<Harness />);

    expect(await screen.findByTestId('speleodb-settings-readonly-badge')).toBeInTheDocument();
    expect(screen.getByTestId('speleodb-settings-name')).toBeDisabled();
    expect(screen.queryByTestId('speleodb-settings-save')).toBeNull();
  });

  it('offers a retry after a failed load', async () => {
    let attempt = 0;
    invokeIpcMock.mockImplementation(() => {
      attempt += 1;
      return attempt === 1 ? Promise.reject(new Error('offline')) : Promise.resolve(META);
    });
    seedRemote('READ_AND_WRITE');
    render(<Harness />);

    await userEvent.click(await screen.findByTestId('speleodb-settings-retry'));
    expect(await screen.findByTestId('speleodb-settings-name')).toHaveValue('Cave A');
  });

  it('keeps the local undo setting available when server metadata is offline', async () => {
    invokeIpcMock.mockRejectedValue(new Error('offline'));
    seedRemote('READ_AND_WRITE');
    useSpeleoStore.setState((state) => ({
      link: state.link ? { ...state.link, editState: 'editing' } : null,
    }));
    render(<Harness />);

    expect(await screen.findByTestId('speleodb-project-settings-error')).toBeInTheDocument();
    expect(screen.getByTestId('speleodb-undo-mode')).toBeInTheDocument();
    expect(screen.getByTestId('speleodb-undo-mode-disabled-input')).toBeEnabled();
  });

  it('does not call the server for a project with no SpeleoDB link', async () => {
    invokeIpcMock.mockResolvedValue(META);
    render(<Harness />);

    await waitFor(() => expect(screen.queryByTestId('speleodb-settings-loading')).toBeNull());
    expect(invokeIpcMock).not.toHaveBeenCalled();
  });
});
