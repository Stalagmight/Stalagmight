/**
 * Floating "← Back to Data · <row>" chip shown in Survey after "Open in
 * Survey" from the Data workspace (store.returnToData). Rendered by
 * SingleSystemView over the map, top centre (clear of the toolbar, the zoom
 * control and the bottom-edge attribution); `className` overrides the slot.
 *
 * Clicking it returns to Data focused on that row. It clears itself when the
 * user leaves Survey any other way, so it never comes back stale.
 */
import { useEffect } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { ChevronLeftIcon, CloseIcon } from '../common/icons';
import { useDataWorkspaceStore } from './store';
import { useRowDisplayName } from './useRowDisplayName';

const SLOT = 'pointer-events-auto absolute left-1/2 top-2 z-20 -translate-x-1/2';

export function BackToDataChip({ className = SLOT }: { className?: string }) {
  const { t } = useTranslation();
  const ret = useDataWorkspaceStore((s) => s.returnToData);
  const appMode = useUIStore((s) => s.appMode);
  const name = useRowDisplayName(ret?.rowKey ?? null);

  // Leaving Survey by any other route (the mode switcher, a project switch)
  // drops the chip: the way back it offers is gone.
  useEffect(
    () =>
      useUIStore.subscribe((s, prev) => {
        if (prev.appMode === 'survey' && s.appMode !== 'survey' && useDataWorkspaceStore.getState().returnToData) {
          useDataWorkspaceStore.getState().setReturnToData(null);
        }
      }),
    [],
  );

  if (!ret || appMode !== 'survey') return null;

  const back = () => {
    const store = useDataWorkspaceStore.getState();
    store.requestFocus({ rowKey: ret.rowKey, align: 'center' });
    store.setReturnToData(null);
    useUIStore.getState().setAppMode('data');
  };

  return (
    <div
      className={`${className} flex max-w-[min(28rem,90%)] items-stretch overflow-hidden rounded-full border border-accent bg-panel text-xs shadow-lg`}
      data-testid="back-to-data-chip"
    >
      <button
        type="button"
        className="flex min-w-0 items-center gap-1 py-1 pl-2.5 pr-2 font-medium text-accent hover:bg-accent-10"
        title={name ? t('dataWorkspace.backToData.title', { name }) : t('dataWorkspace.backToData.label')}
        onClick={back}
        data-testid="back-to-data"
      >
        <ChevronLeftIcon size={12} />
        <span className="shrink-0">{t('dataWorkspace.backToData.label')}</span>
        {name && <span className="truncate font-normal text-text-secondary">{`· ${name}`}</span>}
      </button>
      <button
        type="button"
        className="flex items-center border-l border-border px-1.5 text-text-secondary hover:bg-accent-10 hover:text-text-primary"
        aria-label={t('dataWorkspace.backToData.dismiss')}
        title={t('dataWorkspace.backToData.dismiss')}
        onClick={() => useDataWorkspaceStore.getState().setReturnToData(null)}
        data-testid="back-to-data-dismiss"
      >
        <CloseIcon size={12} />
      </button>
    </div>
  );
}
