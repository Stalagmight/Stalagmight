/**
 * A compact key for the active row tint, drawn from the same palette functions
 * the map uses (section colours, the Survey Issues ramp, the depth and distance
 * colour scales with the user's palette and Levels, the element colours).
 */
import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../../store/uiStore';
import { useColorScaleStore } from '../../../store/colorScaleStore';
import { useEffectivePalette } from '../../../store/colorScalePalette';
import { colorScaleInputOrTheme } from '../../Map/colorScale/useColorScaleInputs';
import { issuePaletteFor } from '../../Map/colorScale/useIssueScale';
import { issueScoreColor } from '../../Map/surveyLayer/issueColor';
import { SCORE_MAX } from '../../Map/surveyLayer/issueScale';
import { legendGradientStops } from '../../../utils/palettes/scale';
import { getMapThemeColors } from '../../../utils/themeColors';
import { syntacticVarName } from '../../../theme/syntacticPalette';
import { formatFixed } from '../../../i18n/format';
import type { DataModelCtx, TintMode } from '../types';
import { sectionEntries } from './SectionsPicker';

const SECTION_SAMPLE = 6;
const ISSUE_STOPS = 12;

/** Depth range of the survey: station z and shot depth endpoints (as the map's legend). */
export function depthRange(ctx: DataModelCtx): { min: number; max: number } | null {
  const { data } = ctx;
  let min = Infinity;
  let max = -Infinity;
  for (let i = 2; i < data.stationCoords.length; i += 3) {
    const z = data.stationCoords[i]!;
    if (z < min) min = z;
    if (z > max) max = z;
  }
  const include = (d: number) => {
    if (!Number.isFinite(d)) return;
    if (d < min) min = d;
    if (d > max) max = d;
  };
  for (let i = 0; i < data.shotCount; i++) {
    include(data.shotDepthIn[i]!);
    include(data.shotDepthOut[i]!);
  }
  return Number.isFinite(min) && Number.isFinite(max) ? { min, max } : null;
}

const gradient = (stops: readonly { offset: number; color: string }[]) =>
  `linear-gradient(to right, ${stops.map((s) => `${s.color} ${s.offset}%`).join(', ')})`;

function Bar({ stops, left, right, testId }: { stops: { offset: number; color: string }[]; left: string; right: string; testId: string }) {
  return (
    <div data-testid={testId}>
      <div className="h-2.5 w-full rounded-sm border border-border" style={{ background: gradient(stops) }} />
      <div className="mt-0.5 flex justify-between text-[11px] text-text-secondary tabular-nums">
        <span>{left}</span>
        <span>{right}</span>
      </div>
    </div>
  );
}

export function TintLegend({ mode, ctx }: { mode: TintMode; ctx: DataModelCtx }) {
  const { t } = useTranslation();
  // Theme switches re-resolve every palette below.
  useUIStore((s) => s.resolvedTheme);
  const distanceField = useUIStore((s) => s.distanceField);
  const mapScale = useColorScaleStore((s) => s.mapScale);
  const depthChoice = useEffectivePalette('depth');
  const distanceChoice = useEffectivePalette('distance');
  const issueChoice = useEffectivePalette('issues');
  const range = useMemo(() => (mode === 'depth' ? depthRange(ctx) : null), [mode, ctx]);
  const unit = t('common.units.meters');

  switch (mode) {
    case 'none':
      return null;
    case 'section': {
      const entries = sectionEntries(ctx);
      return (
        <div className="flex flex-wrap items-center gap-x-3 gap-y-1" data-testid="data-tint-legend">
          {entries.slice(0, SECTION_SAMPLE).map((e) => (
            <span key={e.id} className="inline-flex max-w-[8rem] items-center gap-1">
              <span className="h-2 w-2 shrink-0 rounded-full" style={{ backgroundColor: e.color }} aria-hidden="true" />
              <span className="truncate">{e.name}</span>
            </span>
          ))}
          {entries.length > SECTION_SAMPLE && (
            <span className="text-text-secondary">
              {t('dataToolbar.tint.legend.more', { count: entries.length - SECTION_SAMPLE })}
            </span>
          )}
        </div>
      );
    }
    case 'issues': {
      const theme = getMapThemeColors();
      const palette = issuePaletteFor(issueChoice);
      const stops = Array.from({ length: ISSUE_STOPS + 1 }, (_, k) => ({
        offset: (k / ISSUE_STOPS) * 100,
        color: issueScoreColor(theme, (k / ISSUE_STOPS) * SCORE_MAX, palette),
      }));
      return (
        <Bar
          stops={stops}
          left={t('dataToolbar.tint.legend.agree')}
          right={t('dataToolbar.tint.legend.error')}
          testId="data-tint-legend"
        />
      );
    }
    case 'depth': {
      if (!range) return null;
      const input = colorScaleInputOrTheme('depth', depthChoice, mapScale.depth);
      const min = input.scale?.min ?? range.min;
      const max = input.scale?.max ?? range.max;
      // Shallow (left) to deep (right); z is depth, positive down.
      return (
        <Bar
          stops={legendGradientStops(input, range)}
          left={`${formatFixed(min, undefined, ctx.precision)} ${unit}`}
          right={`${formatFixed(max, undefined, ctx.precision)} ${unit}`}
          testId="data-tint-legend"
        />
      );
    }
    case 'distance': {
      if (!distanceField || !(distanceField.max_reachable > 0)) {
        return (
          <div className="text-text-secondary" data-testid="data-tint-legend">
            {t('dataToolbar.tint.legend.notComputed')}
          </div>
        );
      }
      const auto = { min: 0, max: distanceField.max_reachable };
      const input = colorScaleInputOrTheme('distance', distanceChoice, mapScale.distance);
      const min = input.scale?.min ?? auto.min;
      const max = input.scale?.max ?? auto.max;
      return (
        <Bar
          stops={legendGradientStops(input, auto)}
          left={`${formatFixed(min, undefined, ctx.precision)} ${unit}`}
          right={`${formatFixed(max, undefined, ctx.precision)} ${unit}`}
          testId="data-tint-legend"
        />
      );
    }
    case 'type':
      return (
        <div className="flex items-center gap-3" data-testid="data-tint-legend">
          {(['station', 'shot'] as const).map((kind) => (
            <span key={kind} className="inline-flex items-center gap-1">
              <span
                className="h-2.5 w-2.5 rounded-sm"
                style={{ backgroundColor: `var(${syntacticVarName(kind)})` }}
                aria-hidden="true"
              />
              {t(`dataToolbar.tint.legend.${kind}`)}
            </span>
          ))}
        </div>
      );
  }
}
