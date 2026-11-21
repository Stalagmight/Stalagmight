/**
 * "Add condition" row of the filter menu: column (grouped), operator (by the
 * column's value type), value (number / text input, a date input, or a pick
 * list for enum columns). Refuses an invalid condition with the reason.
 */
import { useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Select, type SelectOptionGroup } from '../../common/Select';
import { DATA_COLUMN_BY_KEY, DATA_COLUMN_GROUPS, DATA_COLUMNS } from '../columnsMeta';
import { conditionProblem, opNeedsValue, opsForValueType } from '../model/filter';
import type { DataModel } from '../DataModelContext';
import type { DataColumnKey, FilterCondition, FilterOp } from '../types';
import { columnLabel, groupLabel, opLabel } from './labels';
import { toolbarButtonClass } from './ToolbarMenu';

/** Columns a condition can filter on: everything sortable / searchable. */
const FILTERABLE = DATA_COLUMNS.filter((c) => c.searchable || c.sortable);

/** Distinct shown values of an enum column (capped), for the value pick list. */
export function distinctValues(model: DataModel, key: DataColumnKey, cap = 50): string[] {
  const col = model.columnByKey.get(key);
  if (!col) return [];
  const seen = new Set<string>();
  for (const row of model.rows) {
    if (row.kind === 'section') continue;
    if (col.appliesTo !== 'both' && col.appliesTo !== row.kind) continue;
    const text = col.text(row, model.ctx).trim();
    if (text !== '') seen.add(text);
    if (seen.size >= cap) break;
  }
  return [...seen].sort((a, b) => a.localeCompare(b));
}

let nextId = 0;
export const newConditionId = (): string => `c${Date.now().toString(36)}${(nextId++).toString(36)}`;

export function ConditionBuilder({ model, onAdd }: { model: DataModel; onAdd: (c: FilterCondition) => void }) {
  const { t, i18n } = useTranslation();
  const [colKey, setColKey] = useState<DataColumnKey>('length');
  const meta = DATA_COLUMN_BY_KEY.get(colKey)!;
  const ops = opsForValueType(meta.valueType);
  const [op, setOp] = useState<FilterOp>(ops[0]!);
  const [value, setValue] = useState('');
  const effectiveOp = ops.includes(op) ? op : ops[0]!;
  const problem = conditionProblem({ op: effectiveOp, value }, meta, model.ctx.locale);
  const enumValues = useMemo(
    () => (meta.valueType === 'enum' ? distinctValues(model, colKey) : []),
    [meta.valueType, model, colKey],
  );

  const columnOptions = useMemo<SelectOptionGroup[]>(
    () =>
      DATA_COLUMN_GROUPS.map((group) => ({
        label: groupLabel(t, i18n, group),
        options: FILTERABLE.filter((c) => c.group === group).map((c) => ({
          value: c.key,
          label: columnLabel(t, i18n, c),
        })),
      })).filter((g) => g.options.length > 0),
    [t, i18n],
  );

  const submit = () => {
    if (problem !== null) return;
    onAdd({ id: newConditionId(), colKey, op: effectiveOp, value: opNeedsValue(effectiveOp) ? value.trim() : '' });
    setValue('');
  };

  const hint =
    meta.appliesTo === 'shot'
      ? t('dataToolbar.filter.shotHint')
      : meta.appliesTo === 'station'
        ? t('dataToolbar.filter.stationHint')
        : null;

  return (
    <div className="flex flex-col gap-1.5 px-2 py-1" data-testid="data-filter-builder">
      <div className="flex items-center gap-1.5">
        <Select
          value={colKey}
          onChange={(v) => {
            const next = v as DataColumnKey;
            setColKey(next);
            const nextOps = opsForValueType(DATA_COLUMN_BY_KEY.get(next)!.valueType);
            if (!nextOps.includes(op)) setOp(nextOps[0]!);
            setValue('');
          }}
          options={columnOptions}
          searchLabel={t('dataToolbar.filter.column')}
          size="sm"
          className="min-w-0 flex-1"
          aria-label={t('dataToolbar.filter.column')}
          data-testid="data-filter-add-column"
        />
        <Select
          value={effectiveOp}
          onChange={(v) => setOp(v as FilterOp)}
          options={ops.map((o) => ({ value: o, label: opLabel(t, o, meta.valueType) }))}
          size="sm"
          className="w-32 shrink-0"
          aria-label={t('dataToolbar.filter.operator')}
          data-testid="data-filter-add-op"
        />
      </div>
      <div className="flex items-center gap-1.5">
        {opNeedsValue(effectiveOp) &&
          (meta.valueType === 'enum' && enumValues.length > 0 ? (
            <Select
              value={value}
              onChange={setValue}
              options={enumValues.map((v) => ({ value: v, label: v }))}
              placeholder={t('dataToolbar.filter.valuePick')}
              size="sm"
              className="min-w-0 flex-1"
              aria-label={t('dataToolbar.filter.value')}
              data-testid="data-filter-add-value"
            />
          ) : (
            <input
              type={meta.valueType === 'date' ? 'date' : 'text'}
              inputMode={meta.valueType === 'number' ? 'decimal' : undefined}
              value={value}
              onChange={(e) => setValue(e.target.value)}
              onKeyDown={(e) => {
                if (e.nativeEvent.isComposing || e.keyCode === 229) return;
                if (e.key === 'Enter') {
                  e.preventDefault();
                  submit();
                }
              }}
              placeholder={t('dataToolbar.filter.value')}
              aria-label={t('dataToolbar.filter.value')}
              aria-invalid={value !== '' && problem !== null ? true : undefined}
              className={`min-w-0 flex-1 rounded border bg-background px-2 py-1 text-xs ${
                value !== '' && problem !== null ? 'border-error' : 'border-border'
              }`}
              data-testid="data-filter-add-value"
            />
          ))}
        <button
          type="button"
          className={`${toolbarButtonClass({})} border border-border`}
          disabled={problem !== null}
          title={problem ? t(`dataToolbar.filter.problem.${problem}`) : t('dataToolbar.filter.addTitle')}
          onClick={submit}
          data-testid="data-filter-add-submit"
        >
          {t('dataToolbar.filter.add')}
        </button>
      </div>
      {value !== '' && problem !== null && (
        <div className="text-error" data-testid="data-filter-add-problem">
          {t(`dataToolbar.filter.problem.${problem}`)}
        </div>
      )}
      {hint && <div className="text-[11px] leading-4 text-text-secondary">{hint}</div>}
    </div>
  );
}
