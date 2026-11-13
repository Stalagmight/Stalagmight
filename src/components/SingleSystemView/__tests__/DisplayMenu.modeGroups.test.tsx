/**
 * Which Display-menu groups each workspace mode gets.
 *
 * Data is an editable mode sharing Survey's map, so its map pane gets the whole
 * menu: the Color group (picking a colour there must NOT eject the user to
 * Survey - exitGasPlan is a no-op in Data) and the Geometry group, which is how
 * a Data user on the read-only Closures Computed View gets back to an editable
 * table. Plan keeps neither (the gas gradient owns colour, the mode forces the
 * closure map).
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn().mockResolvedValue({}) }));
vi.mock('react-i18next', () => ({
  useTranslation: () => ({ t: (key: string) => key }),
  initReactI18next: { type: '3rdParty', init: () => undefined },
}));

import DisplayMenu from '../DisplayMenu';
import { useUIStore, type AppMode } from '../../../store/uiStore';
import { useProjectStore } from '../../../store/projectStore';

async function openIn(appMode: AppMode, geometryMode: 'measured' | 'closureAdjusted' = 'measured') {
  useUIStore.setState({ appMode, geometryMode });
  render(<DisplayMenu />);
  await userEvent.click(screen.getByTestId('display-menu-button'));
}

beforeEach(() => {
  useUIStore.setState({ syntacticLevel: 'none', showGrid: false, showDepthGradient: false });
  useProjectStore.setState({ project: null });
});

describe('DisplayMenu groups per mode', () => {
  it('Data shows the Color and the Geometry groups', async () => {
    await openIn('data');
    expect(screen.getByTestId('display-color-section')).toBeInTheDocument();
    expect(screen.getByTestId('geometry-mode-measured')).toBeInTheDocument();
    expect(screen.getByTestId('geometry-mode-closureAdjusted')).toBeInTheDocument();
  });

  it('picking a colour in Data stays in Data', async () => {
    await openIn('data');
    await userEvent.click(screen.getByTestId('display-toggle-showDepthGradient'));
    expect(useUIStore.getState().appMode).toBe('data');
    expect(useUIStore.getState().showDepthGradient).toBe(true);
  });

  it('a closure-adjusted Data user can switch back to the Raw Data View from Data', async () => {
    await openIn('data', 'closureAdjusted');
    await userEvent.click(screen.getByTestId('geometry-mode-measured'));
    expect(useUIStore.getState().geometryMode).toBe('measured');
    expect(useUIStore.getState().appMode).toBe('data');
  });

  it('Plan shows neither the Color nor the Geometry group', async () => {
    await openIn('plan', 'closureAdjusted');
    expect(screen.queryByTestId('display-color-section')).toBeNull();
    expect(screen.queryByTestId('geometry-mode-measured')).toBeNull();
  });
});
