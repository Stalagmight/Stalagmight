/**
 * The small view controls of the toolbar: the row-type segmented control, the
 * View menu (section headers, collapse / expand all) and the map pane toggle.
 */
import { useContext, type KeyboardEvent } from 'react';
import { useTranslation } from 'react-i18next';
import { useDataWorkspaceStore } from '../store';
import type { DataModelCtx, RowTypeFilter } from '../types';
import { ListIcon, MapIcon } from '../../common/icons';
import { MenuDivider, ToolbarCompactContext, ToolbarMenu, toolbarButtonClass } from './ToolbarMenu';

const ROW_TYPES: readonly RowTypeFilter[] = ['all', 'stations', 'shots'];

/** All / Stations / Shots, styled like the data table's Shots / Stations
 *  switch. A radio group: one tab stop, arrow keys move the choice. */
export function RowTypeControl() {
  const { t } = useTranslation();
  const rowTypes = useDataWorkspaceStore((s) => s.filters.rowTypes);
  const setFilters = useDataWorkspaceStore((s) => s.setFilters);
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const step = e.key === 'ArrowRight' || e.key === 'ArrowDown' ? 1 : e.key === 'ArrowLeft' || e.key === 'ArrowUp' ? -1 : 0;
    if (step === 0) return;
    e.preventDefault();
    const next = ROW_TYPES[(ROW_TYPES.indexOf(rowTypes) + step + ROW_TYPES.length) % ROW_TYPES.length]!;
    if (next !== rowTypes) setFilters({ rowTypes: next });
    e.currentTarget.querySelector<HTMLButtonElement>(`[data-testid="data-rowtype-${next}"]`)?.focus();
  };
  return (
    <div
      className="flex h-7 shrink-0 items-stretch overflow-hidden rounded border border-border text-xs"
      role="radiogroup"
      aria-label={t('dataToolbar.rowType.label')}
      onKeyDown={onKeyDown}
      data-testid="data-rowtype"
    >
      {ROW_TYPES.map((type) => (
        <button
          key={type}
          type="button"
          role="radio"
          aria-checked={rowTypes === type}
          tabIndex={rowTypes === type ? 0 : -1}
          className={`h-full whitespace-nowrap px-2 ${
            rowTypes === type ? 'bg-accent-20 text-text-primary' : 'text-text-secondary hover:bg-accent-10'
          }`}
          onClick={() => setFilters({ rowTypes: type })}
          data-testid={`data-rowtype-${type}`}
        >
          {t(`dataToolbar.rowType.${type}`)}
        </button>
      ))}
    </div>
  );
}

export function ViewMenu({ ctx }: { ctx: DataModelCtx }) {
  const { t } = useTranslation();
  const showSectionHeaders = useDataWorkspaceStore((s) => s.showSectionHeaders);
  const setShowSectionHeaders = useDataWorkspaceStore((s) => s.setShowSectionHeaders);
  const collapsedSections = useDataWorkspaceStore((s) => s.collapsedSections);
  const setCollapsedSections = useDataWorkspaceStore((s) => s.setCollapsedSections);
  const sorted = useDataWorkspaceStore((s) => s.sort !== null);
  const allIds = ctx.data.sections.map((s) => s.id);
  const canCollapse = showSectionHeaders && !sorted;
  const why = !showSectionHeaders ? t('dataToolbar.view.needsHeaders') : sorted ? t('dataToolbar.view.sorted') : undefined;

  return (
    <ToolbarMenu
      testId="data-view-button"
      menuTestId="data-view-menu"
      icon={<ListIcon size={14} />}
      label={t('dataToolbar.view.button')}
      title={t('dataToolbar.view.title')}
      width={240}
    >
      <label className="flex cursor-pointer items-center gap-2 rounded px-2 py-1 hover:bg-accent-10">
        <input
          type="checkbox"
          checked={showSectionHeaders}
          onChange={(e) => setShowSectionHeaders(e.target.checked)}
          data-testid="data-view-section-headers"
        />
        {t('dataToolbar.view.sectionHeaders')}
      </label>
      <MenuDivider />
      <button
        type="button"
        className="flex w-full rounded px-2 py-1 text-left hover:bg-accent-10 disabled:cursor-not-allowed disabled:opacity-50"
        disabled={!canCollapse || collapsedSections.length === allIds.length}
        title={why}
        onClick={() => setCollapsedSections(allIds)}
        data-testid="data-view-collapse-all"
      >
        {t('dataToolbar.view.collapseAll')}
      </button>
      <button
        type="button"
        className="flex w-full rounded px-2 py-1 text-left hover:bg-accent-10 disabled:cursor-not-allowed disabled:opacity-50"
        disabled={!canCollapse || collapsedSections.length === 0}
        title={why}
        onClick={() => setCollapsedSections([])}
        data-testid="data-view-expand-all"
      >
        {t('dataToolbar.view.expandAll')}
      </button>
      {why && <div className="px-2 pb-1 text-[11px] leading-4 text-text-secondary">{why}</div>}
    </ToolbarMenu>
  );
}

export function MapToggle() {
  const { t } = useTranslation();
  const open = useDataWorkspaceStore((s) => s.mapPaneOpen);
  const setMapPaneOpen = useDataWorkspaceStore((s) => s.setMapPaneOpen);
  const compact = useContext(ToolbarCompactContext);
  return (
    <button
      type="button"
      // Pushed to the row's end (the toolbar's last control).
      className={`${toolbarButtonClass({ active: open })} ml-auto`}
      aria-pressed={open}
      aria-label={compact ? t('dataToolbar.map.button') : undefined}
      title={open ? t('dataToolbar.map.hide') : t('dataToolbar.map.show')}
      onClick={() => setMapPaneOpen(!open)}
      data-testid="data-map-toggle"
    >
      <MapIcon size={14} />
      {!compact && t('dataToolbar.map.button')}
    </button>
  );
}
