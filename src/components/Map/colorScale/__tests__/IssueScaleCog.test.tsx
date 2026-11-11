/**
 * The Survey Issues legend cog: palette, combine mode and per-error switches
 * persist through `update_map_palette` (config only, never the project file);
 * the levels are session state; a switched-off error leaves the legend key.
 */
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const invoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...args: unknown[]) => invoke(...args) }));

import '../../../../i18n';
import { IssueLegend } from '../../IssueLegend';
import { useColorScaleStore } from '../../../../store/colorScaleStore';
import { useProjectStore } from '../../../../store/projectStore';
import { defaultConfig } from '../../../../types/config';
import type { Config } from '../../../../types/ipc.generated';

const paletteWrites = () => invoke.mock.calls.filter(([cmd]) => cmd === 'update_map_palette').map(([, a]) => a.palette);

function openCog() {
  render(<IssueLegend showRamp showKey />);
  fireEvent.click(screen.getByTestId('issues-legend-settings-button'));
  return screen.getByTestId('color-scale-popover-issues');
}

describe('Survey Issues colour cog', () => {
  beforeEach(() => {
    invoke.mockReset();
    invoke.mockResolvedValue(null);
    useProjectStore.setState({ config: structuredClone(defaultConfig) as Config });
    useColorScaleStore.getState().resetSession();
  });

  it('sits beside the legend and lists every error, the first one open', () => {
    const popover = openCog();
    expect(screen.getByTestId('issue-legend').contains(screen.getByTestId('issues-legend-settings-button'))).toBe(false);
    for (const id of ['azimuth', 'inclination', 'inclinationDepth', 'length', 'closure']) {
      expect(popover.querySelector(`[data-testid=issue-scale-row-${id}]`)).not.toBeNull();
    }
    expect(screen.getByTestId('issue-scale-azimuth-slider')).toBeInTheDocument();
    expect(screen.queryByTestId('issue-scale-closure-slider')).toBeNull();
    expect(screen.getByTestId('issue-scale-palette-trigger')).toHaveTextContent('Default');
    expect(screen.getByTestId('issue-scale-note')).toHaveTextContent('Project Settings');
  });

  it('defaults the levels to the project thresholds (warn) and three times them (max)', () => {
    openCog();
    expect(screen.getByTestId('issue-scale-azimuth-warn-input')).toHaveValue(
      defaultConfig.validation.azimuth_diff_threshold.toFixed(1),
    );
    expect(screen.getByTestId('issue-scale-azimuth-max-input')).toHaveValue(
      (3 * defaultConfig.validation.azimuth_diff_threshold).toFixed(1),
    );
  });

  it('switching an error off persists it and drops it from the legend key', async () => {
    openCog();
    expect(screen.getByTestId('issue-legend-key-length')).toBeInTheDocument();
    fireEvent.click(screen.getByTestId('issue-scale-enable-length'));
    await waitFor(() => expect(paletteWrites()).toHaveLength(1));
    expect(paletteWrites()[0].issues_hidden).toEqual(['length']);
    expect(screen.queryByTestId('issue-legend-key-length')).toBeNull();
    expect(screen.getByTestId('issue-scale-summary-length')).toHaveTextContent('Off');
    expect(invoke.mock.calls.map(([c]) => c)).not.toContain('save_display_setting');
  });

  it('the combine mode persists and explains itself', async () => {
    openCog();
    expect(screen.getByTestId('issue-scale-combine-worst')).toHaveAttribute('aria-checked', 'true');
    fireEvent.click(screen.getByTestId('issue-scale-combine-average'));
    await waitFor(() => expect(paletteWrites()).toHaveLength(1));
    expect(paletteWrites()[0].issues_combine).toBe('average');
    expect(screen.getByTestId('issue-scale-combine-help')).toHaveTextContent('average');
  });

  it('levels are session state with a per-error reset; typing the default back clears the override', () => {
    openCog();
    const reset = screen.getByTestId('issue-scale-reset-azimuth');
    const warn = screen.getByTestId('issue-scale-azimuth-warn-input');
    fireEvent.change(warn, { target: { value: '2' } });
    fireEvent.keyDown(warn, { key: 'Enter' });
    expect(useColorScaleStore.getState().issueLevels.azimuth?.warn).toBe(2);
    expect(paletteWrites()).toHaveLength(0);
    fireEvent.click(reset);
    expect(useColorScaleStore.getState().issueLevels.azimuth).toBeUndefined();
    const t = defaultConfig.validation.azimuth_diff_threshold;
    fireEvent.change(warn, { target: { value: String(t / 2) } });
    fireEvent.keyDown(warn, { key: 'Enter' });
    fireEvent.change(warn, { target: { value: String(t) } });
    fireEvent.keyDown(warn, { key: 'Enter' });
    expect(useColorScaleStore.getState().issueLevels.azimuth).toBeUndefined();
  });

  it('closing ends a palette preview', () => {
    openCog();
    act(() => useColorScaleStore.getState().setPalettePreview({ kind: 'issues', choice: { id: 'mpl.jet', reversed: false } }));
    act(() => {
      window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true }));
    });
    expect(screen.queryByTestId('color-scale-popover-issues')).toBeNull();
    expect(useColorScaleStore.getState().palettePreview).toBeNull();
  });
});
