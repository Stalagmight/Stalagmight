import type { StoreApi } from 'zustand';
import i18n from '../i18n';
import { invokeIpc } from '../utils/ipc';
import { chainSettingWrite, SETTING_KEYS } from '../utils/settingWriteChain';
import { useUIStore } from './uiStore';
import type { Config } from '../types/config';
import type { GasConfig } from '../types/ipc.generated';
import type { CommandResult } from '../types/commands';
import type { ConfigSaveReport } from '../types/ipc/ConfigSaveReport';
import type { ConfigPatch } from '../types/ipc/ConfigPatch';
import { configPatches } from '../utils/configPatches';
import type { ProjectStore } from './projectStore';
import { emptyCommandResult } from './commandResult';
import { TOAST_KEYS } from '../utils/toastKeys';

/**
 * Split out of `projectStore.ts` rather than rebaselined (#157: split at an API
 * boundary instead of raising the ratchet). This is a clean one: these actions
 * form the whole config read/write surface and nothing else in the store needs
 * their internals, so they're a plain `(set, get)` factory spread into
 * `create<ProjectStore>`, matching how `create()` itself hands `set`/`get` to the
 * store body - the least invasive shape given no other store here already slices
 * itself this way.
 */
export interface ProjectConfigActions {
  loadConfig: () => Promise<void>;
  /**
   * [#576 RV6b-3] `loadConfig` in the config chain: after every config save (and
   * its own re-read) made before this call. A writer outside that chain - the
   * data-table and grid-view layouts - re-reads through here, so its answer cannot
   * land after a later save's re-read and show the store a config without it.
   * [#576 NF-W8FE-4] So does every other re-read: the startup load, the gas writer,
   * the SpeleoDB credential verbs and the recent-projects edits; Settings > Advanced
   * "Reload from disk" runs its own `reload_config` read in the same chain
   * (#576 RV9b-3). Only a save's own
   * re-read, already inside the chain, calls `loadConfig` directly (calling this
   * there would wait on itself).
   */
  reloadConfigInOrder: () => Promise<void>;
  /**
   * Save the settings in which `config` differs from this store's config (#576
   * D27-32(c)). Only those settings are sent (`patchConfig`), and the backend
   * applies them onto its LIVE config, so a value it recorded since this store last
   * loaded the config (the panel layout and widths, the recent projects) is kept
   * rather than put back. A config already put into the store with `setConfig` has
   * nothing left to save here - send that change with `patchConfig`.
   */
  updateConfig: (config: Config) => Promise<CommandResult>;
  /**
   * Save exactly `patches` (#576 D27-32(c)): each names one setting and its new
   * value; the backend applies them onto its live config, then normalizes and saves
   * it. Refused whole (nothing saved) when a path names no setting.
   */
  patchConfig: (patches: ConfigPatch[]) => Promise<CommandResult>;
  /**
   * Persist ONLY the given `[gas]` fields, each as a `[gas, field]` patch. Plan-mode
   * gas controls must not go through `updateConfig`: a whole-`Config` save from this
   * store's snapshot silently reverted any other partial save that landed in between
   * (#357 item 2) - and the whole `[gas]` block did the same to a gas field Settings
   * had just saved (#576 FZ-INT-3-D1).
   */
  updateGasConfig: (gas: Partial<GasConfig>) => Promise<CommandResult>;
  /** Update config state without persisting to disk */
  setConfig: (config: Config) => void;
}

/**
 * [#576 HFI-12] The backend's recent project paths, re-read in the config chain
 * like every other config re-read (see `reloadConfigInOrder`); null when the config
 * cannot be read. `loadRecentProjects` re-reads them with the list, through
 * {@link withRecentPaths}.
 */
export function readRecentPathsInOrder(): Promise<string[] | null> {
  return chainSettingWrite(SETTING_KEYS.config, () =>
    invokeIpc('get_config', undefined, { purpose: 're-read the recent project paths' }),
  ).then(
    (config) => {
      const paths: unknown = (config as { general?: { recent_projects?: unknown } } | undefined)?.general
        ?.recent_projects;
      return Array.isArray(paths) ? (paths as string[]) : null;
    },
    (e: unknown) => {
      console.error('Failed to re-read the recent project paths:', e);
      return null;
    },
  );
}

