import { useEffect, useId, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';
import { useShallow } from 'zustand/react/shallow';
import { selectHasUnresolvedPins, useGasPlanStore } from '../../store/gasPlanStore';
import { useUIStore } from '../../store/uiStore';
import { getThemeColor } from '../../utils/themeColors';
import { formatGasLength } from '../../utils/formatGas';
import { formatFixed } from '../../i18n/format';
import { saveDialog, exportSaveFilters } from '../../utils/fileDialogs';
import { resolveSavePath } from '../../utils/overwriteGuard';
import {
  exportDiveProfile,
  renderDiveProfileText,
  type DiveProfileRequest,
} from '../../utils/divePlanApi';
import { getErrorMessage } from '../../utils/ipc';
import { TOAST_KEYS } from '../../utils/toastKeys';
import {
  buildRouteProfile,
  projectProfile,
  axisTicks,
  depthTicks,
  type ProfileAxis,
  type ProfileMarker,
} from '../../utils/routeProfile';

/** Room for the depth labels, the x-axis labels and the marker flags above the surface line. */
const PAD_LEFT = 46;
const PAD_RIGHT = 12;
const PAD_TOP = 14;
const PAD_BOTTOM = 22;
/** Below this the axis furniture costs more than it explains, so the line is drawn bare. */
const MIN_LABELLED_HEIGHT = 130;

/**
 * The anticipated dive profile (#299) - the Plan panel's "Profile" tab.
 *
 * Depth against either swum distance or runtime, for the whole round trip: out to the turnaround,
 * then home. Both axes come from `GasReachField.route_profile`, which the engine builds from the
 * same vectors it derives the Plan readout's distance and depth from - so this chart cannot drift
 * from the numbers in the sidebar beside it.
 *
 * The two axes are not interchangeable views of one shape. The engine prices the way in per span
 * (survey or segment pace) and the way home at the swim speed - except a scootered span, which
 * rides out at its own pace - so the turnaround sits at the halfway mark on the distance axis but
 * not on the time axis. That difference is the point of the toggle.
 *
 * Hand-rolled SVG, matching the map legends - no charting dependency. A passive consumer of the
 * store: `MapCanvas` owns the compute, and this must never trigger one.
 */
export function ProfileChart() {
  const { t } = useTranslation();
  const gradientId = useId();
  const { reachField, units, hasUnresolvedPins } = useGasPlanStore(
    useShallow((s) => ({
      reachField: s.reachField,
      units: s.gasConfig.units,
      hasUnresolvedPins: selectHasUnresolvedPins(s),
    }))
  );
  const [axis, setAxis] = useState<ProfileAxis>('distance');
  const containerRef = useRef<HTMLDivElement | null>(null);
  const [size, setSize] = useState({ width: 0, height: 0 });

  // The panel is user-resizable (100-500 px) and has a fullscreen mode, so the box is only known at
  // runtime. Guarded for jsdom, which has no ResizeObserver.
  useEffect(() => {
    const el = containerRef.current;
    if (!el || typeof ResizeObserver === 'undefined') return undefined;
    const update = () => setSize({ width: el.clientWidth, height: el.clientHeight });
    update();
    const ro = new ResizeObserver(update);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const geo = useMemo(() => buildRouteProfile(reachField, axis), [reachField, axis]);

  return (
    <div className="h-full flex flex-col" data-testid="plan-profile">
      <ProfileHeader
        axis={axis}
        onAxis={setAxis}
        exportable={geo !== null}
        unresolvedPins={hasUnresolvedPins}
      />
      <div ref={containerRef} className="flex-1 min-h-0 px-2 pb-2" data-testid="plan-profile-plot">
        {geo ? (
          <ProfilePlot geo={geo} units={units} size={size} gradientId={gradientId} />
        ) : (
          <div
            className="h-full flex items-center justify-center text-xs text-text-secondary text-center px-4"
            data-testid="plan-profile-empty"
          >
            {reachField
              ? t('planTables.profile.noRoute')
              : t('gasPanel.readout.computing')}
          </div>
        )}
      </div>
    </div>
  );
}

function ProfileHeader({
  axis,
  onAxis,
  exportable,
  unresolvedPins,
}: {
  axis: ProfileAxis;
  onAxis: (a: ProfileAxis) => void;
  exportable: boolean;
  unresolvedPins: boolean;
}) {
  const { t } = useTranslation();
  return (
    <div className="flex items-center justify-between gap-2 px-2 py-1 shrink-0">
      {/* Axis toggle sits where the title used to: the chart is already under a tab labelled
          "Profile", so a heading repeating that only cost vertical space in a 100px-min panel. */}
      <div
        className="flex rounded overflow-hidden border border-border"
        role="group"
        aria-label={t('planTables.profile.axisLabel')}
        title={t('planTables.profile.subtitle')}
      >
        {(['distance', 'time'] as const).map((a) => (
          <button
            key={a}
            type="button"
            onClick={() => onAxis(a)}
            aria-pressed={axis === a}
            data-testid={`plan-profile-axis-${a}`}
            className={`px-2 py-0.5 text-xs ${axis === a ? 'bg-accent text-accent-on' : 'opacity-70'}`}
          >
            {t(`planTables.profile.axis.${a}`)}
          </button>
        ))}
      </div>
      <ProfileExportControls enabled={exportable} unresolvedPins={unresolvedPins} />
    </div>
  );
}

/**
 * Export the plan straight from the tab it is drawn in, rather than through the project Export
 * panel: a dive profile is not project data, and it only exists while a plan is live.
 *
 * Both actions send the plan INPUTS and let the backend recompute - see `DiveProfileRequest`. The
 * backend refuses on several reachable paths (no route, writing over the open project, any I/O
 * error), and every one of them is surfaced: a refusal that silently un-greys the button reads as
 * "the file was written", which is the failure mode this app's mutation paths are audited for.
 */
function ProfileExportControls({
  enabled,
  unresolvedPins,
}: {
  enabled: boolean;
  unresolvedPins: boolean;
}) {
  const { t } = useTranslation();
  const showToast = useUIStore((s) => s.showToast);
  const [busy, setBusy] = useState(false);
  const [copied, setCopied] = useState(false);
  const copiedTimer = useRef<number | undefined>(undefined);

  // Clear the "Copied" badge timer on unmount - the tab strip unmounts this component on every
  // switch, so copy-then-switch inside 2 s would otherwise fire a setter into a dead tree.
  useEffect(
    () => () => {
      if (copiedTimer.current !== undefined) window.clearTimeout(copiedTimer.current);
    },
    []
  );

  const run = async (fn: () => Promise<void>) => {
    setBusy(true);
    try {
      await fn();
    } catch (e) {
      showToast(
        getErrorMessage(e, t('planTables.profile.export.failed')),
        TOAST_KEYS.PROFILE_EXPORT_FAILED,
        'error'
      );
    } finally {
      setBusy(false);
    }
  };

  // Read the plan at CLICK time, not render time. The XML path awaits a save dialog first, and on
  // Android that dialog is an in-app picker the user can edit the plan behind - a render-time
  // closure would then export inputs that no longer match the plan on screen.
  const currentRequest = (): DiveProfileRequest => {
    const s = useGasPlanStore.getState();
    return {
      origin: s.origin,
      endpoint: s.endpoint,
      waypoints: s.waypoints,
      segments: s.segments,
      diver: s.diver,
      cylinders: s.cylinders,
      gas_config: s.gasConfig,
      stage_drop_overrides: s.stageDropOverrides,
    };
  };

  const onExport = () =>
    run(async () => {
      const path = await saveDialog({
        filters: exportSaveFilters(t('planTables.profile.export.xmlFilter'), 'xml'),
      });
      if (!path) return;
      // [#576 D35] Ask before replacing an existing file the extension fix-up renamed
      // the answer onto (the OS prompt only saw the typed name); `null` = declined.
      const finalPath = await resolveSavePath(path, 'xml');
      if (!finalPath) return;
      // Re-check what the render-time `disabled` proved BEFORE the awaits: the dialog is not
      // modal everywhere (Android's in-app picker), so an edit behind it can strand a pin
      // after the button's gate passed - and `currentRequest()` would then export a route
      // through the stale positional ref the Save gate refuses.
      if (selectHasUnresolvedPins(useGasPlanStore.getState())) {
        throw new Error(
          t(
            'planTables.profile.export.unresolvedPins',
            'A route point no longer matches a station in the survey. Re-place it before exporting.'
          )
        );
      }
      await exportDiveProfile(finalPath, 'subsurface-xml', currentRequest());
      showToast(t('planTables.profile.export.written'), TOAST_KEYS.PROFILE_EXPORT_OK, 'success');
    });

  const onCopy = () =>
    run(async () => {
      const text = await renderDiveProfileText(currentRequest());
      // NOT optional-chained: `navigator.clipboard?.writeText(...)` resolves to `undefined` where
      // the API is absent (it is, on WebKitGTK and in insecure contexts), and the caller would then
      // flip the button to "Copied" over an empty clipboard.
      if (!navigator.clipboard?.writeText) {
        throw new Error(t('planTables.profile.export.clipboardUnavailable'));
      }
      await navigator.clipboard.writeText(text);
      setCopied(true);
      copiedTimer.current = window.setTimeout(() => setCopied(false), 2000);
    });

  // A pin the backend can no longer place (#310) is withheld from the compute gate (`selectComputeGate`)
  // and blocks Save - an export must refuse it too, or it writes a document routed through whichever
  // station the STALE positional ref now happens to name, which the diver never chose and Save would
  // never have persisted.
  const disabledReason = unresolvedPins
    ? t(
        'planTables.profile.export.unresolvedPins',
        'A route point no longer matches a station in the survey. Re-place it before exporting.'
      )
    : undefined;

  return (
    <div className="flex gap-1">
      <button
        type="button"
        onClick={onCopy}
        disabled={!enabled || unresolvedPins || busy}
        data-testid="plan-profile-copy"
        title={disabledReason ?? t('planTables.profile.export.copyHint')}
        className="px-2 py-0.5 text-xs border border-border rounded disabled:opacity-40"
      >
        {copied ? t('planTables.profile.export.copied') : t('planTables.profile.export.copy')}
      </button>
      <button
        type="button"
        onClick={onExport}
        disabled={!enabled || unresolvedPins || busy}
        data-testid="plan-profile-export-xml"
        title={disabledReason ?? t('planTables.profile.export.xmlHint')}
        className="px-2 py-0.5 text-xs border border-border rounded disabled:opacity-40"
      >
        {t('planTables.profile.export.xml')}
      </button>
    </div>
  );
}

function ProfilePlot({
  geo,
  units,
  size,
  gradientId,
}: {
  geo: NonNullable<ReturnType<typeof buildRouteProfile>>;
  units: 'metric' | 'imperial';
  size: { width: number; height: number };
  gradientId: string;
}) {
  const { t } = useTranslation();
  // Fall back to a sane box before the first ResizeObserver callback (and under jsdom, where
  // clientWidth is always 0) so the paths are still well-formed and assertable.
  const width = size.width > 0 ? size.width : 480;
  const height = size.height > 0 ? size.height : 180;
  const labelled = height >= MIN_LABELLED_HEIGHT;
  const box = {
    width,
    height,
    padLeft: labelled ? PAD_LEFT : 4,
    padRight: PAD_RIGHT,
    padTop: PAD_TOP,
    padBottom: labelled ? PAD_BOTTOM : 4,
  };
  const p = projectProfile(geo, box);
  // Each `getThemeColor` is a getComputedStyle call, i.e. a forced style recalc. The panel is
  // live-resized at ~60 fps, so reading six of them per render put six recalcs in the drag loop.
  // Subscribing here (not in the parent) is what re-runs them on a light/dark flip - this is the
  // only component that reads a theme colour, so a second subscription upstairs bought nothing but
  // an extra parent render.
  const theme = useUIStore((s) => s.resolvedTheme);
  const colors = useMemo(
    () => ({
      shallow: getThemeColor('--cc-depth-shallow'),
      deep: getThemeColor('--cc-depth-deep'),
      accent: getThemeColor('--cc-accent'),
      border: getThemeColor('--cc-border'),
      text: getThemeColor('--cc-text-secondary'),
      error: getThemeColor('--cc-error'),
    }),
    [theme]
  );
  const { shallow, deep, accent, border, text, error } = colors;

  const xTicks = axisTicks(geo, units, 4);
  const yTicks = depthTicks(geo, units, 3);
  const turn = geo.markers.find((m) => m.kind === 'turn');
  const turnDepth = turn?.depth_m ?? 0;

  const lengthUnit = t(`common.units.${units === 'imperial' ? 'feet' : 'meters'}`);
  const axisSuffix = geo.axis === 'time' ? t('planTables.profile.units.minutes') : lengthUnit;

  return (
    <svg
      width="100%"
      height="100%"
      viewBox={`0 0 ${width} ${height}`}
      preserveAspectRatio="none"
      role="img"
      data-testid="plan-profile-svg"
      data-axis={geo.axis}
      aria-label={t('planTables.profile.aria', {
        depth: formatGasLength(geo.depthMax, units),
        // PENETRATION is the one-way distance to the turnaround, so it must come from the last
        // OUTBOUND point. The series' last point is the return to the origin at 2L, and announcing
        // that as penetration told a screen-reader user double the real number.
        distance: formatGasLength(geo.turnDistanceM, units),
        runtime: formatFixed(geo.points[geo.points.length - 1]?.elapsed_min ?? 0, undefined, 0),
      })}
    >
      <defs>
        <linearGradient id={`${gradientId}-fill`} x1="0%" y1="0%" x2="0%" y2="100%">
          <stop offset="0%" stopColor={shallow} stopOpacity="0.35" />
          <stop offset="100%" stopColor={deep} stopOpacity="0.15" />
        </linearGradient>
      </defs>

      {labelled &&
        yTicks.map((tick, i) => (
          <g key={`y${tick.display}`}>
            <line
              x1={p.plotLeft}
              y1={p.y(tick.value)}
              x2={p.plotRight}
              y2={p.y(tick.value)}
              stroke={border}
              strokeWidth="1"
            />
            <text
              x={p.plotLeft - 6}
              y={p.y(tick.value) + 3}
              textAnchor="end"
              fontSize="9"
              fill={text}
              data-testid={`plan-profile-depth-tick-${i}`}
            >
              {/* The depth axis carries its unit on the SURFACE tick, at the top: bare numbers down
                  one side of a dive chart are exactly the ambiguity a metric/imperial app cannot
                  afford. It goes on the first tick rather than the deepest because the deepest sits
                  in the bottom-left corner, hard against the x axis's own first label. */}
              {`${formatFixed(tick.display, undefined, 0)}${i === 0 ? ` ${lengthUnit}` : ''}`}
            </text>
          </g>
        ))}

      <path d={p.areaPath} fill={`url(#${gradientId}-fill)`} stroke="none" />
      <path
        d={p.linePath}
        fill="none"
        stroke={accent}
        strokeWidth="1.5"
        strokeLinejoin="round"
        data-testid="plan-profile-line"
      />

      {/* The turnaround: everything right of this line is the way home. Stops AT the profile rather
          than at the plot floor - a full-height rule hangs below the depth curve wherever the turn
          is not also the deepest point, which reads as a drawing error. */}
      <line
        x1={p.x(geo.turnX)}
        y1={p.plotTop}
        x2={p.x(geo.turnX)}
        y2={p.y(turnDepth)}
        stroke={accent}
        strokeWidth="1"
        strokeDasharray="3 3"
        data-testid="plan-profile-turn-line"
      />

      {geo.markers
        .filter((m) => m.kind !== 'turn')
        .map((m) => (
          <Marker
            key={`${m.kind}-${m.ordinal}`}
            marker={m}
            p={p}
            accent={accent}
            error={error}
            label={markerLabel(m, t)}
            showLabel={labelled}
          />
        ))}
      <circle
        cx={p.x(geo.turnX)}
        cy={p.y(turnDepth)}
        r="3.5"
        fill={accent}
        data-testid="plan-profile-marker-turn"
      />
      {labelled && turn && (
        <text
          x={p.x(geo.turnX)}
          y={p.plotTop - 4}
          textAnchor="middle"
          fontSize="9"
          fill={accent}
          data-testid="plan-profile-label-turn"
        >
          {t('planTables.profile.marker.turn')}
        </text>
      )}

      {labelled &&
        xTicks.map((tick, i) => (
          <text
            key={`x${tick.display}`}
            x={p.x(tick.value)}
            y={height - 6}
            /* First and last labels are anchored INWARD rather than centred. Centred, the "0" spills
               half its width left of the plot and lands under the deepest depth label, which sits in
               that same bottom-left corner - the two collided. Anchoring also keeps the last label
               (which carries the unit) from overhanging the right edge. */
            textAnchor={i === 0 ? 'start' : i === xTicks.length - 1 ? 'end' : 'middle'}
            fontSize="9"
            fill={text}
            data-testid={`plan-profile-x-tick-${i}`}
          >
            {`${formatFixed(tick.display, undefined, 0)}${i === xTicks.length - 1 ? ` ${axisSuffix}` : ''}`}
          </text>
        ))}
    </svg>
  );
}

/**
 * Short label for a marker's rule. An unlabelled vertical line is a riddle - the diver cannot tell
 * a stage drop from a way point, which is the whole reason for drawing them separately. Numbered
 * from 1 because the readout beside it already says "Stage 1 drop".
 */
function markerLabel(marker: ProfileMarker, t: TFunction): string {
  switch (marker.kind) {
    case 'stage-drop':
      return t('planTables.profile.marker.stage', { n: marker.ordinal + 1 });
    case 'waypoint':
      return t('planTables.profile.marker.waypoint', { n: marker.ordinal + 1 });
    default:
      return t('planTables.profile.marker.turn');
  }
}

function Marker({
  marker,
  p,
  accent,
  error,
  label,
  showLabel,
}: {
  marker: ProfileMarker;
  p: ReturnType<typeof projectProfile>;
  accent: string;
  error: string;
  label: string;
  showLabel: boolean;
}) {
  const color = marker.overReserve ? error : accent;
  const x = p.x(marker.x);
  return (
    <g
      data-testid={`plan-profile-marker-${marker.kind}-${marker.ordinal}`}
      data-over-reserve={String(marker.overReserve)}
    >
      <line
        x1={x}
        y1={p.plotTop}
        x2={x}
        y2={p.y(marker.depth_m)}
        stroke={color}
        strokeWidth="1"
        strokeDasharray={marker.kind === 'stage-drop' ? '2 2' : undefined}
        opacity="0.8"
      />
      {marker.kind === 'stage-drop' ? (
        <rect x={x - 3} y={p.y(marker.depth_m) - 3} width="6" height="6" fill={color} />
      ) : (
        <circle cx={x} cy={p.y(marker.depth_m)} r="3" fill={color} />
      )}
      {showLabel && (
        <text
          x={x}
          y={p.plotTop - 4}
          textAnchor="middle"
          fontSize="9"
          fill={color}
          data-testid={`plan-profile-label-${marker.kind}-${marker.ordinal}`}
        >
          {label}
        </text>
      )}
    </g>
  );
}
