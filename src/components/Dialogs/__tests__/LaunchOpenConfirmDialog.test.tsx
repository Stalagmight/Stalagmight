/**
 * #424: the "swap out an already-open project?" confirm, raised by
 * `routeOpenedFile` for every `.stalx`-open entry point (OS launch channel,
 * picker, drag-drop, Ctrl+O) - not just the launch channel it was first built
 * for. `peekProjectActionSource()` only picks the copy variant; the confirm
 * itself is the same for both triggers.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';

const closeDialog = vi.fn();

vi.mock('react-i18next', async () => (await import('../../../test/factories/i18n')).translationStub('keyAndOptions'));

const dialogData: { path?: string; fileName?: string } = {};

// `activeDialog` is mutable so a test can simulate a state change claiming the
// dialog slot WHILE `routeOpenedFile` is in flight (the defensive-close check).
// `.getState()` mirrors the real store's imperative-read shape, since
// `handleConfirm` reads it outside render, after the await.
let activeDialog: string | null = 'launch-open-confirm';
vi.mock('../../../store/uiStore', () => ({
  useUIStore: Object.assign(
    (sel: (s: unknown) => unknown) => sel({ closeDialog, dialogData }),
    { getState: () => ({ activeDialog, closeDialog, dialogData }) },
  ),
}));

let projectName: string | undefined;
vi.mock('../../../store/projectStore', () => ({
  useProjectStore: (sel: (s: unknown) => unknown) => sel({ project: projectName ? { name: projectName } : null }),
}));

vi.mock('../../../utils/openOrImport', () => ({ routeOpenedFile: vi.fn() }));

let peekedSource: string | undefined;
vi.mock('../../../utils/telemetry', async () =>
  (await import('../../../test/factories/telemetry')).telemetryStubs({
    clearProjectActionSource: vi.fn(),
    peekProjectActionSource: () => peekedSource,
  }),
);

vi.mock('../DialogWrapper', async () => (await import('../../../test/factories/components')).dialogWrapperStub());

vi.mock('../../common/Button', async () => (await import('../../../test/factories/components')).buttonStub());

import { routeOpenedFile } from '../../../utils/openOrImport';
import { trackDialogComplete, clearProjectActionSource } from '../../../utils/telemetry';
import LaunchOpenConfirmDialog from '../LaunchOpenConfirmDialog';

describe('LaunchOpenConfirmDialog', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    dialogData.path = '/cave/other.stalx';
    dialogData.fileName = 'other.stalx';
    projectName = 'My Cave';
    peekedSource = 'os-file-association';
    activeDialog = 'launch-open-confirm';
  });

  it('renders nothing without a pending path', () => {
    dialogData.path = '';
    const { container } = render(<LaunchOpenConfirmDialog />);
    expect(container).toBeEmptyDOMElement();
  });

  it('confirming re-routes the picked file with swapConfirmed: true', async () => {
    render(<LaunchOpenConfirmDialog />);
    fireEvent.click(screen.getByTestId('launch-open-confirm-open'));

    await waitFor(() =>
      expect(routeOpenedFile).toHaveBeenCalledWith('/cave/other.stalx', 'Open Failed', {
        swapConfirmed: true,
      }),
    );
    expect(trackDialogComplete).toHaveBeenCalledWith('launch-open-confirm');
    expect(closeDialog).toHaveBeenCalled();
  });

  // #445 review: closing before routing let the launch channel's "resume the
  // next deferred path" subscriber race the confirmed route outside its own
  // serialization (repro: multi-select two .stalx, confirm the first). Pins
  // the fix - route settles BEFORE this dialog closes itself, on every trigger.
  it('routes the confirmed open before closing itself', async () => {
    const order: string[] = [];
    let resolveRoute: () => void = () => {};
    (routeOpenedFile as ReturnType<typeof vi.fn>).mockImplementation(() => {
      order.push('route-start');
      return new Promise<void>((resolve) => {
        resolveRoute = () => {
          order.push('route-end');
          resolve();
        };
      });
    });
    closeDialog.mockImplementation(() => order.push('close'));

    render(<LaunchOpenConfirmDialog />);
    fireEvent.click(screen.getByTestId('launch-open-confirm-open'));

    await waitFor(() => expect(order).toEqual(['route-start']));
    expect(closeDialog).not.toHaveBeenCalled();

    resolveRoute();
    await waitFor(() => expect(order).toEqual(['route-start', 'route-end', 'close']));
  });

  // If the route settles into a DIFFERENT dialog (e.g. state changed while this
  // one was open and `routeOpenedFile` handed off to unsaved-changes /
  // speleodb-close instead), this handler must not clobber it.
  it('does not close itself if something else claimed the dialog slot while routing', async () => {
    (routeOpenedFile as ReturnType<typeof vi.fn>).mockImplementation(async () => {
      activeDialog = 'unsaved-changes';
    });

    render(<LaunchOpenConfirmDialog />);
    fireEvent.click(screen.getByTestId('launch-open-confirm-open'));

    await waitFor(() => expect(routeOpenedFile).toHaveBeenCalled());
    expect(closeDialog).not.toHaveBeenCalled();
  });

  it('cancelling closes without routing and clears the pending source', () => {
    render(<LaunchOpenConfirmDialog />);
    fireEvent.click(screen.getByTestId('launch-open-confirm-cancel'));
    expect(routeOpenedFile).not.toHaveBeenCalled();
    expect(closeDialog).toHaveBeenCalled();
    expect(clearProjectActionSource).toHaveBeenCalled();
  });

  it('uses the OS-forwarded copy when the source is os-file-association', () => {
    peekedSource = 'os-file-association';
    render(<LaunchOpenConfirmDialog />);
    expect(screen.getByText(/launchOpenConfirm\.body:/)).toBeInTheDocument();
  });

  it('uses the direct-open copy for a picker/drag-drop/Ctrl+O trigger', () => {
    peekedSource = 'toolbar';
    render(<LaunchOpenConfirmDialog />);
    expect(screen.getByText(/launchOpenConfirm\.bodyDirect:/)).toBeInTheDocument();
  });
});