/**
 * The store update that puts `paths` into the config's recents, or none when there
 * is no config yet, nothing was read, or the recents already match. Only this one
 * field is taken from the re-read: the backend writes it (open, create, remove,
 * delete) and the frontend never does, so it cannot put back a setting changed since.
 */
export function withRecentPaths(config: Config | null, paths: string[] | null): { config?: Config } {
  if (!config || !paths) return {};
  const current = config.general.recent_projects;
  if (current.length === paths.length && current.every((p, i) => p === paths[i])) return {};
  return { config: { ...config, general: { ...config.general, recent_projects: paths } } };
}

export function createProjectConfigActions(
  set: StoreApi<ProjectStore>['setState'],
  get: StoreApi<ProjectStore>['getState']
): ProjectConfigActions {
  /**
   * The shared tail of a config save, whole or patched: run `save`, say what a save
   * can half-do, then pick up the backend-normalized config and drop what a settings
   * change invalidates.
   *
   * [#576 SETTINGS-ORDER] One save at a time, in the order they were made, each with
   * its re-read: two fire-and-forget patches of one setting (the onboarding flags,
   * the wizard's overlap mode) would otherwise race on the backend's pool and the
   * earlier value could land last - and an earlier save's re-read could land after a
   * later one's and show the store a config without it.
   */
  const persistConfig = (save: () => Promise<ConfigSaveReport | null>): Promise<CommandResult> =>
    chainSettingWrite(SETTING_KEYS.config, () => persistConfigNow(save));

  const persistConfigNow = async (save: () => Promise<ConfigSaveReport | null>): Promise<CommandResult> => {
    try {
      const routingBefore = get().config?.gas.include_reference_legs;
      // The Rust command returns Result<ConfigSaveReport, String> - an error throws;
      // the report carries the one thing a save can half-do (see below).
      const report = await save();
      // Turning the "never use the OS keychain" switch on EVICTS the API key an
      // earlier login left in the keyring, and a locked or read-only keyring can
      // refuse that delete. The settings are saved either way (the eviction runs
      // before the write precisely so a partial failure lands in the recoverable
      // direction), so this cannot be an error - but it must not pass as unqualified
      // success either: Settings would be telling the user their key is not in the
      // OS keychain while a working bearer token sits in it. The backend reports
      // only a residue that may GENUINELY have survived, so this does not fire on
      // the headless box that has no keychain to strand anything in.
      if (report?.keychain_residue) {
        useUIStore
          .getState()
          .showToast(
            i18n.t('settings.speleodb.disableKeychain.evictionFailed', {
              reason: report.keychain_residue,
            }),
            TOAST_KEYS.SPELEODB_KEYCHAIN_RESIDUE,
            'warning',
          );
      }
      // The other half of that same move: the key is re-landed wherever the closed
      // gate allows, which under the default "this session only" preference is
      // memory - and the sweep that makes that true has to remove any copy an
      // earlier storage choice wrote to disk. When it cannot, the saved settings say
      // the key is gone when the app closes while the next launch reads the survivor
      // straight back and signs the user in. Deliberately does not name the keyring:
      // the backend cannot tell which store stranded it, and it is mutually
      // exclusive with the message above, which can.
      if (report?.persistent_residue) {
        useUIStore
          .getState()
          .showToast(
            i18n.t('settings.speleodb.disableKeychain.sweepFailed', {
              reason: report.persistent_residue,
            }),
            TOAST_KEYS.SPELEODB_TOKEN_RESIDUE,
            'warning',
          );
      }
      // Re-fetch config to get backend-normalized values (e.g., zoom_step clamped to
      // valid range) - and, after a patch, every value the backend holds that this
      // store had not seen yet.
      await get().loadConfig();
      // The closure-adjustment sigmas feed the adjusted solve, so a settings change
      // can change the adjusted geometry. Drop the cached set; the map re-fetches
      // lazily (the backend memo is keyed on the config, so a stale config would
      // otherwise keep rendering the old weighted solve until an unrelated edit).
      set({ stationsAdjusted: null, surveyDataAdjusted: null });
      // `gas.include_reference_legs` decides whether penetration routes through
      // reference ties, and `stats.penetration_distance` is computed inside
      // `get_project_state` - not lazily like the geometry above. Nothing else
      // here re-runs it, so without this the Detail panel keeps showing the old
      // figure while the distance overlay and ruler (both keyed on the setting)
      // have already switched, until some unrelated command refreshes state.
      // Gated on an actual change: this is a full project refetch, too heavy to
      // fire on every unrelated settings tweak.
      if (get().config?.gas.include_reference_legs !== routingBefore) {
        await get().refreshProjectState();
      }
      return emptyCommandResult(true, null);
    } catch (e) {
      return emptyCommandResult(false, String(e));
    }
  };

  return {
    loadConfig: async () => {
      try {
        const config = await invokeIpc('get_config', undefined, {
          purpose: 'load user configuration',
        });
        set({ config });
      } catch (e) {
        console.error('Failed to load config:', e);
      }
    },

    reloadConfigInOrder: () => chainSettingWrite(SETTING_KEYS.config, () => get().loadConfig()),

    updateConfig: async (config: Config) => {
      const base = get().config;
      // Nothing loaded to tell the changes from (every writer starts from the loaded
      // config, so not in practice): the whole config is all there is to send. This is
      // the one frontend caller `update_config` has left, and the reason the backend
      // keeps it (#576 NF-W4M-1; see the command's doc).
      if (!base) {
        return persistConfig(() =>
          invokeIpc('update_config', { config }, { purpose: 'persist user settings' }),
        );
      }
      const patches = configPatches(base, config);
      if (patches.length === 0) return emptyCommandResult(true, null);
      return get().patchConfig(patches);
    },

    patchConfig: async (patches: ConfigPatch[]) =>
      persistConfig(() =>
        invokeIpc('patch_config', { patches }, { purpose: 'persist user settings' }),
      ),

    updateGasConfig: async (gas: Partial<GasConfig>) => {
      // [#576 FZ-INT-3-D1] Only the fields given, as `patch_config` patches the way
      // Settings saves, applied onto the backend's live config. The whole block from
      // this store's copy put back a gas field another save had changed since. The
      // backend checks each value as it does the whole block (the patched config is
      // read back as a whole, then normalized) - nothing here relied on sending more.
      const patches: ConfigPatch[] = Object.entries(gas)
        .filter(([, value]) => value !== undefined)
        .map(([field, value]) => ({ path: ['gas', field], value }));
      if (patches.length === 0) return emptyCommandResult(true, null);
      try {
        const routingBefore = get().config?.gas.include_reference_legs;
        await invokeIpc('patch_config', { patches }, { purpose: 'persist gas planning settings' });
        // Re-fetch to pick up the backend-normalized values, same as updateConfig -
        // in the config chain [#576 NF-W8FE-4]: issued while a settings save is in
        // flight, this read could answer after that save's re-read and put back a
        // config without it.
        await get().reloadConfigInOrder();
        // Same reasoning as updateConfig's copy of this: `include_reference_legs`
        // decides whether penetration routes through reference ties, and
        // `stats.penetration_distance` is computed eagerly inside get_project_state.
        if (get().config?.gas.include_reference_legs !== routingBefore) {
          await get().refreshProjectState();
        }
        // Deliberately does NOT drop `stationsAdjusted` / `surveyDataAdjusted` the
        // way updateConfig does. That drop is there for the closure-adjustment
        // sigmas, which feed the least-squares solve; nothing in `[gas]` does, and
        // these writes are debounced-but-frequent while typing in a gas field.
        return emptyCommandResult(true, null);
      } catch (e) {
        return emptyCommandResult(false, String(e));
      }
    },

    setConfig: (config: Config) => {
      // Replacing the config (e.g. Settings ▸ Advanced "Reload from disk") can change
      // the closure-adjustment sigmas, so drop the cached adjusted geometry here too -
      // this is the shared config-replacement path that `updateConfig` doesn't use.
      set({ config, stationsAdjusted: null, surveyDataAdjusted: null });
    },
  };
}
