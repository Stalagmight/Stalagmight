/**
 * Component test - #260: a failed "Upload & release" is readable and actionable
 * WHERE THE USER IS.
 *
 * The defect: the dialog correctly stayed open when the upload threw (the work is
 * not done), but said nothing about why. The durable failure surface was the shell
 * banner, which renders behind this dialog's modal scrim - dimmed, unreadable, and
 * with its Retry/Dismiss unclickable. So at the exact moment the user needed the
 * error they could neither read nor act on it.
 *
 * The contract locked here:
 *  1. the failure renders INLINE in the dialog after a failed attempt;
 *  2. the retry is the dialog's EXISTING primary button (no second control), and
 *     re-pressing it re-attempts cleanly - the inline error clears, then updates;
 *  3. a success clears the inline notice AND the store's banner slot, and closes.
 * The "banner stays hidden while this dialog is open / returns after it closes"
 * half of the contract lives in SpeleoBanner.test.tsx.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

vi.mock('../../utils/ipc');
stubGetErrorMessage(fallbackOr('the server said no'));

import SpeleoDoneDialog from './SpeleoDoneDialog';
import { useSpeleoStore, type SpeleoLinkState } from '../../store/speleoStore';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import { invokeIpcMock, stubGetErrorMessage, fallbackOr } from '../../test/factories/ipc';
import { makeSpeleoLink } from '../../test/factories/speleo';

/** [#576 NF-W8FE-2] Done and Push answer the wire envelope, never a bare outcome. */
function report<R>(result: R): { result: R; save_warnings: [] } {
  return { result, save_warnings: [] };
}

const LINK: SpeleoLinkState = makeSpeleoLink({ editState: 'editing', hasUnpushedEdits: true });

beforeEach(() => {
  invokeIpcMock.mockReset();
  // A georeferenced zero datum keeps the unrelated `no_anchor` notice out of the
  // banner slot, so the assertions below are about the push-failure surface only.
  useProjectStore.setState({
    project: { zero_datums: [{}] } as never,
    saveProject: vi.fn(async () => ({ success: true }) as never),
  });
  useSpeleoStore.setState({ link: { ...LINK }, banner: null, busy: false });
  useUIStore.setState({
    activeDialog: 'speleodb-done',
    dialogData: {},
    showToast: vi.fn(),
  });
});

describe('SpeleoDoneDialog - a failed upload is surfaced inside the dialog (#260)', () => {
  it('shows nothing until an attempt actually fails', () => {
    render(<SpeleoDoneDialog />);
    expect(screen.queryByTestId('speleodb-push-failure-inline')).not.toBeInTheDocument();
  });

  it('renders the humanized failure inline and keeps the dialog open', async () => {
    invokeIpcMock.mockRejectedValue(new Error('blackholed'));
    render(<SpeleoDoneDialog />);

    await userEvent.click(screen.getByTestId('speleodb-done-confirm'));

    const notice = await screen.findByTestId('speleodb-push-failure-inline');
    // The SAME humanized line the shell banner carries - one source of truth.
    const failure = useSpeleoStore.getState().banner;
    expect(failure).toMatchObject({ kind: 'push_failed', origin: 'done' });
    expect(notice.textContent).toContain((failure as { message: string }).message);
    // The dialog is still up (the work is NOT done) and the dialog was not closed.
    expect(screen.getByTestId('speleodb-done-dialog')).toBeInTheDocument();
    expect(useUIStore.getState().activeDialog).toBe('speleodb-done');
  });

  it('offers no second retry control - the existing primary button is the retry', async () => {
    invokeIpcMock.mockRejectedValue(new Error('blackholed'));
    render(<SpeleoDoneDialog />);

    await userEvent.click(screen.getByTestId('speleodb-done-confirm'));
    await screen.findByTestId('speleodb-push-failure-inline');

    expect(screen.queryByTestId('speleo-banner-retry-push')).not.toBeInTheDocument();
    expect(screen.getByTestId('speleodb-done-confirm')).toBeEnabled();
  });

  it('re-pressing the button retries with the typed message and updates the inline error', async () => {
    invokeIpcMock.mockRejectedValue(new Error('blackholed'));
    render(<SpeleoDoneDialog />);
    await userEvent.type(screen.getByTestId('speleodb-done-message'), 'sump survey');
    await userEvent.click(screen.getByTestId('speleodb-done-confirm'));
    await screen.findByTestId('speleodb-push-failure-inline');

    await userEvent.click(screen.getByTestId('speleodb-done-confirm'));

    await waitFor(() =>
      expect(
        invokeIpcMock.mock.calls.filter(
          (c) =>
            c[0] === 'speleodb_done' && (c[1] as { message?: string }).message === 'sump survey',
        ).length,
      ).toBe(2),
    );
    // Still shown (the retry failed too) - one notice, from the LATEST attempt.
    expect(screen.getAllByTestId('speleodb-push-failure-inline')).toHaveLength(1);
  });

  it('a successful retry clears the inline notice and the banner slot, and closes', async () => {
    invokeIpcMock.mockRejectedValueOnce(new Error('blackholed'));
    render(<SpeleoDoneDialog />);
    await userEvent.click(screen.getByTestId('speleodb-done-confirm'));
    await screen.findByTestId('speleodb-push-failure-inline');

    invokeIpcMock.mockResolvedValue(report({ kind: 'released' }));
    await userEvent.click(screen.getByTestId('speleodb-done-confirm'));

    await waitFor(() => expect(useUIStore.getState().activeDialog).toBeNull());
    // BOTH surfaces are gone: the store's push_failed slot is cleared, so nothing
    // is left for the shell banner to re-raise once the dialog is down.
    expect(useSpeleoStore.getState().banner).toBeNull();
    expect(screen.queryByTestId('speleodb-push-failure-inline')).not.toBeInTheDocument();
  });

  it('push mode surfaces its failure the same way', async () => {
    useUIStore.setState({ activeDialog: 'speleodb-done', dialogData: { mode: 'push' } });
    invokeIpcMock.mockRejectedValue(new Error('blackholed'));
    render(<SpeleoDoneDialog />);

    await userEvent.click(screen.getByTestId('speleodb-done-confirm'));

    await screen.findByTestId('speleodb-push-failure-inline');
    expect(useSpeleoStore.getState().banner).toMatchObject({
      kind: 'push_failed',
      origin: 'push',
    });
  });
});
