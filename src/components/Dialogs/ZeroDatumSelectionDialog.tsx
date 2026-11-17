import { useMemo, useRef, useCallback } from 'react';
import { useTranslation } from 'react-i18next';
import { DialogWrapper } from './DialogWrapper';
import { Button } from '../common/Button';
import { HelpTooltip } from '../common/HelpTooltip';
import { useProjectStore } from '../../store/projectStore';
import { defaultConfig } from '../../types/config';
import { formatLength } from '../../utils/formatStats';
import { circularMeanDegrees } from '../../utils/angles';
import type { DmpFile, DmpShot } from '../../types/commands';
import type { DmpSelectionConfig } from './DmpSelectionDialog';

/**
 * Lightweight SVG preview of a line shape.
 * Avoids the cost of a full MapLibre instance per card.
 */
function LineMiniPreview({ shots, size = 140 }: { shots: DmpShot[]; size?: number }) {
  const points = useMemo(() => {
    const stations: Array<{ x: number; y: number }> = [{ x: 0, y: 0 }];
    for (const shot of shots) {
      if (shot.shot_type !== 'Standard') continue;
      const prev = stations[stations.length - 1];
      if (!prev) continue;
      const azimuth = circularMeanDegrees(shot.heading_in, shot.heading_out);
      const rad = (azimuth * Math.PI) / 180;
      stations.push({
        x: prev.x + shot.length * Math.sin(rad),
        y: prev.y + shot.length * Math.cos(rad),
      });
    }
    return stations;
  }, [shots]);

  const svgPath = useMemo(() => {
    if (points.length < 2) return null;

    // Compute bounding box
    let minX = Infinity, maxX = -Infinity, minY = Infinity, maxY = -Infinity;
    for (const p of points) {
      if (p.x < minX) minX = p.x;
      if (p.x > maxX) maxX = p.x;
      if (p.y < minY) minY = p.y;
      if (p.y > maxY) maxY = p.y;
    }

    const rangeX = maxX - minX || 1;
    const rangeY = maxY - minY || 1;
    const pad = 12;
    const drawSize = size - pad * 2;
    const scale = Math.min(drawSize / rangeX, drawSize / rangeY);

    // Map to SVG coords (flip Y since SVG Y goes down)
    const mapped = points.map((p) => ({
      x: pad + (p.x - minX) * scale + (drawSize - rangeX * scale) / 2,
      y: pad + (maxY - p.y) * scale + (drawSize - rangeY * scale) / 2,
    }));

    const d = mapped.map((p, i) => `${i === 0 ? 'M' : 'L'}${p.x.toFixed(1)},${p.y.toFixed(1)}`).join(' ');
    return { d, first: mapped[0], last: mapped[mapped.length - 1] };
  }, [points, size]);

  return (
    <svg width={size} height={size} className="bg-background rounded border border-border">
      {svgPath ? (
        <>
          <path d={svgPath.d} fill="none" stroke="var(--cc-accent)" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" />
          {svgPath.first && (
            <circle cx={svgPath.first.x} cy={svgPath.first.y} r={3} fill="var(--cc-accent)" />
          )}
          {svgPath.last && (
            <circle cx={svgPath.last.x} cy={svgPath.last.y} r={3} fill="var(--cc-accent)" />
          )}
        </>
      ) : (
        <text x={size / 2} y={size / 2} textAnchor="middle" dominantBaseline="middle" fill="var(--cc-text-secondary)" fontSize={12}>
          No shots
        </text>
      )}
    </svg>
  );
}

interface ZeroDatumSelectionDialogProps {
  dmpFile: DmpFile;
  selectionConfig: DmpSelectionConfig;
  filePath: string;
  onCancel: () => void;
  onBack: () => void;
  onSkip: () => void;
  onSelectLine: (lineIndex: number) => void;
}

