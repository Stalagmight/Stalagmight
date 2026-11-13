/**
 * Sections filter: a searchable checklist of every section (colour dot, name,
 * id) with "All sections" and a per-row "Only". `filters.sectionIds` is null
 * while every section is shown.
 */
import { useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useDataWorkspaceStore } from '../store';
import type { DataModelCtx } from '../types';
import { SectionsIcon } from '../../common/icons';
import { testIdPart } from '../../../utils/testId';
import { MenuDivider, ToolbarMenu } from './ToolbarMenu';

interface SectionEntry {
  id: number;
  name: string;
  color: string;
}

export function sectionEntries(ctx: DataModelCtx): SectionEntry[] {
  return ctx.data.sections.map((s) => ({
    id: s.id,
    name: ctx.sectionNames.get(s.id) ?? s.name ?? String(s.id),
    color: ctx.sectionsById.get(s.id)?.color ?? s.color,
  }));
}

/** Toggle one id in a selection where null = all. Back to null when all are on. */
export function toggleSection(selected: readonly number[] | null, id: number, allIds: readonly number[]): number[] | null {
  const current = selected === null ? new Set(allIds) : new Set(selected);
  if (current.has(id)) current.delete(id);
  else current.add(id);
  if (allIds.every((x) => current.has(x))) return null;
  return allIds.filter((x) => current.has(x));
}

export function SectionsPicker({ ctx }: { ctx: DataModelCtx }) {
  const { t } = useTranslation();
  const sectionIds = useDataWorkspaceStore((s) => s.filters.sectionIds);
  const setFilters = useDataWorkspaceStore((s) => s.setFilters);
  const [query, setQuery] = useState('');
  const searchRef = useRef<HTMLInputElement | null>(null);
  const entries = useMemo(() => sectionEntries(ctx), [ctx]);
  const allIds = useMemo(() => entries.map((e) => e.id), [entries]);
  const selected = useMemo(() => (sectionIds === null ? null : new Set(sectionIds)), [sectionIds]);
  const shownCount = selected === null ? entries.length : entries.filter((e) => selected.has(e.id)).length;
  const q = query.trim().toLowerCase();
  const visible = q === '' ? entries : entries.filter((e) => e.name.toLowerCase().includes(q) || String(e.id).includes(q));
  const active = sectionIds !== null;

  const allBox = (el: HTMLInputElement | null) => {
    if (el) el.indeterminate = active && shownCount > 0;
  };

  return (
    <ToolbarMenu
      testId="data-sections-button"
      menuTestId="data-sections-menu"
      icon={<SectionsIcon size={14} />}
      label={
        active
          ? t('dataToolbar.sections.buttonCount', { shown: shownCount, total: entries.length })
          : t('dataToolbar.sections.button')
      }
      title={t('dataToolbar.sections.title')}
      active={active}
      width={280}
      maxHeightCap={480}
      remeasureKey={visible.length}
    >
      <div className="px-1 pb-1 pt-0.5">
        <input
          ref={searchRef}
          autoFocus
          type="text"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder={t('dataToolbar.sections.search')}
          aria-label={t('dataToolbar.sections.search')}
          className="w-full rounded border border-border bg-background px-2 py-1 text-xs"
          data-testid="data-sections-search"
        />
      </div>
      <label className="flex cursor-pointer items-center gap-2 rounded px-2 py-1 font-medium hover:bg-accent-10">
        <input
          ref={allBox}
          type="checkbox"
          checked={!active}
          onChange={() => setFilters({ sectionIds: active ? null : [] })}
          data-testid="data-sections-all"
        />
        <span className="flex-1">{t('dataToolbar.sections.all')}</span>
        <span className="text-text-secondary tabular-nums" data-testid="data-sections-count">
          {t('dataToolbar.sections.count', { shown: shownCount, total: entries.length })}
        </span>
      </label>
      <MenuDivider />
      {visible.length === 0 && (
        <div className="px-2 py-1 text-text-secondary">{t('dataToolbar.sections.noResults')}</div>
      )}
      {visible.map((e) => (
        <div key={e.id} className="group flex items-center gap-2 rounded px-2 py-0.5 hover:bg-accent-10">
          <label className="flex min-w-0 flex-1 cursor-pointer items-center gap-2">
            <input
              type="checkbox"
              checked={selected === null || selected.has(e.id)}
              onChange={() => setFilters({ sectionIds: toggleSection(sectionIds, e.id, allIds) })}
              data-testid={`data-sections-item-${testIdPart(e.id)}`}
            />
            <span className="h-2 w-2 shrink-0 rounded-full" style={{ backgroundColor: e.color }} aria-hidden="true" />
            <span className="truncate" title={e.name}>
              {e.name}
            </span>
            <span className="shrink-0 text-text-secondary tabular-nums">#{e.id}</span>
          </label>
          <button
            type="button"
            className="shrink-0 rounded px-1 text-text-secondary opacity-0 hover:text-text-primary focus:opacity-100 group-hover:opacity-100"
            title={t('dataToolbar.sections.onlyTitle', { name: e.name })}
            onClick={() => setFilters({ sectionIds: [e.id] })}
            data-testid={`data-sections-only-${testIdPart(e.id)}`}
          >
            {t('dataToolbar.sections.only')}
          </button>
        </div>
      ))}
    </ToolbarMenu>
  );
}
