/**
 * Component test - the SpeleoDB project dialog shell: which tabs exist, which panel
 * a click swaps in, and the footer Save that the General tab's body no longer owns.
 *
 * The Save gate is the interesting part: it lives in the FOOTER but reads the tab
 * body's diff, so it must appear only on General, only for a user who may write, and
 * only once something actually changed.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

const invokeIpcMock = vi.hoisted(() => vi.fn());
vi.mock('../../utils/ipc', () => ({
  invokeIpc: invokeIpcMock,
  getErrorMessage: (e: unknown) => (e instanceof Error ? e.message : String(e)),
}));

import { SpeleoDbProjectDialog } from './SpeleoDbProjectDialog';
import { useSpeleoStore, type SpeleoLinkState } from '../../store/speleoStore';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import type { Permission, SpeleoProjectMetadata } from '../../types/ipc.generated';

const META: SpeleoProjectMetadata = {
  name: 'Cave A',
  description: 'desc',
  country: 'US',
  visibility: 'PRIVATE',
  latitude: null,
  longitude: null,
  exclude_geojson: false,
};

const LINK: SpeleoLinkState = {
  permission: 'READ_AND_WRITE',
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
};

/** The local file backing project A - the project every test below opens ON. */
const PATH_A = '/caves/a.stalx';

let closeDialog: ReturnType<typeof vi.fn>;
let showToast: ReturnType<typeof vi.fn>;

/** Every `speleodb_patch_project` the dialog has fired, with its arguments. */
const patchCalls = () => invokeIpcMock.mock.calls.filter(([cmd]) => cmd === 'speleodb_patch_project');

const seed = (permission: Permission = 'READ_AND_WRITE') =>
  useSpeleoStore.setState({
    link: { ...LINK, permission },
    remoteProjectId: 'p1',
    refreshLinkStatus: vi.fn(async () => {}),
  });

beforeEach(() => {
  invokeIpcMock.mockReset();
  invokeIpcMock.mockImplementation((cmd: string) => {
    if (cmd === 'speleodb_get_project_metadata') return Promise.resolve(META);
    if (cmd === 'speleodb_patch_project') return Promise.resolve({ ...META, name: 'Cave AB' });
    if (cmd === 'speleodb_get_revisions') return Promise.resolve([]);
    return Promise.resolve(null);
  });
  closeDialog = vi.fn();
  showToast = vi.fn();
  useSpeleoStore.setState({ link: null, remoteProjectId: null, session: null });
  useProjectStore.setState({ project: { name: 'Cave A', file_path: PATH_A } as never });
  useUIStore.setState({
    activeDialog: 'speleodb-project',
    dialogData: {},
    closeDialog,
    openDialog: vi.fn(),
    showToast,
  });
});

