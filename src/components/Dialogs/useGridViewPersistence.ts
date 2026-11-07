/**
 * Persist a grid's column layout (order, visibility, widths) in the app config.
 * Init once when the config arrives; later changes save debounced.
 *
 * Parameterized over the three `panels` field names and the partial-update
 * command, because there are now three of these grids - the new-section dialog,
 * the edit-section dialog, and (through its own older hook) the data table -
 * and they each own a disjoint set of fields.
 *
 * A slimmed `useDataTableViewPersistence`, keeping the four behaviours that file
 * paid for in bugs (#357):
 *   - a SORTED snapshot (`stableWidths`, `.sort()`ed hidden list), so merely
 *     opening the dialog does not schedule a save of state nobody touched;
 *   - an optimistic publish into the in-memory config BEFORE the IPC, so a
 *     remount that races the flush seeds from the fresh values;
 *   - rollback + a toast when the IPC rejects, so a later change retries instead
 *     of the layout silently reverting on next launch;
 *   - a mount-only cleanup that flushes a still-pending save on unmount.
 *
 * That last one matters more in a dialog than in the data table: a dialog
 * unmounts on every confirm and every cancel, which is almost always inside the
 * 500 ms debounce window of whatever the user last resized.
 */
import { useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import { invokeIpc } from '../../utils/ipc';
import { chainSettingWrite } from '../../utils/settingWriteChain';
import { stableWidths } from '../SingleSystemView/useDataTableViewPersistence';

/** The layout a host owns and hands in. */
export interface GridViewState {
  columnOrder: string[];
  hiddenColumns: ReadonlySet<string>;
  columnWidths: Record<string, number>;
}

/** Setters used to hydrate {@link GridViewState} on the one-shot seed. */
export interface GridViewSetters {
  setColumnOrder: (order: string[]) => void;
  setHiddenColumns: (cols: ReadonlySet<string>) => void;
  setColumnWidths: (widths: Record<string, number>) => void;
}

export interface GridViewPersistenceConfig {
  /** `panels` field names this grid owns. */
  orderField: string;
  hiddenField: string;
  widthsField: string;
  /** Partial-update command for those fields (all three take the same arguments). */
  command: 'update_new_section_view' | 'update_edit_section_view' | 'update_import_shots_view';
  /** Hidden set for a config that has never stored one. Only used when the
   *  field is ABSENT - see the seed effect. */
  fallbackHidden: readonly string[];
  /** Toast key, so one grid's failure does not replace another's toast. Callers
   *  must pass a value from `TOAST_KEYS` (src/utils/toastKeys.ts). */
  toastKey: string;
}

export function useGridViewPersistence(
  view: GridViewState,
  setters: GridViewSetters,
  cfg: GridViewPersistenceConfig
): void {
  const { t } = useTranslation();
  const showToast = useUIStore((s) => s.showToast);
  const panelsConfig = useProjectStore((s) => s.config?.panels) as
    | Record<string, unknown>
    | undefined;

  const { columnOrder, hiddenColumns, columnWidths } = view;
  const { setColumnOrder, setHiddenColumns, setColumnWidths } = setters;
  const { orderField, hiddenField, widthsField, command, fallbackHidden, toastKey } = cfg;

  const lastSavedRef = useRef<string | null>(null);
  useEffect(() => {
    if (lastSavedRef.current !== null || !panelsConfig) return;
    // panelsConfig comes straight from get_config with no defaultConfig merge:
    // the only guarantee is the Rust #[serde(default)], so a frontend running
    // against a backend that predates these fields would otherwise take the
    // host down on open.
    //
    // ABSENT and EMPTY are deliberately NOT the same for the hidden list. Once
    // the field is present the persisted list is authoritative, and `[]`
    // genuinely means the user unhid every column - re-defaulting it would make
    // that preference impossible to keep. But `undefined` means the backend
    // never wrote one, so the built-in defaults apply.
    const order = (panelsConfig[orderField] as string[] | undefined) ?? [];
    const hidden = (panelsConfig[hiddenField] as string[] | undefined) ?? [...fallbackHidden];
    const widths = (panelsConfig[widthsField] as Record<string, number> | undefined) ?? {};
    setColumnOrder(order);
    setHiddenColumns(new Set(hidden));
    setColumnWidths(widths);
    // Sorted here because the save snapshot below sorts, and the two strings are
    // compared verbatim - without this, merely opening would schedule a save.
    lastSavedRef.current = JSON.stringify([order, [...hidden].sort(), stableWidths(widths)]);
  }, [
    panelsConfig,
    setColumnOrder,
    setHiddenColumns,
    setColumnWidths,
    fallbackHidden,
    orderField,
    hiddenField,
    widthsField,
  ]);

  const pendingSaveRef = useRef<(() => void) | null>(null);
  useEffect(() => {
    if (lastSavedRef.current === null) return undefined;
    const hidden = [...hiddenColumns].sort();
    const snapshot = JSON.stringify([columnOrder, hidden, stableWidths(columnWidths)]);
    if (snapshot === lastSavedRef.current) {
      pendingSaveRef.current = null;
      return undefined;
    }
    const save = () => {
      pendingSaveRef.current = null;
      const previouslySaved = lastSavedRef.current;
      lastSavedRef.current = snapshot;
      // Deliberately `setState`, NOT the `setConfig` action: that one is the
      // whole-config REPLACEMENT path and also nulls the adjusted-geometry
      // caches, which nothing under `[panels]` invalidates.
      const config = useProjectStore.getState().config;
      if (config) {
        useProjectStore.setState({
          config: {
            ...config,
            panels: {
              ...config.panels,
              [orderField]: columnOrder,
              [hiddenField]: hidden,
              [widthsField]: columnWidths,
            },
          },
        });
      }
      // [#576 SETTINGS-ORDER] In save order: the unmount flush and the next mount's
      // first save can both be in flight, and the older layout must not land last.
      void chainSettingWrite(command, () =>
        invokeIpc(
          command,
          { columnOrder, hiddenColumns: hidden, columnWidths },
          { purpose: `persist ${toastKey} grid view state` }
          // In the config chain, behind the settings saves made before it ([#576 RV6b-3]).
        ).then(() => useProjectStore.getState().reloadConfigInOrder())
      )
        .catch((e) => {
          if (lastSavedRef.current === snapshot) {
            lastSavedRef.current = previouslySaved;
            // Undo the optimistic publish from DISK, so a remount cannot seed
            // from values that never landed and treat them as saved.
            void useProjectStore.getState().reloadConfigInOrder();
          }
          console.error(`Failed to save ${toastKey} view state:`, e);
          showToast(t('dataTable.viewSaveFailed'), toastKey, 'error');
        });
    };
    pendingSaveRef.current = save;
    const timer = setTimeout(save, 500);
    return () => clearTimeout(timer);
  }, [
    showToast,
    t,
    columnOrder,
    hiddenColumns,
    columnWidths,
    orderField,
    hiddenField,
    widthsField,
    command,
    toastKey,
  ]);

  // Flush a still-pending save on unmount. The cleanup above is clearTimeout
  // only and runs on every dep change too, so it cannot tell unmount apart; a
  // mount-only effect can.
  useEffect(
    () => () => {
      pendingSaveRef.current?.();
    },
    []
  );
}
