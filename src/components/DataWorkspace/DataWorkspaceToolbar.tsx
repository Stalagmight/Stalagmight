/**
 * The Data workspace's own toolbar, above the grid:
 *   find box (toggles, count, prev / next, replace, options), with the replace
 *   box under it while replace is open | row type | Sections | Filter |
 *   Columns | Tint | View | ... Map
 *   then the active filter chips (while anything narrows the view).
 * Below COMPACT_BELOW_PX the menu buttons drop their labels (icon + chevron,
 * label kept as the accessible name) so the controls stay on one line.
 * Reads the model / view from the shell's contexts and the workspace store.
 */
import { useEffect, useRef, useState, type RefObject } from 'react';
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

installRevealPolicy();

/** Toolbar width under which the menu buttons show icons only. */
export const COMPACT_BELOW_PX = 1080;

function useCompact(ref: RefObject<HTMLElement>): boolean {
  const [compact, setCompact] = useState(false);
  useEffect(() => {
    const el = ref.current;
    if (!el || typeof ResizeObserver === 'undefined') return undefined;
    const ro = new ResizeObserver(([entry]) => {
      // 0 = not laid out (hidden, or a test DOM): keep the labels.
      const width = entry?.contentRect.width ?? 0;
      if (width > 0) setCompact(width < COMPACT_BELOW_PX);
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, [ref]);
  return compact;
}

export function DataWorkspaceToolbar() {
  const model = useDataModel();
  const view = useDataView();
  if (!model || !view) return null;
  return <Toolbar model={model} view={view} />;
}

function Toolbar({ model, view }: { model: DataModel; view: DataView }) {
  const { t } = useTranslation();
  const rootRef = useRef<HTMLDivElement>(null);
  const compact = useCompact(rootRef);
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
        ref={rootRef}
        className="shrink-0 border-b border-border bg-panel text-xs text-text-primary"
        role="toolbar"
        aria-label={t('dataToolbar.aria')}
        data-compact={compact ? 'true' : undefined}
        data-testid="data-toolbar"
      >
        <div className="flex flex-wrap items-start gap-x-2 gap-y-1 px-3 py-1.5">
          <div className="flex min-w-[17rem] flex-[0_1_26rem] flex-col gap-1">
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
                hasMatches={view.matches.length > 0}
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
          <span className="ml-auto" />
          <MapToggle />
        </div>
        <FilterChips model={model} view={view} />
      </div>
    </ToolbarCompactContext.Provider>
  );
}
