/**
 * #407 follow-up: the Plan/Export mode-only DMP rejection is a choice, not an
 * error. "Switch to Survey" must flip the mode AND continue importing the
 * already-picked file (completing the dialog so the telemetry source survives);
 * "Stay in Plan/Export" must close without touching the mode, and its label
 * must name the mode that raised the prompt.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';

// vi.mock factories are hoisted above imports, so everything they touch lives
// in vi.hoisted. `env.restoredGeometryMode` models the view setAppMode('survey')
// restores - the switch path reads it fresh via getState() after the mode flip.
const h = vi.hoisted(() => {
  const env = {
    closeDialog: vi.fn(),
    setAppMode: vi.fn(),
    setGeometryMode: vi.fn(),
    routeOpenedFile: vi.fn(),
    trackDialogComplete: vi.fn(),
    appMode: 'plan',
    restoredGeometryMode: 'measured',
    dialogData: {} as { path?: string; cause?: string },
  };
  const storeState = () => ({
    closeDialog: env.closeDialog,
    setAppMode: env.setAppMode,
    setGeometryMode: env.setGeometryMode,
    dialogData: env.dialogData,
    appMode: env.appMode,
    geometryMode: env.restoredGeometryMode,
  });
  const useUIStore = (sel: (s: unknown) => unknown) => sel(storeState());
  useUIStore.getState = storeState;
  return { env, useUIStore };
});
const { closeDialog, setAppMode, setGeometryMode, routeOpenedFile, trackDialogComplete } = h.env;

vi.mock('react-i18next', () => ({
  useTranslation: () => ({ t: (key: string) => key }),
}));

vi.mock('../../../store/uiStore', () => ({
  useUIStore: h.useUIStore,
  isEditableMode: (mode: string) => mode === 'survey' || mode === 'data',
}));

vi.mock('../../../utils/openOrImport', () => ({
  routeOpenedFile: (...args: unknown[]) => routeOpenedFile(...args),
}));

vi.mock('../../../utils/telemetry', () => ({
  trackDialogComplete: (...args: unknown[]) => trackDialogComplete(...args),
}));

vi.mock('../DialogWrapper', () => ({
  DialogWrapper: ({ title, children, 'data-testid': testId }: { title: string; children: React.ReactNode; 'data-testid'?: string }) => (
    <div data-testid={testId}>
      <h2>{title}</h2>
      {children}
    </div>
  ),
}));

import ImportBlockedDialog from '../ImportBlockedDialog';
import { useExportStore } from '../../../store/exportStore';

beforeEach(() => {
  vi.clearAllMocks();
  h.env.appMode = 'plan';
  h.env.restoredGeometryMode = 'measured';
  h.env.dialogData.path = '/cave/survey.dmp';
  h.env.dialogData.cause = 'mode';
});

describe('ImportBlockedDialog', () => {
  it('"Switch to Survey" completes the dialog, switches the mode, and re-routes the picked file', () => {
    render(<ImportBlockedDialog />);
    fireEvent.click(screen.getByTestId('import-blocked-switch'));
    expect(trackDialogComplete).toHaveBeenCalledWith('import-blocked');
    expect(closeDialog).toHaveBeenCalled();
    expect(setAppMode).toHaveBeenCalledWith('survey');
    // The restored view was already measured - nothing to silently fix.
    expect(setGeometryMode).not.toHaveBeenCalled();
    // residualReadOnlyToast: the one cause a switch cannot fix (SpeleoDB lock)
    // must toast, not raise a second modal.
    expect(routeOpenedFile).toHaveBeenCalledWith('/cave/survey.dmp', 'Import Failed', {
      residualReadOnlyToast: true,
    });
  });

  it('silently drops a restored closure-adjusted view to measured so the import actually runs', () => {
    h.env.restoredGeometryMode = 'closureAdjusted';
    render(<ImportBlockedDialog />);
    fireEvent.click(screen.getByTestId('import-blocked-switch'));
    expect(setAppMode).toHaveBeenCalledWith('survey');
    expect(setGeometryMode).toHaveBeenCalledWith('measured');
    expect(routeOpenedFile).toHaveBeenCalledWith('/cave/survey.dmp', 'Import Failed', {
      residualReadOnlyToast: true,
    });
  });

  it('"Stay" closes without switching the mode or routing anything', () => {
    render(<ImportBlockedDialog />);
    fireEvent.click(screen.getByTestId('import-blocked-stay'));
    expect(closeDialog).toHaveBeenCalled();
    expect(setAppMode).not.toHaveBeenCalled();
    expect(routeOpenedFile).not.toHaveBeenCalled();
    // Not completed - closeDialog's cancel hook owns dropping the source.
    expect(trackDialogComplete).not.toHaveBeenCalled();
  });

  it('names the mode that raised it: Stay in Plan vs Stay in Export', () => {
    const { unmount } = render(<ImportBlockedDialog />);
    expect(screen.getByTestId('import-blocked-stay').textContent).toBe('dialogs.importBlocked.stayInPlan');
    unmount();
    h.env.appMode = 'export';
    render(<ImportBlockedDialog />);
    expect(screen.getByTestId('import-blocked-stay').textContent).toBe('dialogs.importBlocked.stayInExport');
  });

  it('cause "view" (closure-adjusted in Survey): switches the view, not the mode, and continues', () => {
    h.env.appMode = 'survey';
    h.env.restoredGeometryMode = 'closureAdjusted';
    h.env.dialogData.cause = 'view';
    render(<ImportBlockedDialog />);
    expect(screen.getByTestId('import-blocked-dialog').textContent).toContain(
      'dialogs.importBlocked.bodyView',
    );
    expect(screen.getByTestId('import-blocked-switch').textContent).toBe(
      'dialogs.importBlocked.switchToMeasured',
    );
    expect(screen.getByTestId('import-blocked-stay').textContent).toBe(
      'dialogs.importBlocked.stayInAdjusted',
    );
    fireEvent.click(screen.getByTestId('import-blocked-switch'));
    // Already in Survey - only the view needs fixing.
    expect(setAppMode).not.toHaveBeenCalled();
    expect(setGeometryMode).toHaveBeenCalledWith('measured');
    expect(routeOpenedFile).toHaveBeenCalledWith('/cave/survey.dmp', 'Import Failed', {
      residualReadOnlyToast: true,
    });
  });

  it('cause "view" in Data: stays in Data, switches only the view, and continues', () => {
    // Data edits like Survey, so a closure-adjusted Data user is not sent to
    // Survey - the Raw Data View alone makes the import possible.
    h.env.appMode = 'data';
    h.env.restoredGeometryMode = 'closureAdjusted';
    h.env.dialogData.cause = 'view';
    render(<ImportBlockedDialog />);
    expect(screen.getByTestId('import-blocked-switch').textContent).toBe(
      'dialogs.importBlocked.switchToMeasured',
    );
    expect(screen.getByTestId('import-blocked-stay').textContent).toBe(
      'dialogs.importBlocked.stayInAdjusted',
    );
    fireEvent.click(screen.getByTestId('import-blocked-switch'));
    expect(setAppMode).not.toHaveBeenCalled();
    expect(setGeometryMode).toHaveBeenCalledWith('measured');
    expect(routeOpenedFile).toHaveBeenCalledWith('/cave/survey.dmp', 'Import Failed', {
      residualReadOnlyToast: true,
    });
  });

  it('a missing path still switches the mode but routes nothing (no crash)', () => {
    delete h.env.dialogData.path;
    render(<ImportBlockedDialog />);
    fireEvent.click(screen.getByTestId('import-blocked-switch'));
    expect(setAppMode).toHaveBeenCalledWith('survey');
    expect(routeOpenedFile).not.toHaveBeenCalled();
  });

  it('offers no switch out of Export while an export is being written', () => {
    h.env.appMode = 'export';
    useExportStore.setState({ exporting: true });
    try {
      render(<ImportBlockedDialog />);
      const button = screen.getByTestId('import-blocked-switch');
      expect(button).toBeDisabled();
      expect(button).toHaveAttribute('title', 'exportPanel.inProgress');
      fireEvent.click(button);
      expect(setAppMode).not.toHaveBeenCalled();
      expect(routeOpenedFile).not.toHaveBeenCalled();
      // Stay still closes it.
      fireEvent.click(screen.getByTestId('import-blocked-stay'));
      expect(closeDialog).toHaveBeenCalled();
    } finally {
      useExportStore.setState({ exporting: false });
    }
  });

  it('keeps the switch live in Export while no export is running', () => {
    h.env.appMode = 'export';
    render(<ImportBlockedDialog />);
    expect(screen.getByTestId('import-blocked-switch')).not.toBeDisabled();
    expect(screen.getByTestId('import-blocked-switch')).not.toHaveAttribute('title');
  });
});
