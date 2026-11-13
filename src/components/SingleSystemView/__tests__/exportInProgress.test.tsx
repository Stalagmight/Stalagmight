/**
 * An export being written keeps the Export workspace open: closeExport refuses,
 * Escape and Ctrl+E are inert, and every visible way out (toolbar "Close
 * export", the panel header's close, the menu row, the carousel buttons) is
 * disabled with "Export in progress". ExportControls sets the flag around the
 * export call and clears it when the call settles, success or failure.
 */
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, renderHook, screen, waitFor } from '@testing-library/react';

import '../../../i18n';

vi.mock('../../../utils/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../utils/ipc')>()),
  invokeIpc: vi.fn().mockResolvedValue(undefined),
}));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => undefined) }));
vi.mock('../../../utils/fileDialogs', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../utils/fileDialogs')>()),
  saveDialog: vi.fn(async () => '/synthetic/out.csv'),
}));

import Toolbar from '../Toolbar';
import { ExportPanel } from '../ExportPanel';
import { ModeCarousel } from '../ModeCarousel';
import { useProjectMenuItems } from '../useProjectMenuItems';
import ExportControls from '../../Dialogs/ExportControls';
import { useKeyboardShortcuts } from '../../../hooks/useKeyboardShortcuts';
import { useUIStore } from '../../../store/uiStore';
import { useProjectStore } from '../../../store/projectStore';
import { useSpeleoStore } from '../../../store/speleoStore';
import { DEFAULT_IMAGE_OPTIONS, useExportStore } from '../../../store/exportStore';

beforeAll(() => {
  globalThis.ResizeObserver ??= class {
    observe(): void {}
    unobserve(): void {}
    disconnect(): void {}
  } as unknown as typeof ResizeObserver;
});

beforeEach(() => {
  useUIStore.setState({
    appMode: 'survey',
    geometryMode: 'measured',
    placementMode: null,
    dataTableFullScreen: false,
    activeDialog: null,
    selection: null,
    multiSelection: [],
    measureStart: null,
    measureEnd: null,
    toasts: [],
  });
  useUIStore.setState({ exportReturnMode: null });
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local', session: null });
  useProjectStore.setState({ project: { file_path: '/synthetic.stalx', sections: [], zero_datums: [] } as never });
  useExportStore.setState({ exporting: false });
  useUIStore.getState().setAppMode('data');
  useUIStore.getState().enterExport();
});

afterEach(() => {
  useExportStore.setState({ exporting: false });
});

const startExporting = () => act(() => useExportStore.getState().setExporting(true));

describe('closeExport while an export is being written', () => {
  it('refuses, then works once the export settles', () => {
    startExporting();
    useUIStore.getState().closeExport();
    expect(useUIStore.getState().appMode).toBe('export');
    expect(useUIStore.getState().exportReturnMode).toBe('data');
    act(() => useExportStore.getState().setExporting(false));
    useUIStore.getState().closeExport();
    expect(useUIStore.getState().appMode).toBe('data');
  });

  it('leaves Escape and Ctrl+E inert', () => {
    renderHook(() => useKeyboardShortcuts());
    startExporting();
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    expect(useUIStore.getState().appMode).toBe('export');
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'e', ctrlKey: true, bubbles: true }));
    expect(useUIStore.getState().appMode).toBe('export');
  });
});

describe('the close controls while an export is being written', () => {
  it('disables the toolbar and panel close buttons with "Export in progress"', () => {
    startExporting();
    render(
      <Toolbar
        menuItems={[]}
        onImport={vi.fn()}
        onUndo={vi.fn()}
        onRedo={vi.fn()}
        canUndo={false}
        canRedo={false}
        placementModeActive={false}
        dataTableFullScreen={false}
      />,
    );
    render(<ExportPanel />);
    for (const id of ['export-close-button', 'export-panel-close']) {
      const button = screen.getByTestId(id);
      expect(button).toBeDisabled();
      expect(button).toHaveAttribute('title', 'Export in progress');
      fireEvent.click(button);
    }
    expect(useUIStore.getState().appMode).toBe('export');
  });

  it('disables the menu row and the carousel buttons', () => {
    startExporting();
    const { result } = renderHook(() => useProjectMenuItems({ onSave: async () => ({ success: true }) }));
    const row = result.current.find((it) => it.id === 'toolbar-export-button')!;
    expect(row).toMatchObject({ disabled: true, title: 'Export in progress' });

    render(<ModeCarousel />);
    for (const mode of ['survey', 'data', 'plan']) {
      const button = screen.getByTestId(`mode-${mode}`);
      expect(button).toBeDisabled();
      expect(button).toHaveAttribute('title', 'Export in progress');
    }
  });

  it('enables them again once the export settles', () => {
    render(<ModeCarousel />);
    expect(screen.getByTestId('mode-survey')).not.toBeDisabled();
    startExporting();
    expect(screen.getByTestId('mode-survey')).toBeDisabled();
    act(() => useExportStore.getState().setExporting(false));
    expect(screen.getByTestId('mode-survey')).not.toBeDisabled();
  });
});

describe('ExportControls sets the flag around the export', () => {
  const renderControls = () =>
    render(
      <ExportControls
        exportType="data"
        onExportTypeChange={() => undefined}
        dataFormat="csv"
        onDataFormatChange={() => undefined}
        imageOptions={DEFAULT_IMAGE_OPTIONS}
        onImageOptionsChange={() => undefined}
      />,
    );

  it('on while the file is written, off after', async () => {
    let finish: () => void = () => undefined;
    const exportCsv = vi.fn(() => new Promise<void>((resolve) => (finish = resolve)));
    useProjectStore.setState({ exportCsv } as never);
    renderControls();
    fireEvent.click(screen.getByTestId('export-button'));
    await waitFor(() => expect(exportCsv).toHaveBeenCalledWith('/synthetic/out.csv'));
    expect(useExportStore.getState().exporting).toBe(true);
    await act(async () => finish());
    expect(useExportStore.getState().exporting).toBe(false);
  });

  it('a failed export clears it too', async () => {
    let fail: (e: Error) => void = () => undefined;
    const exportCsv = vi.fn(() => new Promise<void>((_resolve, reject) => (fail = reject)));
    useProjectStore.setState({ exportCsv } as never);
    renderControls();
    fireEvent.click(screen.getByTestId('export-button'));
    await waitFor(() => expect(useExportStore.getState().exporting).toBe(true));
    await act(async () => fail(new Error('disk full')));
    expect(useExportStore.getState().exporting).toBe(false);
    const { toasts } = useUIStore.getState();
    expect(toasts[toasts.length - 1]).toMatchObject({ kind: 'error', message: 'disk full' });
  });

  it('an export that settles after the panel unmounted still clears it', async () => {
    let finish: () => void = () => undefined;
    const exportCsv = vi.fn(() => new Promise<void>((resolve) => (finish = resolve)));
    useProjectStore.setState({ exportCsv } as never);
    const { unmount } = renderControls();
    fireEvent.click(screen.getByTestId('export-button'));
    await waitFor(() => expect(useExportStore.getState().exporting).toBe(true));
    unmount();
    await act(async () => finish());
    expect(useExportStore.getState().exporting).toBe(false);
  });
});
