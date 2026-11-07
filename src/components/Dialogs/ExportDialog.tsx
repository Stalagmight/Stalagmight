import { useState, useCallback, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { DialogWrapper } from './DialogWrapper';
import ExportControls from './ExportControls';
import { DEFAULT_IMAGE_OPTIONS } from '../../store/exportStore';
import type { ExportType, DataFormat } from '../../store/exportStore';
import type { ExportImageOptions } from '../../types/commands';

/**
 * Modal Export dialog (reached from the project menu / Ctrl+E). A thin
 * `DialogWrapper` around the shared {@link ExportControls}. Keeps its OWN local
 * state - defaulting to the data/CSV selection its e2e suite asserts - rather
 * than the Export-mode `exportStore` (that store is panel + center only).
 */
export default function ExportDialog() {
  const { t } = useTranslation();
  const closeDialog = useUIStore((s) => s.closeDialog);
  const [exportType, setExportType] = useState<ExportType>('data');
  const [dataFormat, setDataFormat] = useState<DataFormat>('csv');
  const [imageOptions, setImageOptions] = useState<ExportImageOptions>(DEFAULT_IMAGE_OPTIONS);

  // Read synchronously in the close-guard (not React state) so a close attempt
  // the instant an export starts can't slip through a not-yet-committed render.
  // `ExportControls` calls onExportingChange in the same tick it flips its own
  // in-flight flag (Copilot review).
  const isExportingRef = useRef(false);

  // No-op while exporting so the dialog can't be dismissed mid-export.
  const handleClose = useCallback(() => {
    if (!isExportingRef.current) {
      closeDialog();
    }
  }, [closeDialog]);

  return (
    <DialogWrapper title={t('dialogs.export.title')} onClose={handleClose} size="lg" data-testid="export-dialog">
      <ExportControls
        variant="dialog"
        exportType={exportType}
        onExportTypeChange={setExportType}
        dataFormat={dataFormat}
        onDataFormatChange={setDataFormat}
        imageOptions={imageOptions}
        onImageOptionsChange={setImageOptions}
        onComplete={closeDialog}
        onExportingChange={(v) => {
          isExportingRef.current = v;
        }}
      />
    </DialogWrapper>
  );
}
