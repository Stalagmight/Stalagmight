/**
 * GridColumn factory for the Plan-mode Cylinders grid (#403), the sibling of
 * `segmentsGridColumns` and of `src/components/common/editableShotColumns.tsx`.
 *
 * ⚠ UNIT SPACE. The grid's value space is DISPLAY units (bar or psi); the store
 * is canonical bar. `getValue` converts canonical -> display, and
 * `applyCylinderEdits` converts back exactly once. The cell editor is seeded
 * from `getValue`, and fill/internal-paste transfer that raw result WITHOUT
 * running it through `parse`, so `getValue` must return the number the diver
 * would have typed - returning canonical here would re-price every filled cell
 * in the other unit system.
 */
import type { ReactNode } from 'react';
import type { TFunction } from 'i18next';
import type { GridColumn } from '../common/SpreadsheetGrid/types';
import { parseLocaleNumber } from '../common/SpreadsheetGrid';
import { Select } from '../common/Select';
import type { Cylinder, CylRole, GasUnits } from '../../types/ipc.generated';
import { formatFixed } from '../../i18n/format';
import { pressureToDisplay } from '../../utils/formatGas';
import { CYL_COL, CYL_ROLES, isUsableEditable, parseCylRole } from './cylindersGridEdits';
import { parseGridBoolean } from './segmentsGridEdits';

export interface CylinderColumnContext {
  t: TFunction;
  units: GasUnits;
  /** The Free reserve strategy (#245) hands the diver the usable-gas number;
   *  every other strategy computes it, so the column goes read-only. */
  isFree: boolean;
  /** Locale the host passes to the grid - the custom `parse` below replaces the
   *  grid's default parser entirely, so it has to do its own locale-aware
   *  number parsing with the same locale. */
  locale: string;
  /** Why this row is dimmed (switched off / not a budget role), or undefined
   *  when it counts toward the penetration budget. */
  dimReason: (cylinder: Cylinder) => string | undefined;
  /** The engine's usable bar for input row `i` (`reachField.cylinder_usable_bars`),
   *  or 0 when there is no field yet - the single source of truth for the
   *  read-only column, so it can never drift from the budget/gauges. */
  usableBarAt: (rowIndex: number) => number;
  /** Localized role labels, for display AND for parsing a typed/pasted role. */
  roleLabels: Record<CylRole, string>;
  onToggleEnabled: (rowIndex: number, next: boolean) => void;
  onSetRole: (rowIndex: number, role: CylRole) => void;
  onRemove: (rowIndex: number) => void;
  /** Register the wrapper of row `i`'s role Select so `activate` can open it
   *  from the keyboard (the Select has no imperative open). The host stores
   *  `el?.querySelector('button')`. */
  registerRoleTrigger: (rowIndex: number, el: HTMLElement | null) => void;
  roleTriggerAt: (rowIndex: number) => HTMLElement | null;
}

/** Screen-reader name for a row's controls: the label the Tank column shows,
 *  falling back to the water volume so an unnamed tank is not announced blank. */
function tankName(cylinder: Cylinder): string {
  return cylinder.label ?? `${cylinder.water_volume_l}L`;
}

