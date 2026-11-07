/**
 * Component test - the Plan-mode bottom panel's tab strip after the Equipment split (#403
 * follow-up): Cylinders and DPV are separate tabs, and the two cylinder controls that are
 * not per-row (add a tank, conservative gas) live in the STRIP, only while Cylinders is up.
 *
 * What that buys, and therefore what is pinned here: every tab body is exactly one surface
 * (so each grid can fill the pane), and the strip controls still write the same store the
 * old cylinders footer wrote - moving them must not have left them wired to a copy.
 */
import { describe, it, expect, beforeEach, beforeAll, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';

// The grid is virtualized and jsdom gives the virtualizer no layout to measure, so without
// this mock it renders zero rows - the same shim CylindersTable.test.tsx uses.
vi.mock('@tanstack/react-virtual', () => import('../../test/factories/virtualizer'));

import { PlanTablesPanel } from './PlanTablesPanel';
import { useGasPlanStore } from '../../store/gasPlanStore';
import { useProjectStore } from '../../store/projectStore';
import { defaultConfig } from '../../types/config';
import type { Config } from '../../types/config';
import { NoopResizeObserver } from '../../test/factories/dom';

beforeAll(() => {
  // jsdom has no ResizeObserver; the grid measures its scroller with one, and so does
  // `usePlanGridFillHeight` (which falls back to the content-sized height without it).
  if (typeof ResizeObserver === 'undefined') {
    (globalThis as { ResizeObserver?: unknown }).ResizeObserver = NoopResizeObserver;
  }
  Element.prototype.scrollIntoView = vi.fn();
});

beforeEach(() => {
  useGasPlanStore.getState().reset();
  // The DPV tab's editor writes through to the persisted config on every commit.
  useProjectStore.setState({
    config: structuredClone(defaultConfig) as Config,
    updateGasConfig: vi.fn(async () => ({ success: true, error: null, warnings: [], section_ids: [] })),
  });
});

const TABS = ['plan-tab-cylinders', 'plan-tab-dpv', 'plan-tab-segments', 'plan-tab-profile'];

/** The strip controls that belong to the Cylinders tab alone. */
const ACTIONS = ['gas-add-tank', 'gas-conservative-checkbox'];

describe('PlanTablesPanel tabs', () => {
  it('offers all four tabs and opens on Cylinders, showing only the tank grid', () => {
    render(<PlanTablesPanel />);

    for (const tab of TABS) expect(screen.getByTestId(tab)).toBeInTheDocument();
    expect(screen.getByTestId('plan-tab-cylinders')).toHaveAttribute('aria-pressed', 'true');
    expect(screen.getByTestId('cylinders-table')).toBeInTheDocument();
    // One surface per tab: the rig editor no longer shares the pane with the tank list.
    expect(screen.queryByTestId('dpv-rig-editor')).not.toBeInTheDocument();
    expect(screen.queryByTestId('segments-table')).not.toBeInTheDocument();
    expect(screen.queryByTestId('plan-profile')).not.toBeInTheDocument();
  });

  it('gives the DPV rig editor its own tab', () => {
    render(<PlanTablesPanel />);

    fireEvent.click(screen.getByTestId('plan-tab-dpv'));

    expect(screen.getByTestId('dpv-rig-editor')).toBeInTheDocument();
    expect(screen.getByTestId('toggle-dpv-enabled')).toBeInTheDocument();
    expect(screen.queryByTestId('cylinders-table')).not.toBeInTheDocument();
  });

  it('shows the segments grid and the profile chart on their own tabs', () => {
    render(<PlanTablesPanel />);

    fireEvent.click(screen.getByTestId('plan-tab-segments'));
    expect(screen.getByTestId('segments-table')).toBeInTheDocument();

    fireEvent.click(screen.getByTestId('plan-tab-profile'));
    expect(screen.getByTestId('plan-profile')).toBeInTheDocument();
    expect(screen.queryByTestId('segments-table')).not.toBeInTheDocument();
  });
});

describe('PlanTablesPanel cylinder actions in the tab strip', () => {
  it('renders them on the Cylinders tab and nowhere else', () => {
    render(<PlanTablesPanel />);

    for (const id of ACTIONS) expect(screen.getByTestId(id)).toBeInTheDocument();

    for (const tab of ['plan-tab-dpv', 'plan-tab-segments', 'plan-tab-profile']) {
      fireEvent.click(screen.getByTestId(tab));
      for (const id of ACTIONS) expect(screen.queryByTestId(id)).not.toBeInTheDocument();
    }

    // And they come back with the tab, since the controls are the tab's, not the panel's.
    fireEvent.click(screen.getByTestId('plan-tab-cylinders'));
    for (const id of ACTIONS) expect(screen.getByTestId(id)).toBeInTheDocument();
  });

  it('appends a tank from the preset picker', () => {
    render(<PlanTablesPanel />);
    const before = useGasPlanStore.getState().cylinders.length;

    fireEvent.click(screen.getByTestId('gas-add-tank'));
    fireEvent.click(screen.getByTestId('gas-add-tank-option-al80'));

    const { cylinders } = useGasPlanStore.getState();
    expect(cylinders).toHaveLength(before + 1);
    // The preset carries the whole tank spec, not just a name.
    expect(cylinders[cylinders.length - 1]).toEqual(
      expect.objectContaining({ label: 'AL80', water_volume_l: 11.1, fill_bar: 207, enabled: true })
    );
    // The new row is in the grid it was added from.
    expect(screen.getByTestId(`gas-cylinder-row-${cylinders.length - 1}`)).toBeInTheDocument();
  });

  it('toggles conservative gas straight into the gas config', () => {
    render(<PlanTablesPanel />);
    const checkbox = screen.getByTestId('gas-conservative-checkbox');
    expect(useGasPlanStore.getState().gasConfig.conservative_tank_gas).toBe(true);

    fireEvent.click(checkbox);
    expect(useGasPlanStore.getState().gasConfig.conservative_tank_gas).toBe(false);

    fireEvent.click(checkbox);
    expect(useGasPlanStore.getState().gasConfig.conservative_tank_gas).toBe(true);
  });
});
