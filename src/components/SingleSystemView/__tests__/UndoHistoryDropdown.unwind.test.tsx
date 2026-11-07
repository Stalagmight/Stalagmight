/**
 * [#303] Component test - the Unwind entry pinned below the undo history list.
 *
 * Spec items 17, 18 and 25. What makes it worth testing at this tier: the entry is the
 * ONLY trigger for an action that downloads a revision and replaces the working copy, so
 * three things have to hold together - it appears only where the backend would accept it,
 * pressing it never acts (it opens a confirmation), and the ordinary Undo control is
 * unchanged by its existence.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

vi.mock('../../../utils/ipc');
stubGetErrorMessage(errorMessageOrString);

import UndoHistoryDropdown from '../UndoHistoryDropdown';
import { useSpeleoStore } from '../../../store/speleoStore';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { accepted } from '../../../store/commandResult';
import type { HistoryOutcome } from '../../../types/ipc/HistoryOutcome';
import { invokeIpcMock, stubGetErrorMessage, errorMessageOrString } from '../../../test/factories/ipc';

const originalGetRedoStack = useProjectStore.getState().getRedoStack;
const originalUndo = useProjectStore.getState().undo;
const originalRedo = useProjectStore.getState().redo;

/** A remote link whose write lock we hold (or not), which is what gates the entry. */
const seedLink = (editing: boolean, forceEditing = false) =>
  useSpeleoStore.setState({
    projectLinkKind: 'remote',
    link: {
      permission: 'READ_AND_WRITE',
      editState: editing ? 'editing' : 'read-only',
      lockHolder: null,
      lockedByOther: false,
      hasUnpushedEdits: false,
      unwound: false,
      forceEditing,
      forceEditAllowed: false,
      undoMode: 'full',
      remoteState: 'up_to_date',
      noAccess: false,
      serverError: null,
    },
  });

const TARGET = { hexsha: 'abcdef0123456789', is_current: true, discards_unpushed: true };

/** The undo trigger, with the stack empty (`canUndo === false`) unless told otherwise. */
function renderUndo(opts: { disabled?: boolean; hoverable?: boolean } = {}) {
  const onSingleAction = vi.fn();
  render(
    <UndoHistoryDropdown
      mode="undo"
      disabled={opts.disabled ?? true}
      hoverableWhenEmpty={opts.hoverable ?? true}
      onSingleAction={onSingleAction}
      testId="toolbar-undo-button"
      ariaLabel="Undo"
    >
      Undo
    </UndoHistoryDropdown>,
  );
  return { onSingleAction };
}

/** Hover the trigger and wait for the 300ms open delay. */
async function hoverOpen() {
  await userEvent.hover(screen.getByTestId('toolbar-undo-button'));
  await waitFor(
    () => expect(screen.getByTestId('toolbar-undo-button-dropdown')).toBeInTheDocument(),
    { timeout: 2000 },
  );
}

