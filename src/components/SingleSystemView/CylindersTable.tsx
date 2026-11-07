import { useCallback, useMemo, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { useShallow } from 'zustand/react/shallow';
import { useGasPlanStore } from '../../store/gasPlanStore';
import { SpreadsheetGrid, type CellEdit } from '../common/SpreadsheetGrid';
import { Select } from '../common/Select';
import { useGridLabels } from '../../hooks/useGridLabels';
import { getNumberFormatLocale } from '../../i18n/format';
import { isBudgetRole } from '../../utils/gasRoles';
import { TANK_PRESETS, cylinderFromPreset } from '../../utils/tankPresets';
import type { Cylinder, CylRole } from '../../types/ipc.generated';
import { Tooltip } from '../common/Tooltip';
import { GAS_CYLINDER_GRID_PREFIX } from '../../utils/testId';
import { buildCylinderColumns } from './cylindersGridColumns';
import { applyCylinderEdits, CYL_COL, CYL_ROLES, isUsableEditable } from './cylindersGridEdits';
import { planGridHeight, usePlanGridFillHeight } from './planGridHeight';
import { useBulkEditReportToast } from '../common/useBulkEditReportToast';
import { TOAST_KEYS } from '../../utils/toastKeys';

/** Rows behind the FALLBACK height (before the pane is measured, and under jsdom).
 *  The grid fills its tab now, so this only caps the pre-measurement guess. */
const MAX_VISIBLE_ROWS = 10;

/**
 * The diver's cylinder set - the Plan-mode bottom panel's "Cylinders" tab (#38), which
 * the grid fills edge to edge; the DPV rig editor is its own tab next door (#317).
 * Add tanks from presets, edit fill pressure + role, remove. Budget-role rows (back
 * gas / stage) are emphasised; the rest are dimmed (they don't count toward the swim-in
 * budget - PRD §5.3). Edits flow to gasPlanStore, which nulls the cached reach field so
 * the map overlay refetches. Tank water *capacity* stays in litres regardless of units
 * (it's a tank spec, not a gas volume); only the fill pressure flips bar↔psi.
 *
 * The two controls that are not per-row - "Add cylinder" and the conservative-gas
 * switch - live in {@link CylinderTableActions}, which the panel renders at the far end
 * of the tab strip so this tab is nothing but grid.
 *
 * Hosted on the shared `SpreadsheetGrid` (#403): the columns live in
 * `cylindersGridColumns.tsx`, and every value write - typed, pasted, filled, cleared, or
 * made on the row's own checkbox/role picker - funnels through `applyCylinderEdits` and
 * one `setCylinders` call. Row REMOVAL is the exception, since it is not a cell value: the
 * per-row button and the grid's "Delete tank" context entry share `removeRows` below.
 */
export function CylindersTable() {
  const { t } = useTranslation();
  const { cylinders, gasConfig, reachField } = useGasPlanStore(
    useShallow((s) => ({
      cylinders: s.cylinders,
      gasConfig: s.gasConfig,
      reachField: s.reachField,
    }))
  );
  const units = gasConfig.units;
  // Free (#245) lets the diver set each cylinder's usable gas by hand; every other strategy
  // COMPUTES it, so the column is read-only and shows the engine's per-row value.
  const isFree = gasConfig.default_reserve === 'free';
  // Engine-computed usable bar per input row (aligned 1:1 with `cylinders`); the single source of
  // truth for the read-only column, so it can never drift from the budget/gauges.
  const usableBars = reachField?.cylinder_usable_bars;
  const locale = getNumberFormatLocale();

  // The role Select has no imperative open, so the keyboard `activate` path clicks its
  // trigger - the same thing a mouse does. Each row's render registers the button here.
  const roleTriggers = useRef(new Map<number, HTMLElement | null>());
  const registerRoleTrigger = useCallback((index: number, el: HTMLElement | null) => {
    if (el === null) roleTriggers.current.delete(index);
    else roleTriggers.current.set(index, el.querySelector('button'));
  }, []);
  const roleTriggerAt = useCallback(
    (index: number) => roleTriggers.current.get(index) ?? null,
    []
  );

  const roleLabels = useMemo(() => {
    const labels = {} as Record<CylRole, string>;
    for (const role of CYL_ROLES) labels[role] = t(`gasPanel.roles.${role}`);
    return labels;
  }, [t]);

  // The single write path for cell VALUES. Reads live state rather than the rendered
  // snapshot (the GasPlanPanel pattern), and skips the write when nothing changed -
  // `setCylinders` nulls the cached reach field, and a gratuitous null there costs a full
  // map recompute.
  const applyEdits = useCallback(
    (edits: CellEdit[]) => {
      const s = useGasPlanStore.getState();
      // Units/strategy come from the RENDER, not from live state: the numbers in the batch
      // were typed against the columns as they were displayed, so re-reading a units toggle
      // that landed in between would reinterpret psi as bar.
      const next = applyCylinderEdits(s.cylinders, edits, {
        units,
        isFree,
        conservative: gasConfig.conservative_tank_gas !== false,
        roleLabels,
      });
      if (next !== s.cylinders) s.setCylinders(next);
    },
    [units, isFree, gasConfig.conservative_tank_gas, roleLabels]
  );

  /** Drop the inclusive row range - the per-row ✕ and the grid's row context menu. */
  const removeRows = useCallback((fromRow: number, toRow: number): boolean => {
    const s = useGasPlanStore.getState();
    s.setCylinders(s.cylinders.filter((_c, i) => i < fromRow || i > toRow));
    return true;
  }, []);

  const columns = useMemo(
    () =>
      buildCylinderColumns({
        t,
        units,
        isFree,
        locale,
        // A row is dimmed when it doesn't count toward the swim-in budget: either the
        // tank is switched off for this dive (#245) or its role isn't a budget role.
        dimReason: (cyl) =>
          !cyl.enabled
            ? t('gasPanel.tanks.disabledHint')
            : isBudgetRole(cyl.role)
              ? undefined
              : t('gasPanel.tanks.notCounted'),
        usableBarAt: (index) => usableBars?.[index] ?? 0,
        roleLabels,
        onToggleEnabled: (index, next) =>
          applyEdits([{ row: index, colKey: CYL_COL.enabled, value: next }]),
        onSetRole: (index, role) => applyEdits([{ row: index, colKey: CYL_COL.role, value: role }]),
        onRemove: (index) => removeRows(index, index),
        registerRoleTrigger,
        roleTriggerAt,
      }),
    [
      t,
      units,
      isFree,
      locale,
      usableBars,
      roleLabels,
      applyEdits,
      removeRows,
      registerRoleTrigger,
      roleTriggerAt,
    ]
  );

  const labels = useGridLabels('gasPanel.tanks.tankNoun');
  const onPasteReport = useBulkEditReportToast(TOAST_KEYS.PLAN_CYLINDERS_PASTE);
  const onEdit = useCallback(
    (row: number, colKey: string, value: unknown) => applyEdits([{ row, colKey, value }]),
    [applyEdits]
  );

  // The grid owns the whole tab body, so its height is the MEASURED pane rather than the
  // row count; the content-sized number is only the pre-measurement/jsdom fallback.
  const { ref: gridBoxRef, height } = usePlanGridFillHeight(
    planGridHeight(cylinders.length, MAX_VISIBLE_ROWS)
  );

  return (
    // Flush with the tab body on every edge, the way the Survey data table hosts its
    // grid (`DataTable`'s `flex-1 overflow-hidden` box): a padding frame here just
    // shrank the table inside the panel the diver sized on purpose. `overflow-hidden`
    // for the same reason DataTable's box has it - the grid scrolls internally, and a
    // second scroller around it would double the scrollbars.
    <div
      ref={gridBoxRef}
      className="h-full overflow-hidden text-sm text-text-primary"
      data-testid="cylinders-table"
    >
      <SpreadsheetGrid<Cylinder>
        columns={columns}
        rows={cylinders}
        // Cylinders have no id of their own, and the engine's per-row outputs
        // (`cylinder_usable_bars`) are aligned to this same position.
        rowKey={(_cyl, index) => index}
        testIdPrefix={GAS_CYLINDER_GRID_PREFIX}
        // The fill and usable cells keep their pre-grid test ids. The usable one names which
        // MODE the row is in, so a computed row can never be mistaken for an editable one.
        cellTestId={(cyl, index, colKey) =>
          colKey === CYL_COL.fill
            ? `gas-cylinder-fill-${index}`
            : colKey === CYL_COL.usable
              ? isUsableEditable(cyl, isFree)
                ? `gas-cylinder-usable-${index}`
                : `gas-cylinder-usable-readonly-${index}`
              : undefined
        }
        // The role cell's Select covers the whole cell (see `cylindersGridColumns`),
        // which it can only do from `position: absolute` - so the cell has to be its
        // containing block. Safe on stacking order: the cell keeps `z-index: auto`,
        // while the sticky header (z-30), the row gutter (z-20), every selection
        // overlay (z>=3) and the cell editor (z-20) carry an explicit z-index above it.
        cellClassName={(_cyl, _index, colKey) =>
          colKey === CYL_COL.role ? 'relative' : undefined
        }
        locale={locale}
        height={height}
        onEdit={onEdit}
        onBatchEdit={applyEdits}
        onPasteReport={onPasteReport}
        onDeleteRows={removeRows}
        // Replaces the row `title`/`data-enabled` the <tr> carried: a row that doesn't count
        // toward the penetration budget reads as dimmed, and the reason is the Tank cell's
        // tooltip.
        rowClassName={(cyl) =>
          !cyl.enabled || !isBudgetRole(cyl.role) ? 'opacity-50' : undefined
        }
        labels={labels}
      />
    </div>
  );
}

/**
 * The cylinder controls that belong to the TAB rather than to a row: the preset picker
 * that appends a tank, and the conservative-gas switch. `PlanTablesPanel` renders them at
 * the right-hand end of the tab strip while the Cylinders tab is up, so the grid itself
 * gets the whole pane - but the handlers stay here, next to the table they act on.
 *
 * The conservative switch's explanation ("cap overfills to rated") is a hover tooltip, not
 * a longer label: the strip is a single row shared with four tab buttons.
 */
export function CylinderTableActions() {
  const { t } = useTranslation();
  const { cylinders, gasConfig, setCylinders, setGasConfig } = useGasPlanStore(
    useShallow((s) => ({
      cylinders: s.cylinders,
      gasConfig: s.gasConfig,
      setCylinders: s.setCylinders,
      setGasConfig: s.setGasConfig,
    }))
  );

  const addPreset = (presetId: string) => {
    const preset = TANK_PRESETS.find((p) => p.id === presetId);
    if (preset) setCylinders([...cylinders, cylinderFromPreset(preset)]);
  };

  return (
    <div className="flex items-center gap-4">
      <Select
        value=""
        onChange={(v) => addPreset(v)}
        options={TANK_PRESETS.map((p) => ({
          value: p.id,
          label: `${p.label} (${p.water_volume_l}L)`,
        }))}
        placeholder={t('gasPanel.tanks.addPlaceholder')}
        size="sm"
        aria-label={t('gasPanel.tanks.addPlaceholder')}
        data-testid="gas-add-tank"
      />
      <Tooltip content={t('gasPanel.tanks.conservativeHint')} tooltipTestId="gas-conservative-tooltip">
        <label className="flex items-center gap-2 text-xs whitespace-nowrap">
          <input
            type="checkbox"
            checked={gasConfig.conservative_tank_gas}
            onChange={(e) => setGasConfig({ ...gasConfig, conservative_tank_gas: e.target.checked })}
            data-testid="gas-conservative-checkbox"
          />
          <span>{t('gasPanel.tanks.conservative')}</span>
        </label>
      </Tooltip>
    </div>
  );
}
