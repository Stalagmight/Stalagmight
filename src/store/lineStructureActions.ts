import type { CommandResult } from '../types/commands';
import type { ClosureStrategy, LineData, LineId, StationRef } from '../types/project';
import type { SplitAt } from '../types/ipc/SplitAt';
import type { SplitLineOptions } from '../types/ipc/SplitLineOptions';
import type { CombineLinesOptions } from '../types/ipc/CombineLinesOptions';
import { notifyRemovedClosures } from '../utils/closureRemovalToast';

/** `projectStore`'s own command runner: invoke, then refresh and mark dirty on success. */
type ExecuteCommand = (
  command: string,
  args: Record<string, unknown> | undefined,
  meta: { purpose: string; fields?: Record<string, unknown> }
) => Promise<CommandResult>;

/**
 * Split Line Here (#335) and Combine Lines: the two edits that turn one
 * line into two and two into one; and Create Line and Merge, a new line
 * joined to the survey at a station. Split out of `projectStore.ts` rather
 * than rebaselined (#157), like `projectConfigActions.ts`; both actions are a
 * single undoable backend command run through the store's `executeCommand`.
 */
export interface LineStructureActions {
  splitLine: (lineId: LineId, at: SplitAt, options: SplitLineOptions) => Promise<CommandResult>;
  /**
   * Combine `other` into `kept`; `kept`'s id, direction and shot ids survive.
   * Toasts the ties it removed because as closures they would join the two
   * ends of one shot (#746), which the dialog listed before the confirm.
   */
  combineLines: (kept: LineId, other: LineId, options: CombineLinesOptions) => Promise<CommandResult>;
  /**
   * Create a line and merge its station `importStationIndex` onto
   * `targetStation` (a New Line placed by snapping onto a station), as ONE
   * undo step. `caveSectionId` puts the new line in that Section (#327) inside
   * the same step.
   */
  createLineAndMerge: (
    data: LineData,
    importStationIndex: number,
    targetStation: StationRef,
    closureStrategy: ClosureStrategy,
    caveSectionId?: number | null
  ) => Promise<CommandResult>;
}

export function createLineStructureActions(executeCommand: ExecuteCommand): LineStructureActions {
  return {
    splitLine: (lineId, at, options) =>
      executeCommand('split_line', { lineId, at, options }, { purpose: 'split line', fields: { lineId } }),
    combineLines: async (kept, other, options) =>
      notifyRemovedClosures(
        await executeCommand(
          'combine_lines',
          { kept, other, options },
          { purpose: 'combine lines', fields: { kept, other } }
        )
      ),
    createLineAndMerge: (data, importStationIndex, targetStation, closureStrategy, caveSectionId) =>
      executeCommand(
        'create_line_and_merge',
        {
          name: data.name,
          surveyor: data.surveyor,
          date: data.date,
          color: data.color,
          direction: data.direction,
          quality: data.quality,
          shots: data.shots,
          importStationIndex,
          targetStation,
          closureStrategy,
          caveSectionId: caveSectionId ?? null, // the New Line dialog's target Section (#327)
        },
        {
          purpose: 'add line and merge stations',
          fields: { name: data.name ?? 'unnamed', shot_count: data.shots?.length ?? 0 },
        }
      ),
  };
}