describe('UndoHistoryDropdown - the Unwind entry (#303)', () => {
  beforeEach(() => {
    invokeIpcMock.mockReset();
    invokeIpcMock.mockImplementation((cmd: string) => {
      // An EMPTY undo stack is the state the entry exists for.
      if (cmd === 'get_undo_stack' || cmd === 'get_redo_stack') return Promise.resolve([]);
      if (cmd === 'speleodb_unwind_target') return Promise.resolve(TARGET);
      return Promise.resolve(null);
    });
    useSpeleoStore.setState({ projectLinkKind: 'unknown', link: null });
    useProjectStore.setState({
      canUndo: false,
      canRedo: false,
      getRedoStack: originalGetRedoStack,
      undo: originalUndo,
      redo: originalRedo,
    });
    useUIStore.setState({ activeDialog: null, dialogData: {} });
  });

  it('appears on an empty stack when the write lock is held (spec 17)', async () => {
    seedLink(true);
    renderUndo();

    await hoverOpen();

    await waitFor(() =>
      expect(screen.getByTestId('toolbar-undo-button-unwind')).toBeInTheDocument(),
    );
    // ...and the panel says what is left rather than a bare "nothing to undo".
    expect(screen.getByText(/Unwind below to load earlier history/)).toBeInTheDocument();
  });

  it('is absent without the write lock (spec 17)', async () => {
    seedLink(false);
    renderUndo();
    await userEvent.hover(screen.getByTestId('toolbar-undo-button'));
    // Nothing to show at all: no items, no unwind, so the panel never opens.
    await new Promise((r) => setTimeout(r, 500));
    expect(screen.queryByTestId('toolbar-undo-button-dropdown')).not.toBeInTheDocument();
    expect(invokeIpcMock).not.toHaveBeenCalledWith('speleodb_unwind_target');
  });

  it('is absent for a force-editor, who holds no lock the backend accepts (spec 17)', async () => {
    // #213: force-editing is edit-enabled WITHOUT a lock, and the backend refuses both
    // the swap and the keep-history waiver there - so the entry must not be offered.
    seedLink(true, true);
    renderUndo();
    await userEvent.hover(screen.getByTestId('toolbar-undo-button'));
    await new Promise((r) => setTimeout(r, 500));
    expect(screen.queryByTestId('toolbar-undo-button-unwind')).not.toBeInTheDocument();
  });

  it('is absent for a local project - there is no history to walk (spec 17)', async () => {
    useSpeleoStore.setState({ projectLinkKind: 'local', link: null });
    renderUndo();
    await userEvent.hover(screen.getByTestId('toolbar-undo-button'));
    await new Promise((r) => setTimeout(r, 500));
    expect(screen.queryByTestId('toolbar-undo-button-unwind')).not.toBeInTheDocument();
  });

  // CHANGED: this used to hover the panel OPEN and then assert the entry was missing from
  // it. Opening was the bug (#303 F08): the hover path opened on an empty stack whenever
  // the lock was held, resolved the target only afterwards, and had nothing that closed
  // the panel again - so a linked project with no undo history popped a bare "No undo
  // actions." panel on every pass of the cursor. The target is now resolved BEFORE
  // opening, so "no reachable revision" means no panel at all.
  it('opens nothing at all when the backend reports no reachable revision (spec 21)', async () => {
    seedLink(true);
    invokeIpcMock.mockImplementation((cmd: string) => {
      if (cmd === 'get_undo_stack' || cmd === 'get_redo_stack') return Promise.resolve([]);
      // The root of the history, an empty repo, or Disabled mode.
      if (cmd === 'speleodb_unwind_target') return Promise.resolve(null);
      return Promise.resolve(null);
    });
    renderUndo();

    await userEvent.hover(screen.getByTestId('toolbar-undo-button'));
    // It still ASKS - that is the whole point of asking first.
    await waitFor(() => expect(invokeIpcMock).toHaveBeenCalledWith('speleodb_unwind_target'));
    await new Promise((r) => setTimeout(r, 300));
    expect(screen.queryByTestId('toolbar-undo-button-dropdown')).not.toBeInTheDocument();
    expect(screen.queryByTestId('toolbar-undo-button-unwind')).not.toBeInTheDocument();
  });

  // [#303 F08] The mitigation that pays for asking on the hover path: the resolved target
  // is memoised for 5s, so a cursor crossing the toolbar cannot turn into one
  // `GET /revisions/` per pass.
  it('asks once for a run of hovers, not once per hover', async () => {
    seedLink(true);
    invokeIpcMock.mockImplementation((cmd: string) => {
      if (cmd === 'get_undo_stack' || cmd === 'get_redo_stack') return Promise.resolve([]);
      if (cmd === 'speleodb_unwind_target') return Promise.resolve(null);
      return Promise.resolve(null);
    });
    const countOf = (cmd: string) =>
      invokeIpcMock.mock.calls.filter((c: unknown[]) => c[0] === cmd).length;
    renderUndo();

    const trigger = screen.getByTestId('toolbar-undo-button');
    await userEvent.hover(trigger);
    await waitFor(() => expect(countOf('speleodb_unwind_target')).toBe(1));

    await userEvent.unhover(trigger);
    await userEvent.hover(trigger);
    // The second hover reaches its 300ms timer and refetches the (cheap, local) stack...
    await waitFor(() => expect(countOf('get_undo_stack')).toBeGreaterThan(1));
    await new Promise((r) => setTimeout(r, 100));
    // ...but answers the (remote) target from the memo.
    expect(countOf('speleodb_unwind_target')).toBe(1);
  });

  it('never acts on click: it opens the confirmation (spec 18)', async () => {
    seedLink(true);
    renderUndo();
    await hoverOpen();
    await waitFor(() =>
      expect(screen.getByTestId('toolbar-undo-button-unwind')).toBeInTheDocument(),
    );

    await userEvent.click(screen.getByTestId('toolbar-undo-button-unwind'));

    // The dialog is raised with the target, and NOTHING was restored.
    expect(useUIStore.getState().activeDialog).toBe('speleodb-unwind');
    expect(useUIStore.getState().dialogData).toMatchObject({
      hexsha: TARGET.hexsha,
      isCurrent: true,
      // [#303] Carried through, or the confirmation cannot warn on open and the user
      // meets the consequence as a failure after pressing instead.
      discardsUnpushed: true,
      // [#303 F11] The other direction's flag, stated rather than left `undefined`.
      // `speleodb_unwind_target` has no `is_latest` to report; the target is a union
      // discriminated on direction, so the backward arm cannot be read for one.
      isLatest: false,
      forward: false,
    });
    expect(invokeIpcMock).not.toHaveBeenCalledWith(
      'speleodb_restore_revision',
      expect.anything(),
    );
  });

  // CHANGED: this used to assert the trigger carried the DOM `disabled` attribute. It no
  // longer does when `hoverableWhenEmpty` is set (#303 F09) - a disabled button is out of
  // the tab order, which left the panel behind it with no keyboard route at all. What
  // spec 25 actually promises is that the control PERFORMS nothing, and that is asserted
  // unchanged below; `handleButtonClick` still returns early on `disabled`.
  it('leaves the ordinary Undo control inert on an empty stack (spec 25)', async () => {
    seedLink(true);
    const { onSingleAction } = renderUndo();

    const trigger = screen.getByTestId('toolbar-undo-button');
    // Focusable, so the chain step has a keyboard route...
    expect(trigger).not.toBeDisabled();
    // ...but still styled inert, and Undo / Ctrl+Z still do nothing - the Unwind is
    // deliberately NOT a fall-through from the undo control.
    expect(trigger.className).toContain('cursor-not-allowed');
    await userEvent.click(trigger);
    expect(onSingleAction).not.toHaveBeenCalled();
  });

  it('ignores Ctrl+Z while the empty panel offers only Unwind', async () => {
    const undo = vi.fn(async () => ({ success: false }));
    useProjectStore.setState({ undo: undo as never });
    seedLink(true);
    renderUndo();
    await hoverOpen();
    await screen.findByTestId('toolbar-undo-button-unwind');

    fireEvent.keyDown(document, { key: 'z', ctrlKey: true });

    expect(undo).not.toHaveBeenCalled();
    expect(useUIStore.getState().activeDialog).toBeNull();
  });

  it('does not open at all when placement mode is the reason for disabled', async () => {
    // The toolbar passes `hoverableWhenEmpty={false}` then: that reason for `disabled`
    // must suppress the panel entirely, not just the button.
    seedLink(true);
    renderUndo({ hoverable: false });

    await userEvent.hover(screen.getByTestId('toolbar-undo-button'));
    await new Promise((r) => setTimeout(r, 500));
    expect(screen.queryByTestId('toolbar-undo-button-dropdown')).not.toBeInTheDocument();
  });

  // [#303] The forward half. The walk is not one-way: the commits you came from are still
  // on the server and the chain pointer still knows where you are, so the redo dropdown
  // offers the mirror step past the bottom of ITS stack.
  describe('the forward step on the REDO side', () => {
    const renderRedo = (disabled = true) =>
      render(
        <UndoHistoryDropdown
          mode="redo"
          disabled={disabled}
          hoverableWhenEmpty
          onSingleAction={vi.fn()}
          testId="toolbar-redo-button"
          ariaLabel="Redo"
        >
          Redo
        </UndoHistoryDropdown>,
      );

    const hoverRedo = async () => {
      await userEvent.hover(screen.getByTestId('toolbar-redo-button'));
      await waitFor(
        () => expect(screen.getByTestId('toolbar-redo-button-dropdown')).toBeInTheDocument(),
        { timeout: 2000 },
      );
    };

    it('offers the step, and asks the FORWARD command for its target', async () => {
      invokeIpcMock.mockImplementation((cmd: string) => {
        if (cmd === 'get_undo_stack' || cmd === 'get_redo_stack') return Promise.resolve([]);
        // Only the forward command answers: asking the backward one here would walk the
        // copy the wrong way.
        if (cmd === 'speleodb_rewind_target')
          return Promise.resolve({ hexsha: 'fedcba9876543210', is_latest: false });
        return Promise.resolve(null);
      });
      seedLink(true);
      renderRedo();
      await hoverRedo();

      await waitFor(() =>
        expect(screen.getByTestId('toolbar-redo-button-unwind')).toBeInTheDocument(),
      );
      expect(screen.getByText(/Redo into next SpeleoDB version/)).toBeInTheDocument();
      expect(invokeIpcMock).toHaveBeenCalledWith('speleodb_rewind_target');
      expect(invokeIpcMock).not.toHaveBeenCalledWith('speleodb_unwind_target');
    });

    it('ignores Ctrl+Y while the empty panel offers only Rewind', async () => {
      const redo = vi.fn(async () => ({ success: false }));
      useProjectStore.setState({ redo: redo as never });
      invokeIpcMock.mockImplementation((cmd: string) => {
        if (cmd === 'get_undo_stack' || cmd === 'get_redo_stack') return Promise.resolve([]);
        if (cmd === 'speleodb_rewind_target')
          return Promise.resolve({ hexsha: 'fedcba9876543210', is_latest: false });
        return Promise.resolve(null);
      });
      seedLink(true);
      renderRedo();
      await hoverRedo();
      await screen.findByTestId('toolbar-redo-button-unwind');

      fireEvent.keyDown(document, { key: 'y', ctrlKey: true });

      expect(redo).not.toHaveBeenCalled();
      expect(useUIStore.getState().activeDialog).toBeNull();
    });

    it('stays open when the last local redo reaches the next revision boundary', async () => {
      const localRedo = {
        index: 0,
        description: 'Restore station name',
        timestamp: null,
        commandType: 'update_station',
        previewData: null,
      };
      const getRedoStack = vi
        .fn()
        .mockResolvedValueOnce([localRedo])
        .mockResolvedValueOnce([localRedo])
        .mockResolvedValue([]);
      const redo = vi.fn(async () =>
        accepted<HistoryOutcome>({ reversed_steps: [], warnings: [], remote_link: null }),
      );
      useProjectStore.setState({ getRedoStack, redo });
      invokeIpcMock.mockImplementation((cmd: string) => {
        if (cmd === 'speleodb_rewind_target')
          return Promise.resolve({ hexsha: 'fedcba9876543210', is_latest: false });
        return Promise.resolve(null);
      });
      seedLink(true);
      renderRedo(false);
      await hoverRedo();
      expect(screen.getByText('Restore station name')).toBeInTheDocument();
      const scrollable = document.querySelector('.undo-history-scrollable') as HTMLElement & {
        scrollTo: ReturnType<typeof vi.fn>;
      };
      scrollable.scrollTo = vi.fn();

      await userEvent.click(screen.getByTestId('toolbar-redo-button'));

      await waitFor(() => expect(redo).toHaveBeenCalledOnce());
      await waitFor(() =>
        expect(screen.getByText(/Redo into next SpeleoDB version/)).toBeInTheDocument(),
      );
      expect(screen.getByTestId('toolbar-redo-button-dropdown')).toBeInTheDocument();
    });

    it('raises the confirmation marked forward, and acts on nothing', async () => {
      invokeIpcMock.mockImplementation((cmd: string) => {
        if (cmd === 'get_undo_stack' || cmd === 'get_redo_stack') return Promise.resolve([]);
        if (cmd === 'speleodb_rewind_target')
          return Promise.resolve({ hexsha: 'fedcba9876543210', is_latest: true });
        return Promise.resolve(null);
      });
      seedLink(true);
      renderRedo();
      await hoverRedo();
      await waitFor(() =>
        expect(screen.getByTestId('toolbar-redo-button-unwind')).toBeInTheDocument(),
      );

      await userEvent.click(screen.getByTestId('toolbar-redo-button-unwind'));

      expect(useUIStore.getState().activeDialog).toBe('speleodb-unwind');
      expect(useUIStore.getState().dialogData).toMatchObject({
        hexsha: 'fedcba9876543210',
        forward: true,
        isLatest: true,
        // [#303 F11] The mirror of the backward case: `speleodb_rewind_target` reports
        // neither `is_current` nor `discards_unpushed`, and a forward step can never
        // discard un-pushed work, so both are stated false rather than typed as booleans
        // that are `undefined` at runtime.
        isCurrent: false,
        discardsUnpushed: false,
      });
      expect(invokeIpcMock).not.toHaveBeenCalledWith(
        'speleodb_restore_revision',
        expect.anything(),
      );
    });

    // The user's third requirement, and it needs no code of its own: a forward command
    // drops the chain pointer, the backend can no longer resolve a step relative to it,
    // and the entry disappears rather than offering to discard the edit.
    // CHANGED: this used to hover the redo panel OPEN and assert the entry was missing.
    // Since #303 F08 the target is resolved before opening, so an absent chain position
    // means the redo panel never opens - which is the same promise, made earlier.
    it('disappears once there is no chain position to step forward from', async () => {
      invokeIpcMock.mockImplementation((cmd: string) => {
        if (cmd === 'get_undo_stack' || cmd === 'get_redo_stack') return Promise.resolve([]);
        // What `speleodb_rewind_target` returns after an edit.
        return Promise.resolve(null);
      });
      seedLink(true);
      renderRedo();

      await userEvent.hover(screen.getByTestId('toolbar-redo-button'));
      await waitFor(() => expect(invokeIpcMock).toHaveBeenCalledWith('speleodb_rewind_target'));
      await new Promise((r) => setTimeout(r, 300));
      expect(screen.queryByTestId('toolbar-redo-button-unwind')).not.toBeInTheDocument();
      expect(screen.queryByTestId('toolbar-redo-button-dropdown')).not.toBeInTheDocument();
    });
  });

  // [#303 F09] The entry used to be mouse-only, three ways over: the trigger was
  // `disabled` so Tab skipped it, its ArrowDown branch refused an empty stack, and the
  // entry itself was `tabIndex={-1}` and outside the panel's keyboard model. The one
  // control for an action that replaces the working copy has to be reachable without a
  // mouse.
  describe('the keyboard route', () => {
    it('is in the tab order even though the button is inert', async () => {
      seedLink(true);
      renderUndo();

      await userEvent.tab();
      expect(screen.getByTestId('toolbar-undo-button')).toHaveFocus();
    });

    it('adds no tab stop where the panel could never open', async () => {
      // `hoverableWhenEmpty` is not link-aware - the toolbar passes it as
      // `!placementModeActive` - so on a local project it is set while the panel has
      // nothing it could ever show. The trigger must stay properly disabled there rather
      // than become a tab stop that does nothing at all.
      useSpeleoStore.setState({ projectLinkKind: 'local', link: null });
      renderUndo();

      expect(screen.getByTestId('toolbar-undo-button')).toBeDisabled();
      await userEvent.tab();
      expect(screen.getByTestId('toolbar-undo-button')).not.toHaveFocus();
    });

    it('opens on ArrowDown and lands focus on the entry, which Enter then presses', async () => {
      seedLink(true);
      renderUndo();

      screen.getByTestId('toolbar-undo-button').focus();
      await userEvent.keyboard('{ArrowDown}');

      const entry = await screen.findByTestId('toolbar-undo-button-unwind');
      // The only thing in the panel, so focus goes to it rather than to a list position
      // that does not exist.
      await waitFor(() => expect(entry).toHaveFocus());

      await userEvent.keyboard('{Enter}');

      // Same as the mouse press: a confirmation, never the action.
      expect(useUIStore.getState().activeDialog).toBe('speleodb-unwind');
      expect(useUIStore.getState().dialogData).toMatchObject({ hexsha: TARGET.hexsha });
      expect(invokeIpcMock).not.toHaveBeenCalledWith(
        'speleodb_restore_revision',
        expect.anything(),
      );
    });

    it('does not open on ArrowDown when no chain step resolves', async () => {
      // The keyboard mirrors the hover path exactly (#303 F08): resolve first, open only
      // on an answer. If the two disagreed, one of them would be showing an empty panel.
      seedLink(true);
      invokeIpcMock.mockImplementation((cmd: string) => {
        if (cmd === 'get_undo_stack' || cmd === 'get_redo_stack') return Promise.resolve([]);
        return Promise.resolve(null);
      });
      renderUndo();

      screen.getByTestId('toolbar-undo-button').focus();
      await userEvent.keyboard('{ArrowDown}');

      await waitFor(() => expect(invokeIpcMock).toHaveBeenCalledWith('speleodb_unwind_target'));
      await new Promise((r) => setTimeout(r, 100));
      expect(screen.queryByTestId('toolbar-undo-button-dropdown')).not.toBeInTheDocument();
    });

    it('closes on Escape from the empty panel, which used to be a dead end', async () => {
      seedLink(true);
      renderUndo();

      screen.getByTestId('toolbar-undo-button').focus();
      await userEvent.keyboard('{ArrowDown}');
      const entry = await screen.findByTestId('toolbar-undo-button-unwind');
      await waitFor(() => expect(entry).toHaveFocus());

      await userEvent.keyboard('{Escape}');

      await waitFor(() =>
        expect(screen.queryByTestId('toolbar-undo-button-dropdown')).not.toBeInTheDocument(),
      );
      expect(screen.getByTestId('toolbar-undo-button')).toHaveFocus();
    });
  });
});
