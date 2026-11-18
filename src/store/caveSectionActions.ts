import type { CommandResult } from '../types/commands';
import type { LineId } from '../types/project';

/** `projectStore`'s own command runner: invoke, then refresh and mark dirty on success. */
type ExecuteCommand = (
  command: string,
  args: Record<string, unknown> | undefined,
  meta: { purpose: string; fields?: Record<string, unknown> }
) => Promise<CommandResult>;

/** A Section's id (`Section.id`; `cave_section_id` on a line). */
export type CaveSectionId = number;

/**
 * Sections (#327): named, coloured groupings of lines. Every action is one
 * undoable backend command run through the store's `executeCommand`, so a
 * success refreshes the project and marks it dirty, and a refusal (a taken
 * name, a read-only project) is toasted there. Split out of `projectStore.ts`
 * like `lineStructureActions.ts`, so the frozen store file does not grow.
 *
 * The Section id travels as `caveSectionId`: `sectionId` / `section_id` is
 * the persisted name of a LINE id (docs/saving.md, "Persisted names").
 */
export interface CaveSectionActions {
  /** Create a Section; `lineIds` (optional) move into it in the same step.
   *  The result's `cave_section_id` is the new Section's id. */
  createCaveSection: (name: string, color: string, lineIds?: readonly LineId[]) => Promise<CommandResult>;
  /** Rename and/or recolour a Section; an absent field is kept. */
  updateCaveSection: (id: CaveSectionId, updates: { name?: string; color?: string }) => Promise<CommandResult>;
  /** Delete a Section; its lines become ungrouped (undo puts both back). */
  deleteCaveSection: (id: CaveSectionId) => Promise<CommandResult>;
  /** Move lines into a Section, or out of any with `null`. */
  moveLinesToCaveSection: (lineIds: readonly LineId[], id: CaveSectionId | null) => Promise<CommandResult>;
  /** Give every line of the Section the Section's colour, as one undo step. */
  applyCaveSectionColor: (id: CaveSectionId) => Promise<CommandResult>;
}

export function createCaveSectionActions(executeCommand: ExecuteCommand): CaveSectionActions {
  return {
    createCaveSection: (name, color, lineIds) =>
      executeCommand(
        'create_cave_section',
        { name, color, lineIds: lineIds ? [...lineIds] : [] },
        { purpose: 'create section', fields: { line_count: lineIds?.length ?? 0 } }
      ),
    updateCaveSection: (caveSectionId, updates) =>
      executeCommand(
        'update_cave_section',
        { caveSectionId, ...updates },
        { purpose: 'update section', fields: { caveSectionId } }
      ),
    deleteCaveSection: (caveSectionId) =>
      executeCommand(
        'delete_cave_section',
        { caveSectionId },
        { purpose: 'delete section', fields: { caveSectionId } }
      ),
    moveLinesToCaveSection: (lineIds, caveSectionId) =>
      executeCommand(
        'move_lines_to_cave_section',
        { lineIds: [...lineIds], caveSectionId },
        { purpose: 'move lines to section', fields: { line_count: lineIds.length, caveSectionId } }
      ),
    applyCaveSectionColor: (caveSectionId) =>
      executeCommand(
        'apply_cave_section_color',
        { caveSectionId },
        { purpose: 'apply section colour to lines', fields: { caveSectionId } }
      ),
  };
}
