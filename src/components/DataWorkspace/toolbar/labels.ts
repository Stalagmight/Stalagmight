/**
 * Text helpers shared by the toolbar pieces: column / group names, filter
 * operators, condition and lock-reason wording.
 *
 * Column and group names live under `dataWorkspace.columns.*` /
 * `dataWorkspace.columnGroups.*` (the workspace shell's namespace). Until that
 * lands on this branch they fall back to `dataToolbar.fallback.*`; the app's
 * missing-key handler overrides i18next's `defaultValue`, hence the explicit
 * `exists` check. Once both namespaces are merged the fallback block can go.
 */
import type { TFunction, i18n as I18n } from 'i18next';
import type { DataColumnGroup, DataColumnKey, DataLockReason, DataValueType, FilterCondition, FilterOp } from '../types';
import { opNeedsValue } from '../model/filter';
import { formatNumber } from '../../../i18n/format';

export function columnLabel(t: TFunction, i18n: I18n, meta: { key: DataColumnKey; labelKey: string }): string {
  return i18n.exists(meta.labelKey) ? t(meta.labelKey) : t(`dataToolbar.fallback.columns.${meta.key}`);
}

export function groupLabel(t: TFunction, i18n: I18n, group: DataColumnGroup): string {
  const key = `dataWorkspace.columnGroups.${group}`;
  return i18n.exists(key) ? t(key) : t(`dataToolbar.fallback.columnGroups.${group}`);
}

const DATE_OPS: ReadonlySet<FilterOp> = new Set(['eq', 'neq', 'lt', 'lte', 'gt', 'gte']);

export function opLabel(t: TFunction, op: FilterOp, valueType: DataValueType): string {
  if (valueType === 'date' && DATE_OPS.has(op)) return t(`dataToolbar.filter.dateOps.${op}`);
  if ((valueType === 'text' || valueType === 'enum') && (op === 'eq' || op === 'neq')) {
    return t(`dataToolbar.filter.textOps.${op}`);
  }
  return t(`dataToolbar.filter.ops.${op}`);
}

/** "Length > 30", "Comment contains “sump”", "Depth is empty". */
export function conditionText(
  t: TFunction,
  i18n: I18n,
  cond: FilterCondition,
  meta: { key: DataColumnKey; labelKey: string; valueType: DataValueType },
): string {
  const column = columnLabel(t, i18n, meta);
  const op = opLabel(t, cond.op, meta.valueType);
  if (!opNeedsValue(cond.op)) return t('dataToolbar.chips.conditionNoValue', { column, op });
  const raw = cond.value.trim();
  const value = meta.valueType === 'text' || meta.valueType === 'enum' ? `“${raw}”` : raw;
  return t('dataToolbar.chips.condition', { column, op, value });
}

export function lockReasonText(t: TFunction, reason: DataLockReason): string {
  return t(`dataToolbar.lockReason.${reason}`);
}

/** A plural string: `count` picks the form, `{{n}}` shows it locale-formatted. */
export function tCount(t: TFunction, key: string, count: number, extra: Record<string, unknown> = {}): string {
  return t(key, { count, n: formatNumber(count), ...extra });
}
