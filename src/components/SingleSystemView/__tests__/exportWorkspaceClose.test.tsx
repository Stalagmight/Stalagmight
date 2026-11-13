/**
 * The Export workspace's visible ways out: the toolbar's "Close export" button
 * (in Import's slot, Export only) and the Export panel header's close button.
 * Both return to the mode Export was entered from.
 */
import { describe, it, expect, beforeAll, beforeEach, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';

import '../../../i18n';

vi.mock('../../../utils/ipc', () => ({
  invokeIpc: vi.fn().mockResolvedValue(undefined),
  getErrorMessage: (_e: unknown, f?: string) => f ?? 'error',
}));

import Toolbar from '../Toolbar';
import { ExportPanel } from '../ExportPanel';
import { useUIStore, type AppMode } from '../../../store/uiStore';
import { useProjectStore } from '../../../store/projectStore';

beforeAll(() => {
  // jsdom has no ResizeObserver; the Toolbar measures its header with one.
  globalThis.ResizeObserver ??= class {
    observe(): void {}
    unobserve(): void {}
    disconnect(): void {}
  } as unknown as typeof ResizeObserver;
});

beforeEach(() => {
  useUIStore.setState({ appMode: 'survey', geometryMode: 'measured', placementMode: null, dataTableFullScreen: false });
  useUIStore.setState({ exportReturnMode: null });
  useProjectStore.setState({ project: { file_path: '/synthetic.stalx', sections: [], zero_datums: [] } as never });
});

function renderToolbar(): void {
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
}

describe('toolbar "Close export"', () => {
  it('is absent outside Export', () => {
    renderToolbar();
    expect(screen.queryByTestId('export-close-button')).toBeNull();
    expect(screen.getByTestId('toolbar-import-button')).toBeInTheDocument();
  });

  it.each<Exclude<AppMode, 'export'>>(['survey', 'data', 'plan'])('returns to %s', (from) => {
    useUIStore.getState().setAppMode(from);
    useUIStore.getState().enterExport();
    renderToolbar();
    const button = screen.getByTestId('export-close-button');
    expect(button).toHaveTextContent('Close export');
    expect(screen.queryByTestId('toolbar-import-button')).toBeNull();
    fireEvent.click(button);
    expect(useUIStore.getState().appMode).toBe(from);
  });
});

describe('Export panel header close', () => {
  it('returns to the mode Export was entered from', () => {
    useUIStore.getState().setAppMode('plan');
    useUIStore.getState().enterExport();
    render(<ExportPanel />);
    const close = screen.getByTestId('export-panel-close');
    expect(close).toHaveAccessibleName('Close export');
    fireEvent.click(close);
    expect(useUIStore.getState().appMode).toBe('plan');
  });

  it('opens on the image export with its single Export button', () => {
    useUIStore.getState().enterExport();
    render(<ExportPanel />);
    expect(screen.getByTestId('export-type-image')).toBeChecked();
    expect(screen.getByTestId('export-button')).toBeInTheDocument();
    expect(screen.queryByTestId('export-cancel')).toBeNull();
  });
});
