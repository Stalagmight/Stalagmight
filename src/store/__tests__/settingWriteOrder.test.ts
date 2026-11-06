/**
 * [#576 SETTINGS-ORDER] Quick repeated writes of one setting: the last one made
 * is the one the backend keeps.
 *
 * The config and display-setting commands run on the backend's blocking pool, so
 * two in flight at once are applied in whatever order the pool finishes them. The
 * mock backend here applies each write after a delay, the FIRST write the slowest:
 * that is the order that lost the user's last click (two grid-spacing clicks, a
 * geometry toggle flipped back, a divider dragged twice) when every write was sent
 * the moment it was made.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';

vi.mock('@tauri-apps/api/core', async () => (await import('../../test/factories/tauri')).tauriCore(vi.fn()));

import { useUIStore } from '../uiStore';
import { useProjectStore } from '../projectStore';
import { chainSettingWrite } from '../../utils/settingWriteChain';
import { flushGasConfigWrite, queueGasConfigWrite } from '../../utils/gasConfigDisk';
import { defaultConfig } from '../../types/config';
import type { Config } from '../../types/config';

/** What the mock backend has applied, by setting. */
let applied: Map<string, unknown>;
/** Per-call apply delays in ms for the writes, in call order (first = slowest). */
let delays: number[];
let inFlight: number;

/** The setting(s) one write touches, keyed as `applied` stores them. */
function settingsOf(cmd: string, args: Record<string, unknown>): Array<[string, unknown]> | null {
  switch (cmd) {
    case 'save_display_setting':
      return [[`display:${String(args.key)}`, args.value]];
    case 'update_panel_sizes':
    case 'update_panel_layout':
      return Object.entries(args).map(([field, value]) => [`panels:${field}`, value]);
    case 'patch_config':
      return (args.patches as Array<{ path: string[]; value: unknown }>).map((p) => [p.path.join('.'), p.value]);
    case 'update_gas_config':
      return Object.entries(args.gas as Record<string, unknown>).map(([field, value]) => [`gas.${field}`, value]);
    default:
      return null;
  }
}

let savedConfig: Config;

beforeEach(() => {
  applied = new Map();
  delays = [];
  inFlight = 0;
  savedConfig = structuredClone(defaultConfig) as Config;
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation(async (cmd: string, args?: unknown) => {
    if (cmd === 'get_config') return structuredClone(savedConfig);
    const settings = settingsOf(cmd, (args ?? {}) as Record<string, unknown>);
    if (!settings) return undefined;
    const delay = delays.shift() ?? 0;
    inFlight++;
    await new Promise((resolve) => setTimeout(resolve, delay));
    for (const [key, value] of settings) applied.set(key, value);
    if (cmd === 'update_gas_config') savedConfig = { ...savedConfig, gas: (args as { gas: Config['gas'] }).gas };
    inFlight--;
    return cmd === 'patch_config' ? { keychain_residue: null, persistent_residue: null } : undefined;
  });
  useUIStore.setState({ appMode: 'survey', gridSpacing: 10, geometryMode: 'measured' });
  useProjectStore.setState({ config: structuredClone(defaultConfig) as Config });
});

/** Wait until every write sent has been applied. */
async function settled(expectedWrites: number) {
  await vi.waitFor(() => {
    const writes = vi
      .mocked(invoke)
      .mock.calls.filter(([cmd, args]) => settingsOf(cmd, (args ?? {}) as Record<string, unknown>) !== null);
    expect(writes.length).toBeGreaterThanOrEqual(expectedWrites);
    expect(inFlight).toBe(0);
  });
}

