/**
 * GridColumn factory for the Plan-mode Segments grid (#403), mirroring
 * `src/components/common/editableShotColumns.tsx`: a pure factory taking
 * everything it needs through one context object, so the columns can be built
 * outside React and the component keeps only the store wiring.
 *
 * ⚠ UNIT SPACE. The grid's value space is DISPLAY units (m/min or ft/min); the
 * store is canonical m/min. `getValue` therefore converts canonical -> display,
 * and `applySegmentEdits` converts back exactly once. This is not a style
 * choice: the cell editor is seeded from `getValue`, and fill/internal-paste
 * transfer that raw result WITHOUT running it through `parse`, so `getValue`
 * must return the number the diver would have typed. Returning canonical here
 * would silently re-price every filled span in the other unit system.
 */
import type { ReactNode } from 'react';
import type { TFunction } from 'i18next';
import type { GridColumn } from '../common/SpreadsheetGrid/types';
import type { GasUnits, PlanSegment } from '../../types/ipc.generated';
import { formatFixed } from '../../i18n/format';
import { speedToDisplay } from '../../utils/formatGas';
import { segmentColor } from '../../utils/segmentColors';
import { parseGridBoolean, SEG_COL } from './segmentsGridEdits';

export interface SegmentColumnContext {
  t: TFunction;
  units: GasUnits;
  /** A DPV rig is configured, so the scooter column is live. Without one the
   *  backend drops `on_scooter` entirely, so the box is disabled (not hidden) -
   *  a checked-but-inert box would be a lie. */
  hasRig: boolean;
  /** Role label of boundary `k` (Origin / WP n / Endpoint). Segment `i` spans
   *  boundary `i` -> `i + 1`. */
  boundaryLabelAt: (boundaryIndex: number) => string;
  /** Station id of boundary `k` when it resolves to one - the tooltip that keeps
   *  the id discoverable without putting it on the row. */
  boundaryTitleAt: (boundaryIndex: number) => string | undefined;
  /** The DPV is parked at the boundary AFTER segment `i` (derived by the engine
   *  from the scootered spans, never set here). */
  parksAfter: (rowIndex: number) => boolean;
  /** The plan speed a span with no speed of its own is flown at, already in
   *  DISPLAY units. */
  inheritedSpeedDisplay: number;
  onToggleScooter: (rowIndex: number, next: boolean) => void;
}

export function buildSegmentColumns(ctx: SegmentColumnContext): GridColumn<PlanSegment>[] {
  const {
    t,
    units,
    hasRig,
    boundaryLabelAt,
    boundaryTitleAt,
    parksAfter,
    inheritedSpeedDisplay,
    onToggleScooter,
  } = ctx;
  const speedUnit = t(
    units === 'imperial' ? 'gasPanel.swimSpeed.unitImperial' : 'gasPanel.swimSpeed.unit'
  );

  return [
    {
      key: SEG_COL.from,
      header: t('gasPanel.segments.colFrom'),
      width: 120,
      align: 'left',
      getValue: (_seg, index) => boundaryLabelAt(index),
      // The per-segment identity color the map draws the same span in, folded
      // into the cell so the two surfaces read as one object.
      render: (_seg, index): ReactNode => (
        <span className="flex items-center gap-2 font-mono" title={boundaryTitleAt(index)}>
          <span
            aria-hidden="true"
            data-testid={`segment-color-${index}`}
            className="inline-block rounded-sm"
            style={{ width: '10px', height: '10px', backgroundColor: segmentColor(index) }}
          />
          {boundaryLabelAt(index)}
        </span>
      ),
    },
    {
      key: SEG_COL.to,
      header: t('gasPanel.segments.colTo'),
      width: 150,
      grow: true,
      align: 'left',
      getValue: (_seg, index) => boundaryLabelAt(index + 1),
      render: (_seg, index): ReactNode => (
        <span className="flex items-center gap-2 font-mono" title={boundaryTitleAt(index + 1)}>
          {boundaryLabelAt(index + 1)}
          {parksAfter(index) && (
            <span
              className="text-xs opacity-70"
              title={t('gasPanel.segments.parkHereHint')}
              data-testid={`segment-park-${index}`}
            >
              {t('gasPanel.segments.parkHere')}
            </span>
          )}
        </span>
      ),
    },
    {
      key: SEG_COL.speed,
      // The unit lives in the header: an in-cell suffix would land in the
      // clipboard on every copy.
      header: t('gasPanel.segments.colSpeedUnit', { unit: speedUnit }),
      width: 110,
      align: 'right',
      // Nullable: clearing the cell returns the span to the plan speed, which is
      // the only way back once a number has been typed. `null` is also what the
      // clipboard exports for an inherited span - the model truth, not the
      // number the row happens to be showing.
      kind: 'number-nullable',
      getValue: (seg) =>
        seg.speed_m_per_min == null ? null : speedToDisplay(seg.speed_m_per_min, units),
      format: (value) => (typeof value === 'number' ? formatFixed(value, undefined, 1) : ''),
      editable: true,
      // No `validate`: the reducer CLAMPS out-of-range speeds, preserving the
      // pre-grid contract. Rejecting them instead would leave the editor open on
      // a value the engine would have quietly accepted at the boundary.
      render: (seg, index): ReactNode => {
        if (seg.speed_m_per_min != null) {
          // The Reset button is gone; clearing the cell is the only way back to
          // the plan speed, so the row that NEEDS that gesture must say so.
          return (
            <span title={t('gasPanel.segments.explicitHint')}>
              {formatFixed(speedToDisplay(seg.speed_m_per_min, units), undefined, 1)}
            </span>
          );
        }
        // An inherited span shows the plan speed it is actually flown at, dimmed
        // and italic so it never reads as a number someone typed here.
        return (
          <span
            className="italic opacity-60"
            title={t('gasPanel.segments.inheritedHint')}
            data-testid={`segment-speed-inherited-${index}`}
          >
            {formatFixed(inheritedSpeedDisplay, undefined, 1)}
          </span>
        );
      },
    },
    {
      key: SEG_COL.scooter,
      header: t('gasPanel.segments.colScooter'),
      width: 64,
      align: 'center',
      kind: 'text',
      editable: true,
      isEditable: () => hasRig,
      // The render IS the control, so no cell editor opens over it; paste/fill
      // still write through `parse`.
      noInlineEdit: true,
      getValue: (seg) => hasRig && seg.on_scooter,
      format: (value) => (value ? 'true' : 'false'),
      // Keyboard equivalent of clicking the box (F2 / Space).
      activate: (seg, index) => onToggleScooter(index, !(hasRig && seg.on_scooter)),
      parse: (raw, seg) => parseGridBoolean(raw, seg.on_scooter),
      render: (seg, index): ReactNode => (
        <input
          type="checkbox"
          checked={hasRig && seg.on_scooter}
          disabled={!hasRig}
          onChange={(e) => onToggleScooter(index, e.target.checked)}
          onClick={(e) => e.stopPropagation()}
          tabIndex={-1}
          title={hasRig ? undefined : t('gasPanel.segments.noRigHint')}
          aria-label={t('gasPanel.segments.scooterAria', {
            from: boundaryLabelAt(index),
            to: boundaryLabelAt(index + 1),
          })}
          data-testid={`segment-scooter-${index}`}
        />
      ),
    },
  ];
}
