import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

// The exit-path questions are in-app now; the native dialog must not be reached.
const nativeConfirm = vi.hoisted(() => vi.fn(async () => true));
vi.mock('@tauri-apps/plugin-dialog', () => ({ confirm: nativeConfirm }));

import ExitConfirmDialog from './ExitConfirmDialog';
import { askExitChoice, askExitConfirm, useExitConfirm } from '../../utils/exitConfirm';
import { protectAccountTransition } from '../../utils/protectAccountTransition';
import { useProjectStore } from '../../store/projectStore';

const ask = () => askExitConfirm({
  title: 'Close Stalagmight?', message: 'Import is in progress.', confirmLabel: 'Discard and close', cancelLabel: 'Cancel',
});

beforeEach(() => {
  useExitConfirm.setState({ request: null });
  nativeConfirm.mockClear();
});

describe('ExitConfirmDialog', () => {
  it('asks in-app and answers yes', async () => {
    const answer = ask();
    render(<ExitConfirmDialog />);
    expect(screen.getByTestId('exit-confirm-message')).toHaveTextContent('Import is in progress.');
    await userEvent.click(screen.getByTestId('exit-confirm-ok'));
    expect(await answer).toBe(true);
    expect(nativeConfirm).not.toHaveBeenCalled();
  });

  it('Cancel and Escape answer no', async () => {
    const first = ask();
    const { rerender } = render(<ExitConfirmDialog />);
    await userEvent.click(screen.getByTestId('exit-confirm-cancel'));
    expect(await first).toBe(false);
    const second = ask();
    rerender(<ExitConfirmDialog />);
    await userEvent.keyboard('{Escape}');
    expect(await second).toBe(false);
  });

  it('a second question answers the first one "no" instead of stacking', async () => {
    const first = ask();
    void ask();
    expect(await first).toBe(false);
  });
});

describe('protectAccountTransition', () => {
  it('asks the in-app question, never the native one', async () => {
    useProjectStore.setState({ project: null, isDirty: true });
    const answer = protectAccountTransition();
    render(<ExitConfirmDialog />);
    expect(await screen.findByTestId('exit-confirm-dialog')).toBeVisible();
    await userEvent.click(screen.getByTestId('exit-confirm-cancel'));
    expect(await answer).toBe(false);
    expect(nativeConfirm).not.toHaveBeenCalled();
  });

  it('offers to save unsaved edits on this computer instead of discarding them (frontend#6)', async () => {
    const saveProject = vi.fn(async () => {
      useProjectStore.setState({ isDirty: false });
      return { success: true };
    });
    const closeProject = vi.fn(async () => ({ success: true }));
    useProjectStore.setState({ project: { file_path: '/synthetic/project.stalx' } as never, isDirty: true, saveProject, closeProject } as never);
    const answer = protectAccountTransition();
    render(<ExitConfirmDialog />);
    await userEvent.click(await screen.findByTestId('exit-confirm-save'));
    expect(await answer).toBe(true);
    expect(saveProject).toHaveBeenCalledOnce();
    // Nothing unsaved is left to drop, so the project is not closed and reopened.
    expect(closeProject).not.toHaveBeenCalled();
  });

  it('a project with no file of its own has nothing to save to: only discard or cancel', async () => {
    useProjectStore.setState({ project: { file_path: null } as never, isDirty: true });
    const answer = protectAccountTransition();
    render(<ExitConfirmDialog />);
    await screen.findByTestId('exit-confirm-dialog');
    expect(screen.queryByTestId('exit-confirm-save')).toBeNull();
    await userEvent.click(screen.getByTestId('exit-confirm-cancel'));
    expect(await answer).toBe(false);
  });
});

describe('askExitChoice', () => {
  it('answers save, and the plain confirm question shows no save button', async () => {
    const choice = askExitChoice({ title: 'T', message: 'M', confirmLabel: 'Discard', cancelLabel: 'Cancel', saveLabel: 'Save on this computer' });
    const { rerender } = render(<ExitConfirmDialog />);
    expect(screen.getByTestId('exit-confirm-save')).toHaveAttribute('data-primary-action', '');
    await userEvent.click(screen.getByTestId('exit-confirm-save'));
    expect(await choice).toBe('save');
    void ask();
    rerender(<ExitConfirmDialog />);
    expect(screen.queryByTestId('exit-confirm-save')).toBeNull();
  });
});
