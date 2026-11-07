import { useEffect, useRef, useCallback, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { CheckIcon, CloseIcon } from '../common/icons';
import type { CommandOutcome } from '../../store/commandResult';
import type { EditOutcome } from '../../types/ipc/EditOutcome';
import { useShallow } from 'zustand/react/shallow';

const OVERLAY_WIDTH = 220;
const OVERLAY_MARGIN = 8;

export default function CommentOverlay() {
  const { t } = useTranslation();
  const commentTarget = useUIStore((s) => s.commentTarget);
  const clearCommentTarget = useUIStore((s) => s.clearCommentTarget);
  const { updateStationComment, updateShot } = useProjectStore(
    useShallow((s) => ({ updateStationComment: s.updateStationComment, updateShot: s.updateShot })),
  );

  const [value, setValue] = useState('');
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const overlayRef = useRef<HTMLDivElement>(null);
  // Track whether the user actually changed the comment
  const initialValueRef = useRef('');

  // Sync value when target changes
  useEffect(() => {
    if (commentTarget) {
      setValue(commentTarget.currentComment);
      initialValueRef.current = commentTarget.currentComment;
      // Auto-focus textarea
      requestAnimationFrame(() => {
        textareaRef.current?.focus();
        textareaRef.current?.select();
      });
    }
  }, [commentTarget]);

  const commit = useCallback(async () => {
    if (!commentTarget) return;
    const trimmed = value.trim();
    // Only save if value actually changed
    if (trimmed === initialValueRef.current.trim()) {
      clearCommentTarget();
      return;
    }
    let saved: CommandOutcome<EditOutcome> | undefined;
    if (commentTarget.type === 'station' && commentTarget.stationRef) {
      saved = await updateStationComment(commentTarget.stationRef, trimmed || null);
    } else if (commentTarget.type === 'shot' && commentTarget.sectionId !== undefined && commentTarget.shotId !== undefined) {
      // Use "" not null for clearing: JSON null for Option<Option<String>> deserializes as outer None (= no update)
      saved = await updateShot(commentTarget.sectionId, commentTarget.shotId, { comment: trimmed || "" });
    }
    // A refused save keeps the overlay open with the typed text (DRAFT-1, #576):
    // the store's failure notice is the feedback, and Enter or the check button
    // retries. Escape still discards.
    if (saved && !saved.success) return;
    clearCommentTarget();
  }, [commentTarget, value, clearCommentTarget, updateStationComment, updateShot]);

  const cancel = useCallback(() => {
    clearCommentTarget();
  }, [clearCommentTarget]);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        e.stopPropagation();
        cancel();
      } else if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault();
        commit();
      }
    },
    [cancel, commit]
  );

  // Close on outside click
  useEffect(() => {
    if (!commentTarget) return;

    const handlePointerDown = (e: PointerEvent) => {
      if (overlayRef.current && !overlayRef.current.contains(e.target as Node)) {
        commit();
      }
    };

    // Delay to avoid the click that opened the overlay from closing it
    const timer = setTimeout(() => {
      document.addEventListener('pointerdown', handlePointerDown);
    }, 0);

    return () => {
      clearTimeout(timer);
      document.removeEventListener('pointerdown', handlePointerDown);
    };
  }, [commentTarget, commit]);

  if (!commentTarget) return null;

  // Compute position: place below click point, clamp to viewport
  const { x, y } = commentTarget.screenPos;
  const viewportWidth = window.innerWidth;
  const viewportHeight = window.innerHeight;

  let left = x - OVERLAY_WIDTH / 2;
  let top = y + 16; // 16px below click

  // Clamp horizontal
  if (left < OVERLAY_MARGIN) left = OVERLAY_MARGIN;
  if (left + OVERLAY_WIDTH > viewportWidth - OVERLAY_MARGIN) {
    left = viewportWidth - OVERLAY_MARGIN - OVERLAY_WIDTH;
  }

  // If too close to bottom, flip above
  const estimatedHeight = 110;
  if (top + estimatedHeight > viewportHeight - OVERLAY_MARGIN) {
    top = y - estimatedHeight - 8;
  }

  return (
    <div
      ref={overlayRef}
      className="fixed z-50 bg-panel border border-border rounded shadow-lg p-2"
      style={{ left, top, width: OVERLAY_WIDTH }}
      data-testid="comment-overlay"
    >
      <textarea
        ref={textareaRef}
        className="w-full bg-background border border-border rounded px-2 py-1 text-sm resize-none focus:outline-none focus-visible:border-accent"
        rows={3}
        value={value}
        onChange={(e) => setValue(e.target.value)}
        onKeyDown={handleKeyDown}
        placeholder={t('commentOverlay.placeholder')}
        data-testid="comment-overlay-textarea"
      />
      <div className="flex justify-end gap-1 mt-1">
        <button
          className="p-1 rounded hover:bg-background text-text-secondary hover:text-text-primary"
          onClick={cancel}
          title={t('commentOverlay.cancel')}
          data-testid="comment-overlay-cancel"
        >
          <CloseIcon size={14} />
        </button>
        <button
          className="p-1 rounded hover:bg-background text-accent hover:text-accent"
          onClick={commit}
          title={t('commentOverlay.commit')}
          data-testid="comment-overlay-commit"
        >
          <CheckIcon size={14} />
        </button>
      </div>
    </div>
  );
}
