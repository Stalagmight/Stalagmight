// @vitest-environment jsdom
/**
 * Removing a downloaded SpeleoDB copy from the home screen: an in-app prompt that
 * re-reads the copy first, names un-uploaded (or unreadable) work, and only then
 * sends the consent to lose it.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

const ipc = vi.hoisted(() => vi.fn());
vi.mock('../../../utils/ipc', () => ({ invokeIpc: ipc, getErrorMessage: String }));

import '../../../i18n';
import { RemoveCachedCopyDialog } from '../RemoveCachedCopyDialog';
import { useSpeleoCacheStore } from '../../../store/speleoCacheStore';
import { useSpeleoStore } from '../../../store/speleoStore';
import type { SpeleoCachedProject } from '../../../types/ipc.generated';

const COPY: SpeleoCachedProject = {
  project_id: '11111111-1111-1111-1111-111111111111',
  project_name: 'Synthetic cave',
  local_path: '/cache/speleodb/1/project.stalx',
  ownership: 'signed_out',
  has_unpushed_edits: false,
  state_unknown: false,
  is_open: false,
};

function answer(listing: SpeleoCachedProject[], removal: unknown = { success: true, warnings: [], error: null }) {
  ipc.mockImplementation(async (cmd: string) => {
    if (cmd === 'speleodb_cached_projects') return listing;
    if (cmd === 'speleodb_remove_cached_copy') return removal;
    return null;
  });
}

beforeEach(() => {
  ipc.mockReset();
  useSpeleoCacheStore.setState({ copies: [], loaded: false });
  useSpeleoStore.setState({ busy: false });
});

describe('RemoveCachedCopyDialog', () => {
  it('removes a clean copy without the un-uploaded consent', async () => {
    answer([COPY]);
    const onRemoved = vi.fn();
    render(<RemoveCachedCopyDialog copy={COPY} onClose={vi.fn()} onRemoved={onRemoved} />);
    const confirm = screen.getByTestId('speleodb-remove-copy-confirm');
    await waitFor(() => expect(confirm).toBeEnabled());
    expect(confirm).toHaveTextContent('Remove copy');
    expect(screen.queryByTestId('speleodb-remove-copy-warning')).toBeNull();
    await userEvent.click(confirm);
    expect(ipc).toHaveBeenCalledWith('speleodb_remove_cached_copy',
      { projectId: COPY.project_id, discardUnpushed: false }, expect.anything());
    expect(onRemoved).toHaveBeenCalledWith(COPY, []);
  });

  it('warns about un-uploaded work found when the copy is re-read, and only then consents to losing it', async () => {
    // The caller's snapshot says clean; the fresh listing says otherwise.
    answer([{ ...COPY, has_unpushed_edits: true }]);
    render(<RemoveCachedCopyDialog copy={COPY} onClose={vi.fn()} onRemoved={vi.fn()} />);
    expect(await screen.findByTestId('speleodb-remove-copy-warning')).toHaveTextContent('never uploaded');
    const confirm = screen.getByTestId('speleodb-remove-copy-confirm');
    await waitFor(() => expect(confirm).toBeEnabled());
    expect(confirm).toHaveTextContent('Remove copy and lose my changes');
    await userEvent.click(confirm);
    expect(ipc).toHaveBeenCalledWith('speleodb_remove_cached_copy',
      { projectId: COPY.project_id, discardUnpushed: true }, expect.anything());
  });

  it('treats an unreadable copy as possibly holding un-uploaded work', async () => {
    answer([{ ...COPY, state_unknown: true }]);
    render(<RemoveCachedCopyDialog copy={COPY} onClose={vi.fn()} onRemoved={vi.fn()} />);
    expect(await screen.findByTestId('speleodb-remove-copy-warning')).toHaveTextContent('could not read');
  });

  it('says what happens to the editing lock: handed back for this account, kept signed out', async () => {
    answer([{ ...COPY, ownership: 'mine' }]);
    const { unmount } = render(<RemoveCachedCopyDialog copy={{ ...COPY, ownership: 'mine' }} onClose={vi.fn()} onRemoved={vi.fn()} />);
    expect(screen.getByTestId('speleodb-remove-copy-lock')).toHaveTextContent('it is handed back once the copy is removed');
    await waitFor(() => expect(screen.getByTestId('speleodb-remove-copy-confirm')).toBeEnabled());
    unmount();
    answer([COPY]);
    render(<RemoveCachedCopyDialog copy={COPY} onClose={vi.fn()} onRemoved={vi.fn()} />);
    await waitFor(() => expect(screen.getByTestId('speleodb-remove-copy-confirm')).toBeEnabled());
    expect(screen.getByTestId('speleodb-remove-copy-lock'))
      .toHaveTextContent('it stays until you next connect (which offers to release it), or until it is released on the SpeleoDB website');
  });

  it('cannot confirm while another SpeleoDB operation runs', async () => {
    useSpeleoStore.setState({ busy: true });
    answer([COPY]);
    render(<RemoveCachedCopyDialog copy={COPY} onClose={vi.fn()} onRemoved={vi.fn()} />);
    await waitFor(() => expect(useSpeleoCacheStore.getState().loaded).toBe(true));
    expect(screen.getByTestId('speleodb-remove-copy-confirm')).toBeDisabled();
  });

  it('cannot confirm before the copy was re-read', () => {
    ipc.mockImplementation(() => new Promise(() => {}));
    render(<RemoveCachedCopyDialog copy={COPY} onClose={vi.fn()} onRemoved={vi.fn()} />);
    expect(screen.getByTestId('speleodb-remove-copy-confirm')).toBeDisabled();
  });

  it('keeps the dialog open with the refusal when the backend refuses', async () => {
    answer([COPY], { success: false, warnings: [], error: 'Synthetic refusal' });
    const onRemoved = vi.fn();
    render(<RemoveCachedCopyDialog copy={COPY} onClose={vi.fn()} onRemoved={onRemoved} />);
    const confirm = screen.getByTestId('speleodb-remove-copy-confirm');
    await waitFor(() => expect(confirm).toBeEnabled());
    await userEvent.click(confirm);
    expect(await screen.findByTestId('speleodb-remove-copy-error')).toHaveTextContent('Synthetic refusal');
    expect(onRemoved).not.toHaveBeenCalled();
  });
});
