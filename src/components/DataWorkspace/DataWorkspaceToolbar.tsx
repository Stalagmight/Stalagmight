/**
 * The Data workspace's own toolbar, above the grid:
 *   row 1  find box (toggles, count, prev / next, replace, options) | row type |
 *          Sections | Filter | Columns | Tint | View | Map
 *   row 2  the replace row (while replace is open)
 *   row 3  active filter chips (while anything narrows or reorders the view)
 * Reads the model / view from the shell's contexts and the workspace store.
 */
import { useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { useDataModel, useDataView, type DataModel } from './DataModelContext';
import { useDataWorkspaceStore } from './store';
import type { DataView } from './types';
import { FindBar } from './toolbar/FindBar';
import { ReplaceRow } from './toolbar/ReplaceRow';
import { SectionsPicker } from './toolbar/SectionsPicker';
import { FilterMenu } from './toolbar/FilterMenu';
import { ColumnsMenu } from './toolbar/ColumnsMenu';
import { TintMenu } from './toolbar/TintMenu';
import { MapToggle, RowTypeControl, ViewMenu } from './toolbar/ViewControls';
import { FilterChips } from './toolbar/FilterChips';
import { useFindNavigation } from './toolbar/useFindNavigation';
import { useFindShortcuts } from './toolbar/useFindShortcuts';
import { useReplace } from './toolbar/useReplace';
import { installRevealPolicy } from './toolbar/revealPolicy';

installRevealPolicy();

export function DataWorkspaceToolbar() {
  const model = useDataModel();
  const view = useDataView();
  if (!model || !view) return null;
  return <Toolbar model={model} view={view} />;
}

function Toolbar({ model, view }: { model: DataModel; view: DataView }) {
  const { t } = useTranslation();
  const findRef = useRef<HTMLInputElement>(null);
  const replaceRef = useRef<HTMLInputElement>(null);
  const replaceOpen = useDataWorkspaceStore((s) => s.search.replaceOpen);
  const nav = useFindNavigation(view, model, [findRef, replaceRef]);
  const replace = useReplace(view, model, nav);
  useFindShortcuts({ findInput: findRef, replaceInput: replaceRef, next: nav.next, prev: nav.prev });

  // Escape closed find: give the keyboard back to the grid, on the active row.
  const onFindClosed = () => {
    findRef.current?.blur();
    const { activeRowKey, requestFocus } = useDataWorkspaceStore.getState();
    if (activeRowKey) requestFocus({ rowKey: activeRowKey, align: 'auto' });
  };

  return (
    <div
      className="shrink-0 border-b border-border bg-panel text-xs text-text-primary"
      role="toolbar"
      aria-label={t('dataToolbar.aria')}
      data-testid="data-toolbar"
    >
      <div className="flex flex-wrap items-center gap-x-2 gap-y-1 px-3 py-1.5">
        <FindBar
          view={view}
          nav={nav}
          inputRef={findRef}
          editingBlocked={model.ctx.editingBlocked}
          onClosed={onFindClosed}
        />
        <RowTypeControl />
        <SectionsPicker ctx={model.ctx} />
        <FilterMenu model={model} />
        <ColumnsMenu revealed={view.revealedColumns} />
        <TintMenu ctx={model.ctx} />
        <ViewMenu ctx={model.ctx} />
        <span className="ml-auto" />
        <MapToggle />
      </div>
      {replaceOpen && (
        <ReplaceRow
          inputRef={replaceRef}
          editingBlocked={model.ctx.editingBlocked}
          hasMatches={view.matches.length > 0}
          busy={replace.busy}
          onReplaceOne={() => void replace.replaceOne()}
          onSkip={nav.next}
          previewAll={replace.previewAll}
          onReplaceAll={() => void replace.replaceAll()}
        />
      )}
      <FilterChips model={model} view={view} />
    </div>
  );
}
