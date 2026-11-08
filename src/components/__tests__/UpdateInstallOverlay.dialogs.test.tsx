/**
 * [#576 W86] A dialog asked for while an update installs does not open above the
 * "Installing update" window; it opens once the hold lifts.
 *
 * `openDialog` had no install check. Something that needs no click or key press -
 * a SpeleoDB verb in flight answering with a divergence, a load surfacing a
 * corrupt file, an async failure's error - opened its dialog during the install,
 * and the dialog stack put it on top: the overlay went inert and the new dialog
 * took input, so its actions ran under an install the user had already confirmed.
 *
 * Deferred, not refused: the install either ends the process (nothing waiting
 * opens) or fails and gives the app back, and then the dialog the app asked for is
 * the one the user still needs to see. The restart's own exit prompt rides the same
 * path: its guard asks while the hold is up and lowers the hold right after.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { act, render, screen } from '@testing-library/react';

vi.mock('../../utils/telemetry', async () => (await import('../../test/factories/telemetry')).telemetryWith());

import '../../i18n';
import ErrorDialog from '../Dialogs/ErrorDialog';
import { UpdateInstallOverlay } from '../UpdateInstallOverlay';
import { useUpdateStore } from '../../store/updateStore';
import { useUIStore } from '../../store/uiStore';
import { isTopmost } from '../../utils/dialogStack';

const INFO = { version: '9.9.9', notesUrl: 'https://example.test/notes' };

/** The two hosts as App renders them: the overlay, and the error dialog from the slot. */
function App() {
  const activeDialog = useUIStore((s) => s.activeDialog);
  return (
    <>
      <UpdateInstallOverlay />
      {activeDialog === 'error' && <ErrorDialog />}
    </>
  );
}

const ui = () => useUIStore.getState();
const setInstalling = (on: boolean) => act(() => useUpdateStore.getState().setInstalling(on));
const openError = (message: string) => act(() => ui().openDialog('error', { title: 'Open', message }));

beforeEach(() => {
  useUpdateStore.getState().reset();
  const store = useUpdateStore.getState();
  store.setAvailable(store.beginCheck(), INFO);
  useUIStore.setState({ activeDialog: null, dialogData: {}, screenOccupants: 0 });
});

describe('a dialog asked for during an update install', () => {
  it('does not open above the install window: the window stays the top, live one', () => {
    render(<App />);
    setInstalling(true);
    const overlay = screen.getByTestId('update-install-overlay');

    openError('The file is corrupt.');

    expect(screen.queryByTestId('error-dialog')).toBeNull();
    expect(ui().activeDialog).toBeNull();
    expect(isTopmost(overlay)).toBe(true);
    expect(overlay.hasAttribute('inert')).toBe(false);
  });

  it('opens once a failed install lifts the hold, with the data it was asked with', () => {
    render(<App />);
    setInstalling(true);
    openError('The file is corrupt.');
    expect(screen.queryByTestId('error-dialog')).toBeNull();

    setInstalling(false);

    expect(screen.queryByTestId('update-install-overlay')).toBeNull();
    const dialog = screen.getByTestId('error-dialog');
    expect(dialog).toHaveTextContent('The file is corrupt.');
    expect(ui().activeDialog).toBe('error');
  });

  it('only the last one asked for opens, as the single dialog slot would have left it', () => {
    render(<App />);
    setInstalling(true);
    act(() => ui().openDialog('speleodb-divergence'));
    openError('second');
    expect(ui().activeDialog).toBeNull();

    setInstalling(false);

    expect(ui().activeDialog).toBe('error');
    expect(ui().dialogData).toMatchObject({ message: 'second' });
  });

  it('a close asked for after it during the install drops it', () => {
    render(<App />);
    setInstalling(true);
    openError('stale');
    act(() => ui().closeDialog());

    setInstalling(false);

    expect(ui().activeDialog).toBeNull();
    expect(screen.queryByTestId('error-dialog')).toBeNull();
  });

  it('a dialog already open under the install window stays there, and is live again after a failed install', () => {
    render(<App />);
    openError('before the install');
    const dialog = screen.getByTestId('error-dialog');

    setInstalling(true);
    const overlay = screen.getByTestId('update-install-overlay');
    expect(isTopmost(overlay)).toBe(true);

    setInstalling(false);
    expect(screen.getByTestId('error-dialog')).toBe(dialog);
    expect(ui().dialogData).toMatchObject({ message: 'before the install' });
  });

  it('control: with no install running, a dialog opens at once', () => {
    render(<App />);

    openError('now');

    expect(screen.getByTestId('error-dialog')).toHaveTextContent('now');
  });
});
