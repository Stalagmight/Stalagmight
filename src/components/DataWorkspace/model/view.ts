/**
 * The view pipeline: filters -> collapse -> search (matches, reveal) -> sort.
 *
 * CONTRACT STUB - owned by the toolbar / search work; keep the signature.
 */
import type { DataColumnDef, DataColumnKey, DataModelCtx, DataRow, DataView } from '../types';
import type { DataWorkspaceState } from '../store';

export type DataViewInput = Pick<
  DataWorkspaceState,
  'filters' | 'sort' | 'collapsedSections' | 'search' | 'hiddenColumns' | 'columnOrder' | 'revealRowKeys'
>;

export function buildDataView(
  rows: readonly DataRow[],
  columns: readonly DataColumnDef[],
  ctx: DataModelCtx,
  state: DataViewInput,
): DataView {
  void columns;
  void ctx;
  const collapsed = new Set(state.collapsedSections);
  const shown = rows.filter((r) => r.kind === 'section' || !collapsed.has(r.sectionId));
  const entity = rows.filter((r) => r.kind !== 'section').length;
  return {
    rows: shown,
    matches: [],
    matchIndex: new Map<string, Map<DataColumnKey, number[]>>(),
    revealedRowKeys: new Set(),
    revealedColumns: new Set(),
    filteredCount: entity,
    totalCount: entity,
    surveyOrder: state.sort === null,
  };
}
