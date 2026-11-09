// @vitest-environment jsdom
/**
 * [#270] The FOUR ways to leave an open project must agree about which SpeleoDB
 * state deserves a prompt.
 *
 * `guardQuit` was deliberately re-keyed onto the WORK AT RISK (`hasUnpushedEdits`)
 * because requiring `editState === 'editing'` disarmed it in exactly the states where
 * the work is most stranded - `notifyReverifyDivergence()` drops to read-only while
 * keeping the edits, and any `ReLogin` error runs `clearSession()`, which does too.
 * `useProject.handleCloseProject` was left on the old lock-only predicate, so closing
 * a saved-but-unpushed remote project after a token loss returned straight to the home
 * screen with no notice at all - while quitting the same project prompted.
 *
 * Fixing those two left the OTHER two exits - opening another `.stalx` (Ctrl+O,
 * drag-drop, the in-project "Open Project") and New Project - still gating on the
 * disk-dirty bit alone, so a remote project saved to disk was swapped out with no
 * notice whatsoever. All four now share `needsSpeleoHandBackPrompt` /
 * `hasUnpushedSpeleoWork`, and the matrix below is the contract that keeps them
 * from drifting apart again.
 *
 * A held lock also prompts when clean, so every exit offers deliberate retention.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { renderHook } from '@testing-library/react';

vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ close: vi.fn(async () => {}), destroy: vi.fn(async () => {}) }),
}));
vi.mock('@tauri-apps/plugin-dialog', () => ({
  open: vi.fn(async () => null),
  save: vi.fn(async () => null),
  confirm: vi.fn(async () => false),
}));
vi.mock('../../utils/ipc', () => ({
  invokeIpc: vi.fn(),
  getErrorMessage: (_e: unknown, f?: string) => f ?? 'error',
}));

import { useProject } from '../useProject';
import { guardQuit, resetQuitConfirmationForTests } from '../../utils/quit';
import { routeOpenedFile } from '../../utils/openOrImport';
import { newProjectGuarded } from '../../utils/projectActions';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore, type SpeleoLinkState } from '../../store/speleoStore';
import { useUIStore } from '../../store/uiStore';

const link = (o: Partial<SpeleoLinkState>): SpeleoLinkState => ({
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
  ...o,
});

/** The other project the "open another .stalx" exit is asked for. */
const OTHER = '/cave/other.stalx';

let openDialog: ReturnType<typeof vi.fn>;
let closeProject: ReturnType<typeof vi.fn>;
let loadProject: ReturnType<typeof vi.fn>;

beforeEach(() => {
  openDialog = vi.fn();
  closeProject = vi.fn(async () => ({ success: true }));
  loadProject = vi.fn(async () => ({ success: true }));
  resetQuitConfirmationForTests();
  // Saved to disk throughout: the disk-dirty guard is a separate condition and would
  // mask which SpeleoDB predicate fired.
  useProjectStore.setState({
    project: { name: 'Cave A', file_path: '/cache/p1.stalx' } as never,
    isDirty: false,
    closeProject,
    loadProject,
  });
  useUIStore.setState({ openDialog: openDialog as never, activeDialog: null, dialogData: {} });
});

/** Did the close flow raise the SpeleoDB prompt (rather than closing)? */
const closePrompts = async (): Promise<boolean> => {
  const { result } = renderHook(() => useProject());
  await result.current.handleCloseProject();
  return openDialog.mock.calls.some((c) => c[0] === 'speleodb-close');
};

/** Did the quit flow raise the SpeleoDB prompt? */
const quitPrompts = (): boolean => {
  const guarded = guardQuit();
  return guarded && openDialog.mock.calls.some((c) => c[0] === 'speleodb-quit');
};

/**
 * Did opening ANOTHER `.stalx` over this project raise the prompt? This exit shares
 * the close dialog rather than having one of its own, and carries the chosen path
 * through it so the open is not swallowed by the prompt it raised.
 *
 * `routeOpenedFile` also raises an UNRELATED confirm before a clean, non-SpeleoDB
 * project gets swapped out (#424) - orthogonal to this file's subject (SpeleoDB
 * parity), but it fires on exactly the state most of these cases exercise (nothing
 * for SpeleoDB to hand back). Answer it the way the dialog's "Open" button does, so
 * the exit's actual outcome - the project loads, or doesn't - still shows up instead
 * of stalling on a dialog this suite isn't about.
 */
const openPrompts = async (): Promise<boolean> => {
  await routeOpenedFile(OTHER);
  const speleoPrompted = openDialog.mock.calls.some(
    (c) => c[0] === 'speleodb-close' && (c[1] as { pendingOpenPath?: string })?.pendingOpenPath === OTHER,
  );
  if (!speleoPrompted && openDialog.mock.calls.some((c) => c[0] === 'launch-open-confirm')) {
    await routeOpenedFile(OTHER, 'Open Failed', { swapConfirmed: true });
  }
  return speleoPrompted;
};

/** Did New Project over this project raise the prompt? Same dialog, same carry. */
const newPrompts = (): boolean => {
  newProjectGuarded();
  return openDialog.mock.calls.some(
    (c) => c[0] === 'speleodb-close' && (c[1] as { pendingNewProject?: boolean })?.pendingNewProject === true,
  );
};

/**
 * Run every exit against the state currently in the stores and report which of them
 * prompted. Each exit gets a clean `openDialog` so one exit's call cannot be read as
 * another's, and the destructive side effects (`closeProject`, `loadProject`) are
 * asserted alongside so "prompted" always means "and did NOT leave".
 */
