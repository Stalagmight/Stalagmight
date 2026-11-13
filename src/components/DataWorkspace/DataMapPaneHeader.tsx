/**
 * Header strip over the Data workspace's map pane: the "Map" title, the follow
 * toggle (the map selects and pans to the active row), "Open in Survey" for
 * the active row, and close. Rendered by SingleSystemView at the top of the
 * pane, outside the workspace's providers, so it reads the stores directly.
 */
import { useTranslation } from 'react-i18next';
import { CloseIcon, ExternalLinkIcon, TargetIcon } from '../common/icons';
import { useDataWorkspaceStore } from './store';
import { openRowInSurvey } from './useDataNavigation';
import { useRowDisplayName } from './useRowDisplayName';

export function DataMapPaneHeader() {
  const { t } = useTranslation();
  const follow = useDataWorkspaceStore((s) => s.mapFollow);
  const activeRowKey = useDataWorkspaceStore((s) => s.activeRowKey);
  const activeName = useRowDisplayName(activeRowKey);
  const shortcut = t('dataWorkspace.shortcuts.openInSurvey');

  return (
    <div
      className="flex h-8 shrink-0 items-center gap-2 border-b border-border bg-panel px-2 text-xs"
      data-testid="data-map-pane-header"
    >
      <span className="font-medium text-text-primary" data-testid="data-map-pane-title">
        {t('dataWorkspace.mapPane.title')}
      </span>
      <button
        type="button"
        aria-pressed={follow}
        className={`flex items-center gap-1 rounded border px-1.5 py-0.5 ${
          follow ? 'border-accent bg-accent-20 text-text-primary' : 'border-border text-text-secondary hover:bg-accent-10'
        }`}
        title={t('dataWorkspace.mapPane.followTitle')}
        onClick={() => useDataWorkspaceStore.getState().setMapFollow(!follow)}
        data-testid="data-map-pane-follow"
      >
        <TargetIcon size={12} />
        {t('dataWorkspace.mapPane.follow')}
      </button>
      <button
        type="button"
        disabled={!activeRowKey}
        className="flex min-w-0 items-center gap-1 rounded border border-border px-1.5 py-0.5 text-text-secondary hover:bg-accent-10 hover:text-text-primary disabled:cursor-not-allowed disabled:opacity-50 disabled:hover:bg-transparent"
        title={t('dataWorkspace.mapPane.openInSurveyTitle', { shortcut })}
        onClick={() => {
          if (activeRowKey) openRowInSurvey(activeRowKey);
        }}
        data-testid="data-map-pane-open-in-survey"
      >
        <ExternalLinkIcon size={12} />
        <span className="shrink-0">{t('dataWorkspace.mapPane.openInSurvey')}</span>
        {activeName && <span className="truncate text-text-secondary">{activeName}</span>}
      </button>
      <button
        type="button"
        className="ml-auto flex h-5 w-5 items-center justify-center rounded text-text-secondary hover:bg-accent-10 hover:text-text-primary"
        aria-label={t('dataWorkspace.mapPane.close')}
        title={t('dataWorkspace.mapPane.close')}
        onClick={() => useDataWorkspaceStore.getState().setMapPaneOpen(false)}
        data-testid="data-map-pane-close"
      >
        <CloseIcon size={14} />
      </button>
    </div>
  );
}
