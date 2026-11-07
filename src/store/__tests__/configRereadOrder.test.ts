/**
 * [#576 NF-W8FE-4] Every config re-read after a write goes through the config chain
 * (`reloadConfigInOrder`), not only the data-table / grid-view ones (RV6b-3).
 *
 * The backend answers concurrent `get_config` calls in whatever order its pool
 * finishes them. A re-read issued while a settings save is still in flight reads
 * the config WITHOUT that save, and can answer after the save's own re-read - the
 * store then shows (and the next whole-config writer starts from) a config
 * missing the save. The gas writer's re-read (`updateGasConfig`) did exactly that.
 *
 * The mock backend here answers each `get_config` with the config as it is when
 * the call is PROCESSED, but holds the answers so the test decides their order:
 * the earliest-issued read answers last, the order that lost the save.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('../../utils/ipc');

import { useProjectStore } from '../projectStore';
import { defaultConfig } from '../../types/config';
import type { Config } from '../../types/config';
import { invokeIpcMock } from '../../test/factories/ipc';

let backend: Config;
/** `get_config` answers held back, in call order: [answer, release]. */
let pendingReads: Array<{ answer: Config; release: () => void }>;
let releasePatch: () => void;

beforeEach(() => {
  backend = structuredClone(defaultConfig) as Config;
  pendingReads = [];
  invokeIpcMock.mockReset();
  invokeIpcMock.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
    switch (cmd) {
      case 'get_config': {
        const answer = structuredClone(backend);
        return new Promise<Config>((resolve) => pendingReads.push({ answer, release: () => resolve(answer) }));
      }
      case 'patch_config':
        // The save is slow: it lands only when the test releases it.
        return new Promise((resolve) => {
          releasePatch = () => {
            backend = { ...backend, general: { ...backend.general, undo_history_limit: 55 } };
            resolve({ keychain_residue: null, persistent_residue: null });
          };
        });
      case 'update_gas_config':
        backend = { ...backend, gas: (args as { gas: Config['gas'] }).gas };
        return Promise.resolve(undefined);
      default:
        return Promise.resolve(null);
    }
  });
  useProjectStore.setState({ config: structuredClone(defaultConfig) as Config });
});

const flush = async () => {
  for (let i = 0; i < 10; i += 1) await Promise.resolve();
};

describe('config re-reads land in order (#576 NF-W8FE-4)', () => {
  it("the gas writer's re-read cannot put back a config without a settings save made before it", async () => {
    const store = useProjectStore.getState();
    const save = store.patchConfig([{ path: ['general', 'undo_history_limit'], value: 55 }]);
    await flush();

    const gas = { ...structuredClone(defaultConfig.gas), swim_speed_m_per_min: 12 };
    const gasWrite = store.updateGasConfig(gas);
    await flush();
    // Not issued while the save is in flight: it would read the config without it.
    expect(pendingReads).toHaveLength(0);

    releasePatch();
    await flush();
    // The save's own re-read, then the gas re-read once it has settled. Released
    // in the losing order: the later-issued one first.
    while (pendingReads.length > 0) {
      pendingReads.pop()!.release();
      await flush();
    }
    await save;
    await gasWrite;

    const config = useProjectStore.getState().config!;
    expect(config.general.undo_history_limit).toBe(55);
    expect(config.gas.swim_speed_m_per_min).toBe(12);
  });
});
