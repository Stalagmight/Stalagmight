/**
 * [#576 W86 x W71-NF1] The update install hold on the fuzz branch's dialog stack:
 * a dialog deferred during the install, the project loading window, and focus.
 *
 * #594 (W86) defers a dialog asked for during an install until the hold lifts
 * (`uiStore.openDialog`), and the install window sits on the dialog stack with
 * `pushDialog`. This branch's loading window is a modal layer too, pushed BENEATH
 * any open dialog (`pushDialogBeneath`), and every layer moves focus in when it
 * opens and hands it back when it goes. The three must stack and hand focus in the
 * right order: the install window on top while it holds; after a failed install the
 * deferred dialog on top, live and focused, above a loading window still up; the
 * loading window live again once that dialog is answered.
 *
 * The real components, dialog stack and stores. Synthetic data only.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, render, screen } from '@testing-library/react';

vi.mock('../../utils/ipc');
vi.mock('../../utils/telemetry', async () => (await import('../../test/factories/telemetry')).telemetryWith());

import '../../i18n';
import ErrorDialog from '../Dialogs/ErrorDialog';
import { UpdateInstallOverlay } from '../UpdateInstallOverlay';
import { ProjectLoadingOverlay } from '../LoadingProgress/ProjectLoadingOverlay';
import { useUpdateStore } from '../../store/updateStore';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { initialLoadingState } from '../../store/projectLoadingState';
import { isTopmost } from '../../utils/dialogStack';

const INFO = { version: '9.9.9', notesUrl: 'https://example.test/notes' };

/** The three layers as App renders them, and a control in the app under them. */
function App() {
  const activeDialog = useUIStore((s) => s.activeDialog);
  return (
    <>
      <button type="button" data-testid="behind">
        app
      </button>
      <ProjectLoadingOverlay retrySource="recent-list" occupiesScreen />
      <UpdateInstallOverlay />
      {activeDialog === 'error' && <ErrorDialog />}
    </>
  );
}

const ui = () => useUIStore.getState();
const setInstalling = (on: boolean) => act(() => useUpdateStore.getState().setInstalling(on));
const openError = (message: string) => act(() => ui().openDialog('error', { title: 'Open', message }));

/** The stacked container of the error dialog (the element `DialogWrapper` pushes). */
function errorLayer(): HTMLElement {
  const layer = screen.getByTestId('error-dialog').parentElement;
  if (!layer) throw new Error('error dialog has no container');
  return layer;
}

/** The loading window's stacked container: its ancestor directly under `#dialog-root`. */
function loadingLayer(): HTMLElement {
  let el: HTMLElement | null = screen.getByTestId('project-loading-overlay');
  while (el?.parentElement && el.parentElement.id !== 'dialog-root') el = el.parentElement;
  if (!el?.parentElement) throw new Error('the loading window is not in #dialog-root');
  return el;
}

beforeEach(() => {
  useUpdateStore.getState().reset();
  const store = useUpdateStore.getState();
  store.setAvailable(store.beginCheck(), INFO);
  useUIStore.setState({ activeDialog: null, dialogData: {}, screenOccupants: 0 });
  useProjectStore.setState({ loading: { ...initialLoadingState } });
});

afterEach(() => {
  act(() => useProjectStore.getState().resetLoading());
  act(() => ui().closeDialog());
  useUpdateStore.getState().reset();
});

describe('the install hold, the loading window and the deferred dialog', () => {
  it('a dialog deferred during the install opens on top once a failed install lifts the hold, live and focused', () => {
    render(<App />);
    screen.getByTestId('behind').focus();
    setInstalling(true);
    const overlay = screen.getByTestId('update-install-overlay');
    expect(overlay).toHaveFocus();

    openError('The file is corrupt.');
    expect(screen.queryByTestId('error-dialog')).toBeNull();

    setInstalling(false);

    expect(screen.queryByTestId('update-install-overlay')).toBeNull();
    const layer = errorLayer();
    expect(isTopmost(layer)).toBe(true);
    expect(layer.closest('[inert]')).toBeNull();
    // Focus went to the dialog, not back to the app control the install window took it from.
    expect(layer.contains(document.activeElement)).toBe(true);
    expect(screen.getByTestId('behind')).not.toHaveFocus();
    expect(screen.getByTestId('behind').closest('[inert]')).not.toBeNull();
  });

  it('a load running when the install began: its window stays under the install window, and under the deferred dialog after a failed install', () => {
    render(<App />);
    act(() => {
      useProjectStore.setState({
        loading: { ...initialLoadingState, isLoading: true, clientOpId: 3, projectName: 'CaveB', projectPath: '/caves/CaveB.stalx' },
      });
    });
    const loading = loadingLayer();
    expect(isTopmost(loading)).toBe(true);

    setInstalling(true);
    const overlay = screen.getByTestId('update-install-overlay');
    expect(isTopmost(overlay)).toBe(true);
    expect(loading.hasAttribute('inert')).toBe(true);

    openError('The SpeleoDB copy moved on.');
    expect(screen.queryByTestId('error-dialog')).toBeNull();
    expect(isTopmost(overlay)).toBe(true);

    setInstalling(false);

    // The deferred dialog is the live layer, above the loading window it did not displace.
    const layer = errorLayer();
    expect(isTopmost(layer)).toBe(true);
    expect(layer.hasAttribute('inert')).toBe(false);
    expect(layer.contains(document.activeElement)).toBe(true);
    expect(loadingLayer()).toBe(loading);
    expect(loading.hasAttribute('inert')).toBe(true);

    // Answered: the loading window is the live layer again.
    act(() => ui().closeDialog());
    expect(screen.queryByTestId('error-dialog')).toBeNull();
    expect(isTopmost(loading)).toBe(true);
    expect(loading.hasAttribute('inert')).toBe(false);
  });

  it('control: a load starting while a dialog is up goes beneath it, with no install involved', () => {
    render(<App />);
    openError('first');
    act(() => {
      useProjectStore.setState({
        loading: { ...initialLoadingState, isLoading: true, clientOpId: 4, projectName: 'CaveC', projectPath: '/caves/CaveC.stalx' },
      });
    });

    expect(isTopmost(errorLayer())).toBe(true);
    expect(loadingLayer().hasAttribute('inert')).toBe(true);
  });
});
