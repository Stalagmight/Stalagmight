/**
 * [#303] Component test - spec item 29: the per-project undo-history control renders the
 * explicit value, is disabled without an edit session WITH the reason visible, and
 * writes through on change.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

vi.mock('../../../../utils/ipc');
stubGetErrorMessage(errorMessageOrString);

import { SpeleoUndoModeControl } from '../SpeleoUndoModeControl';
import { useSpeleoStore } from '../../../../store/speleoStore';
import type { UndoMode } from '../../../../types/ipc.generated';
import { invokeIpcMock, stubGetErrorMessage, errorMessageOrString } from '../../../../test/factories/ipc';

/** A remote link in the given edit state, carrying the given undo policy. */
const seed = (opts: {
  editing: boolean;
  undoMode?: UndoMode;
  forceEditing?: boolean;
}) =>
  useSpeleoStore.setState({
    projectLinkKind: 'remote',
    link: {
      permission: 'READ_AND_WRITE',
      editState: opts.editing ? 'editing' : 'read-only',
      lockHolder: null,
      lockedByOther: false,
      hasUnpushedEdits: false,
      unwound: false,
      forceEditing: opts.forceEditing ?? false,
      forceEditAllowed: false,
      undoMode: opts.undoMode ?? 'full',
      remoteState: 'up_to_date',
      noAccess: false,
      serverError: null,
    },
  });

describe('SpeleoUndoModeControl (#303)', () => {
  beforeEach(() => {
    invokeIpcMock.mockReset();
    invokeIpcMock.mockResolvedValue(null);
    useSpeleoStore.setState({ projectLinkKind: 'unknown', link: null });
  });

  it('renders the explicit project value', () => {
    seed({ editing: true, undoMode: 'disabled' });
    render(<SpeleoUndoModeControl />);

    expect(screen.getByTestId('speleodb-undo-mode-disabled-input')).toBeChecked();
    expect(screen.getByTestId('speleodb-undo-mode-full-input')).not.toBeChecked();
  });

  it('writes the chosen mode through and re-reads the resolved value', async () => {
    seed({ editing: true, undoMode: 'full' });
    render(<SpeleoUndoModeControl />);

    await userEvent.click(screen.getByTestId('speleodb-undo-mode-disabled-input'));

    await waitFor(() =>
      expect(invokeIpcMock).toHaveBeenCalledWith('speleodb_set_undo_mode', { mode: 'disabled', expectedProject: null }),
    );
    // ...and re-reads the committed backend value rather than assuming its own.
    await waitFor(() =>
      expect(invokeIpcMock.mock.calls.some((c) => c[0] === 'speleodb_link_status')).toBe(true),
    );
  });

  it('is disabled without the write lock, and says why', () => {
    seed({ editing: false });
    render(<SpeleoUndoModeControl />);

    expect(screen.getByTestId('speleodb-undo-mode-full-input')).toBeDisabled();
    expect(screen.getByTestId('speleodb-undo-mode-disabled-input')).toBeDisabled();
    // The reason is VISIBLE: a greyed control with no explanation reads as a bug, and
    // this one is gated for a reason the user can act on (start editing).
    expect(screen.getByTestId('speleodb-undo-mode-lock-hint')).toBeInTheDocument();
  });

  it('stays enabled for a force-editor who accepted conflict responsibility', async () => {
    seed({ editing: true, forceEditing: true });
    render(<SpeleoUndoModeControl />);

    expect(screen.getByTestId('speleodb-undo-mode-disabled-input')).toBeEnabled();
    expect(screen.queryByTestId('speleodb-undo-mode-lock-hint')).not.toBeInTheDocument();

    await userEvent.click(screen.getByTestId('speleodb-undo-mode-disabled-input'));
    await waitFor(() =>
      expect(invokeIpcMock).toHaveBeenCalledWith('speleodb_set_undo_mode', {
        mode: 'disabled',
        expectedProject: null,
      }),
    );
  });

  it('surfaces a refusal inline instead of throwing it away', async () => {
    seed({ editing: true });
    invokeIpcMock.mockRejectedValueOnce(new Error('acquire the write lock first'));
    render(<SpeleoUndoModeControl />);

    await userEvent.click(screen.getByTestId('speleodb-undo-mode-disabled-input'));

    await waitFor(() =>
      expect(screen.getByTestId('speleodb-undo-mode-error')).toBeInTheDocument(),
    );
  });
});