const allExits = async (): Promise<{
  quit: boolean;
  close: boolean;
  open: boolean;
  new: boolean;
}> => {
  const quit = quitPrompts();
  openDialog.mockClear();
  const close = await closePrompts();
  openDialog.mockClear();
  const open = await openPrompts();
  openDialog.mockClear();
  const nu = newPrompts();
  openDialog.mockClear();
  return { quit, close, open, new: nu };
};

describe('all four exits agree on the SpeleoDB guard (#270)', () => {
  it('lock held + unpushed: ALL FOUR prompt', async () => {
    useSpeleoStore.setState({ link: link({ editState: 'editing', hasUnpushedEdits: true }) });
    expect(await allExits()).toEqual({ quit: true, close: true, open: true, new: true });
    expect(closeProject).not.toHaveBeenCalled();
    expect(loadProject).not.toHaveBeenCalled();
  });

  // The regression this file exists for. Read-only + unpushed is reached by a
  // re-verify divergence or a `clearSession()` after a dead token; the work is at its
  // most stranded there, and closing used to say nothing at all - as did opening
  // another project over it, which is the exit that then overwrites the shell.
  it('no lock + unpushed: ALL FOUR prompt', async () => {
    useSpeleoStore.setState({ link: link({ editState: 'read-only', hasUnpushedEdits: true }) });
    expect(await allExits()).toEqual({ quit: true, close: true, open: true, new: true });
    expect(closeProject).not.toHaveBeenCalled();
    expect(loadProject).not.toHaveBeenCalled();
  });

  it('no lock + nothing unpushed: NONE prompts', async () => {
    useSpeleoStore.setState({ link: link({ editState: 'read-only', hasUnpushedEdits: false }) });
    expect(await allExits()).toEqual({ quit: false, close: false, open: false, new: false });
    // Nothing to protect, so each exit actually happens.
    expect(closeProject).toHaveBeenCalled();
    expect(loadProject).toHaveBeenCalledWith(OTHER);
    expect(openDialog).not.toHaveBeenCalledWith('speleodb-close', expect.anything());
  });

  // All exits offer a deliberate local keep-lock choice, even on a clean copy.
  it('lock held + nothing unpushed: every exit offers the keep-lock prompt', async () => {
    useSpeleoStore.setState({ link: link({ editState: 'editing', hasUnpushedEdits: false }) });
    expect(await allExits()).toEqual({ quit: true, close: true, open: true, new: true });
    expect(closeProject).not.toHaveBeenCalled();
    expect(loadProject).not.toHaveBeenCalled();
  });

  it('a force-edit session with unpushed edits prompts on all four exits', async () => {
    // #213 force-editing holds NO lock, so `editState === 'editing'` alone is not
    // what catches this - the unpushed-work term is.
    useSpeleoStore.setState({
      link: link({ editState: 'editing', forceEditing: true, hasUnpushedEdits: true }),
    });
    expect(await allExits()).toEqual({ quit: true, close: true, open: true, new: true });
  });

  // A force-editor holds no lock (#213), so with nothing unpushed there is genuinely
  // nothing to decide. The close guard used to spell its lock term out as
  // `editState === 'editing'`, which is true here, and raised a prompt whose only
  // button was "Close anyway" - `holdsWriteLock` is the shared predicate that exists
  // precisely so that spelling cannot drift back in.
  it('a force-edit session with NOTHING unpushed prompts nowhere', async () => {
    useSpeleoStore.setState({
      link: link({ editState: 'editing', forceEditing: true, hasUnpushedEdits: false }),
    });
    expect(await allExits()).toEqual({ quit: false, close: false, open: false, new: false });
    expect(closeProject).toHaveBeenCalled();
    expect(loadProject).toHaveBeenCalledWith(OTHER);
  });

  it('a local project is untouched by any of the four SpeleoDB guards', async () => {
    useSpeleoStore.setState({ link: null });
    expect(await allExits()).toEqual({ quit: false, close: false, open: false, new: false });
    expect(closeProject).toHaveBeenCalled();
    expect(loadProject).toHaveBeenCalledWith(OTHER);
  });
});

describe('the disk-dirty guard still runs, but only after the SpeleoDB one', () => {
  // Ordering matters on every exit for the same reason it does at quit time: the
  // SpeleoDB prompt carries its own "save changes locally" checkbox and so subsumes
  // the disk one, while the reverse order would let "Discard Changes" walk straight
  // past a held lock and edits the server has never seen.
  beforeEach(() => {
    useProjectStore.setState({ isDirty: true });
  });

  it('prefers the SpeleoDB prompt over unsaved-changes on open and New Project', async () => {
    useSpeleoStore.setState({ link: link({ editState: 'editing', hasUnpushedEdits: true }) });

    await routeOpenedFile(OTHER);
    expect(openDialog).toHaveBeenCalledTimes(1);
    expect(openDialog).toHaveBeenCalledWith('speleodb-close', { pendingOpenPath: OTHER });

    openDialog.mockClear();
    newProjectGuarded();
    expect(openDialog).toHaveBeenCalledTimes(1);
    expect(openDialog).toHaveBeenCalledWith('speleodb-close', { pendingNewProject: true });
  });

  it('falls back to unsaved-changes for a local project', async () => {
    useSpeleoStore.setState({ link: null });

    await routeOpenedFile(OTHER);
    expect(openDialog).toHaveBeenCalledWith('unsaved-changes', { pendingOpenPath: OTHER });

    openDialog.mockClear();
    newProjectGuarded();
    expect(openDialog).toHaveBeenCalledWith('unsaved-changes', { pendingNewProject: true });
  });
});
