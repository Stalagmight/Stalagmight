// @vitest-environment jsdom
/**
 * [#576 FZ-SDB-5-D3] The refused-open prompt over a LINK-LESS copy at the project's
 * cache path (`unpushed_local.unlinked`), left by an Unlink made before Unlink moved
 * copies out of the cache. Keep-local then runs the move-out: the Save As first (its
 * Cancel leaves the prompt up and calls nothing), the copy opened through
 * `keepLocalCopy` (its hand-back guard), `speleodb_unlink` to the chosen path, and
 * the moved copy loaded.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

const saveDialogMock = vi.hoisted(() => vi.fn());
vi.mock('../../../utils/ipc');
stubGetErrorMessage(fallbackOr('error'));
vi.mock('../../../utils/fileDialogs', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../utils/fileDialogs')>()),
  saveDialog: saveDialogMock,
}));
vi.mock('@tauri-apps/api/path', () => ({ documentDir: async () => '/home/diver/Documents' }));

import SpeleoUnpushedLocalDialog from '../SpeleoUnpushedLocalDialog';
import { useProjectStore } from '../../../store/projectStore';
import { useSpeleoStore } from '../../../store/speleoStore';
import { useUIStore } from '../../../store/uiStore';
import { invokeIpcMock, stubGetErrorMessage, fallbackOr } from '../../../test/factories/ipc';

const SUMMARY = {
  id: 'p1',
  name: 'Cenote Azul',
  permission: 'READ_AND_WRITE',
  lock_holder: null,
  type_tag: 'stalx',
  latest_commit_sha: 'abc123',
};

const LEGACY = {
  project_id: 'p1',
  local_path: '/cache/p1/project.stalx',
  last_edit_at: '2026-07-20T10:30:00Z',
  unlinked: true,
};

const CHOSEN = '/home/diver/Documents/stalagmight/Cenote_Azul.stalx';

let order: string[];
let loadProjectMock: ReturnType<typeof vi.fn>;
let closeDialogMock: ReturnType<typeof vi.fn>;

beforeEach(() => {
  order = [];
  saveDialogMock.mockReset();
  saveDialogMock.mockResolvedValue(CHOSEN);
  invokeIpcMock.mockReset();
  invokeIpcMock.mockImplementation(async (cmd: string, args?: unknown) => {
    if (cmd === 'speleodb_unlink') {
      order.push(`unlink ${JSON.stringify(args)}`);
      return { saved_path: CHOSEN, unlinked_copy_path: null, warnings: [] };
    }
    return undefined;
  });
  loadProjectMock = vi.fn(async (path: string) => {
    order.push(`load ${path}`);
    return { success: true };
  });
  closeDialogMock = vi.fn();
  useProjectStore.setState({ loadProject: loadProjectMock, project: null, isDirty: false });
  useSpeleoStore.setState({
    // `keepLocalCopy`'s optimistic seed: it marks the open going through that path.
    setLinkOnOpen: vi.fn(() => order.push('seed')),
    refreshLinkStatus: vi.fn(async () => {
      order.push('refresh');
    }),
    clearLink: vi.fn(),
    clearSession: vi.fn(),
  });
  useUIStore.setState({
    closeDialog: closeDialogMock,
    openDialog: vi.fn(),
    showToast: vi.fn(),
    dialogData: { summary: SUMMARY, unpushedLocal: LEGACY },
  });
});

const speleoCalls = () =>
  invokeIpcMock.mock.calls.filter((c) => String(c[0]).startsWith('speleodb_'));

describe('SpeleoUnpushedLocalDialog over a link-less cache copy (#576 FZ-SDB-5-D3)', () => {
  it('says why keeping it moves it, and only for a link-less copy', () => {
    const { unmount } = render(<SpeleoUnpushedLocalDialog />);
    expect(screen.getByTestId('speleodb-unpushed-local-unlinked-hint').textContent).toContain(
      'moves it out of the cache',
    );
    unmount();

    useUIStore.setState({ dialogData: { summary: SUMMARY, unpushedLocal: { ...LEGACY, unlinked: false } } });
    render(<SpeleoUnpushedLocalDialog />);
    expect(screen.getByTestId('speleodb-unpushed-local-dialog')).toBeInTheDocument();
    expect(screen.queryByTestId('speleodb-unpushed-local-unlinked-hint')).toBeNull();
  });

  it('a cancelled Save As keeps the prompt open and calls nothing', async () => {
    saveDialogMock.mockResolvedValue(null);
    render(<SpeleoUnpushedLocalDialog />);
    await userEvent.click(screen.getByTestId('speleodb-unpushed-local-keep'));

    // The Save As was asked, in the default projects folder with the name filled in.
    await waitFor(() => expect(saveDialogMock).toHaveBeenCalledTimes(1));
    expect(saveDialogMock.mock.calls[0]![0]).toMatchObject({ defaultPath: CHOSEN });
    // Busy released, nothing opened, nothing called, the prompt still up.
    await waitFor(() => expect(screen.getByTestId('speleodb-unpushed-local-keep')).not.toBeDisabled());
    expect(screen.getByTestId('speleodb-unpushed-local-dialog')).toBeInTheDocument();
    expect(closeDialogMock).not.toHaveBeenCalled();
    expect(loadProjectMock).not.toHaveBeenCalled();
    expect(speleoCalls()).toEqual([]);
    expect(order).toEqual([]);
  });

  it('opens the copy through keepLocalCopy, moves it to the chosen path, then loads the moved copy', async () => {
    render(<SpeleoUnpushedLocalDialog />);
    await userEvent.click(screen.getByTestId('speleodb-unpushed-local-keep'));

    await waitFor(() => expect(order).toContain(`load ${CHOSEN}`));
    expect(order).toEqual([
      'seed',
      'load /cache/p1/project.stalx',
      `unlink ${JSON.stringify({ destination: CHOSEN, expectedProject: null })}`,
      'refresh',
      `load ${CHOSEN}`,
    ]);
    // The one SpeleoDB command is the move; nothing was downloaded.
    expect(speleoCalls().map(([cmd]) => cmd)).toEqual(['speleodb_unlink']);
  });

  it("stops at keepLocalCopy's hand-back refusal and never unlinks", async () => {
    // Another project, with unsaved edits, is on screen: opening the copy would drop them.
    useProjectStore.setState({ project: { file_path: '/home/diver/other.stalx' } as never, isDirty: true });
    render(<SpeleoUnpushedLocalDialog />);
    await userEvent.click(screen.getByTestId('speleodb-unpushed-local-keep'));

    await waitFor(() => expect(screen.getByTestId('speleodb-unpushed-local-error')).toBeInTheDocument());
    expect(loadProjectMock).not.toHaveBeenCalled();
    expect(speleoCalls()).toEqual([]);
    expect(screen.getByTestId('speleodb-unpushed-local-keep')).not.toBeDisabled();
  });

  it('a linked copy keeps the plain keep-local: no Save As, no unlink', async () => {
    useUIStore.setState({ dialogData: { summary: SUMMARY, unpushedLocal: { ...LEGACY, unlinked: false } } });
    render(<SpeleoUnpushedLocalDialog />);
    await userEvent.click(screen.getByTestId('speleodb-unpushed-local-keep'));

    await waitFor(() => expect(loadProjectMock).toHaveBeenCalledWith('/cache/p1/project.stalx'));
    expect(saveDialogMock).not.toHaveBeenCalled();
    expect(speleoCalls()).toEqual([]);
  });
});
