/**
 * Test harness for the toolbar: provides the model and a live view (rebuilt
 * from the store with buildDataView, as the shell does) and renders the
 * toolbar + sort bar.
 */
import { useMemo } from 'react';
import { useShallow } from 'zustand/react/shallow';
import { DataModelContext, DataViewContext, type DataModel } from '../../DataModelContext';
import { DataWorkspaceToolbar } from '../../DataWorkspaceToolbar';
import { DataSortBar } from '../../DataSortBar';
import { useDataWorkspaceStore } from '../../store';
import { buildDataView } from '../../model/view';
import type { DataColumnKey, DataRow } from '../../types';
import {
  makeColumns,
  makeCtx,
  makeData,
  makeRows,
  type CtxOpts,
  type SectionSeed,
} from '../../model/__tests__/fixtures';

export const SECTIONS: SectionSeed[] = [
  {
    id: 1,
    name: 'Main',
    shots: [{ length: 10, comment: 'sump one' }, { length: 35 }, { length: 5, comment: 'dry' }],
    stationComments: { 1: 'Sump pool' },
  },
  {
    id: 2,
    name: 'Side',
    shots: [{ length: 12, comment: 'sump' }, { length: 50 }],
  },
];

export function makeModel(ctxOpts: CtxOpts = {}, sections = SECTIONS): DataModel {
  const data = makeData(sections);
  const rows = makeRows(data, { headers: true });
  const columns = makeColumns();
  return {
    rows,
    rowByKey: new Map<string, DataRow>(rows.map((r) => [r.key, r])),
    columns,
    columnByKey: new Map<DataColumnKey, (typeof columns)[number]>(columns.map((c) => [c.key, c])),
    ctx: makeCtx(data, ctxOpts),
  };
}

export function Harness({ model }: { model: DataModel }) {
  const state = useDataWorkspaceStore(
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
  const view = useMemo(() => buildDataView(model.rows, model.columns, model.ctx, state), [model, state]);
  return (
    <DataModelContext.Provider value={model}>
      <DataViewContext.Provider value={view}>
        <DataWorkspaceToolbar />
        <DataSortBar />
        <output data-testid="harness-rows">{view.rows.map((r) => r.key).join(' ')}</output>
      </DataViewContext.Provider>
    </DataModelContext.Provider>
  );
}

const initialStore = useDataWorkspaceStore.getState();

/** Back to a fresh store with every column shown. */
export function resetStore(): void {
  useDataWorkspaceStore.setState(initialStore, true);
  useDataWorkspaceStore.setState({ hiddenColumns: [] });
}
