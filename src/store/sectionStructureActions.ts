import type { CommandResult } from '../types/commands';
import type { SectionId } from '../types/project';
import type { SplitAt } from '../types/ipc/SplitAt';
import type { SplitSectionOptions } from '../types/ipc/SplitSectionOptions';
import type { CombineSectionsOptions } from '../types/ipc/CombineSectionsOptions';

/** `projectStore`'s own command runner: invoke, then refresh and mark dirty on success. */
type ExecuteCommand = (
  command: string,
  args: Record<string, unknown> | undefined,
  meta: { purpose: string; fields?: Record<string, unknown> }
) => Promise<CommandResult>;

/**
 * Split Section Here (#335) and Combine Sections: the two edits that turn one
 * section into two and two into one. Split out of `projectStore.ts` rather
 * than rebaselined (#157), like `projectConfigActions.ts`; both actions are a
 * single undoable backend command run through the store's `executeCommand`.
 */
export interface SectionStructureActions {
  splitSection: (sectionId: SectionId, at: SplitAt, options: SplitSectionOptions) => Promise<CommandResult>;
  /** Combine `other` into `kept`; `kept`'s id, direction and shot ids survive. */
  combineSections: (kept: SectionId, other: SectionId, options: CombineSectionsOptions) => Promise<CommandResult>;
}

export function createSectionStructureActions(executeCommand: ExecuteCommand): SectionStructureActions {
  return {
    splitSection: (sectionId, at, options) =>
      executeCommand('split_section', { sectionId, at, options }, { purpose: 'split section', fields: { sectionId } }),
    combineSections: (kept, other, options) =>
      executeCommand('combine_sections', { kept, other, options }, { purpose: 'combine sections', fields: { kept, other } }),
  };
}
