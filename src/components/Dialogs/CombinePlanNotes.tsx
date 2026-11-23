import { Trans, useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { DetailLink } from '../common/DetailLink';
import { useIdentityLabels } from '../../hooks/useIdentityLabels';
import type { LengthUnit } from '../../types/config';
import type { CombineCandidate } from '../../types/ipc/CombineCandidate';
import type { CombinePlan } from '../../types/ipc/CombinePlan';
import type { CombineRemovedKind } from '../../types/ipc/CombineRemovedKind';
import type { CombineReplacedKind } from '../../types/ipc/CombineReplacedKind';
import type { TornTieKind } from '../../types/ipc/TornTieKind';
import type { LineId, StationRef } from '../../types/project';
import { formatLength } from '../../utils/formatStats';
import {
  COMBINE_SIDE_BY_SIDE_M,
  combineMoveIsSmall,
  combineMoveShown,
  formatRotation,
} from '../../utils/combineLines';

interface CombinePlanNotesProps {
  candidate: CombineCandidate;
  plan: CombinePlan;
  keptId: LineId;
  keptName: string;
  otherName: string;
  keptQuality: number | undefined;
  otherQuality: number | undefined;
  lengthUnit: LengthUnit;
  lineName: (id: LineId) => string;
  /** A station by name (`17`); with `withLine`, `Main Line / 17`. */
  stationLabel: (ref: StationRef, withLine?: boolean) => string;
}

/**
 * Everything a combine changes besides joining the shots, for the kept
 * line the dialog currently shows (the backend's `CombinePlan`): the side
 * that moves and how far (and what moves with the joined station instead,
 * #755), the ties that become loop closures, the ties
 * removed because as closures they would join the two ends of one shot
 * (#746), the closures replaced by a loop closure on the same two stations
 * that the Closures Computed View adjusts (#749), converted bearings, the
 * quality and manual offset the combined line keeps, and lines that pass
 * through the join. (Shot identities are project-unique on the station graph,
 * so none is renewed: `repeated_shot_ids` is always 0 and has no note.) The user reads all of
 * it before confirming; nothing here is a refusal.
 */
export function CombinePlanNotes({
  candidate,
  plan,
  keptId,
  keptName,
  otherName,
  keptQuality,
  otherQuality,
  lengthUnit,
  lineName,
  stationLabel,
}: CombinePlanNotesProps) {
  const { t } = useTranslation();
  const distance = (m: number) => formatLength(m, lengthUnit, 2);
  const listNames = (ids: LineId[]) => ids.map(lineName).join(', ');
  // The line whose join station the joined station keeps as shown: the
  // fixed side's, so the kept one unless the kept side moves.
  const otherId = candidate.line_a === keptId ? candidate.line_b : candidate.line_a;
  const fixedId = plan.moving_line === keptId ? otherId : keptId;
  const notes: Array<{ key: string; text: string }> = [];

  // A move within the side-by-side tolerance (#740) is planned exactly but
  // listed only when it changes a tie: see `combineMoveShown`.
  if (plan.moving_line !== null && combineMoveShown(plan)) {
    const moving = lineName(plan.moving_line);
    const key = combineMoveIsSmall(plan)
      ? 'moveSideBySide'
      : candidate.join_kind === 'closure'
        ? 'moveClosure'
        : 'moveGap';
    notes.push({
      key: 'move',
      text: t(`dialogs.combineSections.${key}`, {
        moving,
        distance: formatLength(plan.shift_m, lengthUnit, combineMoveIsSmall(plan) ? 4 : 2),
      }),
    });
    const attached = plan.moved_lines.filter((id) => id !== plan.moving_line);
    if (attached.length > 0) {
      notes.push({
        key: 'move-with',
        text: t('dialogs.combineSections.movesWith', { names: listNames(attached), count: attached.length }),
      });
    }
    if (plan.moving_line === keptId) {
      // Why the kept side moves: a zero datum holds the other side, or the
      // other side holds the point the Closures Computed View is held at.
      const key = plan.kept_moves_because === 'adjustment_anchor' ? 'keptMovesAnchor' : 'keptMoves';
      notes.push({ key: 'kept-moves', text: t(`dialogs.combineSections.${key}`, { kept: keptName, other: otherName }) });
    }
  }
  if (plan.azimuth_rotation_deg !== null) {
    notes.push({
      key: 'rotation',
      text:
        // Exactly 0 is the one case the backend leaves the readings alone.
        plan.azimuth_rotation_deg === 0
          ? t('dialogs.combineSections.rotationZero', { kept: keptName, other: otherName })
          : t('dialogs.combineSections.rotation', {
              kept: keptName,
              other: otherName,
              degrees: formatRotation(plan.azimuth_rotation_deg),
            }),
    });
  }
  if (keptQuality !== undefined && otherQuality !== undefined && Math.abs(keptQuality - otherQuality) > 1e-12) {
    notes.push({
      key: 'quality',
      text: t('dialogs.combineSections.quality', {
        kept: keptName,
        other: otherName,
        keptQuality: String(keptQuality),
        otherQuality: String(otherQuality),
      }),
    });
  }
  if (plan.join_offset_change_m !== null) {
    const fixedName = plan.moving_line === keptId ? otherName : keptName;
    const movedName = plan.moving_line === keptId ? keptName : otherName;
    notes.push({
      key: 'offset',
      text: t('dialogs.combineSections.offset', {
        fixed: fixedName,
        moving: movedName,
        distance: distance(plan.join_offset_change_m),
      }),
    });
  }
  // What is merged at the join station whose hand move the joined station
  // does not keep moves with the joined station (#755), listed as a move of
  // the side is: under the side-by-side tolerance only beside a torn tie.
  const joined = plan.moved_with_joined_station;
  if (joined && (joined.shift_m > COMBINE_SIDE_BY_SIDE_M || plan.torn_ties.length > 0)) {
    notes.push({
      key: 'joined-moves',
      text: t('dialogs.combineSections.joinedMoves', {
        names: listNames(joined.lines),
        count: joined.lines.length,
        station: stationLabel(joined.station, true),
        distance: formatLength(joined.shift_m, lengthUnit, joined.shift_m <= COMBINE_SIDE_BY_SIDE_M ? 4 : 2),
      }),
    });
  }
  if (candidate.passing_through.length > 0) {
    notes.push({
      key: 'passing',
      text: t('dialogs.combineSections.passingNote', {
        names: listNames(candidate.passing_through),
        count: candidate.passing_through.length,
      }),
    });
  }

  // The plan the combine itself runs names these (#746): exactly what it removes.
  const removed = plan.closures_removed_across_one_shot ?? [];
  // And these (#749): exactly the closures it replaces.
  const replaced = plan.closures_replaced ?? [];
  const tieId = (a: StationRef, b: StationRef) =>
    `${a.line_id}-${a.station_index}-${b.line_id}-${b.station_index}`;
  const ends = (a: StationRef, b: StationRef) => ({
    a: <TieEnd at={a} lineName={lineName} stationLabel={stationLabel} />,
    b: <TieEnd at={b} lineName={lineName} stationLabel={stationLabel} />,
  });

  if (notes.length === 0 && plan.torn_ties.length === 0 && removed.length === 0 && replaced.length === 0) {
    return null;
  }
  return (
    <div className="space-y-2 text-sm" data-testid="combine-plan-notes">
      <div className="text-text-secondary">{t('dialogs.combineSections.changesTitle')}</div>
      <ul className="list-disc pl-5 space-y-1">
        {notes.map((note) => (
          <li key={note.key} data-testid={`combine-note-${note.key}`}>
            {note.text}
          </li>
        ))}
        {plan.torn_ties.length > 0 && (
          <li data-testid="combine-note-torn">
            {t('dialogs.combineSections.tornTitle', { count: plan.torn_ties.length })}
            <ul className="list-[circle] pl-5 mt-1 space-y-0.5">
              {plan.torn_ties.map((tie) => {
                const id = tieId(tie.station_a, tie.station_b);
                // The datum side is the end that does not move with the moving line.
                const held = [tie.station_a, tie.station_b].find(
                  (s) => !plan.moved_lines.includes(s.line_id)
                );
                return (
                  <li key={id} data-testid={`combine-torn-tie-${id}`}>
                    <Trans
                      i18nKey={`dialogs.combineSections.${TORN_KEY[tie.kind]}`}
                      components={ends(tie.station_a, tie.station_b)}
                    />
                    {tie.reason === 'zero_datum' && held && (
                      <>
                        {/* The tie, then the held line as its own sentence. */}
                        {'. '}
                        <Trans
                          i18nKey="dialogs.combineSections.tornHeld"
                          components={{ held: <LinePill id={held.line_id} name={lineName(held.line_id)} /> }}
                        />
                      </>
                    )}
                    {tie.reason === 'joined_station_moves' && (
                      <>
                        {/* The joined station keeps the fixed side's position as shown (#736). */}
                        {'. '}
                        <Trans
                          i18nKey="dialogs.combineSections.tornJoinedStation"
                          values={{ distance: distance(SNAP_ALIGNMENT_M) }}
                          components={{ fixed: <LinePill id={fixedId} name={lineName(fixedId)} /> }}
                        />
                      </>
                    )}
                  </li>
                );
              })}
            </ul>
            {plan.torn_ties.some((tie) => tie.reason === 'loop') && (
              <span className="block mt-1 text-text-secondary" data-testid="combine-torn-why">
                {t(
                  combineMoveShown(plan)
                    ? 'dialogs.combineSections.tornWhyMoves'
                    : 'dialogs.combineSections.tornWhyStays'
                )}
              </span>
            )}
          </li>
        )}
        {removed.length > 0 && (
          <li data-testid="combine-note-removed">
            {t('dialogs.combineSections.removedTitle', { count: removed.length })}
            <ul className="list-[circle] pl-5 mt-1 space-y-0.5">
              {removed.map((tie) => {
                const id = tieId(tie.station_a, tie.station_b);
                return (
                  <li key={id} data-testid={`combine-removed-tie-${id}`}>
                    <Trans
                      i18nKey={`dialogs.combineSections.${REMOVED_KEY[tie.kind]}`}
                      components={ends(tie.station_a, tie.station_b)}
                    />
                  </li>
                );
              })}
            </ul>
            <span className="block mt-1 text-text-secondary" data-testid="combine-removed-why">
              {t('dialogs.combineSections.removedWhy', { count: removed.length })}
            </span>
          </li>
        )}
        {replaced.length > 0 && (
          <li data-testid="combine-note-replaced">
            {t('dialogs.combineSections.replacedTitle', { count: replaced.length })}
            <ul className="list-[circle] pl-5 mt-1 space-y-0.5">
              {replaced.map((closure, index) => {
                const id = tieId(closure.station_a, closure.station_b);
                return (
                  <li key={`${id}-${index}`} data-testid={`combine-replaced-tie-${id}`}>
                    <Trans
                      i18nKey={`dialogs.combineSections.${REPLACED_KEY[closure.kind]}`}
                      components={ends(closure.station_a, closure.station_b)}
                    />{' '}
                    <Trans
                      i18nKey={`dialogs.combineSections.${REPLACED_BY_KEY[closure.by_kind]}`}
                      components={ends(closure.by_station_a, closure.by_station_b)}
                    />
                  </li>
                );
              })}
            </ul>
            <span className="block mt-1 text-text-secondary" data-testid="combine-replaced-why">
              {t('dialogs.combineSections.replacedWhy', { count: replaced.length })}
            </span>
          </li>
        )}
      </ul>
    </div>
  );
}

/** The dialog line for each kind of tie that becomes a loop closure. */
const TORN_KEY: Record<TornTieKind, string> = {
  merge: 'tornMerge',
  snapped_closure: 'tornSnapped',
  former_snapped_closure: 'tornFormerSnap',
};

/**
 * How close a snapped closure's two stations must be for it to stay snapped
 * (`Project::MERGE_ALIGNMENT_INVARIANT_M`): the distance a pair the joined
 * station takes apart ends up past (#736).
 */
const SNAP_ALIGNMENT_M = 0.01;

/** The dialog line for each kind of tie the combine removes (#746). */
const REMOVED_KEY: Record<CombineRemovedKind, string> = {
  merge: 'removedMerge',
  snapped_closure: 'removedSnapped',
  former_snapped_closure: 'removedFormerSnap',
  loop_closure: 'removedClosure',
};

/** The dialog line for each kind of closure the combine replaces (#749). */
const REPLACED_KEY: Record<CombineReplacedKind, string> = {
  legacy_closure: 'replacedLegacy',
  snapped_closure: 'replacedSnapped',
  former_snap: 'replacedFormerSnap',
};

/** What replaces it, as what it was before the combine (#749). */
const REPLACED_BY_KEY: Record<CombineRemovedKind, string> = {
  merge: 'replacedByMerge',
  snapped_closure: 'replacedBySnapped',
  former_snapped_closure: 'replacedByFormerSnap',
  loop_closure: 'replacedByClosure',
};

/** A line as a link pill (its name), selecting it on click. */
function LinePill({ id, name }: { id: LineId; name: string }) {
  return (
    <DetailLink
      type="line"
      target={{ lineId: id }}
      displayText={name}
      pillLabel={name}
      testId={`combine-tie-line-${id}`}
      onSelect={() => useUIStore.getState().setSelection({ type: 'line', lineId: id })}
      onHoverStart={() => useUIStore.getState().setHoveredLineId(id)}
      onHoverEnd={() => useUIStore.getState().setHoveredLineId(null)}
    />
  );
}

/**
 * One end of a tie the combine turns into a loop closure: its line and its
 * station (by name, beside its line's pill) as the link pills the Details
 * panel uses. Hovering the station rings it on the map behind the dialog.
 * The pill draws a dotted slot's dot and loop mark beside its bare name; the
 * plain text and the spoken name carry the qualifier (`12 (Main)`), the line
 * pill beside it the line.
 */
function TieEnd({
  at,
  lineName,
  stationLabel,
}: {
  at: StationRef;
  lineName: (id: LineId) => string;
  stationLabel: (ref: StationRef) => string;
}) {
  const labels = useIdentityLabels();
  return (
    <span className="inline-flex flex-wrap items-center gap-1 align-middle">
      <LinePill id={at.line_id} name={lineName(at.line_id)} />
      <DetailLink
        type="station"
        target={{ lineId: at.line_id, position: at.station_index }}
        displayText={stationLabel(at)}
        pillLabel={labels.stationPill(at)}
        testId={`combine-tie-station-${at.line_id}-${at.station_index}`}
        onSelect={() => useUIStore.getState().setSelection({ type: 'station', stationRef: at })}
        onHoverStart={() => useUIStore.getState().setHoveredStation(at)}
        onHoverEnd={() => useUIStore.getState().setHoveredStation(null)}
      />
    </span>
  );
}