export function buildCylinderColumns(ctx: CylinderColumnContext): GridColumn<Cylinder>[] {
  const {
    t,
    units,
    isFree,
    locale,
    dimReason,
    usableBarAt,
    roleLabels,
    onToggleEnabled,
    onSetRole,
    onRemove,
    registerRoleTrigger,
    roleTriggerAt,
  } = ctx;
  const roleOptions = CYL_ROLES.map((role) => ({ value: role, label: roleLabels[role] }));
  const negativePressure = (value: unknown): string | null =>
    typeof value === 'number' && value < 0 ? t('gasPanel.tanks.negativePressure') : null;
  /** Gauge-style pressure: whole numbers, with a dash where there is no value. */
  const formatPressureCell = (value: unknown): string =>
    typeof value === 'number' ? formatFixed(value, undefined, 0) : '-';
  const roleLabelOf = (value: unknown): string => {
    const role = CYL_ROLES.find((candidate) => candidate === value);
    return role ? roleLabels[role] : String(value ?? '');
  };
  /** The usable-gas number this row shows, in DISPLAY units: the diver's own
   *  value on an editable row, the engine's otherwise, `null` for a dash. */
  const usableDisplay = (cylinder: Cylinder, index: number): number | null => {
    if (isUsableEditable(cylinder, isFree)) return pressureToDisplay(cylinder.usable_gas_bar, units);
    const engineBar = usableBarAt(index);
    return engineBar > 0 ? pressureToDisplay(engineBar, units) : null;
  };

  return [
    {
      key: CYL_COL.enabled,
      header: t('gasPanel.tanks.colEnabled'),
      width: 48,
      align: 'center',
      kind: 'text',
      editable: true,
      // The render IS the control, so no cell editor opens over it; paste/fill
      // still write through `parse`.
      noInlineEdit: true,
      getValue: (cyl) => cyl.enabled,
      format: (value) => (value ? 'true' : 'false'),
      activate: (cyl, index) => onToggleEnabled(index, !cyl.enabled),
      parse: (raw, cyl) => parseGridBoolean(raw, cyl.enabled),
      render: (cyl, index): ReactNode => (
        <input
          type="checkbox"
          checked={cyl.enabled}
          onChange={(e) => onToggleEnabled(index, e.target.checked)}
          onClick={(e) => e.stopPropagation()}
          tabIndex={-1}
          aria-label={t('gasPanel.tanks.enabledAria', { tank: tankName(cyl) })}
          data-testid={`gas-cylinder-enabled-${index}`}
        />
      ),
    },
    {
      key: CYL_COL.tank,
      header: t('gasPanel.tanks.colTank'),
      width: 120,
      grow: true,
      align: 'left',
      getValue: (cyl) => tankName(cyl),
      // The dim reason used to sit on the <tr>; a grid row cannot carry it, so
      // it becomes this cell's tooltip.
      render: (cyl): ReactNode => (
        <span className="truncate font-medium" title={dimReason(cyl)}>
          {tankName(cyl)}
        </span>
      ),
    },
    {
      key: CYL_COL.water,
      header: t('gasPanel.tanks.colWater'),
      width: 84,
      align: 'right',
      // Water CAPACITY is a tank spec, not a gas volume: it stays in litres in
      // both unit systems (only the pressures flip bar/psi).
      getValue: (cyl) => cyl.water_volume_l,
      format: (value) => `${String(value)} L`,
    },
    {
      key: CYL_COL.fill,
      header: t(units === 'imperial' ? 'gasPanel.tanks.colFillPsi' : 'gasPanel.tanks.colFillBar'),
      width: 90,
      align: 'right',
      kind: 'number',
      getValue: (cyl) => pressureToDisplay(cyl.fill_bar, units),
      format: (value) => (typeof value === 'number' ? formatFixed(value, undefined, 0) : ''),
      editable: true,
      // An emptied fill cell writes nothing (the grid marks it missing and the
      // store keeps its last value) - `Cylinder::new` has no notion of a tank
      // with no pressure in it.
      validate: negativePressure,
    },
    {
      key: CYL_COL.usable,
      header: t(
        units === 'imperial' ? 'gasPanel.tanks.colUsablePsi' : 'gasPanel.tanks.colUsableBar'
      ),
      width: 95,
      align: 'right',
      kind: 'number',
      // Dual mode: the diver's number under Free on a carried budget tank, the
      // engine's `cylinder_usable_bars` entry on every other row.
      isEditable: (cyl) => isUsableEditable(cyl, isFree),
      getValue: usableDisplay,
      format: formatPressureCell,
      // The header hint has nowhere to live on a grid header (plain strings), so
      // it moves onto the cell.
      render: (cyl, index): ReactNode => (
        <span
          className={isUsableEditable(cyl, isFree) ? undefined : 'opacity-70'}
          title={t('gasPanel.tanks.usableHint')}
        >
          {formatPressureCell(usableDisplay(cyl, index))}
        </span>
      ),
      // A custom `parse` REPLACES the grid's default parser (locale handling
      // included), so it does both jobs: an emptied cell means "plan to breathe
      // none of this tank" (0, which `kind: 'number'` would otherwise refuse to
      // write), and anything else goes through the same locale-aware parser the
      // default would have used.
      parse: (raw) => (raw.trim() === '' ? 0 : parseLocaleNumber(raw, locale)),
      validate: negativePressure,
    },
    {
      key: CYL_COL.role,
      header: t('gasPanel.tanks.colRole'),
      width: 130,
      align: 'left',
      kind: 'text',
      editable: true,
      // The embedded Select is the control; its listbox portals out of the grid.
      noInlineEdit: true,
      getValue: (cyl) => cyl.role,
      format: roleLabelOf,
      // The Select has no imperative open, so the keyboard path clicks its
      // trigger - the same thing a mouse does.
      activate: (_cyl, index) => {
        const trigger = roleTriggerAt(index);
        trigger?.focus();
        trigger?.click();
      },
      parse: (raw, cyl) => parseCylRole(raw, cyl.role, roleLabels),
      // The cell IS the control: `variant="cell"` strips the Select's box, and the
      // wrapper covers the WHOLE cell so every pixel of it opens the list - including
      // the grid's own `px-2` padding, which the Select re-applies inside the trigger
      // so the role label lands at the same x-offset as the Tank label beside it and
      // the chevron sits at the cell's right edge.
      //
      // `absolute inset-0` is the only way to reach that padding: the grid wraps a
      // cell's content in an `overflow-hidden` div inset by the padding, which clips a
      // negative-margin bleed (measured: 8px dead strip on each edge). Going
      // out-of-flow escapes that clip and is bounded by the cell's own overflow
      // instead. It needs the cell to be the containing block, so `CylindersTable`
      // gives the role cell `position: relative` via `cellClassName` - the two go
      // together.
      render: (cyl, index): ReactNode => (
        <span className="absolute inset-0 flex" ref={(el) => registerRoleTrigger(index, el)}>
          <Select
            value={cyl.role}
            onChange={(v) => onSetRole(index, v as CylRole)}
            options={roleOptions}
            variant="cell"
            className="min-w-0 flex-1"
            aria-label={t('gasPanel.tanks.roleAria', { tank: tankName(cyl) })}
            data-testid={`gas-cylinder-role-${index}`}
          />
        </span>
      ),
    },
    {
      key: CYL_COL.remove,
      header: '',
      width: 36,
      align: 'center',
      getValue: () => '',
      render: (cyl, index): ReactNode => (
        <button
          type="button"
          className="px-1 text-xs opacity-60 hover:opacity-100"
          onClick={(e) => {
            e.stopPropagation();
            onRemove(index);
          }}
          tabIndex={-1}
          aria-label={t('gasPanel.tanks.removeAria', { tank: tankName(cyl) })}
          data-testid={`gas-cylinder-remove-${index}`}
        >
          ✕
        </button>
      ),
    },
  ];
}
