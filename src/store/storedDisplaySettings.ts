/**
 * [#576 Q-W22S-1-D1] Show the stored display settings again after a project switch's
 * flush of the queued display writes gave up (`flushProjectSettingWrites`).
 *
 * A display write still queued when the flush's bound ran out is dropped unsent
 * (#576 FZ-FZ-INT-6), but the screen kept showing its value: nothing re-read the
 * setting when the switch then kept the project open (ALREADY_OPEN, or a load that
 * failed). The fuzzer saw "showGrid: shown true, stored false". Now the switch
 * re-reads the stored settings and shows the stored value of each key whose newest
 * write was dropped, and once more when the write the flush gave up on settles (it
 * was already sent, and may still land).
 */
import type { DisplaySettings } from '../types/loading';
import { invokeIpc } from '../utils/ipc';
import { droppedWriteShown, SETTING_KEYS } from '../utils/settingWriteChain';
import { openProjectIdentity, projectSessionEpoch } from './projectSession';
import { colorModeFlags, GEOMETRY_MODE_DEFAULT, useUIStore, type GeometryMode } from './uiStore';

type DisplayKey = keyof DisplaySettings;
type UIState = ReturnType<typeof useUIStore.getState>;

const TRUE_BY_DEFAULT: ReadonlySet<DisplayKey> = new Set<DisplayKey>([
  'showStations',
  'showShots',
  'showSectionNames',
  'showReferenceShots',
  'showJumps',
  'showZeroDatum',
  'showSatellite',
]);
const FALSE_BY_DEFAULT: ReadonlySet<DisplayKey> = new Set<DisplayKey>([
  'showStationLabels',
  'showShotData',
  'showGrid',
  'showDepthGradient',
  'showStationComments',
  'showShotComments',
]);
/** The keys an older file may hold a setting under, newest first (`DisplaySettings::from_stored`). */
const FALLBACK_KEYS: Partial<Record<DisplayKey, readonly string[]>> = {
  showReferenceShots: ['showReferenceShots', 'showVirtualShots'],
  showJumps: ['showJumps', 'showReferenceShots', 'showVirtualShots'],
};
const NUMBER = /^[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?$/;
/** The geometry modes a stored row may name (a parse, not an edit gate). */
const GEOMETRY_MODES: ReadonlySet<string> = new Set<GeometryMode>(['measured', 'closureAdjusted']);

/**
 * One setting as a load would show it, read from the stored rows the way the backend
 * builds the load's `display_settings` (`core/loading.rs` `DisplaySettings::from_stored`):
 * a boolean is "true" or not, with its default when no row holds it; the grid spacing
 * is a number (10 otherwise); the geometry mode is one of its two values ("measured"
 * otherwise). Undefined for a key that is not a stored display setting.
 */
export function storedDisplayValue(rows: Record<string, string>, key: string): boolean | number | string | undefined {
  const k = key as DisplayKey;
  if (TRUE_BY_DEFAULT.has(k) || FALSE_BY_DEFAULT.has(k)) {
    const row = (FALLBACK_KEYS[k] ?? [k]).map((name) => rows[name]).find((v) => v !== undefined);
    return row === undefined ? TRUE_BY_DEFAULT.has(k) : row === 'true';
  }
  if (k === 'gridSpacing') {
    const row = rows.gridSpacing;
    return row !== undefined && NUMBER.test(row) ? Number(row) : 10;
  }
  if (k === 'geometryMode') {
    const row = rows.geometryMode;
    return row !== undefined && GEOMETRY_MODES.has(row) ? row : GEOMETRY_MODE_DEFAULT;
  }
  return undefined;
}

const DISPLAY_SETTING_PREFIX = SETTING_KEYS.displaySetting('');

/**
 * Re-read the stored display settings and show the stored value of each of
 * `chainKeys` (the write chain's `display-setting:<name>` keys) whose newest write is
 * one a flush dropped. Skipped
 * with no project open, outside Survey (Plan and Export show a scoped copy of the
 * display, restored on exit), and when another project opened while the read ran.
 * Best effort: a failed read leaves the screen as it is.
 */
export async function showStoredDisplaySettings(chainKeys: readonly string[]): Promise<void> {
  if (chainKeys.length === 0 || openProjectIdentity() === null) return;
  const session = projectSessionEpoch();
  let rows: Record<string, string>;
  try {
    rows = await invokeIpc('load_display_settings', {}, { purpose: 're-read the stored display settings' });
  } catch {
    return;
  }
  if (typeof rows !== 'object' || rows === null) return;
  if (session !== projectSessionEpoch() || openProjectIdentity() === null) return;
  const state = useUIStore.getState();
  if (state.appMode !== 'survey') return;
  const patch: Partial<UIState> = {};
  for (const chainKey of chainKeys) {
    if (!chainKey.startsWith(DISPLAY_SETTING_PREFIX) || !droppedWriteShown(chainKey)) continue;
    const key = chainKey.slice(DISPLAY_SETTING_PREFIX.length);
    const value = storedDisplayValue(rows, key);
    if (value === undefined) continue;
    if (key === 'showDepthGradient') {
      if (value !== state.showDepthGradient) Object.assign(patch, colorModeFlags(value === true ? 'depth' : 'section'));
    } else {
      Object.assign(patch, { [key]: value });
    }
  }
  if (Object.keys(patch).length > 0) useUIStore.setState(patch);
}
