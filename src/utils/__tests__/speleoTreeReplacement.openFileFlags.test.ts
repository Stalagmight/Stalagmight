/**
 * [#576 NF-W2F-1] After a SpeleoDB in-place tree swap the open file's flags are
 * re-read, so the newer-format banner/gate and the unreadable-link notice describe
 * the bytes now open, not the previous ones.
 *
 * Revision View/Restore, fast-forward and take-remote reopen DIFFERENT bytes at the
 * SAME path in the backend; the frontend reconciles through
 * `applySpeleoTreeReplacement` and never calls `loadProject`, which is the only
 * other place these two flags are set.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('../ipc');
stubGetErrorMessage(errorMessageOrString);

import { useProjectStore } from '../../store/projectStore';
import { speleoProjectSessionIssuedFor, useSpeleoStore } from '../../store/speleoStore';
import { applySpeleoTreeReplacement } from '../speleoTreeReplacement';
import { invokeIpcMock, stubGetErrorMessage, errorMessageOrString } from '../../test/factories/ipc';

function serveFlags(flags: { newerSchemaVersion: number | null; speleoLinkUnreadable: boolean }) {
  invokeIpcMock.mockImplementation(async (cmd: string) =>
    cmd === 'speleodb_open_file_flags' ? flags : undefined,
  );
}

beforeEach(() => {
  invokeIpcMock.mockReset();
  useProjectStore.setState({
    refreshProjectState: vi.fn(async () => {}),
    checkProjectIntegrity: vi.fn(async () => []),
    checkHistoryDiscarded: vi.fn(async () => {}),
  });
  useSpeleoStore.setState({
    session: null,
    sessionGeneration: 0,
    remoteProjectId: 'p1',
    refreshLinkStatus: vi.fn(async () => {}),
  });
});

describe('applySpeleoTreeReplacement re-reads the open file flags (#576 NF-W2F-1)', () => {
  it('clears a newer-format and unreadable-link state the swapped-in file does not have', async () => {
    useProjectStore.setState({ newerSchemaVersion: 99, speleoLinkUnreadable: true });
    serveFlags({ newerSchemaVersion: null, speleoLinkUnreadable: false });

    const applied = await applySpeleoTreeReplacement(speleoProjectSessionIssuedFor('p1'), true);

    expect(applied).toBe(true);
    expect(useProjectStore.getState().newerSchemaVersion).toBeNull();
    expect(useProjectStore.getState().speleoLinkUnreadable).toBe(false);
  });

  it('raises them when the swapped-in file is a newer build\'s', async () => {
    useProjectStore.setState({ newerSchemaVersion: null, speleoLinkUnreadable: false });
    serveFlags({ newerSchemaVersion: 99, speleoLinkUnreadable: false });

    await applySpeleoTreeReplacement(speleoProjectSessionIssuedFor('p1'), true);

    expect(useProjectStore.getState().newerSchemaVersion).toBe(99);
  });

  it('keeps the previous flags when they cannot be read', async () => {
    useProjectStore.setState({ newerSchemaVersion: 99, speleoLinkUnreadable: false });
    invokeIpcMock.mockRejectedValue(new Error('backend unavailable'));
    vi.spyOn(console, 'error').mockImplementation(() => {});

    await applySpeleoTreeReplacement(speleoProjectSessionIssuedFor('p1'), true);

    expect(useProjectStore.getState().newerSchemaVersion).toBe(99);
  });
});
