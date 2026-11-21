/**
 * The Data workspace's row tint, wired to the same inputs the map colours by:
 * the Survey Issues scale + palette, the depth palette + Levels and data
 * extent, and the along-cave distance field (fetched with the map's IPC,
 * `get_station_distances_from`, only while the distance tint is on). Provides
 * `colorOf(row)` for the grid and the legend for the toolbar's tint menu
 * (`useDataTintLegend`).
 */
import { createContext, useContext, useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import { invokeIpc } from '../../utils/ipc';
import { getMapThemeColors } from '../../utils/themeColors';
import { interpolateColor } from '../../utils/colors';
import { scaleHex } from '../../utils/palettes';
import { DISTANCE_FAR, DISTANCE_NEAR } from '../../utils/distanceGradient';
import { formatFixed } from '../../i18n/format';
import type { DistanceField } from '../../types/project';
import { computeIssueScores } from '../Map/surveyLayer/issueScore';
import { computeShotQualityMetrics } from '../Map/surveyLayer/shotQuality';
import { issueScoreColor } from '../Map/surveyLayer/issueColor';
import { depthColorFor, normalizeDepthRange } from '../Map/surveyLayer/displayColor';
import { useIssuePalette, useIssueScale } from '../Map/colorScale/useIssueScale';
import { useMapColorScaleInputs } from '../Map/colorScale/useColorScaleInputs';
import { useDataWorkspaceStore } from './store';
import type { DataModel } from './DataModelContext';
import type { DataRow, TintMode } from './types';
import { buildDistanceIndex, depthExtent, rowTint, tintLegend, type TintInputs, type TintLegend } from './model/tints';

export interface DataTint {
  mode: TintMode;
  colorOf: (row: DataRow) => string | null;
  legend: TintLegend | null;
}

export const DataTintContext = createContext<DataTint | null>(null);

/** The current tint's legend (min / max labels + stops), for the tint menu. */
export function useDataTintLegend(): TintLegend | null {
  return useContext(DataTintContext)?.legend ?? null;
}

/** The current tint (mode, per-row colour, legend). */
export function useDataTintContext(): DataTint | null {
  return useContext(DataTintContext);
}

export function useDataTint(model: DataModel | null): DataTint {
  const { t } = useTranslation();
  const mode = useDataWorkspaceStore((s) => s.tintMode);
  const resolvedTheme = useUIStore((s) => s.resolvedTheme);
  const origin = useUIStore((s) => s.distanceGradientOrigin);
  const warnings = useProjectStore((s) => s.warnings);
  const modifiedAt = useProjectStore((s) => s.project?.modified_at);
  const includeReferenceLegs = useProjectStore((s) => s.config?.gas.include_reference_legs);
  const issueScale = useIssueScale();
  const issuePalette = useIssuePalette();
  const scales = useMapColorScaleInputs();
  const ctx = model?.ctx ?? null;
  const data = ctx?.data ?? null;

  // The distance field, fetched only while the distance tint is on.
  const [field, setField] = useState<DistanceField | null>(null);
  useEffect(() => {
    if (mode !== 'distance' || !data) return undefined;
    let cancelled = false;
    invokeIpc<DistanceField>(
      'get_station_distances_from',
      { from: origin ?? null },
      { purpose: 'data workspace distance tint', fields: { origin: origin ?? 'entrance' } },
    )
      .then((f) => {
        if (!cancelled) setField(f);
      })
      .catch(() => {
        if (!cancelled) setField(null);
      });
    return () => {
      cancelled = true;
    };
  }, [mode, data, origin, modifiedAt, includeReferenceLegs]);

  const theme = useMemo(() => getMapThemeColors(), [resolvedTheme]); // eslint-disable-line react-hooks/exhaustive-deps

  const issueScores = useMemo(
    () => (mode === 'issues' && data ? computeIssueScores(data, computeShotQualityMetrics(data), warnings, issueScale) : null),
    [mode, data, warnings, issueScale],
  );

  const depthRange = useMemo(() => (mode === 'depth' && ctx ? normalizeDepthRange(depthExtent(ctx)) : null), [mode, ctx]);

  const inputs = useMemo<TintInputs | null>(() => {
    if (!ctx) return null;
    const base: TintInputs = { mode, ctx };
    if (mode === 'issues' && issueScores) {
      base.issueScores = issueScores;
      base.issueColor = (score) => issueScoreColor(theme, score, issuePalette);
    }
    if (mode === 'depth' && depthRange) {
      const colorCtx = {
        showDepthGradient: true,
        depthRange,
        depthScale: scales.depth,
        themeColors: theme,
        showIssueSeverity: false,
      };
      base.depthColor = (d) => depthColorFor(d, colorCtx);
    }
    if (mode === 'distance' && field) {
      const max = field.max_reachable;
      const scale = scales.distance;
      base.distance = {
        index: buildDistanceIndex(field),
        color: (d) =>
          scale ? scaleHex(d, scale, { min: 0, max }) : interpolateColor(DISTANCE_NEAR, DISTANCE_FAR, max > 0 ? Math.min(1, d / max) : 0),
      };
    }
    return base;
  }, [ctx, mode, issueScores, theme, issuePalette, depthRange, scales.depth, scales.distance, field]);

  const legend = useMemo(() => {
    if (!inputs) return null;
    return tintLegend(mode, inputs, {
      t: (key, opts) => String(t(key, opts ?? {})),
      metres: (v) => `${formatFixed(v, undefined, 1)} m`,
      depthRange,
      distanceMax: field ? field.max_reachable : null,
    });
  }, [inputs, mode, t, depthRange, field]);

  return useMemo<DataTint>(
    () => ({
      mode,
      colorOf: inputs ? (row) => rowTint(row, inputs) : () => null,
      legend,
    }),
    [mode, inputs, legend],
  );
}
