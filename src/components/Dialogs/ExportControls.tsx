import { useState, useEffect, useRef, useCallback, useId } from 'react';
import { useTranslation } from 'react-i18next';
import { listen } from '@tauri-apps/api/event';
import { saveDialog } from '../../utils/fileDialogs';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import { TOAST_KEYS } from '../../utils/toastKeys';
import { getErrorMessage } from '../../utils/ipc';
import { Button } from '../common/Button';
import { Select } from '../common/Select';
import { AlertDialog } from './AlertDialog';
import { LoadingProgress } from '../LoadingProgress/LoadingProgress';
import type { ExportImageOptions, ImageFormat, ColorMode } from '../../types/commands';
import type { LoadingProgressPayload } from '../../types/loading';
import type { ExportType, DataFormat } from '../../store/exportStore';
import { ensureExtension, exportSaveFilters } from '../../utils/fileDialogs';
import { ChevronRightIcon } from '../common/icons';
import { setStationLabelOption } from '../../utils/exportOptions';
import { hasStationNames } from '../../types/survey-data';
import { ExportColorScaleControls } from './ExportColorScaleControls';

/** Phase label mapping for export phases (matches backend LoadingPhase) */
const EXPORT_PHASE_LABELS: Record<string, string> = {
  export_preparing: 'Preparing export...',
  export_generating: 'Generating...',
  export_writing: 'Writing file...',
  export_complete: 'Export complete',
};

/** Branching-threshold slider bounds. Matches backend's u32 field. */
const THRESHOLD_MIN = 0;
const THRESHOLD_MAX = 10;

export interface ExportControlsProps {
  exportType: ExportType;
  onExportTypeChange: (t: ExportType) => void;
  dataFormat: DataFormat;
  onDataFormatChange: (f: DataFormat) => void;
  imageOptions: ExportImageOptions;
  onImageOptionsChange: (o: ExportImageOptions) => void;
}

/**
 * The Export form + export flow of the Export workspace panel (`ExportPanel`).
 * Controlled: the type/format/image options are owned by the host
 * (`exportStore`), so the panel shares them with the large map-area preview.
 */
