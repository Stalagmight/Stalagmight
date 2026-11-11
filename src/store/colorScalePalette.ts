/**
 * The persisted half of the colour scale: which palette depth and distance use,
 * and which palettes are pinned. Stored in config.toml `[map_palette]` through
 * the partial `update_map_palette` command, so it follows the user across
 * projects and never touches the .stalx or SpeleoDB. The session half (Levels
 * ranges, preview) is `colorScaleStore.ts`.
 */
import { useProjectStore } from './projectStore';
import { useColorScaleStore } from './colorScaleStore';
import { invokeIpc } from '../utils/ipc';
import { useUIStore } from './uiStore';
import { TOAST_KEYS } from '../utils/toastKeys';
import i18n from '../i18n';
import { DEFAULT_PALETTE_CHOICE, type ColorScaleKind, type PaletteChoice } from '../utils/palettes/types';
import type { MapPaletteConfig } from '../types/ipc.generated';

const DEFAULT_MAP_PALETTE: MapPaletteConfig = {
  depth: { ...DEFAULT_PALETTE_CHOICE },
  distance: { ...DEFAULT_PALETTE_CHOICE },
  pinned: [],
};

/** The persisted `[map_palette]` table, or the defaults before config loads. */
export function currentMapPalette(): MapPaletteConfig {
  return useProjectStore.getState().config?.map_palette ?? DEFAULT_MAP_PALETTE;
}

/** The persisted palette choice for a kind (hook). */
export function usePersistedPalette(kind: ColorScaleKind): PaletteChoice {
  return useProjectStore((s) => s.config?.map_palette?.[kind]) ?? DEFAULT_PALETTE_CHOICE;
}

/** The palette to draw for a kind: the picker's live preview when there is
 *  one for this kind, else the persisted choice (hook). */
export function useEffectivePalette(kind: ColorScaleKind): PaletteChoice {
  const persisted = usePersistedPalette(kind);
  const preview = useColorScaleStore((s) => s.palettePreview);
  return preview && preview.kind === kind ? preview.choice : persisted;
}

/** The pinned palette ids (hook). */
export function usePinnedPalettes(): readonly string[] {
  return useProjectStore((s) => s.config?.map_palette?.pinned) ?? DEFAULT_MAP_PALETTE.pinned;
}

let writeSeq = 0;

/**
 * Publish `next` into the store first (so the map and picker react at once),
 * then persist it. A failed write rolls back to what was there before - unless
 * a newer write has landed since, which owns the state now - and says so.
 */
async function writeMapPalette(next: MapPaletteConfig): Promise<void> {
  const before = useProjectStore.getState().config;
  if (!before) return;
  const seq = ++writeSeq;
  useProjectStore.setState({ config: { ...before, map_palette: next } });
  try {
    await invokeIpc('update_map_palette', { palette: next }, { purpose: 'save the map colour palette' });
  } catch (e) {
    console.error('Failed to save map palette:', e);
    if (seq === writeSeq) {
      const now = useProjectStore.getState().config;
      if (now) useProjectStore.setState({ config: { ...now, map_palette: before.map_palette } });
    }
    useUIStore.getState().showToast(i18n.t('map.colorScale.saveFailed'), TOAST_KEYS.MAP_PALETTE_SAVE, 'error');
  }
}

export function setPalette(kind: ColorScaleKind, choice: PaletteChoice): Promise<void> {
  const current = currentMapPalette();
  const prev = current[kind];
  if (prev.id === choice.id && prev.reversed === choice.reversed) return Promise.resolve();
  return writeMapPalette({ ...current, [kind]: { id: choice.id, reversed: choice.reversed } });
}

/** The most pins config.toml keeps (`MAX_PINNED_PALETTES` in
 *  `config/map_palette.rs`, which drops the rest on write). */
export const MAX_PINNED_PALETTES = 64;

export function togglePinnedPalette(id: string): Promise<void> {
  const current = currentMapPalette();
  if (current.pinned.includes(id)) {
    return writeMapPalette({ ...current, pinned: current.pinned.filter((p) => p !== id) });
  }
  // Refuse rather than show a pin the backend would drop on the next launch.
  if (current.pinned.length >= MAX_PINNED_PALETTES) {
    useUIStore
      .getState()
      .showToast(i18n.t('map.colorScale.pinLimit', { count: MAX_PINNED_PALETTES }), TOAST_KEYS.MAP_PALETTE_SAVE, 'info');
    return Promise.resolve();
  }
  return writeMapPalette({ ...current, pinned: [...current.pinned, id] });
}
