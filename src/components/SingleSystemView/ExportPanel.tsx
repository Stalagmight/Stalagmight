import { useTranslation } from 'react-i18next';
import ExportControls from '../Dialogs/ExportControls';
import { useExportStore } from '../../store/exportStore';
import { useUIStore } from '../../store/uiStore';
import { IconButton } from '../common/Button';
import { CloseIcon } from '../common/icons';

/**
 * Export workspace side panel (#38 / #239). Hosts the shared {@link ExportControls}
 * backed by the {@link useExportStore}, so the large WYSIWYG preview in the map
 * area mirrors these options live. Export is entered from the project menu or
 * Ctrl+E; the header's close button (like the toolbar's "Close export") returns
 * to the mode it was entered from.
 */
export function ExportPanel() {
  const { t } = useTranslation();
  const exportType = useExportStore((s) => s.exportType);
  const setExportType = useExportStore((s) => s.setExportType);
  const dataFormat = useExportStore((s) => s.dataFormat);
  const setDataFormat = useExportStore((s) => s.setDataFormat);
  const imageOptions = useExportStore((s) => s.imageOptions);
  const setImageOptions = useExportStore((s) => s.setImageOptions);
  const closeExport = useUIStore((s) => s.closeExport);
  // No way out while an export is being written (closeExport refuses too).
  const exporting = useExportStore((s) => s.exporting);

  return (
    <div
      className="h-full bg-panel flex flex-col overflow-auto text-sm text-text-primary"
      data-testid="export-panel"
    >
      <div className="flex items-center justify-between gap-2 px-3 py-2 border-b border-border shrink-0">
        <h2 className="font-semibold">{t('exportPanel.title')}</h2>
        <IconButton
          type="button"
          size="sm"
          variant="ghost"
          icon={<CloseIcon size={14} />}
          onClick={closeExport}
          disabled={exporting}
          aria-label={t('exportPanel.close')}
          tooltip={exporting ? t('exportPanel.inProgress') : t('exportPanel.closeHint')}
          className="text-text-secondary hover:text-text-primary"
          data-testid="export-panel-close"
        />
      </div>
      <div className="p-3">
        <ExportControls
          exportType={exportType}
          onExportTypeChange={setExportType}
          dataFormat={dataFormat}
          onDataFormatChange={setDataFormat}
          imageOptions={imageOptions}
          onImageOptionsChange={setImageOptions}
        />
      </div>
    </div>
  );
}
