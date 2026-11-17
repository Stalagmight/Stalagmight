import type { CommandResult } from '../types/commands';
import type { LineId } from '../types/project';
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
 * line into two and two into one. Split out of `projectStore.ts` rather
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
  };
}