export default function ExportControls({
  exportType,
  onExportTypeChange,
  dataFormat,
  onDataFormatChange,
  imageOptions,
  onImageOptionsChange,
}: ExportControlsProps) {
  const { project, exportCsv, exportJson, exportTml, exportImage, getAnchoredSectionIds } =
    useProjectStore();
  const showToast = useUIStore((s) => s.showToast);
  const { t } = useTranslation();
  const hasNames = useProjectStore((s) => hasStationNames(s.surveyData));
  const stationNamesUnavailable = !hasNames && !imageOptions.show_station_names;

  const [isExporting, setIsExporting] = useState(false);
  const [labelsOptionsExpanded, setLabelsOptionsExpanded] = useState(false);
  // Unique per-instance id, so a second mounted instance (tests, a future
  // surface) can never duplicate it in the DOM and break the ARIA link.
  const labelsRegionId = useId();
  const [exportProgress, setExportProgress] = useState<LoadingProgressPayload | null>(null);
  const [showTmlUnanchoredWarning, setShowTmlUnanchoredWarning] = useState(false);
  const [tmlUnanchoredSections, setTmlUnanchoredSections] = useState<
    Array<{ id: number; name: string | null }>
  >([]);
  // Whether NO section in the project is connected to a zero datum (every
  // section will export as its own separate entrance) vs. only some are
  // (#411). Compared by section count, not `zero_datums.length`: a datum
  // whose station no longer resolves also leaves every section unanchored.
  const [tmlAllSectionsUnanchored, setTmlAllSectionsUnanchored] = useState(false);
  const [tmlTotalSectionCount, setTmlTotalSectionCount] = useState(0);
  // Track the loadingId we're expecting (to filter out events from other operations)
  const exportLoadingIdRef = useRef<bigint | null>(null);

  // Track if component is mounted to prevent setState on unmounted component
  const isMountedRef = useRef(true);
  useEffect(() => {
    isMountedRef.current = true;
    return () => {
      isMountedRef.current = false;
    };
  }, []);

  const formatSectionLabel = useCallback(
    (section: { id: number; name: string | null }): string => {
      const trimmedName = section.name?.trim();
      return trimmedName && trimmedName.length > 0
        ? `${trimmedName} (#${section.id})`
        : t('dialogs.tmlUnanchoredWarning.sectionFallbackName', { id: section.id });
    },
    [t],
  );

  const handleExport = async (options?: { skipTmlAnchorCheck?: boolean }) => {
    // A cancelled OS save picker writes nothing and leaves the panel and its
    // options exactly as they were.
    try {
      if (exportType === 'data') {
        const isTml = dataFormat === 'tml';
        if (isTml && !options?.skipTmlAnchorCheck) {
          try {
            if (project && project.sections.length > 0) {
              const anchoredSectionIds = new Set(await getAnchoredSectionIds());
              const unanchoredSections = project.sections
                .filter((section) => !anchoredSectionIds.has(section.id))
                .map((section) => ({ id: section.id, name: section.name }));
              if (unanchoredSections.length > 0) {
                setTmlUnanchoredSections(unanchoredSections);
                setTmlAllSectionsUnanchored(unanchoredSections.length === project.sections.length);
                setTmlTotalSectionCount(project.sections.length);
                setShowTmlUnanchoredWarning(true);
                return;
              }
            }
          } catch (error) {
            // Deliberately LOG-ONLY, and deliberately not the failure toast below.
            //
            // This check is informational and fails OPEN: the export continues
            // and produces exactly the file the user asked for, so an `error`
            // toast here would report a failure that did not happen
            // (docs/notifications.md: pick the severity honestly, one event one
            // surface). What is actually lost is a notice the user could have
            // dismissed and continued past anyway, which lands on "user-relevant
            // at all? no -> log only" in that document's decision tree.
            //
            // Contrast MapCanvas's `ANCHORED_IDS_ERROR` toast on the SAME IPC:
            // there the failure fails CLOSED and blocks Move Segment, so it
            // does change what the user can do, and it says so.
            console.error(
              'Failed to check zero-datum coverage before TML export; ' +
                'skipping the unanchored-sections notice and exporting anyway:',
              error,
            );
          }
        }

        const filters = isTml
          ? [
              { name: 'TML (ZIP)', extensions: ['tml'] },
              { name: 'TMLU (XML)', extensions: ['tmlu'] },
            ]
          : exportSaveFilters(dataFormat.toUpperCase(), dataFormat);
        const selected = await saveDialog(
          { filters },
          options?.skipTmlAnchorCheck ? 'tml-unanchored-warning-continue' : 'export-button',
        );

        if (selected) {
          // For TML, preserve the extension the OS dialog appended.
          // If the path already has .tmlu or .tml, keep it as-is;
          // otherwise default to .tml.
          let finalPath: string;
          if (isTml) {
            const lower = selected.toLowerCase();
            if (lower.endsWith('.tmlu')) {
              finalPath = selected;
            } else if (lower.endsWith('.tml')) {
              finalPath = selected;
            } else {
              finalPath = ensureExtension(selected, 'tml');
            }
          } else {
            finalPath = ensureExtension(selected, dataFormat);
          }
          // Disable button during data export (no progress events for CSV/JSON/TML)
          setIsExporting(true);
          try {
            if (dataFormat === 'csv') {
              await exportCsv(finalPath);
            } else if (dataFormat === 'json') {
              await exportJson(finalPath);
            } else {
              await exportTml(finalPath);
            }
          } finally {
            if (isMountedRef.current) {
              setIsExporting(false);
            }
          }
        }
      } else {
        const extension = imageOptions.format;
        const selected = await saveDialog(
          { filters: exportSaveFilters(imageOptions.format.toUpperCase(), extension) },
          'export-button',
        );

        if (selected) {
          const finalPath = ensureExtension(selected, extension);
          // Register listener BEFORE starting export to avoid race condition with early events
          // Uses shared loading-progress events for consistency with LoadingProgress infrastructure
          const unlisten = await listen<LoadingProgressPayload>('loading-progress', (event) => {
            if (isMountedRef.current) {
              // Only process export-related events (filter by loadingId once we know it,
              // or accept all export_* phase events if loadingId not yet known)
              const phase = event.payload.phase;
              const isExportPhase = phase.startsWith('export_');

              if (isExportPhase) {
                const eventLoadingId = event.payload.loadingId;
                // Accept this event and track its loadingId for future filtering
                if (exportLoadingIdRef.current === null) {
                  exportLoadingIdRef.current = eventLoadingId;
                }

                // Only process events matching our export's loadingId
                if (eventLoadingId === exportLoadingIdRef.current) {
                  setExportProgress(event.payload);
                }
              }
            }
          });

          setIsExporting(true);
          setExportProgress(null);
          exportLoadingIdRef.current = null;

          try {
            await exportImage(finalPath, imageOptions);
          } finally {
            // Clean up listener and reset state only if still mounted
            unlisten();
            exportLoadingIdRef.current = null;
            if (isMountedRef.current) {
              setIsExporting(false);
              setExportProgress(null);
            }
          }
        }
      }
    } catch (e) {
      console.error('Export failed:', e);
      // A failed export used to be console-only, so the panel did nothing at
      // all - indistinguishable from "the file was written". Every reachable
      // refusal arrives here: the
      // backend's "Cannot export over the open project file", `export_png`'s
      // DPI/size bails, a `Tree::from_str` parse failure, and any I/O error.
      //
      // Toast rather than a dialog, per docs/notifications.md's tree: the
      // condition is not ongoing, nothing must be decided before continuing,
      // and the outcome is invisible where the user is looking. `error` kind,
      // because it failed and will not self-heal. The backend message is
      // preferred over the generic string for the same reason ProfileChart
      // prefers it - it names which of those refusals happened.
      showToast(getErrorMessage(e, t('dialogs.export.failed')), TOAST_KEYS.EXPORT_FAILED, 'error');
      if (isMountedRef.current) {
        setIsExporting(false);
        setExportProgress(null);
      }
    }
  };

  const svgSelected = imageOptions.format === 'svg';

  return (
    <>
      <div className="flex flex-col gap-4">
        {/* Options (the large preview lives in the Export workspace's map area) */}
        <div className="flex-1 space-y-4">
          {/* Export type */}
          <div>
            <label className="block text-sm text-text-secondary mb-2">Export Type</label>
            <div className="flex gap-4">
              <label className="flex items-center gap-2">
                <input
                  type="radio"
                  checked={exportType === 'data'}
                  onChange={() => onExportTypeChange('data')}
                  data-testid="export-type-data"
                />
                <span>Survey Data</span>
              </label>
              <label className="flex items-center gap-2">
                <input
                  type="radio"
                  checked={exportType === 'image'}
                  onChange={() => onExportTypeChange('image')}
                  data-testid="export-type-image"
                />
                <span>Map Image</span>
              </label>
            </div>
          </div>

          {/* Data format options */}
          {exportType === 'data' && (
            <div>
              <label className="block text-sm text-text-secondary mb-2">Format</label>
              <Select
                value={dataFormat}
                onChange={(v) => onDataFormatChange(v as DataFormat)}
                options={[
                  { value: 'csv', label: 'CSV' },
                  { value: 'json', label: 'JSON' },
                  { value: 'tml', label: 'TML (Ariane)' },
                ]}
                data-testid="data-format-select"
              />
            </div>
          )}

          {/* Image format options */}
          {exportType === 'image' && (
            <>
              <div>
                <label className="block text-sm text-text-secondary mb-2">Format</label>
                <Select
                  value={imageOptions.format}
                  onChange={(v) =>
                    onImageOptionsChange({ ...imageOptions, format: v as ImageFormat })
                  }
                  options={[
                    { value: 'png', label: 'PNG' },
                    { value: 'svg', label: 'SVG' },
                    { value: 'pdf', label: 'PDF' },
                  ]}
                  data-testid="image-format-select"
                />
              </div>

              {imageOptions.format === 'png' && (
                <div>
                  <label className="block text-sm text-text-secondary mb-2">Resolution</label>
                  <Select
                    value={String(imageOptions.dpi)}
                    onChange={(v) => onImageOptionsChange({ ...imageOptions, dpi: parseInt(v) })}
                    options={[
                      { value: '72', label: 'Low (72 DPI)' },
                      { value: '150', label: 'Medium (150 DPI)' },
                      { value: '300', label: 'High (300 DPI)' },
                      { value: '600', label: 'Print (600 DPI)' },
                    ]}
                    data-testid="resolution-select"
                  />
                </div>
              )}

              {/* Branching threshold slider: segments whose branching factor is
                  >= threshold are drawn thick; the rest are drawn thin. */}
              <div>
                <label
                  htmlFor="branching-threshold"
                  className="block text-sm text-text-secondary mb-2"
                >
                  Line-weight threshold
                </label>
                <div className="flex items-center gap-3">
                  <input
                    id="branching-threshold"
                    type="range"
                    min={THRESHOLD_MIN}
                    max={THRESHOLD_MAX}
                    step={1}
                    value={imageOptions.branching_threshold}
                    onChange={(e) =>
                      onImageOptionsChange({
                        ...imageOptions,
                        branching_threshold: parseInt(e.target.value, 10),
                      })
                    }
                    className="flex-1"
                    data-testid="branching-threshold-slider"
                    aria-valuemin={THRESHOLD_MIN}
                    aria-valuemax={THRESHOLD_MAX}
                    aria-valuenow={imageOptions.branching_threshold}
                  />
                  <span
                    className="w-8 text-right text-sm tabular-nums"
                    data-testid="branching-threshold-value"
                  >
                    {imageOptions.branching_threshold}
                  </span>
                </div>
                <p className="mt-1 text-xs text-text-secondary">
                  Segments connected to ≥ this many other segments via reference shots are drawn
                  thick.
                </p>
              </div>

              {/* Comments & Labels Options: what the renderer draws. Collapsible,
                  default collapsed (mirrors the app's local-collapse pattern, e.g.
                  DetailPanel's closures section). */}
              <div>
                <button
                  type="button"
                  onClick={() => setLabelsOptionsExpanded(!labelsOptionsExpanded)}
                  className="flex items-center gap-1 text-sm text-text-secondary hover:text-text-primary w-full text-left"
                  data-testid="export-labels-options-toggle"
                  aria-expanded={labelsOptionsExpanded}
                  aria-controls={labelsRegionId}
                >
                  <ChevronRightIcon
                    size={12}
                    className={labelsOptionsExpanded ? 'rotate-90 transition-transform' : 'transition-transform'}
                  />
                  <span>Comments &amp; Labels Options</span>
                </button>
                {labelsOptionsExpanded && (
                  <div
                    className="mt-2 space-y-2"
                    id={labelsRegionId}
                    data-testid="export-labels-options-content"
                  >
                    <label className="flex items-center gap-2 text-sm">
                      <input
                        type="checkbox"
                        checked={imageOptions.show_section_names}
                        onChange={(e) =>
                          onImageOptionsChange({
                            ...imageOptions,
                            show_section_names: e.target.checked,
                          })
                        }
                        data-testid="export-show-section-names"
                      />
                      <span>Section names</span>
                    </label>
                    <label className="flex items-center gap-2 text-sm">
                      <input
                        type="checkbox"
                        checked={imageOptions.show_station_labels}
                        onChange={(e) =>
                          onImageOptionsChange(
                            setStationLabelOption(imageOptions, 'show_station_labels', e.target.checked),
                          )
                        }
                        data-testid="export-show-station-labels"
                      />
                      <span>Station IDs</span>
                    </label>
                    {/* Names replace the IDs, so the two are one choice. Without any
                        names the box would label nothing (and untick the IDs), so it
                        is offered only when the project has names - or is already
                        ticked, so it can always be unticked. */}
                    <label
                      className={`flex items-center gap-2 text-sm${stationNamesUnavailable ? ' opacity-50' : ''}`}
                      title={stationNamesUnavailable ? t('displayMenu.descriptions.stationNamesUnavailable') : undefined}
                    >
                      <input
                        type="checkbox"
                        checked={imageOptions.show_station_names}
                        disabled={stationNamesUnavailable}
                        onChange={(e) =>
                          onImageOptionsChange(
                            setStationLabelOption(imageOptions, 'show_station_names', e.target.checked),
                          )
                        }
                        data-testid="export-show-station-names"
                      />
                      <span>Station names</span>
                    </label>
                    <label className="flex items-center gap-2 text-sm">
                      <input
                        type="checkbox"
                        checked={imageOptions.show_station_comments}
                        onChange={(e) =>
                          onImageOptionsChange({
                            ...imageOptions,
                            show_station_comments: e.target.checked,
                          })
                        }
                        data-testid="export-show-station-comments"
                      />
                      <span>Station comments</span>
                    </label>
                    <label className="flex items-center gap-2 text-sm">
                      <input
                        type="checkbox"
                        checked={imageOptions.show_shot_comments}
                        onChange={(e) =>
                          onImageOptionsChange({
                            ...imageOptions,
                            show_shot_comments: e.target.checked,
                          })
                        }
                        data-testid="export-show-shot-comments"
                      />
                      <span>Shot comments</span>
                    </label>
                    <label className="flex items-center gap-2 text-sm">
                      <input
                        type="checkbox"
                        checked={imageOptions.show_shot_data}
                        onChange={(e) =>
                          onImageOptionsChange({
                            ...imageOptions,
                            show_shot_data: e.target.checked,
                          })
                        }
                        data-testid="export-show-shot-data"
                      />
                      <span>Shot data</span>
                    </label>
                    <label className="flex items-center gap-2 text-sm">
                      <input
                        type="checkbox"
                        checked={imageOptions.show_penetration_labels}
                        onChange={(e) =>
                          onImageOptionsChange({
                            ...imageOptions,
                            show_penetration_labels: e.target.checked,
                          })
                        }
                        data-testid="export-show-labels"
                      />
                      <span>Penetration-distance labels</span>
                    </label>
                    <div className={svgSelected ? 'opacity-50' : ''}>
                      <label
                        htmlFor="export-color-mode"
                        className="block text-sm text-text-secondary mb-1"
                      >
                        Color
                      </label>
                      <Select
                        value={imageOptions.color_mode}
                        onChange={(v) =>
                          onImageOptionsChange({ ...imageOptions, color_mode: v as ColorMode })
                        }
                        disabled={svgSelected}
                        options={[
                          { value: 'none', label: 'None (monochrome)' },
                          { value: 'section', label: 'By section' },
                          { value: 'depth', label: 'By depth' },
                          { value: 'distance', label: 'By distance' },
                        ]}
                        data-testid="export-color-mode"
                      />
                    </div>
                    <p className="text-xs text-text-secondary">
                      {svgSelected
                        ? 'SVG export is always monochrome. Choose PNG or PDF to color the map.'
                        : 'Depth / distance gradients use the palette chosen on the map. Color applies to PNG / PDF.'}
                    </p>
                    {!svgSelected && (imageOptions.color_mode === 'depth' || imageOptions.color_mode === 'distance') && (
                      <ExportColorScaleControls kind={imageOptions.color_mode} />
                    )}
                  </div>
                )}
              </div>
            </>
          )}
        </div>
      </div>

      {/* Export Progress - uses shared LoadingProgress component for consistent UI */}
      {isExporting &&
        (() => {
          const phase = EXPORT_PHASE_LABELS[exportProgress?.phase ?? ''] ?? 'Preparing...';
          const overall = exportProgress?.overallPercent ?? 0;
          const isIndeterminate = exportProgress?.isIndeterminate ?? true;
          // Compute phase progress from current/total (not overallPercent)
          const current = exportProgress?.current;
          const total = exportProgress?.total;
          const phasePercent =
            current != null && total != null && total > 0
              ? Math.round((current / total) * 100)
              : undefined;
          return (
            <div className="mt-4 flex justify-center" data-testid="export-progress-container">
              {isIndeterminate || phasePercent === undefined ? (
                <LoadingProgress title="Exporting..." phase={phase} overallProgress={overall} />
              ) : (
                <LoadingProgress
                  title="Exporting..."
                  phase={phase}
                  phaseProgress={phasePercent}
                  overallProgress={overall}
                />
              )}
            </div>
          );
        })()}

      {/* Actions */}
      <div className="pt-4 mt-4 border-t border-border">
        <Button
          variant="primary"
          onClick={() => {
            void handleExport();
          }}
          disabled={isExporting}
          data-testid="export-button"
          className="w-full"
        >
          {isExporting ? 'Exporting...' : 'Export'}
        </Button>
      </div>

      {/* Nothing here is ever discarded: `build_export_shots` (src-tauri/src/export/tml.rs)
          emits every section. The copy deliberately claims only "not georeferenced", never
          entrance topology: entrance-hood is decided by STATION MERGES (`build_parent_map`
          walks project.station_merges only), while this dialog's classifier is zero-datum
          reachability (`get_anchored_section_ids` = merges + aligned closures from each
          datum) - the two predicates disagree in both directions (merged-but-undatumed
          exports as ONE system; a closure-only-anchored section still exports as its own
          entrance), so any entrance claim keyed on this list would lie for some project.
          This is informational, not a risk warning - see ExportControls.test.tsx. */}
      <AlertDialog
        open={showTmlUnanchoredWarning}
        title={t(
          tmlAllSectionsUnanchored
            ? 'dialogs.tmlUnanchoredWarning.titleAllUnanchored'
            : 'dialogs.tmlUnanchoredWarning.titlePartial',
        )}
        onClose={() => setShowTmlUnanchoredWarning(false)}
        data-testid="tml-unanchored-warning-dialog"
        actions={
          <>
            <Button
              onClick={() => setShowTmlUnanchoredWarning(false)}
              data-testid="tml-unanchored-warning-cancel"
            >
              {t('common.actions.cancel')}
            </Button>
            <Button
              variant="primary"
              onClick={() => {
                setShowTmlUnanchoredWarning(false);
                void handleExport({ skipTmlAnchorCheck: true });
              }}
              data-testid="tml-unanchored-warning-continue"
            >
              {t('common.actions.continue')}
            </Button>
          </>
        }
      >
        <div className="space-y-3">
          {/* `bodyAllUnanchored`/`bodyPartial` (en.json) quote "Set Zero Datum" - the literal
              English label at ContextMenu.tsx:426, which `iconForLabel` switches on by exact
              string. Translating that menu item later will desync these quotes; update both
              together. */}
          <p>
            {tmlAllSectionsUnanchored
              ? t('dialogs.tmlUnanchoredWarning.bodyAllUnanchored')
              : t('dialogs.tmlUnanchoredWarning.bodyPartial', {
                  count: tmlUnanchoredSections.length,
                  total: tmlTotalSectionCount,
                })}
          </p>
          <div className="rounded border border-border bg-panel p-3">
            <p className="text-xs font-medium text-text-secondary mb-2">
              {t('dialogs.tmlUnanchoredWarning.listHeading')}
            </p>
            <ul className="list-disc pl-5 space-y-1 text-sm max-h-40 overflow-y-auto">
              {tmlUnanchoredSections.map((section) => (
                <li key={section.id} data-testid={`tml-unanchored-section-item-${section.id}`}>
                  {formatSectionLabel(section)}
                </li>
              ))}
            </ul>
          </div>
        </div>
      </AlertDialog>
    </>
  );
}
