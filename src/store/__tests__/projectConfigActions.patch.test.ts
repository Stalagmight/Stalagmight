/**
 * #576 D27-32(c) (decided: "Patch changed fields"). Config writers used to post the
 * whole `Config` built from this store's snapshot, so a value the backend recorded
 * after that snapshot - the panel layout the user changed, a project they opened -
 * was put back by any unrelated settings save. `updateConfig` now sends only the
 * settings that differ from the store's config (`patch_config`), and the backend
 * applies them onto its live config.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('../../utils/ipc');

import { useProjectStore } from '../projectStore';
import { defaultConfig, type Config } from '../../types/config';
import { invokeIpcMock } from '../../test/factories/ipc';

beforeEach(() => {
  invokeIpcMock.mockReset();
  invokeIpcMock.mockImplementation(async (cmd: string) => {
    if (cmd === 'get_config') return useProjectStore.getState().config;
    if (cmd === 'patch_config' || cmd === 'update_config') return { keychain_residue: null };
    return null;
  });
  useProjectStore.setState({ config: structuredClone(defaultConfig) as Config });
});

const writes = () =>
  invokeIpcMock.mock.calls.filter(([cmd]) => cmd === 'patch_config' || cmd === 'update_config');

describe('config saves send only what changed (#576 D27-32(c))', () => {
  it('updateConfig sends the changed setting alone, never the whole config', async () => {
    const config = useProjectStore.getState().config!;
    const next: Config = { ...config, import: { ...config.import, overlap_mode: 'existing_data' } };

    const result = await useProjectStore.getState().updateConfig(next);

    expect(result.success).toBe(true);
    expect(writes()).toEqual([
      [
        'patch_config',
        { patches: [{ path: ['import', 'overlap_mode'], value: 'existing_data' }] },
        expect.anything(),
      ],
    ]);
  });

  it('a stale panel layout in the store is not sent back', async () => {
    // The store last loaded the config with the data table open; the backend has since
    // recorded it closed (update_panel_layout). A save of an unrelated setting must not
    // carry the store's `data_table_open: true` with it.
    const config = useProjectStore.getState().config!;
    const next: Config = { ...config, general: { ...config.general, auto_save: !config.general.auto_save } };

    await useProjectStore.getState().updateConfig(next);

    const sent = JSON.stringify(writes());
    expect(sent).not.toContain('data_table_open');
    expect(sent).not.toContain('recent_projects');
  });

  it('an unchanged config sends nothing', async () => {
    await useProjectStore.getState().updateConfig(structuredClone(useProjectStore.getState().config!));
    expect(writes()).toEqual([]);
  });

  it('patchConfig sends the patches as given and reloads the saved config', async () => {
    await useProjectStore
      .getState()
      .patchConfig([{ path: ['general', 'onboarding_enabled'], value: false }]);

    expect(writes()[0]?.[1]).toEqual({ patches: [{ path: ['general', 'onboarding_enabled'], value: false }] });
    expect(invokeIpcMock.mock.calls.some(([cmd]) => cmd === 'get_config')).toBe(true);
  });
});
