import { useCallback, useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { useShallow } from 'zustand/react/shallow';
import { planSpeed, useGasPlanStore } from '../../store/gasPlanStore';
import { SpreadsheetGrid, type CellEdit } from '../common/SpreadsheetGrid';
import { useGridLabels } from '../../hooks/useGridLabels';
import { getNumberFormatLocale } from '../../i18n/format';
import { formatStationId } from '../../utils/formatters';
import { speedToDisplay } from '../../utils/formatGas';
import { buildSegmentColumns } from './segmentsGridColumns';
import { applySegmentEdits, SEG_COL } from './segmentsGridEdits';
import { planGridHeight, usePlanGridFillHeight } from './planGridHeight';
import { usePasteReportToast } from './usePasteReportToast';
import { TOAST_KEYS } from '../../utils/toastKeys';
import type { PlanSegment } from '../../types/ipc.generated';
import type { StationRef } from '../../types/project';

/** Rows behind the FALLBACK height (before the pane is measured, and under jsdom).
 *  The grid fills its tab now, so this only caps the pre-measurement guess. */
const MAX_VISIBLE_ROWS = 12;

function sameStation(a: StationRef | null, b: StationRef | null): boolean {
  return a != null && b != null && a.section_id === b.section_id && a.station_index === b.station_index;
}

/**
 * The per-span pace/propulsion editor (#317) - the "Segments" tab of the Plan-mode bottom
 * panel. A plan has exactly `waypoints.length + 1` segments: segment k spans way point
 * k-1 -> k, and the last covers the open field past the final way point (the whole route,
 * when there are no way points at all). Edits flow to gasPlanStore, which nulls the cached
 * reach field so the map overlay refetches - a segment is a gas input, same as the
 * cylinders/config it sits next to.
 *
 * Hosted on the shared `SpreadsheetGrid` (#403): the columns live in
 * `segmentsGridColumns.tsx` and every write - typed, pasted, filled, cleared, or
 * clicked straight on the checkbox - funnels through `applySegmentEdits`, so the
 * scooter/speed coupling below cannot be bypassed by whichever gesture the diver
 * used. Rows are STRUCTURAL (one per span, minted by the store when a way point is
 * added), so the grid gets no insert/delete/move wiring.
 *
 * `on_scooter` covers both towing and parking a DPV - they are numerically identical
 * (swim-speed gas, no scooter range spent) and differ only in narrative, so there is no
 * separate "parked" state to set here. The park point itself is DERIVED by the engine (the
 * boundary after the last scootered segment, `GasReachField.scooter_drop_ref`) and can only
 * ever land on one of the way points (never the origin or the open-ended tail - see
 * `scooter_drop_index`, gas/scooter.rs) - the panel just marks whichever boundary it names,
 * it never places it.
 */
export function SegmentsTable() {
  const { t } = useTranslation();
  const { segments, waypoints, origin, endpoint, gasConfig, reachField } = useGasPlanStore(
    useShallow((s) => ({
      segments: s.segments,
      waypoints: s.waypoints,
      origin: s.origin,
      endpoint: s.endpoint,
      gasConfig: s.gasConfig,
      reachField: s.reachField,
    }))
  );
  const units = gasConfig.units;
  // Riding a DPV needs a rig configured in the DPV tab; with none, the checkbox is
  // disabled (not hidden) so the reason stays visible in place, mirroring the backend:
  // `resolve_segments` drops `on_scooter` entirely without a rig, so a checked-but-inert
  // box would be a lie.
  const hasRig = gasConfig.dpv != null;
  const dropRef = reachField?.scooter_drop_ref ?? null;
  // A span with no speed of its own is flown at the plan's speed, so that is what its cell
  // shows (dimmed, so it never reads as a number someone typed there). Typing sets an
  // explicit speed for that span only; the rest keep following the Swim-speed field above
  // and Settings > Gas.
  const inheritedSpeedDisplay = speedToDisplay(planSpeed(gasConfig), units);

  // Every boundary the panel can NAME, in visit order: before segment 0 (the origin, or the
  // entrance default), each way point in between, and after the last segment (the chosen
  // endpoint, or nothing when the route simply ends wherever the budget does). Segment k
  // spans boundary[k] -> boundary[k + 1]. Labeled by ROLE (Origin/WP n/Endpoint), not by
  // station id - the id is still discoverable via a `title` tooltip (`boundaryRefs`) for
  // whichever boundaries actually resolve to one.
  const boundaryLabels = useMemo(
    () => [
      origin ? t('gasPanel.segments.origin') : t('gasPanel.origin.entrance'),
      ...waypoints.map((_, k) => t('gasPanel.segments.waypointN', { n: k + 1 })),
      endpoint ? t('gasPanel.segments.endpoint') : t('gasPanel.segments.onward'),
    ],
    [t, origin, waypoints, endpoint]
  );
  const boundaryRefs = useMemo<(StationRef | null)[]>(
    () => [origin, ...waypoints, endpoint],
    [origin, waypoints, endpoint]
  );

  // The single write path. Reads live state rather than the rendered snapshot (the
  // GasPlanPanel pattern), so a commit that lands after an unrelated store change still
  // folds into the current segments; an unchanged result skips the write, because
  // `setSegments` nulls the cached reach field and a gratuitous null there costs a full
  // map recompute.
  const applyEdits = useCallback(
    (edits: CellEdit[]) => {
      const s = useGasPlanStore.getState();
      const next = applySegmentEdits(s.segments, edits, { units, dpv: s.gasConfig.dpv });
      if (next !== s.segments) s.setSegments(next);
    },
    [units]
  );

  const columns = useMemo(
    () =>
      buildSegmentColumns({
        t,
        units,
        hasRig,
        inheritedSpeedDisplay,
        boundaryLabelAt: (k) => boundaryLabels[k] ?? '',
        boundaryTitleAt: (k) => {
          const ref = boundaryRefs[k] ?? null;
          return ref ? formatStationId(ref.section_id, ref.station_index) : undefined;
        },
        parksAfter: (i) => sameStation(dropRef, boundaryRefs[i + 1] ?? null),
        onToggleScooter: (i, next) => applyEdits([{ row: i, colKey: SEG_COL.scooter, value: next }]),
      }),
    [t, units, hasRig, inheritedSpeedDisplay, boundaryLabels, boundaryRefs, dropRef, applyEdits]
  );

  const labels = useGridLabels('gasPanel.segments.segmentNoun');
  const onPasteReport = usePasteReportToast(TOAST_KEYS.PLAN_SEGMENTS_PASTE);
  const onEdit = useCallback(
    (row: number, colKey: string, value: unknown) => applyEdits([{ row, colKey, value }]),
    [applyEdits]
  );

  // The grid owns the whole tab body (minus the no-rig hint below it), so its height is the
  // MEASURED pane rather than the row count; the content-sized number is only the
  // pre-measurement/jsdom fallback.
  const { ref: gridBoxRef, height } = usePlanGridFillHeight(
    planGridHeight(segments.length, MAX_VISIBLE_ROWS)
  );

  return (
    // Flush with the tab body, the way the Survey data table hosts its grid: the only
    // thing between the grid and the panel edge is the no-rig hint, which is a compact
    // strip along the bottom rather than a padding frame around everything.
    <div className="h-full flex flex-col text-sm text-text-primary" data-testid="segments-table">
      {/* `overflow-hidden` for DataTable's reason: the grid scrolls internally, and a
          second scroller around it would double the scrollbars. */}
      <div ref={gridBoxRef} className="flex-1 min-h-0 overflow-hidden">
        <SpreadsheetGrid<PlanSegment>
          columns={columns}
          rows={segments}
          // Segments have no id of their own - position IS the identity (segment k spans
          // boundary k -> k + 1), and the store keeps the list exactly `waypoints.length + 1`
          // long.
          rowKey={(_seg, index) => index}
          testIdPrefix="segment"
          // The speed cell keeps its pre-grid test id, which is what the e2e specs and the
          // inherited-speed marker next to it are addressed by.
          cellTestId={(_seg, index, colKey) =>
            colKey === SEG_COL.speed ? `segment-speed-${index}` : undefined
          }
          locale={getNumberFormatLocale()}
          height={height}
          onEdit={onEdit}
          onBatchEdit={applyEdits}
          onPasteReport={onPasteReport}
          labels={labels}
        />
      </div>
      {!hasRig && (
        <p
          className="shrink-0 border-t border-border px-3 py-1 text-xs opacity-60"
          data-testid="segments-no-rig-hint"
        >
          {t('gasPanel.segments.noRigHint')}
        </p>
      )}
    </div>
  );
}
