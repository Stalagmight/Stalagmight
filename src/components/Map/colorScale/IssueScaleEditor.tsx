import { useCallback, useMemo, useState, type RefObject } from 'react';
import { useTranslation } from 'react-i18next';
import { ResetButton } from '../../common/ResetButton';
import { useColorScaleStore } from '../../../store/colorScaleStore';
import {
  setIssueCombine,
  setIssueHidden,
  useEffectivePalette,
  useHiddenIssues,
  useIssueCombine,
} from '../../../store/colorScalePalette';
import { useProjectStore } from '../../../store/projectStore';
import { useDecimalPrecision } from '../../../hooks/useDecimalPrecision';
import { formatFixed } from '../../../i18n/format';
import { convertLength, convertLengthToMeters } from '../../../utils/formatStats';
import { getMapThemeColors } from '../../../utils/themeColors';
import type { SurveyData } from '../../../types/survey-data';
import type { LoopClosure } from '../../../types/project';
import type { LengthUnit } from '../../../types/config';
import { computeShotQualityMetrics, LENGTH_DELTA_MIN_INCLINATION_DEG } from '../surveyLayer/shotQuality';
import { issueScoreColor } from '../surveyLayer/issueColor';
import {
  defaultIssueLevels,
  ISSUE_METRICS,
  levelScore,
  sameLevels,
  type IssueLevels,
  type IssueMetricId,
} from '../surveyLayer/issueScale';
import type { IssueCombine } from '../../../types/ipc.generated';
import { computeHistogram } from './histogram';
import { IssueLevelsSlider } from './IssueLevelsSlider';
import { PaletteRow } from './ColorScaleEditor';
import { issuePaletteFor, useIssueThresholds } from './useIssueScale';

export interface IssueScaleEditorProps {
  surveyData: SurveyData | null;
  closures: readonly LoopClosure[];
  pickerPortalInto?: RefObject<HTMLElement | null> | undefined;
}

/** Each error's size, in metres / degrees / percent, for its histogram. */
function metricValues(id: IssueMetricId, data: SurveyData | null, closures: readonly LoopClosure[]): ArrayLike<number> {
  if (id === 'closure') return closures.map((c) => Math.abs(c.error_percentage));
  if (!data) return [];
  const m = computeShotQualityMetrics(data);
  const column = { azimuth: m.azDelta, inclination: m.pitchDelta, inclinationDepth: m.inclDepthDelta, length: m.lengthDelta }[id];
  return Array.from(column, Math.abs);
}

/** "Nice" upper end for a slider: 1, 2 or 5 times a power of ten. */
function niceCeil(v: number): number {
  if (!(v > 0)) return 1;
  const p = 10 ** Math.floor(Math.log10(v));
  for (const m of [1, 2, 5, 10]) if (m * p >= v) return m * p;
  return 10 * p;
}

/**
 * The Survey Issues colour settings: palette, how a shot's errors combine, and
 * per error an on / off switch and its Min / Warn / Max levels. All of this
 * changes only what the map draws; the warnings and their thresholds stay in
 * Project Settings.
 */
export function IssueScaleEditor({ surveyData, closures, pickerPortalInto }: IssueScaleEditorProps) {
  const { t } = useTranslation();
  const combine = useIssueCombine();
  const hidden = useHiddenIssues();
  const [open, setOpen] = useState<IssueMetricId | null>(ISSUE_METRICS[0]);

  const setCombine = (c: IssueCombine) => void setIssueCombine(c);

  return (
    <div className="flex flex-col gap-2" data-testid="issue-scale-editor">
      <PaletteRow kind="issues" testIdPrefix="issue-scale" pickerPortalInto={pickerPortalInto} />
      <div className="flex flex-col gap-1">
        <span className="text-[11px] font-medium text-text-secondary" id="issue-scale-combine-label">
          {t('map.issueScale.combine')}
        </span>
        <div className="flex gap-1" role="radiogroup" aria-labelledby="issue-scale-combine-label">
          {(['worst', 'average'] as const).map((c) => (
            <button
              key={c}
              type="button"
              role="radio"
              aria-checked={combine === c}
              title={t(`map.issueScale.combineHelp.${c}`)}
              onClick={() => setCombine(c)}
              className={`flex-1 px-1.5 py-1 rounded border text-[11px] ${
                combine === c
                  ? 'border-accent bg-accent-20 text-text-primary'
                  : 'border-border text-text-secondary hover:text-text-primary'
              }`}
              data-testid={`issue-scale-combine-${c}`}
            >
              {t(`map.issueScale.combineMode.${c}`)}
            </button>
          ))}
        </div>
        <span className="text-[10px] text-text-secondary" data-testid="issue-scale-combine-help">
          {t(`map.issueScale.combineHelp.${combine}`)}
        </span>
      </div>
      <div className="flex flex-col gap-1">
        <span className="text-[11px] font-medium text-text-secondary">{t('map.issueScale.errors')}</span>
        <div className="flex flex-col rounded border border-border divide-y divide-border">
          {ISSUE_METRICS.map((id) => (
            <IssueMetricRow
              key={id}
              id={id}
              enabled={!hidden.includes(id)}
              expanded={open === id}
              onToggleExpanded={() => setOpen((o) => (o === id ? null : id))}
              surveyData={surveyData}
              closures={closures}
            />
          ))}
        </div>
      </div>
      <span className="text-[10px] text-text-secondary" data-testid="issue-scale-note">
        {t('map.issueScale.note')}
      </span>
    </div>
  );
}

interface MetricUnits {
  unit: string;
  precision: number;
  /** Stored (metres / degrees / percent) to display units, and back. */
  toDisplay: (v: number) => number;
  fromDisplay: (v: number) => number;
}

