import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import { trackDialogComplete } from '../../utils/telemetry';
import type { StationRef, SectionId, ShotId } from '../../types/project';
import type { CommentHolder } from '../../types/commands';

interface CommentHolderUI {
  type: 'station' | 'shot';
  stationRef?: StationRef;
  sectionId?: SectionId;
  shotId?: ShotId;
  comment: string;
}

/** Build a CommentHolder IPC payload from a UI holder */
function toIpcHolder(h: CommentHolderUI): CommentHolder {
  if (h.type === 'station' && h.stationRef) {
    return { type: 'station', station_ref: h.stationRef };
  }
  return { type: 'shot', section_id: h.sectionId!, shot_id: h.shotId! };
}

export default function SwapCommentsDialog() {
  const closeDialog = useUIStore((s) => s.closeDialog);
  const dialogData = useUIStore((s) => s.dialogData);
  const clearMoveCommentSource = useUIStore((s) => s.clearMoveCommentSource);
  const [busy, setBusy] = useState(false);
  const { t } = useTranslation();

  const source = dialogData.source as CommentHolderUI | undefined;
  const target = dialogData.target as CommentHolderUI | undefined;

  if (!source || !target) return null;

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
    <DialogWrapper title={t('dialogs.swapComments.title')} onClose={handleCancel} size="md" data-testid="swap-comments-dialog">
      <div className="space-y-4">
        <div>
          <p className="text-text-secondary text-sm mb-1">{t('dialogs.swapComments.beingMoved')}</p>
          <blockquote className="border-l-2 border-accent pl-3 text-text-primary italic">
            {source.comment}
          </blockquote>
        </div>
        <div>
          <p className="text-text-secondary text-sm mb-1">{t('dialogs.swapComments.targetHas')}</p>
          <blockquote className="border-l-2 border-warning pl-3 text-text-primary italic">
            {target.comment}
          </blockquote>
        </div>
        <div className="flex justify-end gap-2 pt-4 border-t border-border">
          <Button onClick={handleCancel} disabled={busy} data-testid="swap-comments-cancel">{t('common.actions.cancel')}</Button>
          <Button variant="danger" onClick={handleOverwrite} disabled={busy} data-testid="swap-overwrite">
            {t('dialogs.swapComments.overwrite')}
          </Button>
          <Button variant="primary" onClick={handleSwap} disabled={busy} data-primary-action="" data-testid="swap-swap">
            {t('dialogs.swapComments.swap')}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