describe('same-setting writes land in the order they were made', () => {
  it('grid spacing: three quick clicks keep the third', async () => {
    delays = [60, 30, 0];
    useUIStore.getState().setGridSpacing(5);
    useUIStore.getState().setGridSpacing(20);
    useUIStore.getState().setGridSpacing(50);
    await settled(3);
    console.log('backend gridSpacing after clicks 5, 20, 50:', applied.get('display:gridSpacing'));
    expect(applied.get('display:gridSpacing')).toBe('50');
  });

  it('geometry mode: toggled there and back keeps the second toggle', async () => {
    delays = [60, 0];
    useUIStore.getState().setGeometryMode('closureAdjusted');
    useUIStore.getState().setGeometryMode('measured');
    await settled(2);
    console.log('backend geometryMode after closureAdjusted, measured:', applied.get('display:geometryMode'));
    expect(applied.get('display:geometryMode')).toBe('measured');
  });

  it('a display toggle flipped twice keeps the second flip', async () => {
    delays = [60, 0];
    const { saveDisplaySetting } = useUIStore.getState();
    await Promise.all([saveDisplaySetting('showGrid', true), saveDisplaySetting('showGrid', false)]);
    console.log('backend showGrid after true, false:', applied.get('display:showGrid'));
    expect(applied.get('display:showGrid')).toBe('false');
  });

  it('a panel divider dragged twice keeps the second size', async () => {
    delays = [60, 0];
    const { savePanelSizes } = useUIStore.getState();
    await Promise.all([savePanelSizes('sections', 300), savePanelSizes('sections', 220)]);
    const sizes = [...applied].filter(([k]) => k.startsWith('panels:'));
    console.log('backend panel sizes after 300, 220:', JSON.stringify(sizes));
    expect(sizes).toEqual([[expect.any(String), 220]]);
  });

  it('the panel layout snapshot: the later toggle is the one kept', async () => {
    delays = [60, 0];
    useUIStore.setState({ detailPanelOpen: false });
    const first = useUIStore.getState().savePanelLayout();
    useUIStore.setState({ detailPanelOpen: true });
    const second = useUIStore.getState().savePanelLayout();
    await Promise.all([first, second]);
    console.log('backend detailPanelOpen after false, true:', applied.get('panels:detailPanelOpen'));
    expect(applied.get('panels:detailPanelOpen')).toBe(true);
  });

  it('a config setting patched twice keeps the second value', async () => {
    delays = [60, 0];
    const { patchConfig } = useProjectStore.getState();
    await Promise.all([
      patchConfig([{ path: ['general', 'onboarding_enabled'], value: false }]),
      patchConfig([{ path: ['general', 'onboarding_enabled'], value: true }]),
    ]);
    console.log('backend general.onboarding_enabled after false, true:', applied.get('general.onboarding_enabled'));
    expect(applied.get('general.onboarding_enabled')).toBe(true);
  });

  it('gas fields: the gas writer (their one writer) keeps the last value too', async () => {
    delays = [60, 0];
    queueGasConfigWrite({ swim_speed_m_per_min: 11 });
    await flushGasConfigWrite();
    queueGasConfigWrite({ swim_speed_m_per_min: 13 });
    await flushGasConfigWrite();
    await settled(2);
    expect(applied.get('gas.swim_speed_m_per_min')).toBe(13);
  });

  it('a [gas] field Settings saved is not put back by a Plan-mode gas write (#576 FZ-INT-3)', async () => {
    // Settings' patch has landed on the backend; its re-read has not reached the store,
    // whose copy still holds the old value - the window the gas writer's whole [gas]
    // block, spread from that copy, put the old value back in.
    const backendGas: Record<string, unknown> = { ...savedConfig.gas, conservative_tank_gas: false };
    const storeConfig = structuredClone(defaultConfig) as Config;
    useProjectStore.setState({ config: { ...storeConfig, gas: { ...storeConfig.gas, conservative_tank_gas: true } } });
    vi.mocked(invoke).mockImplementation(async (cmd: string, args?: unknown) => {
      const a = (args ?? {}) as Record<string, unknown>;
      if (cmd === 'get_config') return { ...structuredClone(savedConfig), gas: { ...backendGas } };
      if (cmd === 'patch_config') {
        for (const p of a.patches as Array<{ path: string[]; value: unknown }>) {
          if (p.path[0] === 'gas' && p.path.length === 2) backendGas[p.path[1]!] = p.value;
        }
        return { keychain_residue: null, persistent_residue: null };
      }
      if (cmd === 'update_gas_config') Object.assign(backendGas, a.gas as Record<string, unknown>);
      return undefined;
    });

    queueGasConfigWrite({ swim_speed_m_per_min: 20 });
    await flushGasConfigWrite();

    console.log('backend gas after the Plan-mode write:', JSON.stringify({
      conservative_tank_gas: backendGas.conservative_tank_gas,
      swim_speed_m_per_min: backendGas.swim_speed_m_per_min,
    }));
    expect(backendGas.swim_speed_m_per_min).toBe(20);
    expect(backendGas.conservative_tank_gas).toBe(false);
    expect(useProjectStore.getState().config?.gas.conservative_tank_gas).toBe(false);
  });
});

describe('chainSettingWrite', () => {
  it('runs same-key writes one at a time, and a failure does not block the next', async () => {
    const order: string[] = [];
    const slow = chainSettingWrite('k', async () => {
      await new Promise((resolve) => setTimeout(resolve, 30));
      order.push('first');
      throw new Error('refused');
    });
    const fast = chainSettingWrite('k', async () => {
      order.push('second');
      return 2;
    });
    await expect(slow).rejects.toThrow('refused');
    await expect(fast).resolves.toBe(2);
    expect(order).toEqual(['first', 'second']);
  });

  it('leaves different keys independent', async () => {
    const order: string[] = [];
    const slow = chainSettingWrite('a', async () => {
      await new Promise((resolve) => setTimeout(resolve, 30));
      order.push('a');
    });
    const fast = chainSettingWrite('b', async () => {
      order.push('b');
    });
    await Promise.all([slow, fast]);
    expect(order).toEqual(['b', 'a']);
  });
});