export default function ZeroDatumSelectionDialog({
  dmpFile,
  selectionConfig,
  filePath: _filePath,
  onCancel,
  onBack,
  onSkip,
  onSelectLine,
}: ZeroDatumSelectionDialogProps) {
  const { t } = useTranslation();
  const config = useProjectStore((s) => s.config);
  const lengthUnit = config?.units?.length ?? defaultConfig.units.length;

  // Single-fire guard: prevent double-click from calling onSelectLine twice
  const selectedRef = useRef(false);
  const handleSelectLine = useCallback((index: number) => {
    if (selectedRef.current) return;
    selectedRef.current = true;
    onSelectLine(index);
  }, [onSelectLine]);

  const selectedLines = useMemo(() => {
    return Array.from(selectionConfig.selectedIndices)
      .sort((a, b) => a - b)
      .map((idx) => {
        const line = dmpFile.lines[idx];
        if (!line) return null;

        const standardShots = line.shots.filter((s) => s.shot_type === 'Standard');
        const totalLength = standardShots.reduce((sum, s) => sum + (s.length >= 0 ? s.length : 0), 0);

        // Extract time from first shot
        const firstShot = standardShots[0];
        const dateStr = line.date?.split(/[T ]/)[0] ?? null;
        let dateTimeStr = dateStr;
        if (dateStr && firstShot) {
          const hh = String(firstShot.hour).padStart(2, '0');
          const mm = String(firstShot.minute).padStart(2, '0');
          dateTimeStr = `${dateStr} ${hh}:${mm}`;
        }

        return {
          index: idx,
          line,
          shotCount: standardShots.length,
          totalLength,
          dateTimeStr,
        };
      })
      .filter((s): s is NonNullable<typeof s> => s !== null);
  }, [dmpFile.lines, selectionConfig.selectedIndices]);

  return (
    <DialogWrapper
      title={t('dialogs.zeroDatumSelection.title')}
      onClose={onCancel}
      size="4xl"
      overflow="hidden"
      bodyClassName="flex-1 flex flex-col p-4 min-h-0"
      closeOnBackdropClick={false}
      data-testid="zero-datum-selection-dialog"
    >
      <p className="text-sm text-text-secondary mb-2 px-1 shrink-0 flex items-start gap-1.5">
        <span>{t('dialogs.zeroDatumSelection.subtitle')}</span>
        <HelpTooltip
          text={t('dialogs.zeroDatumSelection.helpTooltip')}
          className="mt-0.5 shrink-0"
          iconTestId="zero-datum-selection-help-icon"
          tooltipTestId="zero-datum-selection-help-tooltip"
        />
      </p>

      <div className="flex-1 min-h-0 overflow-y-auto">
        <div className="grid grid-cols-[repeat(auto-fill,minmax(160px,1fr))] gap-2 p-1">
          {selectedLines.map(({ index, line, shotCount, totalLength, dateTimeStr }) => (
            <button
              key={index}
              type="button"
              onClick={() => handleSelectLine(index)}
              className="flex flex-col items-center gap-1 p-2 bg-panel border border-border rounded-lg cursor-pointer transition-colors hover:border-accent focus-visible:border-accent focus:outline-none focus-visible:ring-1 focus-visible:ring-accent"
              data-testid={`zero-datum-line-card-${index}`}
            >
              <LineMiniPreview shots={line.shots} size={110} />
              <span className="text-sm font-medium text-text-primary leading-tight">{line.name}</span>
              <span className="text-xs text-text-secondary leading-tight">
                {t('dialogs.zeroDatumSelection.shotCount', { count: shotCount })}
                {' \u00b7 '}
                {t('dialogs.zeroDatumSelection.totalLength', {
                  length: formatLength(totalLength, lengthUnit),
                })}
              </span>
              {dateTimeStr && (
                <span className="text-xs text-text-secondary leading-tight">
                  {dateTimeStr}
                </span>
              )}
            </button>
          ))}
        </div>
      </div>

      <div className="flex items-center justify-between pt-3 border-t border-border mt-1 shrink-0">
        <div className="flex gap-2">
          <Button variant="secondary" onClick={onCancel} data-testid="zero-datum-selection-cancel">
            {t('common.actions.cancel')}
          </Button>
          <Button variant="secondary" onClick={onBack} data-testid="zero-datum-selection-back">
            {t('common.actions.back')}
          </Button>
        </div>
        <div className="flex flex-col items-end gap-1">
          <Button variant="secondary" onClick={onSkip} data-testid="zero-datum-selection-skip">
            {t('dialogs.zeroDatumSelection.skip')}
          </Button>
          <span className="text-xs text-text-secondary" data-testid="zero-datum-selection-skip-cost">
            {t('dialogs.zeroDatumSelection.skipCost')}
          </span>
        </div>
      </div>
    </DialogWrapper>
  );
}
