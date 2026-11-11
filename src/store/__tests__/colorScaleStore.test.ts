/**
 * The colour scale's two halves and where each may persist:
 *   - the palette choice + pins go to config.toml through `update_map_palette`
 *     and nowhere else - never `save_display_setting` (the .stalx), which is
 *     also what keeps them out of SpeleoDB uploads;
 *   - the Levels ranges are session-only and reset on every project load.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

const invoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...args: unknown[]) => invoke(...args) }));

import { useColorScaleStore, isColorScaleLive } from '../colorScaleStore';
import { currentMapPalette, MAX_PINNED_PALETTES, setPalette, togglePinnedPalette } from '../colorScalePalette';
import { useProjectStore } from '../projectStore';
import { useUIStore } from '../uiStore';
import { DISPLAY_DEFAULTS } from '../displaySelectors';
import { defaultConfig } from '../../types/config';
import type { Config } from '../../types/ipc.generated';

const commandsCalled = () => invoke.mock.calls.map((c) => c[0] as string);

function freshConfig(): Config {
  return structuredClone(defaultConfig) as Config;
}

describe('colour scale persistence', () => {
  beforeEach(() => {
    invoke.mockReset();
    invoke.mockResolvedValue(null);
    useProjectStore.setState({ config: freshConfig() });
    useColorScaleStore.getState().resetSession();
  });

  it('writes a palette choice only through update_map_palette, with the whole table', async () => {
    await setPalette('depth', { id: 'mpl.viridis', reversed: true });
    expect(commandsCalled()).toEqual(['update_map_palette']);
    expect(invoke.mock.calls[0]![1]).toEqual({
      palette: {
        depth: { id: 'mpl.viridis', reversed: true },
        distance: { id: 'theme', reversed: false },
        pinned: [],
      },
    });
    expect(currentMapPalette().depth).toEqual({ id: 'mpl.viridis', reversed: true });
  });

  it('never touches the project file (save_display_setting) for any palette or range action', async () => {
    await setPalette('distance', { id: 'mpl.magma', reversed: false });
    await togglePinnedPalette('mpl.magma');
    const s = useColorScaleStore.getState();
    s.setMapScale('depth', { min: 1, max: 9, gamma: 2 });
    s.setExportScale('distance', { min: 0, max: 50, gamma: 1 });
    s.setPalettePreview({ kind: 'depth', choice: { id: 'mpl.plasma', reversed: false } });
    s.setDragging(true);
    s.setDragging(false);
    s.setPalettePreview(null);
    expect(commandsCalled()).not.toContain('save_display_setting');
    expect(commandsCalled().every((c) => c === 'update_map_palette')).toBe(true);
  });

  it('skips a write that changes nothing', async () => {
    await setPalette('depth', { id: 'theme', reversed: false });
    expect(invoke).not.toHaveBeenCalled();
  });

  it('pins and unpins in order', async () => {
    await togglePinnedPalette('mpl.viridis');
    await togglePinnedPalette('grad.cenote');
    expect(currentMapPalette().pinned).toEqual(['mpl.viridis', 'grad.cenote']);
    await togglePinnedPalette('mpl.viridis');
    expect(currentMapPalette().pinned).toEqual(['grad.cenote']);
  });

  it('stops at the backend pin cap instead of showing a pin that will not survive', async () => {
    const full = Array.from({ length: MAX_PINNED_PALETTES }, (_, i) => `mpl.p${i}`);
    const config = freshConfig();
    useProjectStore.setState({ config: { ...config, map_palette: { ...config.map_palette, pinned: full } } });
    const showToast = vi.fn();
    useUIStore.setState({ showToast });
    await togglePinnedPalette('grad.cenote');
    expect(currentMapPalette().pinned).toEqual(full);
    expect(invoke).not.toHaveBeenCalled();
    expect(showToast).toHaveBeenCalledWith(expect.any(String), 'map-palette-save', 'info');
    // Unpinning still works at the cap.
    await togglePinnedPalette('mpl.p0');
    expect(currentMapPalette().pinned).toHaveLength(MAX_PINNED_PALETTES - 1);
  });

  it('rolls back a failed write and tells the user', async () => {
    invoke.mockRejectedValueOnce(new Error('disk full'));
    const showToast = vi.fn();
    useUIStore.setState({ showToast });
    await setPalette('depth', { id: 'mpl.viridis', reversed: false });
    expect(currentMapPalette().depth).toEqual({ id: 'theme', reversed: false });
    expect(showToast).toHaveBeenCalledWith(expect.any(String), 'map-palette-save', 'error');
  });
});

describe('colour scale session state', () => {
  beforeEach(() => {
    invoke.mockReset();
    invoke.mockResolvedValue(null);
    useProjectStore.setState({ config: freshConfig() });
    useColorScaleStore.getState().resetSession();
  });

  it('a project load resets the ranges and preview but keeps the persisted palette', async () => {
    await setPalette('depth', { id: 'mpl.cividis', reversed: false });
    const s = useColorScaleStore.getState();
    s.setMapScale('depth', { min: 2, max: 40, gamma: 1.5 });
    s.setExportScale('depth', { min: 0, max: 10, gamma: 1 });
    s.setPalettePreview({ kind: 'depth', choice: { id: 'mpl.magma', reversed: false } });

    useUIStore.getState().applyDisplaySettings({ ...DISPLAY_DEFAULTS, geometryMode: 'measured' });

    const after = useColorScaleStore.getState();
    expect(after.mapScale).toEqual({ depth: null, distance: null });
    expect(after.exportScale).toEqual({ depth: null, distance: null });
    expect(after.palettePreview).toBeNull();
    expect(currentMapPalette().depth.id).toBe('mpl.cividis');
  });

  it('is live while dragging or previewing a palette', () => {
    expect(isColorScaleLive(useColorScaleStore.getState())).toBe(false);
    useColorScaleStore.getState().setDragging(true);
    expect(isColorScaleLive(useColorScaleStore.getState())).toBe(true);
    useColorScaleStore.getState().setDragging(false);
    useColorScaleStore.getState().setPalettePreview({ kind: 'distance', choice: { id: 'mpl.jet', reversed: false } });
    expect(isColorScaleLive(useColorScaleStore.getState())).toBe(true);
  });
});
