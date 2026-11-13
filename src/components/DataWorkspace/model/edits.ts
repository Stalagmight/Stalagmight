/**
 * Turn parsed cell edits into ONE batch (one undo step): `update_shot` per shot
 * (with the averaged-azimuth / pitch expansion of dataTableEdits), credit edits,
 * `set_station_comment` / `set_station_name` per station.
 *
 * Every edit is checked against its column's `lockReason` first (a frozen
 * geometry field, a computed column, a cell that does not apply, a blocked
 * workspace); a dropped edit is reported in `skipped`, never silently lost.
 * An edit that would not change anything (the same comment, the same kind)
 * emits no command. Find / replace calls this too.
 */
import type { BatchCommand } from '../../../types/commands';
import type { ShotUpdates } from '../../../types/project';
import { AZIMUTH_AVG_KEY, PITCH_AVG_KEY, buildShotUpdates, creditEditForCells } from '../../SingleSystemView/dataTableEdits';
import { DATA_COLUMN_BY_KEY } from '../columnsMeta';
import type { DataCellEdit, DataColumnDef, DataColumnKey, DataEditPlan, DataModelCtx, ShotRow, StationRow } from '../types';
import { buildColumnDefs, shotKindOf, shotLegs } from './columns';
import { parsePeopleCell } from '../../SingleSystemView/dataTableCreditColumns';
import { shotCredit } from './ctx';

let defsByKey: Map<DataColumnKey, DataColumnDef> | null = null;
function columnDef(key: DataColumnKey): DataColumnDef | undefined {
  if (!defsByKey) defsByKey = new Map(buildColumnDefs().map((d) => [d.key, d]));
  return defsByKey.get(key);
}

/** The DataTable / ShotUpdates field a shot column writes. */
const SHOT_FIELD: Partial<Record<DataColumnKey, string>> = {
  length: 'length',
  azimuth: AZIMUTH_AVG_KEY,
  azimuth_in: 'azimuth_in',
  azimuth_out: 'azimuth_out',
  inclination: PITCH_AVG_KEY,
  pitch_in: 'pitch_in',
  pitch_out: 'pitch_out',
  depth_in: 'depth_in',
  depth_out: 'depth_out',
  left: 'left',
  right: 'right',
  up: 'up',
  down: 'down',
  kind: 'kind',
  comment: 'comment',
  temperature: 'temperature',
};

/** Credit columns -> the keys `creditEditForCells` reads. */
const CREDIT_FIELD: Partial<Record<DataColumnKey, string>> = {
  surveyors: 'surveyors',
  explorers: 'explorers',
  date: 'survey_date',
};

export function buildEditCommands(edits: readonly DataCellEdit[], ctx: DataModelCtx): DataEditPlan {
  const skipped: DataEditPlan['skipped'] = [];
  // Group by row, keeping first-seen row order; a later edit of the same cell wins.
  const byRow = new Map<string, { row: DataCellEdit['row']; cells: Map<DataColumnKey, DataCellEdit> }>();
  for (const edit of edits) {
    const def = columnDef(edit.colKey);
    const lock = def ? def.lockReason(edit.row, ctx) : 'na';
    if (lock) {
      skipped.push({ edit, reason: lock });
      continue;
    }
    if (edit.value === undefined || !DATA_COLUMN_BY_KEY.has(edit.colKey)) {
      skipped.push({ edit, reason: 'invalid' });
      continue;
    }
    let entry = byRow.get(edit.row.key);
    if (!entry) byRow.set(edit.row.key, (entry = { row: edit.row, cells: new Map() }));
    entry.cells.set(edit.colKey, edit);
  }

  const commands: BatchCommand[] = [];
  for (const { row, cells } of byRow.values()) {
    if (row.kind === 'shot') commands.push(...shotCommands(row, [...cells.values()], ctx));
    else commands.push(...stationCommands(row, cells, ctx));
  }
  return { commands, skipped };
}

function shotCommands(row: ShotRow, cells: DataCellEdit[], ctx: DataModelCtx): BatchCommand[] {
  const out: BatchCommand[] = [];
  const creditCells = cells.flatMap((e) => {
    const field = CREDIT_FIELD[e.colKey];
    if (!field) return [];
    // People arrive as a list; a cell's text (a move of the display value)
    // is parsed rather than read as "nobody".
    const value = field !== 'survey_date' && typeof e.value === 'string' ? parsePeopleCell(e.value) : e.value;
    return [{ colKey: field, value }];
  });
  if (creditCells.length > 0) {
    const credit = shotCredit(ctx, row);
    const edit = creditEditForCells(credit ? { credit } : {}, creditCells);
    if (edit) out.push({ type: 'edit_shot_credit', targets: { type: 'shots', shots: [[row.sectionId, row.shotId]] }, edit });
  }

  const fieldEdits = cells.flatMap((e) => {
    const field = SHOT_FIELD[e.colKey];
    return field ? [{ colKey: field, value: e.value }] : [];
  });
  if (fieldEdits.length === 0) return out;
  const updates: ShotUpdates = buildShotUpdates(shotLegs(ctx, row), fieldEdits);
  // Unchanged text / kind fields are no edit (numbers always go through: the
  // SoA holds float32 copies, so equality there would be a lie).
  if ('comment' in updates && (updates.comment ?? null) === (ctx.data.shotComments.get(row.g) ?? null)) delete updates.comment;
  if ('kind' in updates && updates.kind === shotKindOf(ctx, row)) delete updates.kind;
  if (Object.keys(updates).length > 0) {
    out.push({ type: 'update_shot', section_id: row.sectionId, shot_id: row.shotId, updates });
  }
  return out;
}

function stationCommands(row: StationRow, cells: Map<DataColumnKey, DataCellEdit>, ctx: DataModelCtx): BatchCommand[] {
  const out: BatchCommand[] = [];
  const station_ref = { section_id: row.sectionId, station_index: row.stationIndex };
  const comment = cells.get('comment');
  if (comment) {
    const next = typeof comment.value === 'string' && comment.value.trim() !== '' ? comment.value : null;
    if (next !== (ctx.data.stationComments.get(row.g) ?? null)) {
      out.push({ type: 'set_station_comment', station_ref, comment: next });
    }
  }
  const name = cells.get('name');
  if (name) {
    const next = typeof name.value === 'string' && name.value.trim() !== '' ? name.value.trim() : null;
    if (next !== (ctx.data.stationNames.get(row.g) ?? null)) {
      out.push({ type: 'set_station_name', station_ref, name: next });
    }
  }
  return out;
}
