/**
 * The Data workspace: one spreadsheet with every section's stations and shots
 * interleaved in survey order, shown when `appMode === 'data'`.
 *
 * The shell builds the model (rows, column logic, ctx) and the view (filters,
 * collapse, search, sort - `buildDataView`), provides both to its children
 * (toolbar, sort bar, map pane header) through DataModelContext /
 * DataViewContext, and lays out: toolbar, sort bar, read-only banner, the grid
 * (sized to its body by a ResizeObserver) and the status bar. Workspace-local
 * shortcuts: Alt+M shows the active row on the map, Alt+S opens it in Survey.
 */
import { useCallback, useEffect, useMemo, useRef, useState, type KeyboardEvent } from 'react';
import { useTranslation } from 'react-i18next';
import { useShallow } from 'zustand/react/shallow';
import { useProjectStore } from '../../store/projectStore';
import { useEditingBlocked } from '../../hooks/useEditingBlocked';
import { DataModelContext, DataViewContext } from './DataModelContext';
import { useDataWorkspaceStore } from './store';
import { buildDataView } from './model/view';
import { useDataWorkspaceModel } from './useDataWorkspaceModel';
import { useDataWorkspaceViewPersistence } from './useDataWorkspaceViewPersistence';
import { DataNavContext, useDataNavigation } from './useDataNavigation';
import { useDataEditing } from './useDataEditing';
import { DataTintContext, useDataTint } from './useDataTint';
import { DataWorkspaceToolbar } from './DataWorkspaceToolbar';
import { DataSortBar } from './DataSortBar';
import { DataReadOnlyBanner } from './DataReadOnlyBanner';
import { DataStatusBar } from './DataStatusBar';
import { DataGrid } from './DataGrid';
import type { SelectionSummary } from './selectionSummary';
import './DataWorkspace.css';
import './projectReset';

const MIN_GRID_HEIGHT = 120;

function isTypingTarget(target: EventTarget | null): boolean {
  return (
    target instanceof HTMLInputElement ||
    target instanceof HTMLTextAreaElement ||
    target instanceof HTMLSelectElement ||
    (target instanceof HTMLElement && target.isContentEditable)
  );
}

function EmptyState({ message, action }: { message: string; action?: { label: string; onClick: () => void } }) {
  return (
    <div className="flex h-full flex-col items-center justify-center gap-2 p-6 text-center text-sm text-text-secondary" data-testid="data-empty">
      <span>{message}</span>
      {action && (
        <button
          type="button"
          className="rounded border border-border px-2 py-0.5 text-xs text-text-primary hover:bg-accent-10"
          onClick={action.onClick}
          data-testid="data-empty-clear-filters"
        >
          {action.label}
        </button>
      )}
    </div>
  );
}

export function DataWorkspace() {
  const { t } = useTranslation();
  useDataWorkspaceViewPersistence();
  const project = useProjectStore((s) => s.project);
  const editingBlocked = useEditingBlocked();
  const model = useDataWorkspaceModel();
  const viewInput = useDataWorkspaceStore(
    useShallow((s) => ({
      filters: s.filters,
      sort: s.sort,
      collapsedSections: s.collapsedSections,
      search: s.search,
      hiddenColumns: s.hiddenColumns,
      columnOrder: s.columnOrder,
      revealRowKeys: s.revealRowKeys,
    })),
  );
  const view = useMemo(
    () => (model ? buildDataView(model.rows, model.columns, model.ctx, viewInput) : null),
    [model, viewInput],
  );

  const [partners, setPartners] = useState<ReadonlySet<string>>(() => new Set());
  const setHoveredPartners = useCallback((keys: readonly string[] | null) => {
    setPartners((prev) => (keys === null ? (prev.size === 0 ? prev : new Set()) : new Set(keys)));
  }, []);
  const nav = useDataNavigation(model, view, setHoveredPartners);
  const editing = useDataEditing(model, view);
  const tint = useDataTint(model);
  const [selection, setSelection] = useState<SelectionSummary | null>(null);

  // The grid must fit its body exactly (it scrolls internally).
  const bodyRef = useRef<HTMLDivElement | null>(null);
  const [height, setHeight] = useState(400);
  useEffect(() => {
    const el = bodyRef.current;
    if (!el) return undefined;
    const measure = () => setHeight((prev) => {
      const h = Math.max(MIN_GRID_HEIGHT, Math.floor(el.clientHeight));
      return Math.abs(prev - h) > 0.5 ? h : prev;
    });
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    measure();
    return () => ro.disconnect();
  }, []);

  // Entering Data puts the keyboard in the grid (the active row is restored by
  // the grid from store.activeRowKey).
  const hasRows = (view?.rows.length ?? 0) > 0;
  useEffect(() => {
    if (!hasRows) return;
    const grid = bodyRef.current?.querySelector<HTMLElement>('[role="grid"]');
    if (grid && !grid.contains(document.activeElement)) grid.focus({ preventScroll: true });
  }, [hasRows]);

  const onKeyDown = useCallback(
    (e: KeyboardEvent<HTMLDivElement>) => {
      if (e.defaultPrevented || !e.altKey || e.ctrlKey || e.metaKey || isTypingTarget(e.target)) return;
      const action = e.code === 'KeyM' ? 'map' : e.code === 'KeyS' ? 'survey' : null;
      if (!action || !model) return;
      const key = useDataWorkspaceStore.getState().activeRowKey;
      const row = key ? model.rowByKey.get(key) : undefined;
      if (!row) return;
      e.preventDefault();
      e.stopPropagation();
      if (action === 'map') nav.showOnMap(row);
      else nav.openInSurvey(row);
    },
    [model, nav],
  );

  const viewHasEntities = useMemo(() => view?.rows.some((r) => r.kind !== 'section') ?? false, [view]);
  let body;
  if (!project || !model || !view) {
    body = <EmptyState message={t('dataWorkspace.empty.noProject')} />;
  } else if (model.rows.length === 0) {
    body = <EmptyState message={t('dataWorkspace.empty.noSections')} />;
  } else if (view.rows.length === 0 || (viewInput.collapsedSections.length === 0 && !viewHasEntities)) {
    body = (
      <EmptyState
        message={t('dataWorkspace.empty.noRows')}
        action={{ label: t('dataWorkspace.empty.clearFilters'), onClick: () => useDataWorkspaceStore.getState().clearFilters() }}
      />
    );
  } else {
    body = (
      <DataGrid
        model={model}
        view={view}
        height={height}
        nav={nav}
        editing={editing}
        tint={tint}
        partners={partners}
        onSelectionSummary={setSelection}
      />
    );
  }

  return (
    <DataModelContext.Provider value={model}>
      <DataViewContext.Provider value={view}>
        <DataTintContext.Provider value={tint}>
          <DataNavContext.Provider value={nav}>
            <div
              className="flex h-full w-full min-w-0 flex-col bg-panel"
              data-testid="data-workspace"
              data-editing-blocked={editingBlocked ? 'true' : 'false'}
              onKeyDown={onKeyDown}
            >
              <DataWorkspaceToolbar />
              <DataSortBar />
              {editingBlocked && project && <DataReadOnlyBanner />}
              <div ref={bodyRef} className="relative min-h-0 flex-1 overflow-hidden">
                {body}
              </div>
              {model && view && <DataStatusBar rows={model.rows} view={view} ctx={model.ctx} selection={selection} />}
            </div>
          </DataNavContext.Provider>
        </DataTintContext.Provider>
      </DataViewContext.Provider>
    </DataModelContext.Provider>
  );
}
