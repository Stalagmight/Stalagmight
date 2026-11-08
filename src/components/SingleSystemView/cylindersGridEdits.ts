/**
 * Pure edit reducer for the Plan-mode Cylinders grid (#403).
 *
 * Same contract as `segmentsGridEdits`: the grid emits `{ row, colKey, value }`
 * with values already coerced by each column's parser, and this folds a batch of
 * them into the next `Cylinder[]` for `gasPlanStore.setCylinders`. It is the
 * single CELL-EDIT write path for the table (checkbox, role Select, grid
 * paste/fill/clear all funnel through it). Row REMOVAL is the one exception:
 * `removeRows` in `CylindersTable.tsx` filters the array directly.
 *
 * ⚠ Unit space: pressures arrive in DISPLAY units (bar or psi), because the grid
 * seeds its editor from `getValue` and fill/internal-paste transfer that raw
 * result without re-parsing. The store is canonical bar, so this reducer is the
 * one place display -> canonical conversion (and clamping) happens.
 *
 * Kept free of React/store imports so it unit-tests in the node vitest env.
 */
import type { CellEdit } from '../common/SpreadsheetGrid/types';
import type { Cylinder, CylRole, GasUnits } from '../../types/ipc.generated';
import { isBudgetRole } from '../../utils/gasRoles';
import { pressureFromDisplay } from '../../utils/formatGas';
import { clamp } from '../../utils/clamp';
import { parseGridBoolean } from './segmentsGridEdits';

/** Column keys of the Cylinders grid. `tank`/`water`/`remove` are read-only, so
 *  no edit ever names them; they are listed so the column factory and the
 *  reducer key off the same constants. */
export const CYL_COL = {
  enabled: 'enabled',
  tank: 'tank',
  water: 'water',
  fill: 'fill',
  usable: 'usable',
  role: 'role',
  remove: 'remove',
} as const;

/** Selectable roles, in the order the role picker lists them. */
export const CYL_ROLES: readonly CylRole[] = ['back_gas', 'stage', 'dil_out', 'bailout', 'deco'];

/** Fold a role id or a localized role label into one comparable token:
 *  trimmed, lowercased, with spaces and hyphens collapsed to `_` so "Dil-out",
 *  "dil out" and "dil_out" all land on the id. */
function roleToken(raw: string): string {
  return raw.trim().toLowerCase().replace(/[\s-]+/g, '_');
}

/**
 * Parse a raw role cell into a `CylRole`, or `undefined` to SKIP the write.
 *
 * Accepts the role id (`dil_out`), the localized label as displayed ("Dil-out"),
 * or an unambiguous prefix of either. An EMPTY string returns the row's current
 * role, matching the boolean columns: clearing a role cell is a no-op rather
 * than a silent demotion of the tank out of the penetration budget. An ambiguous
 * prefix ("b" - back gas or bailout) is skipped, never guessed.
 */
export function parseCylRole(
  raw: string,
  currentRole: CylRole,
  roleLabels: Record<CylRole, string>
): CylRole | undefined {
  const token = roleToken(raw);
  if (token === '') return currentRole;
  for (const role of CYL_ROLES) {
    if (token === role || token === roleToken(roleLabels[role])) return role;
  }
  // Prefix match, judged by the set of roles hit rather than the set of strings:
  // a label that normalises to its own id ("Stage" -> `stage`) is one candidate,
  // not two.
  const hits = new Set<CylRole>();
  for (const role of CYL_ROLES) {
    if (role.startsWith(token) || roleToken(roleLabels[role]).startsWith(token)) hits.add(role);
  }
  if (hits.size !== 1) return undefined;
  for (const role of hits) return role;
  return undefined;
}

/** Whether the usable-gas cell of this row is the diver's to set: only the Free
 *  reserve strategy hands over that number, and only for a carried budget-role
 *  tank (every other row's usable gas is COMPUTED by the engine). */
export function isUsableEditable(cylinder: Cylinder, isFree: boolean): boolean {
  return isFree && isBudgetRole(cylinder.role) && cylinder.enabled;
}

/** Clamp a CANONICAL usable pressure (bar) to what the cylinder can actually
 *  credit: its fill, capped to the rated pressure while conservative gas is on
 *  (the same ceiling the engine budgets against). Zero is allowed - "carry it,
 *  but plan to breathe none of it" is a real answer. */
