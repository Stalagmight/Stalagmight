import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import SpeleoUnpushedLocalDialog from './SpeleoUnpushedLocalDialog';
import { useUIStore } from '../../store/uiStore';

const { takeRemote, openLocal } = vi.hoisted(() => ({ takeRemote: vi.fn(), openLocal: vi.fn() }));
vi.mock('../../hooks/useOpenRemoteProject', () => ({ useOpenRemoteProject: () => ({
  takeRemoteDiscardingLocal: takeRemote, openLocalCopy: openLocal,
}) }));

beforeEach(() => {
  takeRemote.mockReset().mockResolvedValue({ status: 'cancelled' });
  openLocal.mockReset().mockResolvedValue({ status: 'cancelled' });
  useUIStore.setState({ activeDialog: 'speleodb-unpushed-local', dialogData: {
    summary: { id: 'synthetic', name: 'Synthetic cave' },
    unpushedLocal: { local_path: '/synthetic/cave.stalx', last_edit_at: null },
  } });
});

/**
 * The app's dialog layout (`UnsavedChangesDialog`): every button in the one footer
 * row - Cancel, the destructive answer, the primary last - and none anywhere else
 * in the body (the header's X aside), with no hint under a button.
 */
function expectButtonsOnlyInFooter(expected: string[]) {
  const dialog = screen.getByTestId('speleodb-unpushed-local-dialog');
  const footer = screen.getByTestId('speleodb-unpushed-local-footer');
  expect(footer).toHaveClass('flex', 'flex-wrap', 'justify-end', 'gap-2', 'pt-4', 'border-t', 'border-border');
  expect(within(footer).getAllByRole('button').map((b) => b.getAttribute('data-testid'))).toEqual(expected);
  const elsewhere = within(dialog).getAllByRole('button')
    .filter((b) => !footer.contains(b) && b.getAttribute('data-testid') !== 'speleodb-unpushed-local-dialog-close');
  expect(elsewhere).toEqual([]);
  expect(footer.nextElementSibling).toBeNull();
  // What each answer does sits in the body, above any warning and the footer.
  const hint = screen.getByTestId('speleodb-unpushed-local-open-hint');
  expect(hint.compareDocumentPosition(footer) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  const warning = screen.queryByTestId('speleodb-unpushed-local-discard-warning');
  if (warning) expect(hint.compareDocumentPosition(warning) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
}

const FOOTER_ORDER = [
  'speleodb-unpushed-local-cancel',
  'speleodb-unpushed-local-take-remote',
  'speleodb-unpushed-local-open-local',
];

it('lays the choices out like every other dialog: Cancel, the discard, then the primary open', async () => {
  await import('../../i18n');
  render(<SpeleoUnpushedLocalDialog />);
  expectButtonsOnlyInFooter(FOOTER_ORDER);
  expect(screen.getByTestId('speleodb-unpushed-local-take-remote')).toHaveClass('bg-error');
  expect(screen.getByTestId('speleodb-unpushed-local-open-local')).toHaveClass('bg-accent');
  const hint = screen.getByTestId('speleodb-unpushed-local-open-hint');
  // What each answer does is said once, in the body, by the buttons' own names.
  expect(hint).toHaveTextContent('"Open my local copy" opens the copy on this computer');
  expect(hint).toHaveTextContent('"Discard my changes" deletes them and opens the version on SpeleoDB');
  expect(hint).toHaveTextContent('Cancel leaves them as they are');
  // Short labels, so the row fits on one line of the dialog.
  expect(screen.getByTestId('speleodb-unpushed-local-take-remote')).toHaveTextContent(/^Discard my changes$/);
  expect(screen.getByTestId('speleodb-unpushed-local-open-local')).toHaveTextContent(/^Open my local copy$/);
  // Arming the discard adds its warning to the body; the row stays as it was.
  await userEvent.click(screen.getByTestId('speleodb-unpushed-local-take-remote'));
  expect(screen.getByTestId('speleodb-unpushed-local-discard-warning')).toBeVisible();
  expect(screen.getByTestId('speleodb-unpushed-local-take-remote')).toHaveTextContent(/^Yes, discard my changes$/);
  expectButtonsOnlyInFooter(FOOTER_ORDER);
});

it('restores retry and dismissal after a discarded-copy open is superseded', async () => {
  render(<SpeleoUnpushedLocalDialog />);
  const take = screen.getByTestId('speleodb-unpushed-local-take-remote');
  await userEvent.click(take); // Arm the destructive choice.
  await userEvent.click(take);
  await waitFor(() => expect(take).toBeEnabled());
  expect(takeRemote).toHaveBeenCalledOnce();
  expect(screen.queryByTestId('speleodb-unpushed-local-error')).not.toBeInTheDocument();
  await userEvent.click(take);
  expect(takeRemote).toHaveBeenCalledTimes(2);
  await userEvent.click(screen.getByTestId('speleodb-unpushed-local-cancel'));
  expect(useUIStore.getState().activeDialog).toBeNull();
});


it('a superseded handler cannot clear a replacement prompt operation', async () => {
  let cancelOld!: (outcome: unknown) => void;
  takeRemote.mockReturnValueOnce(new Promise((resolve) => { cancelOld = resolve; }))
    .mockReturnValueOnce(new Promise(() => {}));
  render(<SpeleoUnpushedLocalDialog />);
  const take = screen.getByTestId('speleodb-unpushed-local-take-remote');
  await userEvent.click(take);
  await userEvent.click(take);
  expect(take).toBeDisabled();
  act(() => useUIStore.setState((state) => ({ dialogData: { ...state.dialogData } })));
  await waitFor(() => expect(take).toBeEnabled());
  await userEvent.click(take);
  await userEvent.click(take);
  expect(take).toBeDisabled();
  await act(async () => { cancelOld({ status: 'cancelled' }); });
  expect(take).toBeDisabled();
  expect(screen.getByTestId('speleodb-unpushed-local-cancel')).toBeDisabled();
});

describe('unsynced work protection during a remote open', () => {
  it('cancel touches nothing', async () => {
    render(<SpeleoUnpushedLocalDialog />);
    await userEvent.click(screen.getByTestId('speleodb-unpushed-local-cancel'));
    expect(takeRemote).not.toHaveBeenCalled();
    expect(openLocal).not.toHaveBeenCalled();
    expect(useUIStore.getState().activeDialog).toBeNull();
  });

  // Users stranded by earlier builds (un-uploaded edits, lock already released) need
  // a way back to their work that does not throw either copy away.
  it('"Open my local copy" opens the copy on this computer, with no second confirmation', async () => {
    render(<SpeleoUnpushedLocalDialog />);
    const openButton = screen.getByTestId('speleodb-unpushed-local-open-local');
    expect(openButton).toHaveAttribute('data-primary-action', '');
    await userEvent.click(openButton);
    expect(openLocal).toHaveBeenCalledWith(
      { id: 'synthetic', name: 'Synthetic cave' },
      { local_path: '/synthetic/cave.stalx', last_edit_at: null },
    );
    expect(takeRemote).not.toHaveBeenCalled();
  });

  it('a failed local open stays on the prompt with the reason', async () => {
    openLocal.mockResolvedValue({ status: 'error', message: 'Synthetic load failure', reLogin: false });
    render(<SpeleoUnpushedLocalDialog />);
    await userEvent.click(screen.getByTestId('speleodb-unpushed-local-open-local'));
    expect(await screen.findByTestId('speleodb-unpushed-local-error')).toHaveTextContent('Synthetic load failure');
    expect(screen.getByTestId('speleodb-unpushed-local-open-local')).toBeEnabled();
  });
  it('requires an explicit second confirmation before replacing unsynced work', async () => {
    render(<SpeleoUnpushedLocalDialog />);
    await userEvent.click(screen.getByTestId('speleodb-unpushed-local-take-remote'));
    expect(takeRemote).not.toHaveBeenCalled();
    expect(screen.getByTestId('speleodb-unpushed-local-discard-warning')).toBeVisible();
    await userEvent.click(screen.getByTestId('speleodb-unpushed-local-take-remote'));
    expect(takeRemote).toHaveBeenCalledTimes(1);
  });
});

// Must-fix 1: one account never takes over another account's un-uploaded work.
describe("another account's un-uploaded work", () => {
  const THEIRS = {
    local_path: '/synthetic/cave.stalx', last_edit_at: null, other_owner: 'someone@example.com',
  };
  beforeEach(async () => {
    await import('../../i18n');
    useUIStore.setState({ activeDialog: 'speleodb-unpushed-local', dialogData: {
      summary: { id: 'synthetic', name: 'Synthetic cave' },
      unpushedLocal: THEIRS,
    } });
  });

  it('names the account whose changes they are, and offers to open the copy read-only', async () => {
    render(<SpeleoUnpushedLocalDialog />);
    const message = screen.getByTestId('speleodb-unpushed-local-message');
    expect(message).toHaveTextContent('opened on this computer by someone@example.com');
    expect(message).not.toHaveTextContent('your local changes');
    const openButton = screen.getByTestId('speleodb-unpushed-local-open-local');
    expect(openButton).toHaveTextContent('Open it read-only');
    expect(screen.getByTestId('speleodb-unpushed-local-open-hint'))
      .toHaveTextContent('without taking the editing lock');
    expect(screen.getByTestId('speleodb-unpushed-local-take-remote'))
      .toHaveTextContent(/^Discard their changes$/);
    await userEvent.click(openButton);
    expect(openLocal).toHaveBeenCalledWith({ id: 'synthetic', name: 'Synthetic cave' }, THEIRS);
  });

  it('keeps the same footer row, with what each answer does in the body', () => {
    render(<SpeleoUnpushedLocalDialog />);
    expectButtonsOnlyInFooter(FOOTER_ORDER);
    const hint = screen.getByTestId('speleodb-unpushed-local-open-hint');
    expect(hint).toHaveTextContent('"Open it read-only" opens the copy');
    expect(hint).toHaveTextContent("only someone@example.com can edit and upload them");
    expect(hint).toHaveTextContent('"Discard their changes" deletes them');
    expect(hint).toHaveTextContent('Cancel leaves them as they are');
  });

  it('the armed discard names whose changes are lost, in a label that still fits the row', async () => {
    render(<SpeleoUnpushedLocalDialog />);
    await userEvent.click(screen.getByTestId('speleodb-unpushed-local-take-remote'));
    expect(screen.getByTestId('speleodb-unpushed-local-take-remote')).toHaveTextContent(/^Yes, discard their changes$/);
    expect(screen.getByTestId('speleodb-unpushed-local-discard-warning')).toHaveTextContent('permanently deletes');
    expectButtonsOnlyInFooter(FOOTER_ORDER);
  });
});
