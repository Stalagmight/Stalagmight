/**
 * Right-click menu on a Data grid column header: sort ascending / descending /
 * back to survey order, hide the column, filter by it (adds a condition the
 * toolbar then shows for editing), reset its width. Same look and dismissal as
 * the DataTable's header menu.
 */
import { useEffect, useLayoutEffect, useRef, useState, type RefObject } from 'react';
import { useTranslation } from 'react-i18next';
import { useDismissableLayer } from '../../hooks/useDismissableLayer';
import { useDataWorkspaceStore } from './store';
import type { DataColumnDef, DataColumnKey, FilterOp } from './types';

export interface HeaderMenuState {
  x: number;
  y: number;
  colKey: DataColumnKey;
}

/** The condition "Filter by this column" starts with. */
export function defaultFilterOp(def: Pick<DataColumnDef, 'valueType'>): FilterOp {
  if (def.valueType === 'number' || def.valueType === 'date') return 'gte';
  if (def.valueType === 'enum') return 'eq';
  return 'contains';
}

let conditionSeq = 0;

export function DataHeaderMenu({
  menu,
  def,
  onClose,
  anchorScope,
}: {
  menu: HeaderMenuState;
  def: DataColumnDef | undefined;
  onClose: () => void;
  anchorScope: RefObject<HTMLElement | null>;
}) {
  const { t } = useTranslation();
  const ref = useRef<HTMLDivElement | null>(null);
  const [pos, setPos] = useState({ x: menu.x, y: menu.y });
  const sort = useDataWorkspaceStore((s) => s.sort);
  const widths = useDataWorkspaceStore((s) => s.columnWidths);

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    let { x, y } = menu;
    if (y + r.height > window.innerHeight) y = Math.max(4, y - r.height);
    if (x + r.width > window.innerWidth) x = Math.max(4, x - r.width);
    setPos({ x, y });
  }, [menu]);
  useDismissableLayer({ open: true, onDismiss: onClose, contains: [ref], anchorScope, survivesUnrelatedScroll: true });
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose]);

  const key = menu.colKey;
  const name = t(`dataWorkspace.columns.${key}`);
  const store = useDataWorkspaceStore.getState;
  const items: { id: string; label: string; onClick: () => void; disabled?: boolean; separatorBefore?: boolean }[] = [];
  if (def?.sortable) {
    items.push(
      { id: 'sort-asc', label: t('dataWorkspace.headerMenu.sortAsc'), onClick: () => store().setSort({ colKey: key, dir: 'asc' }) },
      { id: 'sort-desc', label: t('dataWorkspace.headerMenu.sortDesc'), onClick: () => store().setSort({ colKey: key, dir: 'desc' }) },
      { id: 'sort-clear', label: t('dataWorkspace.headerMenu.clearSort'), onClick: () => store().setSort(null), disabled: sort === null },
    );
  }
  items.push({
    id: 'hide',
    label: t('dataWorkspace.headerMenu.hide', { name }),
    separatorBefore: items.length > 0,
    onClick: () => {
      const hidden = store().hiddenColumns;
      if (!hidden.includes(key)) store().setHiddenColumns([...hidden, key]);
    },
  });
  if (def) {
    items.push({
      id: 'filter',
      label: t('dataWorkspace.headerMenu.filterBy'),
      onClick: () => store().addCondition({ id: `hdr-${Date.now().toString(36)}-${++conditionSeq}`, colKey: key, op: defaultFilterOp(def), value: '' }),
    });
  }
  items.push({
    id: 'reset-width',
    label: t('dataWorkspace.headerMenu.resetWidth'),
    disabled: widths[key] === undefined,
    onClick: () => {
      const next = { ...store().columnWidths };
      delete next[key];
      store().setColumnWidths(next);
    },
  });

  return (
    <div
      ref={ref}
      className="fixed z-50 w-52 rounded border border-border bg-panel p-1 text-xs shadow-lg"
      style={{ left: pos.x, top: pos.y }}
      onMouseDown={(e) => e.stopPropagation()}
      data-dropdown-open
      role="menu"
      data-testid="data-header-menu"
    >
      {items.map((item) => (
        <div key={item.id}>
          {item.separatorBefore && <div className="my-1 h-px bg-border" />}
          <button
            type="button"
            role="menuitem"
            disabled={item.disabled}
            className="block w-full rounded px-2 py-0.5 text-left hover:bg-accent-10 disabled:opacity-50 disabled:hover:bg-transparent"
            onClick={() => {
              item.onClick();
              onClose();
            }}
            data-testid={`data-header-menu-${item.id}`}
          >
            {item.label}
          </button>
        </div>
      ))}
    </div>
  );
}
