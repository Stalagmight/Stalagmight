import { useId } from 'react';
import { useTranslation } from 'react-i18next';
import { formatFixed } from '../../i18n/format';
import { formatStationId } from '../../utils/formatters';
import { useDecimalPrecision } from '../../hooks/useDecimalPrecision';
import { getThemeColor } from '../../utils/themeColors';
import { useUIStore } from '../../store/uiStore';
import { DISTANCE_NEAR, DISTANCE_FAR } from '../../utils/distanceGradient';
import { effectiveScale, legendGradientStops, type ColorScaleInput } from '../../utils/palettes/scale';

interface DistanceLegendProps {
  /** Largest reachable distance (meters) - the far end of the scale. */
  max: number;
  visible: boolean;
  /** The user's palette + Levels range. Null / absent = the fixed near/far ramp
   *  over 0..max. The bar then spans the scale's range. */
  colorScale?: ColorScaleInput | null;
  /** Laid out by a wrapper (MapColorLegends) instead of positioning itself. */
  embedded?: boolean;
}

/**
 * Distance-gradient legend (#34): a gold→purple scale (0 → max reachable),
 * bottom-left while the distance overlay is active. Doubles as the color key and
 * as the cue for which overlay you're in (vs depth). Shows the current origin
 * (the entrance default, or the picked station).
 */
export function DistanceLegend({ max, visible, colorScale = null, embedded = false }: DistanceLegendProps) {
  const gradientId = useId();
  const { t } = useTranslation();
  const precision = useDecimalPrecision();
  // Re-render on theme change so text colors stay correct.
  useUIStore((s) => s.resolvedTheme);
  const origin = useUIStore((s) => s.distanceGradientOrigin);
  const textColor = getThemeColor('--cc-depth-text');
  const textHaloAlt = getThemeColor('--cc-text-halo-alt');

  if (!visible) return null;

  const auto = { min: 0, max: max > 0 ? max : 1 };
  const scaled = colorScale ? effectiveScale(colorScale, auto) : null;
  const stops = colorScale
    ? legendGradientStops(colorScale, auto)
    : [
        { offset: 0, color: DISTANCE_NEAR },
        { offset: 100, color: DISTANCE_FAR },
      ];
  const minLabel = scaled
    ? `${formatFixed(scaled.min, undefined, precision.coordinates)}${t('common.units.meters')}`
    : `0${t('common.units.meters')}`;
  const maxLabel = `${formatFixed(scaled ? scaled.max : max, undefined, precision.coordinates)}${t('common.units.meters')}`;
  const originLabel = origin
    ? t('map.distanceLegend.fromStation', {
        id: formatStationId(origin.section_id, origin.station_index),
      })
    : t('map.distanceLegend.fromEntrance');

  return (
    <div
      data-testid="distance-legend"
      className="distance-legend"
      style={{
        ...(embedded ? {} : { position: 'absolute' as const, bottom: 50, left: 10, zIndex: 1 }),
        background: textHaloAlt,
        borderRadius: 4,
        padding: 8,
        display: 'flex',
        flexDirection: 'column',
        gap: 4,
      }}
    >
      <span style={{ fontSize: 11, fontWeight: 600, color: textColor }}>
        {t('map.distanceLegend.title')}
      </span>
      <span
        data-testid="distance-legend-origin"
        style={{ fontSize: 10, opacity: 0.8, color: textColor }}
      >
        {originLabel}
      </span>
      <div style={{ display: 'flex', gap: 6 }}>
        <svg
          width="16"
          height="80"
          role="img"
          aria-label={t('map.distanceLegend.aria', { min: minLabel, max: maxLabel })}
        >
          <defs>
            <linearGradient id={gradientId} x1="0%" y1="0%" x2="0%" y2="100%">
              {stops.map((stop, i) => (
                <stop key={i} offset={`${stop.offset}%`} stopColor={stop.color} />
              ))}
            </linearGradient>
          </defs>
          <rect width="16" height="80" fill={`url(#${gradientId})`} rx="2" />
        </svg>
        <div
          style={{
            display: 'flex',
            flexDirection: 'column',
            justifyContent: 'space-between',
            fontSize: 11,
            color: textColor,
            textShadow: `0 1px 2px ${textHaloAlt}`,
          }}
        >
          <span data-testid="distance-legend-min">{minLabel}</span>
          <span data-testid="distance-legend-max">{maxLabel}</span>
        </div>
      </div>
    </div>
  );
}
