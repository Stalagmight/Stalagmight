import { invokeIpc } from './ipc';
import type { LineId } from '../types/project';
import type { SplitAt } from '../types/ipc/SplitAt';
import type { SplitPreview } from '../types/ipc/SplitPreview';
import type { SplitLineOptions } from '../types/ipc/SplitLineOptions';

/**
 * What Split Line Here would do beyond the cut, without doing it: how many
 * loop closures it would turn into merges (a closure from before the cut to past
 * it, left as the halves' only tie), and which half that moves, how far. Rejects
 * with the reason when the split would be refused.
 */
export function previewSplitLine(
  lineId: LineId,
  at: SplitAt,
  options: SplitLineOptions
): Promise<SplitPreview> {
  return invokeIpc<SplitPreview>('split_line_preview', { lineId, at, options }, {
    purpose: 'preview what a line split would move',
    fields: { lineId },
  });
}
