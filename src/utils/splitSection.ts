import { invokeIpc } from './ipc';
import type { SectionId } from '../types/project';
import type { SplitAt } from '../types/ipc/SplitAt';
import type { SplitPreview } from '../types/ipc/SplitPreview';
import type { SplitSectionOptions } from '../types/ipc/SplitSectionOptions';

/**
 * What Split Section Here would do beyond the cut, without doing it: how many
 * loop closures it would turn into merges (a closure from before the cut to past
 * it, left as the halves' only tie), and which half that moves, how far. Rejects
 * with the reason when the split would be refused.
 */
export function previewSplitSection(
  sectionId: SectionId,
  at: SplitAt,
  options: SplitSectionOptions
): Promise<SplitPreview> {
  return invokeIpc<SplitPreview>('split_section_preview', { sectionId, at, options }, {
    purpose: 'preview what a section split would move',
    fields: { sectionId },
  });
}
