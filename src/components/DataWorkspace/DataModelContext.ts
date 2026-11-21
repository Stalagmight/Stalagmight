/**
 * React contexts the Data workspace shell provides to its children (toolbar,
 * sort bar, map pane header): the built model and the current view.
 */
import { createContext, useContext } from 'react';
import type { DataColumnDef, DataColumnKey, DataModelCtx, DataRow, DataView } from './types';

export interface DataModel {
  /** Every row in survey order (headers included when enabled), unfiltered. */
  rows: DataRow[];
  rowByKey: Map<string, DataRow>;
  /** Every column with its logic, in default order. */
  columns: DataColumnDef[];
  columnByKey: Map<DataColumnKey, DataColumnDef>;
  ctx: DataModelCtx;
}

export const DataModelContext = createContext<DataModel | null>(null);
export const DataViewContext = createContext<DataView | null>(null);

export function useDataModel(): DataModel | null {
  return useContext(DataModelContext);
}

export function useDataView(): DataView | null {
  return useContext(DataViewContext);
}