describe('SpeleoDbProjectDialog', () => {
  it('renders all three tabs for a remote project, starting on General', async () => {
    seed();
    render(<SpeleoDbProjectDialog />);
    for (const tk of ['general', 'history', 'admin']) {
      expect(screen.getByTestId(`speleodb-project-tab-${tk}`)).toBeInTheDocument();
    }
    expect(screen.getByTestId('speleodb-project-tab-general')).toHaveAttribute(
      'aria-selected',
      'true',
    );
    expect(await screen.findByTestId('speleodb-settings-tab')).toBeInTheDocument();
  });

  it('swaps the panel when another tab is clicked', async () => {
    seed('ADMIN');
    render(<SpeleoDbProjectDialog />);
    await screen.findByTestId('speleodb-settings-tab');

    await userEvent.click(screen.getByTestId('speleodb-project-tab-history'));
    expect(await screen.findByTestId('speleodb-revisions')).toBeInTheDocument();
    expect(screen.queryByTestId('speleodb-settings-tab')).toBeNull();

    await userEvent.click(screen.getByTestId('speleodb-project-tab-admin'));
    expect(screen.getByTestId('speleodb-admin-tab')).toBeInTheDocument();
    expect(screen.queryByTestId('speleodb-revisions')).toBeNull();

    await userEvent.click(screen.getByTestId('speleodb-project-tab-history'));
    expect(await screen.findByTestId('speleodb-revisions')).toBeInTheDocument();
  });

  it('seeds the initial tab from dialogData', async () => {
    seed('ADMIN');
    useUIStore.setState({ dialogData: { tab: 'admin' } });
    render(<SpeleoDbProjectDialog />);
    expect(await screen.findByTestId('speleodb-admin-tab')).toBeInTheDocument();
    expect(screen.getByTestId('speleodb-project-tab-admin')).toHaveAttribute(
      'aria-selected',
      'true',
    );
  });

  it('falls back to General for an unknown dialogData tab', async () => {
    seed();
    useUIStore.setState({ dialogData: { tab: 'nope' } });
    render(<SpeleoDbProjectDialog />);
    expect(await screen.findByTestId('speleodb-settings-tab')).toBeInTheDocument();
  });

  it('shows Save only on General, and only once the form differs', async () => {
    seed();
    render(<SpeleoDbProjectDialog />);
    const name = await screen.findByTestId('speleodb-settings-name');
    // Nothing changed yet: the footer button exists but is inert.
    expect(screen.getByTestId('speleodb-settings-save')).toBeDisabled();
    await userEvent.type(name, 'B');
    expect(screen.getByTestId('speleodb-settings-save')).toBeEnabled();

    // Every other tab writes to the server on click - no staged Save to offer.
    await userEvent.click(screen.getByTestId('speleodb-project-tab-history'));
    expect(screen.queryByTestId('speleodb-settings-save')).toBeNull();
    // The Close button is on every tab so the footer does not jump.
    expect(screen.getByTestId('speleodb-project-close')).toBeInTheDocument();
  });

  it('omits Save entirely for a read-only user', async () => {
    seed('READ_ONLY');
    render(<SpeleoDbProjectDialog />);
    expect(await screen.findByTestId('speleodb-settings-readonly-badge')).toBeInTheDocument();
    expect(screen.queryByTestId('speleodb-settings-save')).toBeNull();
    expect(screen.getByTestId('speleodb-project-close')).toBeInTheDocument();
  });

  it('closes from the footer', async () => {
    seed();
    render(<SpeleoDbProjectDialog />);
    await userEvent.click(screen.getByTestId('speleodb-project-close'));
    expect(closeDialog).toHaveBeenCalled();
  });

  /**
   * `closeOnBackdropClick={false}` signals "do not dismiss this by accident", but it
   * only ever covered the backdrop: Escape and the X ran straight through and threw
   * away staged General-tab metadata edits with no notice. DialogWrapper has the
   * confirm overlay built in; the dialog just never told it it was dirty.
   */
  it('asks before discarding staged metadata edits on Escape', async () => {
    seed();
    render(<SpeleoDbProjectDialog />);
    await userEvent.type(await screen.findByTestId('speleodb-settings-name'), 'B');

    await userEvent.keyboard('{Escape}');

    expect(screen.getByText('Unsaved Changes')).toBeInTheDocument();
    expect(closeDialog).not.toHaveBeenCalled();
  });

  it('does not ask when nothing was staged', async () => {
    seed();
    render(<SpeleoDbProjectDialog />);
    await screen.findByTestId('speleodb-settings-name');

    await userEvent.keyboard('{Escape}');

    expect(closeDialog).toHaveBeenCalled();
  });

  /**
   * The dialog is ABOUT the open project, but not one of the verbs it fires carries
   * a project id - `speleodb_patch_project` and friends are addressed backend-side
   * to whatever project is open NOW. `loadProject` leaves `activeDialog` alone and
   * `syncProjectLink` hands a remote -> remote switch a fresh NON-null link, so the
   * dialog survived the switch with its `link !== null`-keyed metadata load never
   * re-running: project A's form on screen, project B's metadata under the Save
   * button. Reachable via the held-lock overlay's "Continue editing".
   */
  describe('when the open project changes underneath it', () => {
    /**
     * What `loadProject` -> `syncProjectLink` commit for a remote -> remote switch:
     * a different local file, and a REPLACEMENT link that is still non-null (its
     * permission unknown until the next status poll re-seeds it).
     */
    const openProjectB = () =>
      act(() => {
        useProjectStore.setState({
          project: { name: 'Cave B', file_path: '/caves/b.stalx' } as never,
        });
        useSpeleoStore.setState({ link: { ...LINK, permission: null }, remoteProjectId: null });
      });

    it('cannot Save the loaded form against the project that replaced it', async () => {
      seed();
      render(<SpeleoDbProjectDialog />);
      await userEvent.type(await screen.findByTestId('speleodb-settings-name'), 'B');
      expect(screen.getByTestId('speleodb-settings-save')).toBeEnabled();

      openProjectB();
      // The status poll then resolves the NEW project's permission - which is what
      // used to bring Save back to life over the old project's form.
      act(() => {
        useSpeleoStore.setState({ link: { ...LINK, permission: 'READ_AND_WRITE' } });
      });

      // Whatever is left on screen, the one thing that must never happen is a PATCH
      // addressed to B carrying A's values. Press the button if it is still offered.
      const save = screen.queryByTestId('speleodb-settings-save');
      if (save) await userEvent.click(save);
      expect(patchCalls()).toEqual([]);
    });

    it('closes itself, and says why', async () => {
      seed();
      render(<SpeleoDbProjectDialog />);
      await screen.findByTestId('speleodb-settings-name');

      openProjectB();

      expect(closeDialog).toHaveBeenCalled();
      expect(showToast).toHaveBeenCalledWith(
        expect.stringContaining('another project was opened'),
        'speleodb-project-dialog-stale',
        'warning',
      );
      // Nothing of the old project is left to type into or act on - and that covers
      // every tab, not just General: the roster, revision and admin verbs are
      // addressed to the open project in exactly the same way.
      expect(screen.queryByTestId('speleodb-settings-tab')).toBeNull();
      expect(screen.queryByTestId('speleodb-project-tab-admin')).toBeNull();
    });

    /**
     * DO NOT REGRESS: the guard keys on the open FILE, so the churn that does NOT
     * change the open project must leave the dialog alone - the status poll hands out
     * a fresh link object every tick, and a metadata Save re-writes `project` with a
     * new object to fold the server-side rename in.
     */
    it('stays put when the project state is reallocated for the same file', async () => {
      seed();
      render(<SpeleoDbProjectDialog />);
      await screen.findByTestId('speleodb-settings-name');

      act(() => {
        useProjectStore.setState({
          project: { name: 'Renamed on the server', file_path: PATH_A } as never,
        });
        useSpeleoStore.setState({ link: { ...LINK, permission: 'READ_AND_WRITE' } });
      });

      expect(closeDialog).not.toHaveBeenCalled();
      expect(screen.getByTestId('speleodb-settings-tab')).toBeInTheDocument();
    });
  });

  /** DO NOT REGRESS: the ordinary same-project Save still reaches the server. */
  it('PATCHes the open project when nothing changed underneath it', async () => {
    seed();
    render(<SpeleoDbProjectDialog />);
    await userEvent.type(await screen.findByTestId('speleodb-settings-name'), 'B');

    await userEvent.click(screen.getByTestId('speleodb-settings-save'));

    await waitFor(() => expect(patchCalls()).toHaveLength(1));
    expect(patchCalls()).toEqual([
      ['speleodb_patch_project', { patch: expect.objectContaining({ name: 'Cave AB' }) }],
    ]);
    expect(closeDialog).not.toHaveBeenCalled();
  });

  it('says so - and asks the server nothing - when the project is not linked', async () => {
    render(<SpeleoDbProjectDialog />);
    expect(screen.getByTestId('speleodb-project-not-remote')).toBeInTheDocument();
    expect(screen.queryByTestId('speleodb-project-tab-general')).toBeNull();
    // The footer still renders so the dialog has a way out.
    expect(screen.getByTestId('speleodb-project-close')).toBeInTheDocument();
    await waitFor(() => expect(invokeIpcMock).not.toHaveBeenCalled());
  });
});
