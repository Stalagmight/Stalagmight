/**
 * The Data workspace's own toolbar, above the grid:
 *   find box (toggles, count, prev / next, replace, options), with the replace
 *   box under it while replace is open | row type | Sections | Filter |
 *   Columns | Tint | View | ... Map
 *   then the active filter chips (while anything narrows the view).
 * The find box shrinks first (to its narrowest, then up to MAX_FIND_PX when
 * there is room); only when the labelled row still does not fit on one line
 * do the menu buttons drop their labels (icon + chevron, label kept as the
 * accessible name and tooltip) - measured, see useToolbarCompact.
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
import { ToolbarCompactContext } from './toolbar/ToolbarMenu';
import { useToolbarCompact } from './toolbar/useToolbarCompact';

installRevealPolicy();

export function DataWorkspaceToolbar() {
  const model = useDataModel();
  const view = useDataView();
  if (!model || !view) return null;
  return <Toolbar model={model} view={view} />;
}

function Toolbar({ model, view }: { model: DataModel; view: DataView }) {
  const { t } = useTranslation();
  const rowRef = useRef<HTMLDivElement>(null);
  const compact = useToolbarCompact(rowRef);
  const findRef = useRef<HTMLInputElement>(null);
  const replaceRef = useRef<HTMLInputElement>(null);
  const replaceOpen = useDataWorkspaceStore((s) => s.search.replaceOpen);
  const nav = useFindNavigation(view, model, [findRef, replaceRef]);
  const replace = useReplace(view, model, nav);
  useFindShortcuts({ findInput: findRef, replaceInput: replaceRef, next: nav.next, prev: nav.prev });

  // Escape closed find: give the keyboard back to the grid, on the active row.
  const onFindClosed = () => {
    findRef.current?.blur();
    replaceRef.current?.blur();
    const { activeRowKey, requestFocus } = useDataWorkspaceStore.getState();
    if (activeRowKey) requestFocus({ rowKey: activeRowKey, align: 'auto' });
  };

  return (
    <ToolbarCompactContext.Provider value={compact}>
      <div
        className="shrink-0 border-b border-border bg-panel text-xs text-text-primary"
        role="group"
        aria-label={t('dataToolbar.aria')}
        data-data-toolbar
        data-compact={compact ? 'true' : undefined}
        data-testid="data-toolbar"
      >
        {/* One line: the find box gives way first (its own minimum is its
            controls + a short input), then the labels go; only a toolbar too
            narrow even for the icons wraps. */}
        <div
          ref={rowRef}
          className={`flex items-start gap-x-2 gap-y-1 px-3 py-1.5 ${compact ? 'flex-wrap' : 'flex-nowrap'}`}
          data-testid="data-toolbar-row"
        >
          <div className="flex max-w-[360px] flex-[1_1_260px] flex-col gap-1" data-toolbar-shrink>
            <FindBar
              view={view}
              nav={nav}
              inputRef={findRef}
              editingBlocked={model.ctx.editingBlocked}
              onClosed={onFindClosed}
            />
            {replaceOpen && (
              <ReplaceRow
                inputRef={replaceRef}
                editingBlocked={model.ctx.editingBlocked}
                matches={view.matches}
                busy={replace.busy}
                onReplaceOne={() => void replace.replaceOne()}
                onSkip={nav.next}
                previewAll={replace.previewAll}
                onReplaceAll={() => void replace.replaceAll()}
                onClosed={onFindClosed}
              />
            )}
          </div>
          <RowTypeControl />
          <SectionsPicker ctx={model.ctx} />
          <FilterMenu model={model} />
          <ColumnsMenu revealed={view.revealedColumns} />
          <TintMenu ctx={model.ctx} />
          <ViewMenu ctx={model.ctx} />
          <MapToggle />
        </div>
        <FilterChips model={model} view={view} />
      </div>
    </ToolbarCompactContext.Provider>
  );
}
