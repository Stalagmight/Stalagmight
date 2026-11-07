/**
 * [#576 RV4-R-S5] After a SpeleoDB in-place tree swap the unread-settings list is
 * re-read with the other open-file flags.
 *
 * The list of stored project settings an open could not read (B15) refuses every
 * save and drives the settings-unreadable banner. The backend re-sets it when a
 * swap reloads the file, but the frontend set it only from `loadProject`, which
 * the swap seam does not call: after a Restore of a revision whose settings are
 * unreadable, saves were refused with no banner, and after restoring a healthy
 * revision over an unreadable one the banner kept saying saving is off.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('../ipc');
stubGetErrorMessage(errorMessageOrString);

import { useProjectStore } from '../../store/projectStore';
import { speleoProjectSessionIssuedFor, useSpeleoStore } from '../../store/speleoStore';
import { applySpeleoTreeReplacement } from '../speleoTreeReplacement';
import { invokeIpcMock, stubGetErrorMessage, errorMessageOrString } from '../../test/factories/ipc';

function serveUnread(unreadableProjectSettings: string[]) {
  invokeIpcMock.mockImplementation(async (cmd: string) =>
    cmd === 'speleodb_open_file_flags'
      ? { newerSchemaVersion: null, speleoLinkUnreadable: false, unreadableProjectSettings }
      : undefined,
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

describe('applySpeleoTreeReplacement re-reads the unread settings (#576 RV4-R-S5)', () => {
  it('raises the list when the swapped-in file could not read its settings', async () => {
    useProjectStore.setState({ unreadableProjectSettings: [] });
    serveUnread(['validation', 'default_azimuth_reference']);

    await applySpeleoTreeReplacement(speleoProjectSessionIssuedFor('p1'), true);

    expect(useProjectStore.getState().unreadableProjectSettings).toEqual([
      'validation',
      'default_azimuth_reference',
    ]);
  });

  it('clears a stale list when the swapped-in file read them all', async () => {
    useProjectStore.setState({ unreadableProjectSettings: ['validation'] });
    serveUnread([]);

    await applySpeleoTreeReplacement(speleoProjectSessionIssuedFor('p1'), true);

    expect(useProjectStore.getState().unreadableProjectSettings).toEqual([]);
  });
});
