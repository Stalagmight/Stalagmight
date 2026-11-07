import { useEffect, useRef, useState, type MutableRefObject } from 'react';
import * as maplibregl from 'maplibre-gl';
import type { Station } from '../../types/project';
import type { GpsOrigin } from '../../types/config';
import type { ProjectStore } from '../../store/projectStore';
import type { UIStoreState } from '../../store/uiSelectKeys';
import { selectComputeGate, useGasPlanStore } from '../../store/gasPlanStore';
import { useUIStore } from '../../store/uiStore';
import { invokeIpc } from '../../utils/ipc';
import { TOAST_KEYS } from '../../utils/toastKeys';
import i18n from '../../i18n';
import { buildDistanceFeatureState, overlayApplyAction } from '../../utils/distanceGradient';
import { buildGasReachFeatureState } from '../../utils/gasReach';
import { segmentColor, routeLegSegmentIndices } from '../../utils/segmentColors';
import { metersToMapCoords } from '../../utils/coordinates';
import type SurveyLayer from './SurveyLayer';

type GasPlanState = ReturnType<typeof useGasPlanStore.getState>;

interface AnalyticalOverlaysOptions {
  map: MutableRefObject<maplibregl.Map | null>;
  surveyLayerRef: MutableRefObject<SurveyLayer | null>;
  mapLoaded: boolean;
  project: ProjectStore['project'];
  config: ProjectStore['config'];
  /** Measured SoA: both fields are computed on surveyed geometry. */
  surveyData: ProjectStore['surveyData'];
  activeStations: Station[];
  gpsOrigin: GpsOrigin | null;
  effectiveIssueSeverity: boolean;
  showDistanceGradient: boolean;
  distanceGradientOrigin: UIStoreState['distanceGradientOrigin'];
  distanceField: UIStoreState['distanceField'];
  setDistanceField: UIStoreState['setDistanceField'];
  showGasPlan: boolean;
  gasOriginChosen: GasPlanState['originChosen'];
  gasOrigin: GasPlanState['origin'];
  gasComputeGate: ReturnType<typeof selectComputeGate>;
  gasWaypoints: GasPlanState['waypoints'];
  gasSegments: GasPlanState['segments'];
  gasDiver: GasPlanState['diver'];
  gasCylinders: GasPlanState['cylinders'];
  gasPlanConfig: GasPlanState['gasConfig'];
  gasReachField: GasPlanState['reachField'];
  setGasReachField: GasPlanState['setReachField'];
}

/**
 * The analytical overlays: the distance gradient (#34) and the gas reachable
 * frontier (#38). Fetches each field, applies it to the survey layer (holding
 * the old ramp through a successful recompute, dropping it on a failed one), the
 * Plan idle tint, the picked gas route, and the two origin-ring markers.
 *
 * Moved out of MapCanvas.tsx verbatim (#576 D3). MapCanvas calls it where the
 * distance fetch used to be, so every effect here keeps its place except the
 * markers, which used to run after MapCanvas's measured-path tint effect and now
 * run before it: the tint only re-uploads the measure-path layer and the markers
 * only create DOM markers, so neither reads what the other writes.
 */
/** What the overlays tell MapCanvas back. */
export interface AnalyticalOverlaysState {
  /**
   * The last distance-field fetch failed (and no later one has succeeded): the field
   * is not being computed, so MapCanvas does not show its "computing" cue while the
   * error toast says it failed (#576 C7-CUE, decided: "Hide computing cue while the
   * field has failed").
   */
  distanceFieldFailed: boolean;
}

