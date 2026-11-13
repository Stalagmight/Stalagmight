/**
 * Survey credit in the Detail panel: who surveyed and explored the selected
 * shot (or shots, or section), and when, editable in place.
 *
 * Every change is applied at once as ONE `editShotCredit` on exactly the
 * shots the selection covers, so each pill added or removed is one undo
 * step. Over several shots the fields show the union, with partial (dashed)
 * pills for people on only some of them: adding a pill puts that person on
 * every covered shot, its x takes them off every one, and clicking a partial
 * pill puts them on the rest. A date that differs per shot shows its range
 * until "Set for all shots".
 *
 * The change shows immediately (an overlay of the edit just sent) and is
 * replaced by the real credits when the project refreshes, so a pill never
 * blinks out between Enter and the refresh.
 */
import { useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import { useEditingBlocked } from '../../hooks/useEditingBlocked';
import { useStablePeopleOrder } from '../../hooks/useStablePeopleOrder';
import { setSource } from '../../utils/telemetry';
import { CreditDateInput, CreditPeopleFields } from '../common/CreditFields';
import { Panel } from '../common/Panel';
import { shotCredit } from '../../utils/sectionCredit';
import { applyCreditEdit, aggregateCredits, type CreditRole } from '../../utils/creditEdit';
import type { CreditEdit, CreditTargets, Project, Section, Shot, SurveyCredit } from '../../types/ipc.generated';
import type { PersonLike } from '../../utils/people';
import { formatNumber } from '../../i18n/format';
import { toPerson } from '../../store/recentPeople';

export interface CreditDetailProps {
  targets: CreditTargets;
  /** The credits of exactly the shots `targets` covers. */
  credits: readonly SurveyCredit[];
  /** `shot`, `shots` or `section`: read by tests and styling only. */
  scope: 'shot' | 'shots' | 'section';
}

/**
 * The editable Surveyors / Explorers / date block. Credit lives on shots, so
 * when the targets cover none (a section with no shots yet) there is nothing
 * to record it on: the fields are disabled with a hint instead of taking
 * input that would be dropped.
 */
export function CreditDetail({ targets, credits, scope }: CreditDetailProps) {
  const { t } = useTranslation();
  const editShotCredit = useProjectStore((s) => s.editShotCredit);
  const noShots = credits.length === 0;
  const editingBlocked = useEditingBlocked() || noShots;
  // Edits sent but not refreshed yet, valid only for the credits they were made on.
  const [overlay, setOverlay] = useState<{ on: readonly SurveyCredit[]; edits: CreditEdit[] } | null>(null);
  const edits = overlay && overlay.on === credits ? overlay.edits : [];

  const aggregate = useMemo(
    () => aggregateCredits(edits.length === 0 ? credits : credits.map((c) => edits.reduce(applyCreditEdit, c))),
    [credits, edits],
  );

  const send = (edit: CreditEdit) => {
    if (editingBlocked) return;
    setSource('detail-panel');
    setOverlay((prev) => ({ on: credits, edits: [...(prev && prev.on === credits ? prev.edits : []), edit] }));
    void editShotCredit(targets, edit).then((result) => {
      if (!result.success) setOverlay(null);
    });
  };
  const add = (role: CreditRole, person: PersonLike) => send({ [`add_${role}`]: [toPerson(person)] });
  const remove = (role: CreditRole, person: PersonLike) => send({ [`remove_${role}`]: [toPerson(person)] });

  // Pills keep their places while this selection is shown; a person just
  // added goes at the end, beside the input, even after the refresh re-sorts
  // the shots' aggregate by first appearance.
  const orderScope = useMemo(() => JSON.stringify(targets), [targets]);
  const surveyors = useStablePeopleOrder(aggregate.surveyors, orderScope);
  const explorers = useStablePeopleOrder(aggregate.explorers, orderScope);

  return (
    <div className="space-y-3" data-testid="detail-credit" data-scope={scope}>
      {noShots && (
        <p className="text-sm text-text-tertiary" data-testid="detail-credit-no-shots">
          {t('credit.noShotsHint')}
        </p>
      )}
      <CreditPeopleFields
        surveyors={surveyors}
        explorers={explorers}
        onAdd={add}
        onRemove={remove}
        onPromote={add}
        disabled={editingBlocked}
        testIdPrefix="detail-credit"
      />
      <div>
        <span className="mb-1 block text-sm text-text-secondary">{t('credit.dateLabel')}</span>
        <CreditDateInput
          value={aggregate.date}
          onChange={(date) => send({ set_date: date })}
          disabled={editingBlocked}
          testId="detail-credit-date"
        />
      </div>
    </div>
  );
}

/** The credits of a section's shots, in order. */
export function sectionCredits(section: Pick<Section, 'shots'>): SurveyCredit[] {
  return section.shots.map(shotCredit);
}

export type ShotPick = { section: Section; shot: Shot };

/**
 * The selected shots, when the selection is two or more shots and nothing
 * else (hook); null otherwise, and the Detail panel shows the single item.
 */
export function useSelectedShots(project: Pick<Project, 'sections'> | null): ShotPick[] | null {
  const multiSelection = useUIStore((s) => s.multiSelection);
  return useMemo(() => {
    if (multiSelection.length < 2 || multiSelection.some((s) => s.type !== 'shot')) return null;
    const picks = multiSelection.flatMap((s) => {
      const section = project?.sections.find((x) => x.id === s.sectionId);
      const shot = section?.shots.find((x) => x.id === s.shotId);
      return section && shot ? [{ section, shot }] : [];
    });
    return picks.length > 1 ? picks : null;
  }, [multiSelection, project]);
}

/** The Detail panel for several selected shots: their shared survey credit. */
export function MultiShotDetail({ picks }: { picks: readonly ShotPick[] }) {
  const { t } = useTranslation();
  const targets = useMemo<CreditTargets>(
    () => ({ type: 'shots', shots: picks.map(({ section, shot }) => [section.id, shot.id] as [number, number]) }),
    [picks],
  );
  const credits = useMemo(() => picks.map(({ shot }) => shotCredit(shot)), [picks]);
  const sectionCount = new Set(picks.map(({ section }) => section.id)).size;
  return (
    <div className="h-full bg-panel overflow-auto" data-testid="multi-shot-detail">
      <Panel bare title={t('detailPanel.multiShot.title', { count: picks.length })}>
        <div className="space-y-3">
          <p className="text-sm text-text-secondary" data-testid="multi-shot-detail-summary">
            {t('detailPanel.multiShot.summary', {
              count: sectionCount,
              shots: formatNumber(picks.length),
            })}
          </p>
          <CreditDetail targets={targets} credits={credits} scope="shots" />
        </div>
      </Panel>
    </div>
  );
}
