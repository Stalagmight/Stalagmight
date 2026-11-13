import { useTranslation } from 'react-i18next';
import type { LengthUnit } from '../../types/config';
import type { CombineCandidate } from '../../types/ipc/CombineCandidate';
import type { CombinePlan } from '../../types/ipc/CombinePlan';
import type { SectionId, StationRef } from '../../types/project';
import { formatLength } from '../../utils/formatStats';
import { formatRotation } from '../../utils/combineSections';

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
 * that moves and how far, the ties that become loop closures, converted
 * bearings, the quality and manual offset the combined section keeps, shot
 * identities that are renewed, and sections that pass through the join. The
 * user reads all of it before confirming; nothing here is a refusal.
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

  if (plan.moving_section !== null) {
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
      notes.push({ key: 'kept-moves', text: t('dialogs.combineSections.keptMoves', { kept: keptName, other: otherName }) });
    }
  }
  if (plan.azimuth_rotation_deg !== null) {
    notes.push({
      key: 'rotation',
      text:
        Math.abs(plan.azimuth_rotation_deg) < 0.05
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

  if (notes.length === 0 && plan.torn_ties.length === 0) return null;
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
              {plan.torn_ties.map((tie) => (
                <li
                  key={`${tie.station_a.section_id}:${tie.station_a.station_index}-${tie.station_b.section_id}:${tie.station_b.station_index}`}
                  data-testid="combine-torn-tie"
                >
                  {t(tie.kind === 'merge' ? 'dialogs.combineSections.tornMerge' : 'dialogs.combineSections.tornSnapped', {
                    a: stationLabel(tie.station_a),
                    b: stationLabel(tie.station_b),
                  })}
                </li>
              ))}
            </ul>
            <span className="block mt-1 text-text-secondary">{t('dialogs.combineSections.tornWhy')}</span>
          </li>
        )}
      </ul>
    </div>
  );
}
