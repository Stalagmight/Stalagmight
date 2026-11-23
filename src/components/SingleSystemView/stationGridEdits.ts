/**
 * The data table's stations grid edits (RS 5.4, review finding ui-2). Only
 * the Name and the Comment are editable, and set_station_name /
 * set_station_comment change the identity a slot SHOWS, on every line it is
 * shown on. A cell on a foreign slot or a loop end therefore says whose
 * station it is first (the `shown-identity-edit` prompt).
 *
 * The grid hands one paste, fill or range clear over as one batch. Every
 * edit of a batch that needs the prompt is listed in ONE prompt, and the
 * native edits of the same batch wait for its answer, so a paste is applied
 * whole or dropped whole: before, each such cell opened the prompt on its
 * own (one dialog at a time, so only the last survived) while the native
 * cells were sent at once. A lone edit keeps the single-station prompt, with
 * its Detach and Remove the closure ways back.
 */
import type { CellEdit } from '../common/SpreadsheetGrid/types';
import type { Station, StationRef } from '../../types/project';
import type { CommandResult } from '../../types/commands';
import { editNeedsOwnerPrompt } from '../Dialogs/ShownIdentityPrompt';
import { stationRenameToSend } from './dataTableStationColumns';

/** One station edit the grid would send. */
export interface StationGridEdit {
  stationRef: StationRef;
  kind: 'name' | 'comment';
  /** The new name (never empty), or the comment (null clears it). */
  value: string | null;
  /** The slot is foreign or a loop end: the prompt must say whose it is. */
  ask: boolean;
}

/** The edits of one grid commit, in the grid's order, without the no-ops (a
 *  name that is empty or unchanged; a comment unchanged on a slot that would
 *  ask). Rows are indices into `rows`. */
export function planStationGridEdits(edits: readonly CellEdit[], rows: readonly Station[]): StationGridEdit[] {
  const planned: StationGridEdit[] = [];
  for (const edit of edits) {
    const station = rows[edit.row];
    if (!station) continue;
    const ask = editNeedsOwnerPrompt(station.shown);
    if (edit.colKey === 'name') {
      const name = stationRenameToSend(edit.value, station);
      if (name !== null) planned.push({ stationRef: station.station_ref, kind: 'name', value: name, ask });
    } else if (edit.colKey === 'comment') {
      const comment = typeof edit.value === 'string' && edit.value.trim() !== '' ? edit.value : null;
      if (ask && comment === (station.comment ?? null)) continue;
      planned.push({ stationRef: station.station_ref, kind: 'comment', value: comment, ask });
    }
  }
  return planned;
}

export interface StationEditActions {
  setStationName: (ref: StationRef, name: string) => Promise<CommandResult>;
  updateStationComment: (ref: StationRef, comment: string | null) => Promise<CommandResult>;
}

/** Send `edits` one by one, in order (the commands have no batch form). */
export async function applyStationGridEdits(edits: readonly StationGridEdit[], actions: StationEditActions): Promise<void> {
  for (const edit of edits) {
    if (edit.kind === 'name') {
      if (edit.value) await actions.setStationName(edit.stationRef, edit.value);
    } else {
      await actions.updateStationComment(edit.stationRef, edit.value);
    }
  }
}

/**
 * Commit `edits` (one grid commit): at once when none needs the prompt;
 * else through the prompt, one for a lone edit (`stationRef`, `kind`,
 * `value`, as before) and one listing every station for a batch (`edits`:
 * all of them, sent together on going on). Returns whether it asked: the
 * grid then keeps its cursor on the edited cell while the prompt is open
 * (RS-FX8, N6).
 */
export function commitStationGridEdits(
  edits: readonly StationGridEdit[],
  openDialog: (dialog: string, data: Record<string, unknown>) => void,
  actions: StationEditActions,
): boolean {
  if (edits.length === 0) return false;
  if (!edits.some((e) => e.ask)) {
    void applyStationGridEdits(edits, actions);
    return false;
  }
  const [only] = edits;
  if (edits.length === 1 && only) {
    openDialog('shown-identity-edit', { stationRef: only.stationRef, kind: only.kind, value: only.value });
    return true;
  }
  openDialog('shown-identity-edit', { edits: [...edits] });
  return true;
}