export function useAnalyticalOverlays({
  map,
  surveyLayerRef,
  mapLoaded,
  project,
  config,
  surveyData,
  activeStations,
  gpsOrigin,
  effectiveIssueSeverity,
  showDistanceGradient,
  distanceGradientOrigin,
  distanceField,
  setDistanceField,
  showGasPlan,
  gasOriginChosen,
  gasOrigin,
  gasComputeGate,
  gasWaypoints,
  gasSegments,
  gasDiver,
  gasCylinders,
  gasPlanConfig,
  gasReachField,
  setGasReachField,
}: AnalyticalOverlaysOptions): AnalyticalOverlaysState {
  const originMarkerRef = useRef<maplibregl.Marker | null>(null);
  const gasOriginMarkerRef = useRef<maplibregl.Marker | null>(null);
  // Clear-on-failure flags for the analytical overlays (#34 distance / #38 gas):
  // set when a field recompute REJECTS so the apply effect drops the stale ramp
  // (base colors) instead of `overlayApplyAction` holding it against the new
  // inputs indefinitely. Reset on the next successful field (see the fetch effects).
  const [distanceFieldFailed, setDistanceFieldFailed] = useState(false);
  const [gasFieldFailed, setGasFieldFailed] = useState(false);
  // A failed distance field is told once per failure BURST (#576 C7-MSG): every
  // edit refetches, and a broken computation fails each time. Re-armed by a
  // successful field, or by the overlay being switched off.
  const distanceFailureToldRef = useRef(false);

  // Distance-gradient overlay (#34): fetch the per-station distance field when the
  // overlay is on and the origin or project data changes. Distances use MEASURED
  // geometry (surveyed lengths), so neither closure-adjustment config nor the
  // display geometryMode affects them - no need to refetch on either. `from` null
  // = the entrance (zero-datum) default.
  //
  // `gas.include_reference_legs` IS an input, though (#27): the backend reads it
  // per call to decide whether a reference tie is traversable, so flipping it in
  // Settings changes the whole field. It has to be in the deps or the ramp keeps
  // showing the pre-toggle routing until some unrelated edit bumps modified_at.
  useEffect(() => {
    if (!showDistanceGradient) {
      distanceFailureToldRef.current = false;
      // [#576 RV7b-14] The failure hides the "computing" cue (C7-CUE); left set,
      // the next enable recomputed with no cue at all.
      setDistanceFieldFailed(false);
      return undefined;
    }
    // [#576 RV9b-2] A new fetch is under way, so nothing has failed yet and the
    // computing cue shows for it: an edit, a re-root or an include_reference_legs
    // flip after a failure used to recompute with the flag still set. The layer
    // is already base after that failure and 'hold' keeps it there - no blink.
    setDistanceFieldFailed(false);
    let cancelled = false;
    invokeIpc(
      'get_station_distances_from',
      { from: distanceGradientOrigin ?? null },
      { purpose: 'distance gradient field', fields: { origin: distanceGradientOrigin ?? 'entrance' } }
    )
      .then((field) => {
        if (!cancelled) {
          setDistanceField(field);
          setDistanceFieldFailed(false);
          distanceFailureToldRef.current = false;
        }
      })
      .catch((e) => {
        console.warn('[distance-gradient] field fetch failed', e);
        // Clear on failure (#34): drop any stale field + flag the failure so the
        // apply effect restores base colors instead of holding the old ramp
        // against the new origin/geometry. Guard on the cancel token so a
        // superseded fetch never clears a newer in-flight one.
        if (cancelled) return;
        setDistanceField(null);
        setDistanceFieldFailed(true);
        // The ramp silently dropping back to section colours read as "the
        // overlay is off" (#576 C7-MSG): say why, once for this failure.
        if (!distanceFailureToldRef.current) {
          distanceFailureToldRef.current = true;
          useUIStore.getState().showToast(
            i18n.t('map.distanceLegend.computeFailed'),
            TOAST_KEYS.DISTANCE_GRADIENT_FAILED,
            'error'
          );
        }
      });
    return () => { cancelled = true; };
  }, [
    showDistanceGradient,
    distanceGradientOrigin,
    project?.modified_at,
    config?.gas.include_reference_legs,
    setDistanceField,
  ]);

  // Gas reachable-frontier overlay (#38): fetch the reachability field when the
  // sub-mode is on and any input (origin / diver / cylinders / config) or the
  // project data changes. Like distances it uses MEASURED geometry, so the display
  // geometryMode / closure config don't affect it. No debounce: the compute is
  // ms-fast and the inputs are discrete (number fields / presets), not continuous
  // sliders. `origin` null = the entrance (zero-datum) default; the command
  // returns null when no project is open (Ok(None)) or throws on invalid input.
  useEffect(() => {
    if (!showGasPlan) return undefined;
    // R9/R10 (#395): before the diver has answered "Where does this dive start?" there is
    // nothing to shade - no shading, no markers, no legend (the legend is already gated on
    // `gasReachField`). Clear any stale field left from a previous session/project so the entry
    // card never resurrects a leftover ramp.
    if (!gasOriginChosen) {
      // Not merely "field not yet fetched" - flag it like a failed fetch, or
      // `overlayApplyAction` reads the null field as "recompute in flight" and
      // HOLDS the previous project's ramp under the entry card (#395). The next
      // origin choice + successful fetch clears the flag.
      setGasReachField(null);
      setGasFieldFailed(true);
      return undefined;
    }
    if (gasComputeGate.skip) {
      // A stalled plan (#310) runs NO compute at all, and the store has already nulled the
      // field - flag it like a failed fetch, or `overlayApplyAction` reads the null field as
      // "recompute in flight" and holds the last ramp's reachable colors under the
      // unresolved-pin warning indefinitely. The next successful fetch clears the flag.
      setGasFieldFailed(true);
      return undefined;
    }
    let cancelled = false;
    invokeIpc(
      'compute_gas_reach_field',
      {
        origin: gasComputeGate.origin,
        endpoint: gasComputeGate.endpoint,
        // Ordered way points (#298): the engine re-bases the whole field through them, so this is
        // a gas input, not a drawing hint.
        waypoints: gasWaypoints,
        // Per-span pace/propulsion (#317): also a gas input - the engine derates in-leg
        // consumption while a span is `on_scooter`.
        segments: gasSegments,
        diver: gasDiver,
        cylinders: gasCylinders,
        gasConfig: gasPlanConfig,
        // Per-stage drop overrides (#246): the station each ▼ marker was dragged to. Empty when
        // nothing is dragged; the backend clamps each pin before it can move the budget.
        stageDropOverrides: gasComputeGate.stageDropOverrides,
      },
      { purpose: 'gas reachable-frontier field', fields: { origin: gasComputeGate.origin ?? 'entrance' } }
    )
      .then((field) => { if (!cancelled) { setGasReachField(field); setGasFieldFailed(false); } })
      .catch((e) => {
        console.warn('[gas-plan] field fetch failed', e);
        // Clear on failure (#38): mirror the distance overlay - drop the stale
        // field + flag the failure so the apply effect restores base colors
        // rather than holding the old ramp against the new gas inputs.
        if (!cancelled) { setGasReachField(null); setGasFieldFailed(true); }
      });
    return () => { cancelled = true; };
  }, [showGasPlan, gasOriginChosen, gasComputeGate, gasWaypoints, gasSegments, gasDiver, gasCylinders, gasPlanConfig, project?.modified_at, setGasReachField]);

  // Apply the fetched field to the layer (feature-state + paint swap). Separate
  // from the rebuild path so re-rooting never triggers a feature rebuild.
  useEffect(() => {
    if (!surveyLayerRef.current || !mapLoaded) return;
    if (showDistanceGradient && !effectiveIssueSeverity && distanceField && surveyData) {
      // Measured SoA: distances use surveyed geometry, and the builder only needs
      // section metadata + shot ids - never the lazy `section.shots` objects.
      const state = buildDistanceFeatureState(surveyData, distanceField);
      surveyLayerRef.current.setDistanceGradient(true, state);
    } else if (overlayApplyAction(showDistanceGradient && !effectiveIssueSeverity, distanceField != null, surveyData != null, distanceFieldFailed) === 'off') {
      // Real off (mode disabled, severity coloring wins, no geometry, or a FAILED
      // recompute) -> base now. NOT while the origin is recomputing successfully
      // (field transiently null, mode still on, no failure): 'hold' keeps the old
      // ramp so a re-root never blinks base (#customRender).
      surveyLayerRef.current.setDistanceGradient(false, null);
    }
  }, [showDistanceGradient, effectiveIssueSeverity, distanceField, surveyData, mapLoaded, surveyLayerRef, distanceFieldFailed]);

  // Apply the gas field to the layer (feature-state + paint swap). Separate from
  // the fetch so re-rooting / re-computing never triggers a feature rebuild.
  useEffect(() => {
    if (!surveyLayerRef.current || !mapLoaded) return;
    if (showGasPlan && gasReachField && surveyData) {
      // Measured SoA (see distance-gradient effect): builder reads shot ids from
      // typed arrays, never the lazy `section.shots` objects.
      const state = buildGasReachFeatureState(surveyData, gasReachField);
      surveyLayerRef.current.setGasReach(true, state);
    } else if (overlayApplyAction(showGasPlan, gasReachField != null, surveyData != null, gasFieldFailed) === 'off') {
      // 'hold' during a SUCCESSFUL gas param/origin recompute (field transiently
      // null) keeps the old ramp; a real off OR a failed recompute restores base
      // (mirrors the distance guard).
      surveyLayerRef.current.setGasReach(false, null);
    }
  }, [showGasPlan, gasReachField, surveyData, mapLoaded, surveyLayerRef, gasFieldFailed]);

  // Plan-mode idle tint (#395 revision): while Plan is active but no gas origin has
  // been chosen yet, tint the whole survey the gas ramp's dim far color instead of
  // leaking section colors under the gas color scheme. Driven purely off
  // `gasOriginChosen`, not off `gasReachField` - SurveyLayer itself yields to a real
  // field the moment one is set (see setGasReach / isPlanIdleActive), so this clears
  // the instant an origin is chosen even before the field's first fetch resolves.
  useEffect(() => {
    if (!surveyLayerRef.current || !mapLoaded) return;
    surveyLayerRef.current.setPlanIdleTint(showGasPlan && !gasOriginChosen);
  }, [showGasPlan, gasOriginChosen, mapLoaded, surveyLayerRef]);

  // Highlight the picked route over the survey (#233): full-opacity swum legs + faded jumps (#243).
  useEffect(() => {
    if (!surveyLayerRef.current || !mapLoaded) return;
    if (showGasPlan && gasReachField && project) {
      surveyLayerRef.current.setGasRoute(
        gasReachField.route,
        routeLegSegmentIndices(gasReachField).map(segmentColor),
      );
    } else {
      surveyLayerRef.current.setGasRoute([]);
    }
  }, [showGasPlan, gasReachField, project, mapLoaded, surveyLayerRef]);

  // Distance-gradient origin marker (#34): a ring on the chosen origin station.
  // A MapLibre Marker so it auto-tracks the camera and shows even when the
  // stations layer is hidden (common with the gradient on). Only for a *picked*
  // origin - the entrance default is the zero datum(s), already starred.
  useEffect(() => {
    originMarkerRef.current?.remove();
    originMarkerRef.current = null;
    // Suppressed while issue-severity owns the paint (the distance overlay isn't
    // drawn then), so the picked-origin ring doesn't linger over a severity map.
    if (!map.current || !mapLoaded || !showDistanceGradient || effectiveIssueSeverity || !distanceGradientOrigin) {
      return undefined;
    }
    const originStation = activeStations.find(
      (s) =>
        s.station_ref.section_id === distanceGradientOrigin.section_id &&
        s.station_ref.station_index === distanceGradientOrigin.station_index
    );
    if (!originStation) return undefined;
    const lngLat = metersToMapCoords(
      originStation.coordinates.x,
      originStation.coordinates.y,
      gpsOrigin
    ) as [number, number];
    const el = document.createElement('div');
    el.setAttribute('data-testid', 'distance-origin-marker');
    el.style.cssText =
      'width:18px;height:18px;border-radius:50%;border:3px solid #7d3c98;' +
      'box-shadow:0 0 0 2px #ffffff,0 0 4px rgba(0,0,0,0.6);' +
      'background:rgba(255,255,255,0.15);pointer-events:none;';
    originMarkerRef.current = new maplibregl.Marker({ element: el })
      .setLngLat(lngLat)
      .addTo(map.current);
    return () => {
      originMarkerRef.current?.remove();
      originMarkerRef.current = null;
    };
  }, [showDistanceGradient, effectiveIssueSeverity, distanceGradientOrigin, activeStations, gpsOrigin, mapLoaded]);

  // Gas-plan origin marker (#38): a teal ring on the chosen origin (mirrors the
  // distance origin marker; teal to distinguish "start" from the gold→purple
  // reachable ramp). Only for a *picked* origin - the entrance default is the
  // zero datum(s), already starred.
  useEffect(() => {
    gasOriginMarkerRef.current?.remove();
    gasOriginMarkerRef.current = null;
    if (!map.current || !mapLoaded || !showGasPlan || !gasOrigin) return undefined;
    const originStation = activeStations.find(
      (s) =>
        s.station_ref.section_id === gasOrigin.section_id &&
        s.station_ref.station_index === gasOrigin.station_index
    );
    if (!originStation) return undefined;
    const lngLat = metersToMapCoords(
      originStation.coordinates.x,
      originStation.coordinates.y,
      gpsOrigin
    ) as [number, number];
    const el = document.createElement('div');
    el.setAttribute('data-testid', 'gas-origin-marker');
    el.style.cssText =
      'width:18px;height:18px;border-radius:50%;border:3px solid #16a085;' +
      'box-shadow:0 0 0 2px #ffffff,0 0 4px rgba(0,0,0,0.6);' +
      'background:rgba(255,255,255,0.15);pointer-events:none;';
    gasOriginMarkerRef.current = new maplibregl.Marker({ element: el })
      .setLngLat(lngLat)
      .addTo(map.current);
    return () => {
      gasOriginMarkerRef.current?.remove();
      gasOriginMarkerRef.current = null;
    };
  }, [showGasPlan, gasOrigin, activeStations, gpsOrigin, mapLoaded]);

  return { distanceFieldFailed };
}
