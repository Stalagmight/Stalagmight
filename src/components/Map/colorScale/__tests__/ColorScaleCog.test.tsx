import { act, fireEvent, render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const invoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...args: unknown[]) => invoke(...args) }));

import '../../../../i18n';
import { MapColorLegends } from '../../MapColorLegends';
import { useColorScaleStore } from '../../../../store/colorScaleStore';
import { useProjectStore } from '../../../../store/projectStore';
import { defaultConfig } from '../../../../types/config';
import type { Config } from '../../../../types/ipc.generated';

const depthProps = {
  showDepth: true,
  depthRange: { min: 0, max: 40 },
  surveyData: null,
  showDistance: false,
  distanceField: null,
};

describe('legend colour-scale cog', () => {
  beforeEach(() => {
    invoke.mockReset();
    invoke.mockResolvedValue(null);
    useProjectStore.setState({ config: structuredClone(defaultConfig) as Config });
    useColorScaleStore.getState().resetSession();
  });

  it('sits beside the depth legend, outside the legend element', () => {
    render(<MapColorLegends {...depthProps} />);
    const legend = screen.getByTestId('depth-legend');
    const cog = screen.getByTestId('depth-legend-settings-button');
    expect(legend.contains(cog)).toBe(false);
    expect(legend.querySelectorAll('svg')).toHaveLength(1);
  });

  it('opens the popover, edits the map range, and closes on Escape', () => {
    render(<MapColorLegends {...depthProps} />);
    fireEvent.click(screen.getByTestId('depth-legend-settings-button'));
    const popover = screen.getByTestId('color-scale-popover-depth');
    expect(popover).toHaveAttribute('data-dropdown-open');
    const max = screen.getByTestId('color-scale-levels-max-input');
    fireEvent.change(max, { target: { value: '25' } });
    fireEvent.keyDown(max, { key: 'Enter' });
    expect(useColorScaleStore.getState().mapScale.depth).toEqual({ min: 0, max: 25, gamma: 1 });
    expect(useColorScaleStore.getState().exportScale.depth).toBeNull();
    // The legend follows the range.
    expect(screen.getByTestId('depth-legend-max')).toHaveTextContent('25');
    fireEvent.click(screen.getByTestId('color-scale-range-reset'));
    expect(useColorScaleStore.getState().mapScale.depth).toBeNull();
    act(() => {
      window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }));
    });
    expect(screen.queryByTestId('color-scale-popover-depth')).toBeNull();
  });

  it('reverses and resets the persisted palette through update_map_palette only', async () => {
    render(<MapColorLegends {...depthProps} />);
    fireEvent.click(screen.getByTestId('depth-legend-settings-button'));
    const reset = screen.getByTestId('color-scale-palette-reset');
    expect(reset).toBeDisabled();
    await act(async () => {
      fireEvent.click(screen.getByTestId('color-scale-reverse'));
    });
    expect(useProjectStore.getState().config?.map_palette.depth).toEqual({ id: 'theme', reversed: true });
    expect(screen.getByTestId('color-scale-reverse')).toHaveAttribute('aria-pressed', 'true');
    expect(reset).not.toBeDisabled();
    await act(async () => {
      fireEvent.click(reset);
    });
    expect(useProjectStore.getState().config?.map_palette.depth).toEqual({ id: 'theme', reversed: false });
    expect(invoke.mock.calls.map((c) => c[0])).toEqual(['update_map_palette', 'update_map_palette']);
  });

  it('shows the distance cog while the distance overlay owns the slot', () => {
    render(
      <MapColorLegends
        {...depthProps}
        showDistance
        distanceField={{ distances: [0, 5, null, 12], station_refs: [], max_reachable: 12 } as never}
      />,
    );
    expect(screen.queryByTestId('depth-legend')).toBeNull();
    expect(screen.getByTestId('distance-legend')).toBeInTheDocument();
    fireEvent.click(screen.getByTestId('distance-legend-settings-button'));
    expect(screen.getByTestId('color-scale-popover-distance')).toBeInTheDocument();
    expect(screen.getByTestId('color-scale-levels-histogram').querySelectorAll('rect').length).toBeGreaterThan(0);
  });
});
