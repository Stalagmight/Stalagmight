/**
 * The shared gas-config disk-write queue (#317 review round). Two behaviors matter and each
 * has a test that fails without its mechanism: rapid patches MERGE into one write (the
 * per-keystroke IPC/LSQ-drop regression), and a write queued while another is in flight
 * merges against the first write's LANDED state, not the pre-write snapshot (the
 * two-writers-clobber-each-other bug: the debounced swim-speed write used to read a config
 * whose `gas.dpv` predated a rig toggle inside the same window, and wrote the old rig back).
 *
 * A third now: the write must carry ONLY the `[gas]` block (#357 item 2). Chaining fixed
 * gas-vs-gas but nothing else, because the payload was a whole `Config` spread from a store
 * snapshot - so a data-table column resize landing inside a gas field's debounce window was
 * reverted by this module's stale copy of `panels`.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';

import { queueGasConfigWrite, flushGasConfigWrite } from '../gasConfigDisk';
import { useProjectStore } from '../../store/projectStore';
import { defaultConfig } from '../../types/config';
import type { Config } from '../../types/config';
import type { DpvRig, GasConfig } from '../../types/ipc.generated';

const RIG: DpvRig = {
  primary: { speed_m_per_min: 45, burn_minutes: 60 },
  backup: { speed_m_per_min: 45, burn_minutes: 45 },
  range_reserve_fraction: 0.1,
};

const OK = { success: true, error: null, warnings: [], line_ids: [] };

beforeEach(async () => {
  // Drain a previous test's queue into its own mock before installing a fresh one.
  await flushGasConfigWrite();
  useProjectStore.setState({
    config: structuredClone(defaultConfig) as Config,
    updateGasConfig: vi.fn(async (gas: GasConfig) => {
      // Simulate the real updateGasConfig's post-IPC `loadConfig()`: the saved config only
      // reflects the write once the round trip completes.
      useProjectStore.setState({ config: { ...useProjectStore.getState().config!, gas } });
      return OK;
    }),
  });
});

describe('queueGasConfigWrite', () => {
  it('merges rapid patches into ONE write carrying all of them, latest value per field winning', async () => {
    queueGasConfigWrite({ swim_speed_m_per_min: 11 });
    queueGasConfigWrite({ swim_speed_m_per_min: 12 });
    queueGasConfigWrite({ dpv: RIG });
    await flushGasConfigWrite();

    const updateGasConfig = useProjectStore.getState().updateGasConfig as ReturnType<typeof vi.fn>;
    expect(updateGasConfig).toHaveBeenCalledTimes(1);
    const written = updateGasConfig.mock.calls[0]?.[0] as GasConfig;
    expect(written.swim_speed_m_per_min).toBe(12);
    expect(written.dpv).toEqual(RIG);
  });

  it('writes through the gas-only command, never a whole-Config write (#357 item 2)', async () => {
    // The regression this pins: the queue used to call `updateConfig` with a whole
    // `Config` spread from the store snapshot, so it carried a stale copy of every
    // OTHER block. A data-table column resize (500 ms debounce) landing inside a gas
    // field's 450 ms debounce was silently reverted by this module's stale `panels`.
    // Chaining could never fix that - the payload was already stale when it was
    // built, so ordering only decides which stale copy wins.
    const updateConfig = vi.fn(async () => OK);
    useProjectStore.setState({ updateConfig });

    queueGasConfigWrite({ swim_speed_m_per_min: 33 });
    await flushGasConfigWrite();

    expect(updateConfig).not.toHaveBeenCalled();
    const updateGasConfig = useProjectStore.getState().updateGasConfig as ReturnType<typeof vi.fn>;
    expect(updateGasConfig).toHaveBeenCalledTimes(1);
    // A GasConfig, not a Config: no `panels`, no `display`, nothing else to go stale.
    const written = updateGasConfig.mock.calls[0]?.[0] as Record<string, unknown>;
    expect(written).not.toHaveProperty('panels');
    expect(written).not.toHaveProperty('display');
    expect(written.swim_speed_m_per_min).toBe(33);
  });

  it('is a no-op with no loaded config', async () => {
    useProjectStore.setState({ config: null });
    queueGasConfigWrite({ swim_speed_m_per_min: 9 });
    await flushGasConfigWrite();
    const updateGasConfig = useProjectStore.getState().updateGasConfig as ReturnType<typeof vi.fn>;
    expect(updateGasConfig).not.toHaveBeenCalled();
  });

  it('a write queued while another is in flight merges against the first write LANDING, not the stale snapshot', async () => {
    let releaseFirst = () => {};
    const gate = new Promise<void>((resolve) => {
      releaseFirst = resolve;
    });
    const applied: GasConfig[] = [];
    useProjectStore.setState({
      updateGasConfig: vi.fn(async (gas: GasConfig) => {
        applied.push(gas);
        if (applied.length === 1) await gate; // hold the first round trip open
        useProjectStore.setState({ config: { ...useProjectStore.getState().config!, gas } });
        return OK;
      }),
    });

    queueGasConfigWrite({ swim_speed_m_per_min: 42 });
    void flushGasConfigWrite(); // first write now in flight, held at the gate

    queueGasConfigWrite({ dpv: RIG });
    const all = flushGasConfigWrite(); // second write chains behind the first

    releaseFirst();
    await all;

    expect(applied).toHaveLength(2);
    // The clobber shape this guards: a second writer reading the pre-write snapshot would
    // carry swim_speed's DEFAULT here and silently revert the first write on disk.
    expect(applied[1]?.swim_speed_m_per_min).toBe(42);
    expect(applied[1]?.dpv).toEqual(RIG);
  });
});