function useMetricUnits(id: IssueMetricId): MetricUnits {
  const { t } = useTranslation();
  const precision = useDecimalPrecision();
  const lengthUnit: LengthUnit = useProjectStore((s) => s.config?.units.length ?? 'meters');
  return useMemo(() => {
    const same = (v: number) => v;
    if (id === 'length') {
      return {
        unit: lengthUnit === 'feet' ? t('common.units.feet') : t('common.units.meters'),
        precision: precision.length,
        toDisplay: (v: number) => convertLength(v, lengthUnit),
        fromDisplay: (v: number) => convertLengthToMeters(v, lengthUnit),
      };
    }
    if (id === 'closure') return { unit: t('common.units.percent'), precision: 1, toDisplay: same, fromDisplay: same };
    return { unit: '°', precision: id === 'azimuth' ? precision.azimuth : precision.inclination, toDisplay: same, fromDisplay: same };
  }, [id, lengthUnit, precision, t]);
}

function scaleLevels(l: IssueLevels, f: (v: number) => number): IssueLevels {
  return { min: f(l.min), warn: f(l.warn), max: f(l.max) };
}

function IssueMetricRow({
  id,
  enabled,
  expanded,
  onToggleExpanded,
  surveyData,
  closures,
}: {
  id: IssueMetricId;
  enabled: boolean;
  expanded: boolean;
  onToggleExpanded: () => void;
  surveyData: SurveyData | null;
  closures: readonly LoopClosure[];
}) {
  const { t } = useTranslation();
  const thresholds = useIssueThresholds();
  const committed = useColorScaleStore((s) => s.issueLevels[id]);
  const preview = useColorScaleStore((s) => (s.issuePreview?.id === id ? s.issuePreview.levels : null));
  const setIssueLevels = useColorScaleStore((s) => s.setIssueLevels);
  const units = useMetricUnits(id);
  const choice = useEffectivePalette('issues');
  const palette = useMemo(() => issuePaletteFor(choice), [choice]);

  const defaults = useMemo(() => defaultIssueLevels(thresholds)[id], [thresholds, id]);
  const levels = preview ?? committed ?? defaults;
  const shown = scaleLevels(levels, units.toDisplay);

  // The slider's span: twice the default Max, widened to the committed Max (not
  // the dragged one, so the track does not rescale under the pointer).
  const domainMax = units.toDisplay(niceCeil(Math.max(2 * defaults.max, 1.25 * (committed ?? defaults).max)));
  const values = useMemo(
    () => (expanded ? metricValues(id, surveyData, closures) : null),
    [expanded, id, surveyData, closures],
  );
  const histogram = useMemo(() => {
    if (!values || values.length === 0) return null;
    const display = Array.from(values, units.toDisplay);
    return computeHistogram({ data: display }, 0, domainMax);
  }, [values, units, domainMax]);

  const colorAt = useCallback(
    (v: number) => issueScoreColor(getMapThemeColors(), levelScore(units.fromDisplay(v), levels), palette),
    [units, levels, palette],
  );

  const onChange = (next: IssueLevels, live: boolean) => {
    const stored = scaleLevels(next, units.fromDisplay);
    setIssueLevels(id, !live && sameLevels(stored, defaults) ? null : stored, live);
  };

  const name = t(`map.issueScale.metric.${id}`, { degrees: LENGTH_DELTA_MIN_INCLINATION_DEG });
  const fmt = (v: number) => `${formatFixed(v, undefined, units.precision)}${units.unit}`;
  const summary = enabled ? `${fmt(shown.warn)} / ${fmt(shown.max)}` : t('map.issueScale.off');

  return (
    <div className="flex flex-col" data-testid={`issue-scale-row-${id}`}>
      <div className="flex items-center gap-1.5 px-1.5 py-1">
        <input
          type="checkbox"
          checked={enabled}
          onChange={(e) => void setIssueHidden(id, !e.target.checked)}
          aria-label={t('map.issueScale.enableAria', { name })}
          className="rounded border-border"
          data-testid={`issue-scale-enable-${id}`}
        />
        <button
          type="button"
          className="flex flex-1 min-w-0 items-center gap-1 text-left text-[11px] text-text-primary"
          aria-expanded={expanded}
          onClick={onToggleExpanded}
          data-testid={`issue-scale-expand-${id}`}
        >
          <span aria-hidden="true" className="inline-block w-2.5 text-text-secondary">
            {expanded ? '▾' : '▸'}
          </span>
          <span className={`truncate ${enabled ? '' : 'text-text-secondary line-through'}`}>{name}</span>
          <span
            className="ml-auto shrink-0 text-[10px] text-text-secondary"
            title={t('map.issueScale.summaryTitle')}
            data-testid={`issue-scale-summary-${id}`}
          >
            {summary}
          </span>
        </button>
        <ResetButton
          onReset={() => setIssueLevels(id, null, false)}
          isDefault={committed === undefined}
          title={t('map.issueScale.resetLevels')}
          data-testid={`issue-scale-reset-${id}`}
        />
      </div>
      {expanded && (
        <div className="px-1.5 pb-1.5">
          <IssueLevelsSlider
            levels={shown}
            domainMax={domainMax}
            histogram={histogram}
            colorAt={colorAt}
            unit={units.unit}
            precision={units.precision}
            disabled={!enabled}
            onChange={onChange}
            histogramLabel={t('map.issueScale.histogram', { name })}
            testIdPrefix={`issue-scale-${id}`}
          />
        </div>
      )}
    </div>
  );
}
