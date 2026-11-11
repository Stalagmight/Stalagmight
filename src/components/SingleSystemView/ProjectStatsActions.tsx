import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';

/** Ordinary local project settings. Remote controls live in the left workspace. */
export function ProjectStatsActions() {
  const { t } = useTranslation();
  const openDialog = useUIStore((s) => s.openDialog);
  return (
    <div className="pt-2 border-t border-border">
      <button onClick={() => openDialog('project-settings')}
        className="w-full text-center text-sm px-2 py-1 rounded bg-background-50 hover:bg-background text-text-primary"
        data-testid="open-project-settings">
        {t('detailPanel.projectSettings.open')}
      </button>
    </div>
  );
}
