import { useState } from 'react';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import { trackDialogComplete } from '../../utils/telemetry';
import type { StationRef, LineId, ShotId } from '../../types/project';
import type { CommentHolder } from '../../types/commands';
import { useProse } from '../../hooks/useProse';
import type { ProseTokens } from '../../utils/prose';
import { Prose } from '../common/Prose';

interface CommentHolderUI {
  type: 'station' | 'shot';
  stationRef?: StationRef;
  lineId?: LineId;
  shotId?: ShotId;
  comment: string;
}

/**
 * `station Main Line / 17` or `shot Main Line: 4 → 5`, the name as a prose
 * pill (RS-PROSE); null when unknown.
 */
function holderLabel(prose: ProseTokens, h: CommentHolderUI): string | null {
  if (h.type === 'station' && h.stationRef) return `station ${prose.station(h.stationRef, true)}`;
  if (h.type === 'shot' && h.lineId !== undefined && h.shotId !== undefined) {
    const shot = prose.shot(h.lineId, h.shotId, true);
    return shot === null ? null : `shot ${shot}`;
  }
  return null;
}

/** Build a CommentHolder IPC payload from a UI holder */
function toIpcHolder(h: CommentHolderUI): CommentHolder {
  if (h.type === 'station' && h.stationRef) {
    return { type: 'station', station_ref: h.stationRef };
  }
  return { type: 'shot', line_id: h.lineId!, shot_id: h.shotId! };
}

export default function SwapCommentsDialog() {
  const closeDialog = useUIStore((s) => s.closeDialog);
  const dialogData = useUIStore((s) => s.dialogData);
  const clearMoveCommentSource = useUIStore((s) => s.clearMoveCommentSource);
  const [busy, setBusy] = useState(false);
  const prose = useProse();

  const source = dialogData.source as CommentHolderUI | undefined;
  const target = dialogData.target as CommentHolderUI | undefined;

  if (!source || !target) return null;
  const sourceLabel = holderLabel(prose, source);
  const targetLabel = holderLabel(prose, target);

  // Cancel: no backend mutation happened on pickup, just clear UI state
  const handleCancel = () => {
    clearMoveCommentSource();
    closeDialog();
  };

  // Overwrite: atomic move (not swap)
  const handleOverwrite = async () => {
    setBusy(true);
    try {
      await useProjectStore.getState().moveComment(toIpcHolder(source), toIpcHolder(target), false);
      clearMoveCommentSource();
      trackDialogComplete('swap-comments');
      closeDialog();
    } finally {
      setBusy(false);
    }
  };

  // Swap: atomic swap
  const handleSwap = async () => {
    setBusy(true);
    try {
      await useProjectStore.getState().moveComment(toIpcHolder(source), toIpcHolder(target), true);
      clearMoveCommentSource();
      trackDialogComplete('swap-comments');
      closeDialog();
    } finally {
      setBusy(false);
    }
  };

  return (
    <DialogWrapper title="Move comment" onClose={handleCancel} size="md" data-testid="swap-comments-dialog">
      <div className="space-y-4">
        <div>
          <p className="text-text-secondary text-sm mb-1" data-testid="swap-comments-source">
            <Prose text={sourceLabel ? `Comment being moved, from ${sourceLabel}:` : 'Comment being moved:'} />
          </p>
          <blockquote className="border-l-2 border-accent pl-3 text-text-primary italic">
            {source.comment}
          </blockquote>
        </div>
        <div>
          <p className="text-text-secondary text-sm mb-1" data-testid="swap-comments-target">
            <Prose text={targetLabel ? `The ${targetLabel} already has a comment:` : 'Target already has a comment:'} />
          </p>
          <blockquote className="border-l-2 border-warning pl-3 text-text-primary italic">
            {target.comment}
          </blockquote>
        </div>
        <div className="flex justify-end gap-2 pt-4 border-t border-border">
          <Button onClick={handleCancel} disabled={busy} data-testid="swap-comments-cancel">Cancel</Button>
          <Button variant="danger" onClick={handleOverwrite} disabled={busy} data-testid="swap-overwrite">
            Overwrite
          </Button>
          <Button variant="primary" onClick={handleSwap} disabled={busy} data-primary-action="" data-testid="swap-swap">
            Swap
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