export function clampUsableBar(
  valueBar: number,
  cylinder: Cylinder,
  conservative: boolean
): number {
  const basis = conservative
    ? Math.min(cylinder.fill_bar, cylinder.rated_pressure_bar)
    : cylinder.fill_bar;
  return clamp(valueBar, 0, basis);
}

/** Context the reducer needs from the live store. */
export interface CylinderEditContext {
  units: GasUnits;
  /** `gasConfig.default_reserve === 'free'` - gates the usable-gas column. */
  isFree: boolean;
  /** `gasConfig.conservative_tank_gas` - picks the usable-gas ceiling. */
  conservative: boolean;
  /** Localized role labels, for parsing a typed/pasted role. */
  roleLabels: Record<CylRole, string>;
}

function isCylRole(value: unknown): value is CylRole {
  return typeof value === 'string' && (CYL_ROLES as readonly string[]).includes(value);
}

/** Coerce whatever a boolean-column edit carries into a boolean. `activate` and
 *  the column's `parse` already deliver a boolean; a string can still arrive from
 *  a direct caller, so both are handled and everything else is skipped. */
function booleanValue(value: unknown, current: boolean): boolean | undefined {
  if (typeof value === 'boolean') return value;
  if (typeof value === 'string') return parseGridBoolean(value, current);
  return undefined;
}

function sameCylinder(a: Cylinder, b: Cylinder): boolean {
  return (
    a.enabled === b.enabled &&
    a.fill_bar === b.fill_bar &&
    a.usable_gas_bar === b.usable_gas_bar &&
    a.role === b.role
  );
}

/** Apply one edit to one cylinder. Returns the row unchanged (identity) when the
 *  edit is a skip or a no-op. */
function applyOne(cyl: Cylinder, edit: CellEdit, ctx: CylinderEditContext): Cylinder {
  switch (edit.colKey) {
    case CYL_COL.enabled: {
      const next = booleanValue(edit.value, cyl.enabled);
      return next === undefined ? cyl : { ...cyl, enabled: next };
    }
    case CYL_COL.fill: {
      if (typeof edit.value !== 'number') return cyl;
      const bar = pressureFromDisplay(edit.value, ctx.units);
      // `Cylinder::new` requires a positive fill, so a zero/negative/non-finite
      // pressure is skipped rather than written - the row keeps its last value
      // and the grid shows the cell as missing.
      if (!Number.isFinite(bar) || bar <= 0) return cyl;
      return { ...cyl, fill_bar: bar };
    }
    case CYL_COL.usable: {
      // The column is read-only for every row the engine computes, so a paste
      // sweeping across the table cannot overwrite a computed usable gas.
      if (!isUsableEditable(cyl, ctx.isFree)) return cyl;
      if (typeof edit.value !== 'number' || !Number.isFinite(edit.value)) return cyl;
      const bar = clampUsableBar(pressureFromDisplay(edit.value, ctx.units), cyl, ctx.conservative);
      return { ...cyl, usable_gas_bar: bar };
    }
    case CYL_COL.role: {
      if (isCylRole(edit.value)) return { ...cyl, role: edit.value };
      if (typeof edit.value !== 'string') return cyl;
      const next = parseCylRole(edit.value, cyl.role, ctx.roleLabels);
      return next === undefined ? cyl : { ...cyl, role: next };
    }
    default:
      return cyl;
  }
}

/**
 * Fold a batch of grid edits into the next cylinder list.
 *
 * Edits are applied IN ORDER against a single working copy (so a batch that
 * raises a fill and then sets usable gas clamps against the NEW fill), and the
 * IDENTICAL input array is returned when nothing changed - `setCylinders` nulls
 * the cached reach field, so a gratuitous write costs a full map recompute.
 */
export function applyCylinderEdits(
  cylinders: Cylinder[],
  edits: CellEdit[],
  ctx: CylinderEditContext
): Cylinder[] {
  let next: Cylinder[] | null = null;
  for (const edit of edits) {
    const current = next ?? cylinders;
    const cyl = current[edit.row];
    if (cyl === undefined) continue;
    const updated = applyOne(cyl, edit, ctx);
    if (sameCylinder(updated, cyl)) continue;
    if (next === null) next = cylinders.slice();
    next[edit.row] = updated;
  }
  return next ?? cylinders;
}
