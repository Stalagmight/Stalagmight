/**
 * [W71-NF1] The loading window holds keyboard focus while it is up.
 *
 * It used to be a plain full-screen div: it covered the app for the pointer, but
 * Tab and Shift+Tab walked on to the controls under it, where Enter and Space
 * acted. It is now a modal layer like a dialog: focus moves into it when it opens
 * (and again when its error state replaces the progress), wraps inside it, and
 * goes back to the element that had it when it closes - when that element is
 * still there. Real i18n, `.tsx` so vitest runs it under jsdom.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

import '../../../i18n';
import { ProjectLoadingOverlay } from '../ProjectLoadingOverlay';
import { useProjectStore } from '../../../store/projectStore';
import { initialLoadingState } from '../../../store/projectLoadingState';

// The shared IPC mock (src/utils/__mocks__/ipc.ts): nothing here presses Cancel.
vi.mock('../../../utils/ipc');

function holdLoad(): void {
  act(() => {
    useProjectStore.setState({
      loading: { ...initialLoadingState, isLoading: true, clientOpId: 3, projectName: 'CaveB', projectPath: '/caves/CaveB.stalx' },
    });
  });
}

function failLoad(): void {
  act(() => {
    useProjectStore.getState().setLoadingError('Could not read the project file.', 'ENOENT');
  });
}

function endLoad(): void {
  act(() => {
    useProjectStore.getState().resetLoading();
  });
}

function Scene({ withButton = true }: { withButton?: boolean }) {
  return (
    <>
      {withButton && <button type="button" data-testid="behind">Behind</button>}
      <ProjectLoadingOverlay retrySource="recent-list" occupiesScreen />
    </>
  );
}

function renderBehind() {
  return render(<Scene />);
}

beforeEach(() => {
  useProjectStore.setState({ loading: { ...initialLoadingState } });
});

afterEach(() => {
  endLoad();
});

describe('ProjectLoadingOverlay keyboard focus (W71-NF1)', () => {
  it('moves focus into the window when it opens, and makes what is behind it inert', () => {
    renderBehind();
    screen.getByTestId('behind').focus();
    holdLoad();
    expect(screen.getByTestId('loading-progress-cancel')).toHaveFocus();
    expect(document.getElementById('dialog-root')).toContainElement(screen.getByTestId('project-loading-overlay'));
    expect(screen.getByTestId('behind').closest('[inert]')).not.toBeNull();
  });

  it('keeps Tab and Shift+Tab on its one control while the load runs', async () => {
    const user = userEvent.setup();
    renderBehind();
    screen.getByTestId('behind').focus();
    holdLoad();
    const cancel = screen.getByTestId('loading-progress-cancel');
    await user.tab();
    expect(cancel).toHaveFocus();
    await user.tab({ shift: true });
    expect(cancel).toHaveFocus();
  });

  it('moves focus to the error state when it replaces the progress, and wraps inside it', async () => {
    const user = userEvent.setup();
    renderBehind();
    screen.getByTestId('behind').focus();
    holdLoad();
    failLoad();
    const first = screen.getByTestId('loading-error-details-toggle');
    const last = screen.getByTestId('loading-error-copy');
    expect(first).toHaveFocus();
    await user.tab({ shift: true });
    expect(last).toHaveFocus();
    await user.tab();
    expect(first).toHaveFocus();
  });

  it('gives focus back to the element that had it when the window closes', () => {
    renderBehind();
    const behind = screen.getByTestId('behind');
    behind.focus();
    holdLoad();
    expect(behind).not.toHaveFocus();
    endLoad();
    expect(screen.queryByTestId('project-loading-overlay')).toBeNull();
    expect(behind.closest('[inert]')).toBeNull();
    expect(behind).toHaveFocus();
  });

  it('does not throw when the element that had focus is gone by the time it closes', () => {
    const view = renderBehind();
    screen.getByTestId('behind').focus();
    holdLoad();
    expect(screen.getByTestId('loading-progress-cancel')).toHaveFocus();
    view.rerender(<Scene withButton={false} />);
    expect(screen.queryByTestId('behind')).toBeNull();
    expect(() => endLoad()).not.toThrow();
    expect(screen.queryByTestId('project-loading-overlay')).toBeNull();
  });
});

// [NF4] The window's error state holds the app until Retry or Dismiss: a modal
// alert dialog named by its heading, which focus moves into.
describe('ProjectLoadingOverlay error state (NF4)', () => {
  it('is a modal alert dialog named by its heading and described by its message, with focus inside', () => {
    renderBehind();
    screen.getByTestId('behind').focus();
    holdLoad();
    expect(screen.getByRole('dialog', { name: 'Loading CaveB...' })).toHaveAttribute('aria-modal', 'true');
    failLoad();
    const dialog = screen.getByRole('alertdialog', { name: 'Loading error' });
    expect(dialog).toHaveAttribute('aria-modal', 'true');
    expect(dialog).toHaveAccessibleDescription('Could not read the project file.');
    expect(dialog).toContainElement(screen.getByTestId('loading-error-details-toggle'));
    expect(screen.getByTestId('loading-error-details-toggle')).toHaveFocus();
  });
});
