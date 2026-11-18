import type { CommandResult } from '../types/commands';
import type { CreditEdit, CreditTargets } from '../types/ipc.generated';
import type { LineId, ShotId, StationRef } from '../types/project';

/** `projectStore`'s `executeCommand`: invoke, refresh on success, mark dirty. */
export type ExecuteCommand = (
  command: string,
  args: Record<string, unknown> | undefined,
  meta: { purpose: string; fields?: Record<string, unknown> },
) => Promise<CommandResult>;

/**
 * Shot-level actions split out of `projectStore.ts`, which is past the 1500-LOC
 * ratchet and may shrink but not grow (#157; see `projectConfigActions.ts`). They
 * go through the store's own `executeCommand`, so a success refreshes the project
 * state and marks it dirty exactly like every other mutation.
 */
export interface ProjectShotActions {
  splitShotAtDistance: (lineId: LineId, shotId: ShotId, distance: number) => Promise<CommandResult>;
  /**
   * Edit the survey credit (surveyors, explorers, date) of a whole line or of
   * explicit shots, as ONE undo step however many shots it touches. Per role the
   * edit sets, then removes, then adds people (matched on casefolded name and
   * email); `set_date` sets a date or, with `null`, clears it.
   */
  editShotCredit: (targets: CreditTargets, edit: CreditEdit) => Promise<CommandResult>;
  /**
   * Rename a station (one undo step). The backend trims the name and refuses
   * an empty one or one another station of the line holds, with a message
   * ("Line \"X\" already has a station named \"Y\"") the caller shows inline.
   */
  setStationName: (stationRef: StationRef, name: string) => Promise<CommandResult>;
}

export function createProjectShotActions(executeCommand: ExecuteCommand): ProjectShotActions {
  return {
    splitShotAtDistance: async (lineId, shotId, distance) =>
      executeCommand(
        'split_shot_at_distance',
        { lineId, shotId, distance },
        { purpose: 'insert station mid-shot', fields: { lineId, shotId } },
      ),
    editShotCredit: async (targets, edit) =>
      executeCommand(
        'edit_shot_credit',
        { targets, edit },
        { purpose: 'edit survey credit', fields: { targets: targets.type } },
      ),
    setStationName: async (stationRef, name) =>
      executeCommand(
        'set_station_name',
        { stationRef, name },
        { purpose: 'rename station', fields: { stationRef } },
      ),
  };
}
