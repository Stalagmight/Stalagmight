/**
 * Column logic for the Data workspace (values, display text, edit locks, parse).
 *
 * CONTRACT STUB - owned by the workspace-shell work; implement every column,
 * keep the exported names.
 */
import { DATA_COLUMNS } from '../columnsMeta';
import type { DataColumnDef } from '../types';

export function buildColumnDefs(): DataColumnDef[] {
  return DATA_COLUMNS.map((meta) => ({
    ...meta,
    value: () => null,
    text: () => '',
    lockReason: () => 'computed' as const,
  }));
}
