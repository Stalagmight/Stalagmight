import { useId, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { formatStationId } from '../../utils/formatters';
import { getThemeColor } from '../../utils/themeColors';
import { useUIStore } from '../../store/uiStore';
import { useGasPlanStore } from '../../store/gasPlanStore';
import { formatVolume } from '../../utils/formatGas';
import { GAS_NEAR, GAS_FAR, stageDropReached } from '../../utils/gasReach';
import { GAS_DIM_ALPHA } from './surveyLayer/customRender/shaders';
import { segmentColor } from '../../utils/segmentColors';
import { U_TURN_ARROW_PATH } from '../common/icons';
import type { GasReachField, GasUnits } from '../../types/ipc.generated';

interface GasLegendProps {
  /** The reachability field - its `budget_litres` is the far (purple) end of the scale. */
  field: GasReachField;
  units: GasUnits;
  visible: boolean;
}

/**
 * Gas reachable-frontier legend (#38): a gold→purple scale (0 → the turn budget),
 * bottom-left while the gas sub-mode is active. Doubles as the color key and the
 * cue for which overlay you're in. Shows the current origin (entrance default or a
 * picked station). Mirrors {@link ./DistanceLegend}.
 */
export function GasLegend({ field, units, visible }: GasLegendProps) {
  const gradientId = useId();
  const { t } = useTranslation();
  // Re-render on theme change so text colors stay correct.
  useUIStore((s) => s.resolvedTheme);
  const origin = useGasPlanStore((s) => s.origin);
  // Only key the symbols the map is actually showing, so the legend stays short on a plain plan.
  const waypointCount = useGasPlanStore((s) => s.waypoints.length);
  const markedCount = useGasPlanStore((s) => s.markedPoints.length);
  const textColor = getThemeColor('--cc-depth-text');
  const textHaloAlt = getThemeColor('--cc-text-halo-alt');

  if (!visible) return null;

  const minLabel = formatVolume(0, units);
  const maxLabel = formatVolume(field.budget_litres, units);
  const originLabel = origin
    ? t('map.gasLegend.fromStation', {
        id: formatStationId(origin.section_id, origin.station_index),
      })
    : t('map.gasLegend.fromEntrance');

  return (
    <div
      data-testid="gas-legend"
      className="gas-legend"
      style={{
        position: 'absolute',
        bottom: 50,
        left: 10,
        background: textHaloAlt,
        borderRadius: 4,
        padding: 8,
        zIndex: 1,
        display: 'flex',
        flexDirection: 'column',
        gap: 4,
      }}
    >
      <span style={{ fontSize: 11, fontWeight: 600, color: textColor }}>
        {t('map.gasLegend.title')}
      </span>
      <span
        data-testid="gas-legend-origin"
        style={{ fontSize: 10, opacity: 0.8, color: textColor }}
      >
        {originLabel}
      </span>
      <div style={{ display: 'flex', gap: 6 }}>
        <svg
          width="16"
          height="80"
          role="img"
          aria-label={t('map.gasLegend.aria', { min: minLabel, max: maxLabel })}
        >
          <defs>
            <linearGradient id={gradientId} x1="0%" y1="0%" x2="0%" y2="100%">
              <stop offset="0%" stopColor={GAS_NEAR} />
              <stop offset="100%" stopColor={GAS_FAR} />
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
          <span data-testid="gas-legend-min">{minLabel}</span>
          <span data-testid="gas-legend-max">{maxLabel}</span>
        </div>
      </div>

      {/* Symbol key - what the map markers mean. Drop / final-turn rows appear only for a staged
          strategy (when the backend returns them). The beyond-range dim swatch is always present. */}
      <div
        data-testid="gas-legend-key"
        style={{ display: 'flex', flexDirection: 'column', gap: 3, marginTop: 2, color: textColor }}
      >
        {/* The route line itself (#298). Both rows mirror the `survey-gas-route` layer's paint:
            its data-driven opacity (swum vs jump), and its per-segment identity colors (#317) -
            with way points the swatch shows the same first segment colors the map draws. */}
        {field.route.length > 0 && (
          <>
            <KeyRow label={t('map.gasLegend.key.route')}>
              <RouteSwatch opacity={0.9} segmentCount={waypointCount + 1} />
            </KeyRow>
            {field.route.some((leg) => leg.is_jump) && (
              <KeyRow label={t('map.gasLegend.key.routeJump')}>
                <RouteSwatch opacity={0.45} segmentCount={waypointCount + 1} />
              </KeyRow>
            )}
          </>
        )}
        {/* Beyond gas range (#395): a short line sample at the same dimmed opacity
            the renderer paints beyond-budget geometry with, clamped to the ramp's
            far color - explains why some legs look faded but still colored
            (vs. the fully grey unconnected parts, which need no key of their own). */}
        <KeyRow label={t('map.gasLegend.key.beyondRange')}>
          <span
            style={{
              width: 14,
              height: 3,
              borderRadius: 2,
              background: GAS_FAR,
              opacity: GAS_DIM_ALPHA,
              flexShrink: 0,
            }}
          />
        </KeyRow>
        {waypointCount > 0 && (
          <KeyRow label={t('map.gasLegend.key.waypoint')}>
            <span
              style={{
                width: 11,
                height: 11,
                borderRadius: '50%',
                background: '#16a085',
                border: '1.5px solid #ffffff',
                boxShadow: '0 0 2px rgba(0,0,0,0.6)',
                flexShrink: 0,
              }}
            />
          </KeyRow>
        )}
        {markedCount > 0 && (
          <KeyRow label={t('map.gasLegend.key.markedPoint')}>
            <svg width="11" height="11" viewBox="0 0 24 24" aria-hidden="true" style={{ flexShrink: 0 }}>
              <path
                d="M6 2h12v20l-6-5-6 5z"
                fill="#d81b60"
                stroke="#ffffff"
                strokeWidth="1.5"
                strokeLinejoin="round"
              />
            </svg>
          </KeyRow>
        )}
        {/* Keyed off the drops actually DRAWN, not the gauge rows: `stage_drop_bars` always has one
            entry per counted stage, but `stage_drop_refs` empties when there is no route (an
            unflyable way-point chain), and a key for a symbol that is nowhere on the map is noise.
            The orange drop is the one the dive reaches: a drop past the turnaround is drawn grey
            (#576 RV10b-1), so a plan whose every drop is not reached keys only the grey one. */}
        {field.stage_drop_refs.some((_ref, i) => stageDropReached(field, i)) && (
          <KeyRow label={t('map.gasLegend.key.stageDrop')}>
            <svg width="11" height="11" viewBox="0 0 24 24" aria-hidden="true" style={{ flexShrink: 0 }}>
              <path
                d="M3 6h18L12 20z"
                fill="#e08a1e"
                stroke="#ffffff"
                strokeWidth="1.5"
                strokeLinejoin="round"
              />
            </svg>
          </KeyRow>
        )}
        {/* The grey drop the dive never gets to (#576 RV8c-2-D3): keyed only when one is drawn. */}
        {field.stage_drop_refs.some((_ref, i) => !stageDropReached(field, i)) && (
          <KeyRow label={t('map.gasLegend.key.stageDropNotReached')}>
            <svg width="11" height="11" viewBox="0 0 24 24" aria-hidden="true" style={{ flexShrink: 0 }}>
              <path
                d="M3 6h18L12 20z"
                fill="#8a8f98"
                stroke="#ffffff"
                strokeWidth="1.5"
                strokeLinejoin="round"
              />
            </svg>
          </KeyRow>
        )}
        {field.final_turn_ref && (
          <KeyRow label={t('map.gasLegend.key.finalTurn')}>
            <span
              style={{
                width: 11,
                height: 11,
                borderRadius: 3,
                background: '#7b2fbf',
                border: '2px solid #ffffff',
                boxShadow: '0 0 2px rgba(0,0,0,0.6)',
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'center',
                flexShrink: 0,
              }}
            >
              <svg width="7" height="7" viewBox="0 0 24 24" aria-hidden="true">
                <path
                  d={U_TURN_ARROW_PATH}
                  fill="none"
                  stroke="#ffffff"
                  strokeWidth={3}
                  strokeLinecap="round"
                  strokeLinejoin="round"
                />
              </svg>
            </span>
          </KeyRow>
        )}
      </div>
    </div>
  );
}

/** A short line swatch standing in for the drawn route (#298). `opacity` mirrors the layer's
 *  data-driven line-opacity: full for a swum leg, faded for a jump leg. `segmentCount` mirrors
 *  the layer's per-segment `line-color` (#317): with way points the route is painted one
 *  identity color per segment, so the swatch shows the first few of those colors as stripes;
 *  with none it is the single `segmentColor(0)` cyan the layer falls back to. */
function RouteSwatch({ opacity, segmentCount }: { opacity: number; segmentCount: number }) {
  const stripes = Math.max(1, Math.min(segmentCount, 4));
  return (
    <span style={{ display: 'flex', gap: 1, flexShrink: 0 }}>
      {Array.from({ length: stripes }, (_, i) => (
        <span
          key={i}
          style={{
            width: Math.round(14 / stripes),
            height: 3,
            borderRadius: 2,
            background: segmentColor(i),
            opacity,
          }}
        />
      ))}
    </span>
  );
}

/** One symbol + label row in the gas legend key. */
function KeyRow({ children, label }: { children: ReactNode; label: string }) {
  return (
    <span style={{ display: 'flex', alignItems: 'center', gap: 5, fontSize: 10 }}>
      {children}
      <span>{label}</span>
    </span>
  );
}
