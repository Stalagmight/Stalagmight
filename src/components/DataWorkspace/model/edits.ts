/**
 * Turn parsed cell edits into ONE batch (one undo step): `update_shot` per shot
 * (with the averaged-azimuth / pitch expansion of dataTableEdits), credit edits,
 * `set_station_comment` / `set_station_name` per station.
 *
 * CONTRACT STUB - owned by the workspace-shell work; keep the signature. Find /
 * replace calls this too.
 */
import type { DataCellEdit, DataEditPlan, DataModelCtx } from '../types';

export function buildEditCommands(edits: readonly DataCellEdit[], ctx: DataModelCtx): DataEditPlan {
  void ctx;
  return { commands: [], skipped: edits.map((edit) => ({ edit, reason: 'invalid' as const })) };
}
