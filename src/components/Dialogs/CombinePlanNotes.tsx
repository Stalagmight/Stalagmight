import { Trans, useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { DetailLink } from '../common/DetailLink';
import type { LengthUnit } from '../../types/config';
import type { CombineCandidate } from '../../types/ipc/CombineCandidate';
import type { CombinePlan } from '../../types/ipc/CombinePlan';
import type { CombineRemovedKind } from '../../types/ipc/CombineRemovedKind';
import type { CombineReplacedKind } from '../../types/ipc/CombineReplacedKind';
import type { SectionId, StationRef } from '../../types/project';
import { formatLength } from '../../utils/formatStats';
import { combineMoveShown, formatRotation } from '../../utils/combineSections';

interface CombinePlanNotesProps {
  candidate: CombineCandidate;
  plan: CombinePlan;
  keptId: SectionId;
  keptName: string;
  otherName: string;
  keptQuality: number | undefined;
  otherQuality: number | undefined;
  lengthUnit: LengthUnit;
  sectionName: (id: SectionId) => string;
  stationLabel: (ref: StationRef) => string;
}

/**
 * Everything a combine changes besides joining the shots, for the kept
 * section the dialog currently shows (the backend's `CombinePlan`): the side
 * that moves and how far, the ties that become loop closures, the ties
 * removed because as closures they would join the two ends of one shot
 * (#746), the closures replaced by a loop closure on the same two stations
 * that the Closures Computed View adjusts (#749), converted bearings, the
 * quality and manual offset the combined section keeps, shot identities that
 * are renewed, and sections that pass through the join. The user reads all of
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
  sectionName,
  stationLabel,
}: CombinePlanNotesProps) {
  const { t } = useTranslation();
  const distance = (m: number) => formatLength(m, lengthUnit, 2);
  const listNames = (ids: SectionId[]) => ids.map(sectionName).join(', ');
  const notes: Array<{ key: string; text: string }> = [];

  // A move within the side-by-side tolerance (#740) is planned exactly but
  // not listed: see `combineMoveShown`.
  if (plan.moving_section !== null && combineMoveShown(plan)) {
    const moving = sectionName(plan.moving_section);
    notes.push({
      key: 'move',
      text: t(
        candidate.join_kind === 'closure' ? 'dialogs.combineSections.moveClosure' : 'dialogs.combineSections.moveGap',
        { moving, distance: distance(plan.shift_m) }
      ),
    });
    const attached = plan.moved_sections.filter((id) => id !== plan.moving_section);
    if (attached.length > 0) {
      notes.push({
        key: 'move-with',
        text: t('dialogs.combineSections.movesWith', { names: listNames(attached), count: attached.length }),
      });
    }
    if (plan.moving_section === keptId) {
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
    const fixedName = plan.moving_section === keptId ? otherName : keptName;
    const movedName = plan.moving_section === keptId ? keptName : otherName;
    notes.push({
      key: 'offset',
      text: t('dialogs.combineSections.offset', {
        fixed: fixedName,
        moving: movedName,
        distance: distance(plan.join_offset_change_m),
      }),
    });
  }
  if (candidate.repeated_shot_ids > 0) {
    notes.push({
      key: 'repeated-ids',
      text: t('dialogs.combineSections.repeatedIds', { other: otherName, kept: keptName, count: candidate.repeated_shot_ids }),
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
    `${a.section_id}-${a.station_index}-${b.section_id}-${b.station_index}`;
  const ends = (a: StationRef, b: StationRef) => ({
    a: <TieEnd at={a} sectionName={sectionName} stationLabel={stationLabel} />,
    b: <TieEnd at={b} sectionName={sectionName} stationLabel={stationLabel} />,
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
                const key = tie.kind === 'merge' ? 'tornMerge' : 'tornSnapped';
                // The datum side is the end that does not move with the moving section.
                const held = [tie.station_a, tie.station_b].find(
                  (s) => !plan.moved_sections.includes(s.section_id)
                );
                return (
                  <li key={id} data-testid={`combine-torn-tie-${id}`}>
                    <Trans i18nKey={`dialogs.combineSections.${key}`} components={ends(tie.station_a, tie.station_b)} />
                    {tie.reason === 'zero_datum' && held && (
                      <>
                        {' '}
                        <Trans
                          i18nKey="dialogs.combineSections.tornHeld"
                          components={{ held: <SectionPill id={held.section_id} name={sectionName(held.section_id)} /> }}
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

/** The dialog line for each kind of tie the combine removes (#746). */
const REMOVED_KEY: Record<CombineRemovedKind, string> = {
  merge: 'removedMerge',
  snapped_closure: 'removedSnapped',
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
  loop_closure: 'replacedByClosure',
};

/** A section as a link pill (name and id), selecting it on click. */
function SectionPill({ id, name }: { id: SectionId; name: string }) {
  return (
    <DetailLink
      type="section"
      target={{ sectionId: id }}
      displayText={name}
      pillLabel={name}
      testId={`combine-tie-section-${id}`}
      onSelect={() => useUIStore.getState().setSelection({ type: 'section', sectionId: id })}
      onHoverStart={() => useUIStore.getState().setHoveredSectionId(id)}
      onHoverEnd={() => useUIStore.getState().setHoveredSectionId(null)}
    />
  );
}

/**
 * One end of a tie the combine turns into a loop closure: its section and its
 * station as the link pills the Details panel uses. Hovering the station
 * rings it on the map behind the dialog.
 */
function TieEnd({
  at,
  sectionName,
  stationLabel,
}: {
  at: StationRef;
  sectionName: (id: SectionId) => string;
  stationLabel: (ref: StationRef) => string;
}) {
  return (
    <span className="inline-flex flex-wrap items-center gap-1 align-middle">
      <SectionPill id={at.section_id} name={sectionName(at.section_id)} />
      <DetailLink
        type="station"
        target={{ sectionId: at.section_id, position: at.station_index }}
        displayText={stationLabel(at)}
        testId={`combine-tie-station-${at.section_id}-${at.station_index}`}
        onSelect={() => useUIStore.getState().setSelection({ type: 'station', stationRef: at })}
        onHoverStart={() => useUIStore.getState().setHoveredStation(at)}
        onHoverEnd={() => useUIStore.getState().setHoveredStation(null)}
      />
    </span>
  );
}
