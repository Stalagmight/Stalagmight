import { useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { useDismissableLayer } from '../../hooks/useDismissableLayer';
import type { SpeleoCachedProject, SpeleoProjectSummary } from '../../types/ipc.generated';

/** What a right-click on a SpeleoDB card, row or map marker is about. */
export interface SpeleoMenuTarget {
  x: number;
  y: number;
  summary: SpeleoProjectSummary;
  /** This computer's cached copy, when the session may see one. */
  cached?: SpeleoCachedProject | undefined;
  /** The recents path that puts the project on the map, for "Zoom to system". */
  mapPath?: string | null | undefined;
}

const ITEM_CLS = 'w-full px-4 py-2 text-left text-sm hover:bg-background-50 flex items-center gap-2';

/**
 * The context menu of a SpeleoDB project on the home screen. A SpeleoDB project
 * has no local file actions (no Show in folder, no rename, no local delete): what
 * it offers is opening it, finding it on the map, and removing the copy this
 * computer downloaded - which leaves the project on SpeleoDB.
 */
export function SpeleoProjectContextMenu({
  target,
  onClose,
  onOpen,
  onZoom,
  onRemoveCopy,
}: {
  target: SpeleoMenuTarget;
  onClose: () => void;
  onOpen: (summary: SpeleoProjectSummary) => void;
  onZoom: (path: string) => void;
  onRemoveCopy: (copy: SpeleoCachedProject) => void;
}) {
  const { t } = useTranslation();
  const ref = useRef<HTMLDivElement>(null);
  useDismissableLayer({ open: true, onDismiss: onClose, contains: [ref] });
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return;
      e.stopPropagation();
      onClose();
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, [onClose]);

  const { summary, cached, mapPath } = target;
  return (
    <div
      ref={ref}
      className="fixed z-50 bg-panel border border-border rounded-lg shadow-xl py-1 min-w-[180px]"
      style={{ left: target.x, top: target.y }}
      role="menu"
      aria-label={t('speleodb.cachedCopies.menu.aria')}
      data-testid="speleodb-project-context-menu"
    >
      <button
        type="button"
        className={ITEM_CLS}
        role="menuitem"
        data-testid="speleodb-context-menu-open"
        onClick={() => { onClose(); onOpen(summary); }}
      >
        {t('speleodb.cachedCopies.menu.open')}
      </button>
      {mapPath && (
        <button
          type="button"
          className={ITEM_CLS}
          role="menuitem"
          data-testid="speleodb-context-menu-zoom"
          onClick={() => { onClose(); onZoom(mapPath); }}
        >
          {t('speleodb.cachedCopies.menu.zoom')}
        </button>
      )}
      {cached && (
        <>
          <div className="border-t border-border my-1" />
          <button
            type="button"
            className={`${ITEM_CLS} text-error`}
            role="menuitem"
            data-testid="speleodb-context-menu-remove-copy"
            onClick={() => { onClose(); onRemoveCopy(cached); }}
          >
            {t('speleodb.cachedCopies.menu.removeCopy')}
          </button>
        </>
      )}
    </div>
  );
}
