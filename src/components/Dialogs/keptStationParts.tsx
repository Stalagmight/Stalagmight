/**
 * Parts the four kept-station dialogs share (plan 2026-11-23, RS 7): Merge
 * Stations, Create Loop Closure, Detach Line from Station and Remove Closure.
 *
 * - An identity a preview names (a `StationIdentity`: its Station ID, its own
 *   name, its home line) is said as `Side / 0`, its home line first, since it
 *   may be no slot of the survey the user sees (a record a detach restores, an
 *   end a closure hides).
 * - A name collision (RS 4.1 step 6, N8) is one row with its two answers: the
 *   backend's rule-L letter (the default, "Rename Side / 12 to 12a") or the
 *   other station of the pair lettered instead (usually the kept one). The
 *   command then carries the answers as `renames`, and refuses a collision
 *   left unanswered, so a dialog that has a preview always sends them (an
 *   empty list included).
 */
import { useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useIdentityLabels } from '../../hooks/useIdentityLabels';
import { displayStationName, formatStationLabel } from '../../utils/formatters';
import type { StationRef } from '../../types/project';
import { EntityBlob } from '../common/EntityBlob';
import type { NameCollision } from '../../types/ipc/NameCollision';
import type { StationIdentity } from '../../types/ipc/StationIdentity';
import type { StationRename } from '../../types/ipc/StationRename';

/** Which of a collision's two answers the user picked. */
export type CollisionAnswer = 'letter' | 'alternative';

/** `Side / 0`: an identity with its home line, or its name alone when it has none. */
export function identityLabelOf(identity: StationIdentity, lineName: (lineId: number) => string): string {
  return formatStationLabel(identity.name, identity.home_line === undefined ? null : lineName(identity.home_line));
}

/** {@link identityLabelOf} bound to the open project's line names. */
export function useIdentityLabel(): (identity: StationIdentity) => string {
  const labels = useIdentityLabels();
  return useMemo(() => (identity: StationIdentity) => identityLabelOf(identity, labels.lineName), [labels]);
}

/**
 * A keep candidate's pill (RS 7): the slot's line as the context, the
 * station's OWN name and Station ID (not what a closure shows it as: the
 * radio says that beside it), coloured by the slot's line.
 */
export function IdentityPill({
  slot,
  identity,
  testId,
}: {
  slot: StationRef;
  identity: StationIdentity;
  testId?: string | undefined;
}) {
  const labels = useIdentityLabels();
  const name = displayStationName(identity.name);
  const line = labels.lineName(slot.line_id);
  return (
    <EntityBlob
      entity="station"
      lineId={slot.line_id}
      context={line}
      stationId={identity.id}
      plain={formatStationLabel(identity.name, line)}
      testId={testId}
    >
      {name}
    </EntityBlob>
  );
}

/**
 * The renames that answer `collisions` (one per collision, the letter unless
 * `answers` says otherwise). One rename per Station ID: a station that
 * collides on two lines takes the first answer's name, which is free on every
 * line it is shown on.
 */
export function renamesFromAnswers(
  collisions: readonly NameCollision[] | undefined,
  answers: readonly CollisionAnswer[],
): StationRename[] {
  const renames: StationRename[] = [];
  const seen = new Set<number>();
  (collisions ?? []).forEach((collision, i) => {
    const pick = answers[i] === 'alternative' ? collision.alternative : collision.letter;
    if (seen.has(pick.station.id)) return;
    seen.add(pick.station.id);
    renames.push({ station_id: pick.station.id, name: pick.to });
  });
  return renames;
}

/**
 * The user's answers to `collisions`, each the letter until changed, reset
 * whenever the collisions change (another keep, another line).
 */
export function useCollisionAnswers(
  collisions: readonly NameCollision[] | undefined,
): [CollisionAnswer[], (index: number, answer: CollisionAnswer) => void] {
  const key = useMemo(
    () => (collisions ?? []).map((c) => `${c.line_id}:${c.name}:${c.letter.station.id}:${c.alternative.station.id}`).join('|'),
    [collisions],
  );
  const [answers, setAnswers] = useState<CollisionAnswer[]>([]);
  useEffect(() => {
    setAnswers((collisions ?? []).map(() => 'letter'));
    // The key stands for the collisions' content.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);
  const set = (index: number, answer: CollisionAnswer) =>
    setAnswers((prev) => {
      const next = [...prev];
      next[index] = answer;
      return next;
    });
  return [answers, set];
}

/**
 * One row per name the operation would show twice on a line, each with its
 * two answers as radios (RS 7). `testIdPrefix` roots the test ids:
 * `{prefix}-collision-{i}`, `{prefix}-collision-{i}-letter`,
 * `{prefix}-collision-{i}-alternative`.
 */
export function CollisionRows({
  collisions,
  answers,
  onAnswer,
  testIdPrefix,
  disabled = false,
}: {
  /** The preview's collisions (an older preview without them: none). */
  collisions: readonly NameCollision[] | undefined;
  answers: readonly CollisionAnswer[];
  onAnswer: (index: number, answer: CollisionAnswer) => void;
  testIdPrefix: string;
  disabled?: boolean;
}) {
  const { t } = useTranslation();
  const labels = useIdentityLabels();
  const label = useIdentityLabel();
  if (!collisions || collisions.length === 0) return null;
  return (
    <div className="space-y-2 text-sm" data-testid={`${testIdPrefix}-collisions`}>
      <p className="text-text-secondary">{t('dialogs.keptStations.collisionsLead', { count: collisions.length })}</p>
      {collisions.map((collision, i) => {
        const answer = answers[i] ?? 'letter';
        const name = `${testIdPrefix}-collision-${i}`;
        return (
          <fieldset key={`${collision.line_id}:${collision.name}:${i}`} className="p-2 rounded border border-border space-y-1" data-testid={name}>
            <legend className="px-1 text-text-primary">
              {t('dialogs.keptStations.collision', { line: labels.lineName(collision.line_id), name: collision.name })}
            </legend>
            {(['letter', 'alternative'] as const).map((which) => {
              const suggestion = collision[which];
              return (
                <label key={which} className="flex items-center gap-2 p-1 rounded hover:bg-background-50">
                  <input
                    type="radio"
                    name={name}
                    value={which}
                    checked={answer === which}
                    disabled={disabled}
                    onChange={() => onAnswer(i, which)}
                    data-testid={`${name}-${which}`}
                  />
                  <span>
                    {t('dialogs.keptStations.rename', { station: label(suggestion.station), to: suggestion.to })}
                  </span>
                </label>
              );
            })}
          </fieldset>
        );
      })}
    </div>
  );
}

/**
 * The comments an action hides on its own (RS 12): a promotion it makes or a
 * loop closure end it moves can leave a station with a comment shown nowhere.
 * Each is the identity as it showed the comment, `Side / 0`; Undo brings it
 * back. Nothing when there are none.
 */
export function HiddenComments({ hidden, testId }: { hidden: readonly StationIdentity[] | undefined; testId: string }) {
  const { t } = useTranslation();
  const label = useIdentityLabel();
  if (!hidden || hidden.length === 0) return null;
  return (
    <ul className="text-xs text-text-secondary space-y-0.5" data-testid={testId}>
      {hidden.map((h) => (
        <li key={h.id} data-testid={`${testId}-${h.id}`}>
          {t('dialogs.keptStations.commentHidden', { station: label(h), comment: h.comment ?? '' })}
        </li>
      ))}
    </ul>
  );
}
