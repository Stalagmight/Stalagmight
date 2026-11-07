import { useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import { invokeIpc } from '../../utils/ipc';
import { chainSettingWrite } from '../../utils/settingWriteChain';
import { TOAST_KEYS } from '../../utils/toastKeys';

type AngleMode = 'avg' | 'inout' | 'off';

/**
 * Column widths as sorted entries, for the change-detection snapshot only.
 *
 * `JSON.stringify` of an object preserves insertion order, and a resize appends a
 * newly-overridden column at the end - while the same map comes back from the
 * backend as an alphabetically-sorted `BTreeMap`. Comparing those two directly
 * reports a change that isn't one, costing a redundant save-and-resync cycle. The
 * sibling hidden-column lists are `.sort()`ed just above for the same reason.
 *
 * Exported for `useNewSectionViewPersistence`, which persists the new-section
 * dialog's grid layout through the same seed/snapshot/debounce machinery and so
 * needs the identical normalization - two copies would be free to drift into
 * exactly the spurious-save bug this exists to prevent.
 */
export function stableWidths(widths: Record<string, number>): Array<[string, number]> {
  return Object.entries(widths).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
}

/** The data-table view state DataTable owns and hands in - order, visibility,
 *  angle modes and widths for both the shots and stations views. */
export interface DataTableViewState {
  shotColumnOrder: string[];
  hiddenShotCols: ReadonlySet<string>;
  azimuthMode: AngleMode;
  inclinationMode: AngleMode;
  stationColumnOrder: string[];
  hiddenStationCols: ReadonlySet<string>;
  shotColumnWidths: Record<string, number>;
  stationColumnWidths: Record<string, number>;
}

/** Setters for {@link DataTableViewState}, used to hydrate it from the
 *  persisted config on the one-shot seed. */
export interface DataTableViewSetters {
  setShotColumnOrder: (order: string[]) => void;
  setHiddenShotCols: (cols: ReadonlySet<string>) => void;
  setAzimuthMode: (mode: AngleMode) => void;
  setInclinationMode: (mode: AngleMode) => void;
  setStationColumnOrder: (order: string[]) => void;
  setHiddenStationCols: (cols: ReadonlySet<string>) => void;
  setShotColumnWidths: (widths: Record<string, number>) => void;
  setStationColumnWidths: (widths: Record<string, number>) => void;
}

/**
 * Persist the data-table view state (order, visibility, angle modes) in the app
 * config. Init once when the config arrives; later changes save debounced. The
 * snapshot guard keeps the init itself (and no-op renders) from re-saving.
 */
export function useDataTableViewPersistence(
  view: DataTableViewState,
  setters: DataTableViewSetters
): void {
  const { t } = useTranslation();
  const showToast = useUIStore((s) => s.showToast);
  const panelsConfig = useProjectStore((s) => s.config?.panels);

  const {
    shotColumnOrder,
    hiddenShotCols,
    azimuthMode,
    inclinationMode,
    stationColumnOrder,
    hiddenStationCols,
    shotColumnWidths,
    stationColumnWidths,
  } = view;
  const {
    setShotColumnOrder,
    setHiddenShotCols,
    setAzimuthMode,
    setInclinationMode,
    setStationColumnOrder,
    setHiddenStationCols,
    setShotColumnWidths,
    setStationColumnWidths,
  } = setters;

  const lastSavedViewRef = useRef<string | null>(null);
  useEffect(() => {
    if (lastSavedViewRef.current !== null || !panelsConfig) return;
    const isMode = (m: string): m is AngleMode => m === 'avg' || m === 'inout' || m === 'off';
    const azimuth = isMode(panelsConfig.data_table_azimuth_mode)
      ? panelsConfig.data_table_azimuth_mode
      : 'avg';
    const inclination = isMode(panelsConfig.data_table_inclination_mode)
      ? panelsConfig.data_table_inclination_mode
      : 'off';
    setAzimuthMode(azimuth);
    setInclinationMode(inclination);
    // `?? []` / `?? {}` ONCE, up here, because panelsConfig comes straight from
    // get_config with no defaultConfig merge: the only guarantee is the Rust
    // #[serde(default)]. A frontend running against a backend that predates these
    // fields would otherwise take the whole data table down on open (spread and
    // Object.entries both throw on undefined) instead of starting with no
    // overrides. Guarded here rather than at each use because the guard was
    // originally applied to the snapshot below but not to these setters, which
    // seeded the state `undefined` and left the crash exactly where it was.
    const shotHidden = panelsConfig.data_table_shot_hidden_columns ?? [];
    const shotOrder = panelsConfig.data_table_shot_column_order ?? [];
    // The persisted list is authoritative: the backend fills a missing/legacy
    // config with the coordinate-column defaults (serde default), so an EMPTY
    // array genuinely means the user unhid every station column ("show all") -
    // it must not be re-defaulted, or that preference could never persist.
    const stationHidden = panelsConfig.data_table_station_hidden_columns ?? [];
    const stationOrder = panelsConfig.data_table_station_column_order ?? [];
    const shotWidths = panelsConfig.data_table_shot_column_widths ?? {};
    const stationWidths = panelsConfig.data_table_station_column_widths ?? {};
    setHiddenShotCols(new Set(shotHidden));
    setShotColumnOrder(shotOrder);
    setHiddenStationCols(new Set(stationHidden));
    setStationColumnOrder(stationOrder);
    setShotColumnWidths(shotWidths);
    setStationColumnWidths(stationWidths);
    // Both hidden lists are sorted here because the save snapshot below sorts them,
    // and this string is compared against that one verbatim. Storing the config's own
    // order made the two differ whenever the persisted list was not already sorted -
    // the station default `['_x','_y','_lat','_long']` is not - so merely OPENING the
    // data table scheduled a save of state the user never touched. Pre-existing (the
    // widths entries below always sorted via stableWidths); it stayed invisible until
    // a test asserted "no save yet" and lost the race to the 500ms debounce.
    // Built from the same guarded locals the setters above seeded the state with,
    // for that same reason: reading the raw config here would record a legacy
    // `undefined` order as `null` while the state holds `[]` - the same spurious
    // save in a different disguise.
    lastSavedViewRef.current = JSON.stringify([
      shotOrder,
      [...shotHidden].sort(),
      azimuth,
      inclination,
      stationOrder,
      [...stationHidden].sort(),
      stableWidths(shotWidths),
      stableWidths(stationWidths),
    ]);
  }, [
    panelsConfig,
    setAzimuthMode,
    setInclinationMode,
    setHiddenShotCols,
    setShotColumnOrder,
    setHiddenStationCols,
    setStationColumnOrder,
    setShotColumnWidths,
    setStationColumnWidths,
  ]);
  // The debounced save that has been scheduled but has not fired yet, so unmount
  // can flush it (#357 item 4). Cleared the moment it runs, and cleared on the
  // no-change path below - otherwise changing a width and then undoing it inside
  // the debounce window would leave a closure here that writes the OLD state over
  // the value the user actually settled on.
  const pendingViewSaveRef = useRef<(() => void) | null>(null);
  useEffect(() => {
    if (lastSavedViewRef.current === null) return undefined;
    const hidden = [...hiddenShotCols].sort();
    const stationHidden = [...hiddenStationCols].sort();
    const snapshot = JSON.stringify([
      shotColumnOrder,
      hidden,
      azimuthMode,
      inclinationMode,
      stationColumnOrder,
      stationHidden,
      stableWidths(shotColumnWidths),
      stableWidths(stationColumnWidths),
    ]);
    if (snapshot === lastSavedViewRef.current) {
      pendingViewSaveRef.current = null;
      return undefined;
    }
    const save = () => {
      pendingViewSaveRef.current = null;
      // Mark it saved up front so a re-render mid-flight does not queue a duplicate
      // write, but remember what to fall back to: if the IPC rejects, the state never
      // reached disk, and leaving the snapshot marked would mean no later change ever
      // retries it - the column silently returns to its old width on next launch.
      const previouslySaved = lastSavedViewRef.current;
      lastSavedViewRef.current = snapshot;
      // Publish the same values into the IN-MEMORY config before the IPC, not
      // only in the loadConfig re-sync below (#357 item 4, the reopen half).
      // The unmount flush races the user reopening the data table: a fresh
      // mount SEEDS from `config.panels`, and until the re-sync lands that
      // still holds the pre-save values. The new mount would pin those stale
      // values as "last saved", then ignore the fresher config when it arrives
      // moments later (the seed is one-shot and never re-runs), and the next
      // edit would write the stale widths straight back over what was just
      // flushed. Publishing first makes the remount seed correct no matter
      // which side of the IPC it lands on.
      //
      // Deliberately `setState`, NOT the `setConfig` action: that one is the
      // whole-config REPLACEMENT path (Settings > Advanced "Reload from disk")
      // and so also nulls `stationsAdjusted` / `surveyDataAdjusted`, correctly,
      // because a replaced config can carry different closure sigmas. Nothing
      // under `[panels]` feeds the least-squares solve, so reached from HERE
      // that invalidation is pure collateral damage on every resize / hide /
      // angle-mode edit: MapCanvas clears an active ruler measurement whenever
      // the adjusted geometry changes in `closureAdjusted` mode, and re-fetches
      // and re-solves it while the selectors fall back to measured geometry - a
      // visible jump, 500ms after a column resize.
      const config = useProjectStore.getState().config;
      if (config) {
        useProjectStore.setState({
          config: {
            ...config,
            panels: {
              ...config.panels,
              data_table_shot_column_order: shotColumnOrder,
              data_table_shot_hidden_columns: hidden,
              data_table_azimuth_mode: azimuthMode,
              data_table_inclination_mode: inclinationMode,
              data_table_station_column_order: stationColumnOrder,
              data_table_station_hidden_columns: stationHidden,
              data_table_shot_column_widths: shotColumnWidths,
              data_table_station_column_widths: stationColumnWidths,
            },
          },
        });
      }
      // [#576 SETTINGS-ORDER] In save order: the unmount flush and a remount's first
      // save can both be in flight, and the older view must not land last.
      void chainSettingWrite('update_data_table_view', () =>
        invokeIpc(
          'update_data_table_view',
          {
            columnOrder: shotColumnOrder,
            hiddenColumns: hidden,
            azimuthMode,
            inclinationMode,
            stationColumnOrder,
            stationHiddenColumns: stationHidden,
            columnWidths: shotColumnWidths,
            stationColumnWidths,
          },
          { purpose: 'persist data-table view state' }
        )
          // Re-sync the in-memory config: a remount re-seeds from it, so a stale
          // copy would silently revert what was just saved to disk. In the config
          // chain, behind the settings saves made before it ([#576 RV6b-3]).
          .then(() => useProjectStore.getState().reloadConfigInOrder())
      )
        .catch((e) => {
          if (lastSavedViewRef.current === snapshot) {
            lastSavedViewRef.current = previouslySaved;
            // Undo the optimistic publish from DISK - the same re-sync the
            // success path does. Without it a remount would seed from values
            // that never reached disk and treat them as saved, which is exactly
            // the retry the rollback above exists to preserve (#357 item 5).
            void useProjectStore.getState().reloadConfigInOrder();
          }
          console.error('Failed to save data-table view state:', e);
          // #357 item 5: the rollback above makes a later change RETRY, but on its
          // own it is silent - a user who resizes a column and quits saw nothing
          // and lost the layout with no indication it had not been kept. Keyed so
          // a burst of failures refreshes one toast instead of stacking.
          showToast(t('dataTable.viewSaveFailed'), TOAST_KEYS.DATA_TABLE_VIEW_SAVE, 'error');
        });
    };
    pendingViewSaveRef.current = save;
    const timer = setTimeout(save, 500);
    return () => clearTimeout(timer);
  }, [
    showToast,
    t,
    shotColumnOrder,
    hiddenShotCols,
    azimuthMode,
    inclinationMode,
    stationColumnOrder,
    hiddenStationCols,
    shotColumnWidths,
    stationColumnWidths,
  ]);

  // Flush a still-pending view save on unmount (#357 item 4). The cleanup above
  // is `clearTimeout` only, and it runs on EVERY dep change as well as on
  // unmount, so it cannot tell the two apart - which meant closing the data
  // table, or the project, inside the 500 ms window dropped the write silently.
  // A mount-only effect can: its cleanup runs exactly once, at unmount, by which
  // point `pendingViewSaveRef` holds the newest scheduled save or nothing.
  useEffect(
    () => () => {
      pendingViewSaveRef.current?.();
    },
    []
  );
}
