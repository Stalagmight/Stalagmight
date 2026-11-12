import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { DepthLegend } from './DepthLegend';
import { DistanceLegend } from './DistanceLegend';
import { ColorScaleCog } from './colorScale/ColorScaleCog';
import { useMapColorScaleInputs } from './colorScale/useColorScaleInputs';
import type { HistogramSource } from './colorScale/histogram';
import { useDecimalPrecision } from '../../hooks/useDecimalPrecision';
import type { SurveyData } from '../../types/survey-data';
import type { DistanceField } from '../../types/project';

interface MapColorLegendsProps {
  /** Depth colouring is on and owns the legend slot. */
  showDepth: boolean;
  depthRange: { min: number; max: number };
  /** The geometry the map draws (station z feeds the depth histogram). */
  surveyData: SurveyData | null;
  /** The distance overlay is on and owns the legend slot. */
  showDistance: boolean;
  distanceField: DistanceField | null;
}

const SLOT_STYLE = { position: 'absolute', bottom: 50, left: 10, zIndex: 1 } as const;

/**
 * The bottom-left colour key for depth or distance colouring, with the cog that
 * opens its colour-scale popover (palette + Levels range). The cog sits beside
 * the legend box rather than inside it, so the legend's own element (and its
 * e2e selectors) are unchanged. Gas reach keeps its own legend in MapCanvas.
 */
export function MapColorLegends({ showDepth, depthRange, surveyData, showDistance, distanceField }: MapColorLegendsProps) {
  const { t } = useTranslation();
  const precision = useDecimalPrecision();
  const scales = useMapColorScaleInputs();

  const depthSource = useMemo<HistogramSource | null>(
    () => (surveyData ? { data: surveyData.stationCoords, stride: 3, offset: 2 } : null),
    [surveyData],
  );
  const distanceSource = useMemo<HistogramSource | null>(
    () => (distanceField ? { data: distanceField.distances } : null),
    [distanceField],
  );
  const distanceDomain = useMemo(
    () => ({ min: 0, max: distanceField && distanceField.max_reachable > 0 ? distanceField.max_reachable : 1 }),
    [distanceField],
  );

  if (showDistance) {
    if (!distanceField) {
      // While the field is being (re)computed - first toggle / re-root - show a
      // lightweight cue so a slow closure solve doesn't look frozen.
      return (
        <div
          data-testid="distance-legend-loading"
          style={{
            ...SLOT_STYLE,
            background: 'var(--cc-text-halo-alt)',
            color: 'var(--cc-depth-text)',
            borderRadius: 4,
            padding: 8,
            fontSize: 11,
          }}
        >
          {t('map.distanceLegend.computing')}
        </div>
      );
    }
    return (
      <div style={SLOT_STYLE} className="flex items-start gap-1" data-legend-slot>
        <DistanceLegend max={distanceField.max_reachable} visible colorScale={scales.distance} embedded />
        <ColorScaleCog
          kind="distance"
          domain={distanceDomain}
          histogramSource={distanceSource}
          precision={precision.coordinates}
        />
      </div>
    );
  }

  if (showDepth) {
    return (
      <div style={SLOT_STYLE} className="flex items-start gap-1" data-legend-slot>
        <DepthLegend
          minDepth={depthRange.min}
          maxDepth={depthRange.max}
          visible
          colorScale={scales.depth}
          embedded
        />
        <ColorScaleCog kind="depth" domain={depthRange} histogramSource={depthSource} precision={precision.depth} />
      </div>
    );
  }

  return null;
}
