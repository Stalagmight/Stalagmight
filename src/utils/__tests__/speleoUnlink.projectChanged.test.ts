/**
 * [#576 Q-W44-7-D1] Unlink is a verb the user started: refused because the project
 * changed under it (the backend's PROJECT_CHANGED - a load committed while the Save As
 * was up), its Detach Failed error dialog says so in the short line. It used to go
 * through the save router, which drops that refusal as quiet the way it does for a
 * save, so the user who asked for the move saw nothing at all. Nothing else runs:
 * no link re-seed, no load.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('../ipc');
stubGetErrorMessage(fallbackOr('error'));

import { unlinkOpenProjectTo } from '../speleoUnlink';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { useUIStore } from '../../store/uiStore';
import { IpcCommandError } from '../commandError';
import i18n from '../../i18n';
import { invokeIpcMock, stubGetErrorMessage, fallbackOr } from '../../test/factories/ipc';

const SHORT = 'The project changed before this finished.';
const MADE_FOR = { createdAt: '2026-07-11T00:00:00Z', filePath: '/cache/p1/project.stalx' };

let openDialog: ReturnType<typeof vi.fn>;
let loadProject: ReturnType<typeof vi.fn>;
let refreshLinkStatus: ReturnType<typeof vi.fn>;

beforeEach(() => {
  invokeIpcMock.mockReset();
  openDialog = vi.fn();
  loadProject = vi.fn(async () => ({ success: true }));
  refreshLinkStatus = vi.fn(async () => {});
  useUIStore.setState({ openDialog } as never);
  useProjectStore.setState({ project: { name: 'Cave A' }, loadProject } as never);
  useSpeleoStore.setState({ refreshLinkStatus } as never);
});

describe('unlinkOpenProjectTo: a refusal because the project changed (#576 Q-W44-7-D1)', () => {
  it('opens its error dialog with the short line, and does nothing else', async () => {
    invokeIpcMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'speleodb_unlink') {
        throw new IpcCommandError({
          code: 'PROJECT_CHANGED',
          message: 'Unlinking was not applied: another project is open now.',
        });
      }
      return undefined;
    });

    expect(await unlinkOpenProjectTo('/home/diver/Cave_A.stalx', MADE_FOR)).toBe(false);

    expect(openDialog).toHaveBeenCalledTimes(1);
    expect(openDialog).toHaveBeenCalledWith('error', {
      message: SHORT,
      title: i18n.t('dialogs.saveFailure.detachTitle'),
    });
    expect(refreshLinkStatus).not.toHaveBeenCalled();
    expect(loadProject).not.toHaveBeenCalled();
  });
});
