/**
 * Parts the four kept-station dialogs share (plan 2026-11-23, RS 7): Merge
 * Stations, Create Loop Closure, Detach Line from Station and Remove Closure.
 *
 * - An identity a preview names (a `StationIdentity`: its Station ID, its own
 *   name, its home line) is said as `Side / 0`, its home line first, since it
 *   may be no slot of the survey the user sees (a record a detach restores, an
 *   end a closure hides).
 * - A name collision (RS 4.1 step 6, N8) is one row with its two answers: the
 *   backend's rule-L letter (the default) or the other station of the pair
 *   lettered instead (usually the kept one). The two stations share a name,
 *   so each answer names the station it renames by its role and Station ID
 *   (RS-FX6, P1-1): "Rename the station that comes back (Side / 0, Station
 *   ID 15) to 0a", "Rename the 0 that is on Side now (Station ID 29) to 0a". The command
 *   then carries the answers as `renames`, and refuses a collision left
 *   unanswered, so a dialog that has a preview always sends them (an empty
 *   list included).
 */
import { useEffect, useMemo, useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { useIdentityLabels } from '../../hooks/useIdentityLabels';
import { displayStationName, formatStationLabel } from '../../utils/formatters';
import type { StationRef } from '../../types/project';
import { EntityBlob } from '../common/EntityBlob';
import type { ArchivedStation } from '../../types/ipc/ArchivedStation';
import type { NameCollision } from '../../types/ipc/NameCollision';
import type { RenameSuggestion } from '../../types/ipc/RenameSuggestion';
import type { StationIdentity } from '../../types/ipc/StationIdentity';
import type { StationRename } from '../../types/ipc/StationRename';
import { stationNameProblemMessage, stationRenameProblem } from '../../utils/nameValidation';
import i18n from '../../i18n';
import { Prose } from '../common/Prose';
import { proseToPlain } from '../../utils/prose';
import { useKeptProse, type KeptProse } from './keptStationProse';

/** Which of a collision's two answers the user picked. */
export type CollisionAnswer = 'letter' | 'alternative';

/**
 * The station a dialog is about, so a collision answer can name it by its
 * role: the one that comes back (Detach, Remove Closure) or the one kept
 * (Merge Stations).
 */
export interface CollisionSubject {
  id: number;
  role: 'comesBack' | 'kept';
}

/** `Side / 0`: an identity with its home line, or its name alone when it has none. */
export function identityLabelOf(identity: StationIdentity, lineName: (lineId: number) => string): string {
  return formatStationLabel(identity.name, identity.home_line === undefined ? null : lineName(identity.home_line));
}

/** A record as an identity (its first own line standing for its home line). */
export function recordIdentity(record: ArchivedStation): StationIdentity {
  const line = record.lines[0];
  return line === undefined ? { id: record.id, name: record.name } : { id: record.id, name: record.name, home_line: line };
}

/** {@link identityLabelOf} bound to the open project's line names. */
export function useIdentityLabel(): (identity: StationIdentity) => string {
  const labels = useIdentityLabels();
  return useMemo(() => (identity: StationIdentity) => identityLabelOf(identity, labels.lineName), [labels]);
}

/**
 * A keep candidate's pill (RS 7): the station's OWN name and Station ID
 * (not what a closure shows it as: the radio says that beside it), coloured
 * and named by the line the station belongs to (its home line, RS-OWNERSHIP;
 * the slot's line when it has none or is native there).
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
      homeLineId={identity.home_line ?? null}
      plain={formatStationLabel(identity.name, line)}
      testId={testId}
    >
      {name}
    </EntityBlob>
  );
}

/**
 * An identity's pill by its home line (a record a detach restores, which is
 * no slot of the survey yet): the home line as the context, the own name and
 * the Station ID, coloured by that line. Without a home line, the plain text.
 */
export function HomeIdentityPill({ identity, testId }: { identity: StationIdentity; testId?: string | undefined }) {
  const labels = useIdentityLabels();
  const line = identity.home_line === undefined ? null : labels.lineName(identity.home_line);
  return (
    <EntityBlob
      entity="station"
      lineId={identity.home_line ?? null}
      context={line}
      stationId={identity.id}
      plain={formatStationLabel(identity.name, line)}
      testId={testId}
    >
      {displayStationName(identity.name)}
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
 * Why the renames a dialog would send cannot go: the first name Edit Station
 * refuses too (`stationRenameProblem`: empty once trimmed, or holding a control
 * or invisible formatting character), which the command refuses as Set Station
 * Name does (RS-FX9). The answers are the backend's letters of the stations'
 * own names, so only a name that already held such a character (an import's)
 * trips it, and the user renames that station first. `null` when all are fine.
 */
export function renamesProblem(renames: readonly StationRename[]): string | null {
  for (const rename of renames) {
    const problem = stationRenameProblem(rename.name, null);
    if (problem !== null) return stationNameProblemMessage(problem, '', rename.name);
  }
  return null;
}

/**
 * The name station `id` has once the answers apply: its new name when an
 * answer renames it, its own name when it is in a collision the answers
 * leave it out of, else `fallback` (a preview's name, which is rule L's
 * default letter). The Detach lead and its After Detach list follow the
 * chosen answer this way (RS-FX6, P2-2).
 */
export function answeredName(
  id: number,
  fallback: string,
  collisions: readonly NameCollision[] | undefined,
  answers: readonly CollisionAnswer[],
): string {
  const renamed = renamesFromAnswers(collisions, answers).find((r) => r.station_id === id);
  if (renamed) return renamed.name;
  for (const collision of collisions ?? []) {
    for (const suggestion of [collision.letter, collision.alternative]) {
      if (suggestion.station.id === id) return suggestion.station.name;
    }
  }
  return fallback;
}

/** How an answer names its station and the collision's line: plain labels or prose tokens. */
interface AnswerNames {
  /** A station by its home line (`Side / 0`), its Station ID said in words. */
  identity: (identity: StationIdentity) => string;
  line: (lineId: number) => string;
  /** The collision line's own station by its bare name ("the 0 that is on Side now"). */
  own: (identity: StationIdentity) => string;
}

function answerWords(
  suggestion: RenameSuggestion,
  collision: NameCollision,
  subject: CollisionSubject | undefined,
  names: AnswerNames,
): string {
  const station = suggestion.station;
  const params = { station: names.identity(station), id: station.id, to: suggestion.to };
  if (subject && station.id === subject.id) {
    return i18n.t(subject.role === 'kept' ? 'dialogs.keptStations.renameKept' : 'dialogs.keptStations.renameComesBack', params);
  }
  if (station.home_line === collision.line_id) {
    return i18n.t('dialogs.keptStations.renameOwn', {
      ...params,
      line: names.line(collision.line_id),
      name: names.own(station),
    });
  }
  return i18n.t('dialogs.keptStations.rename', params);
}

/**
 * One answer's words (RS-FX6, P1-1): both stations of a collision have one
 * name, so each answer says which it renames, by its role and Station ID.
 * The dialog's own station by its role ("the station that comes back", "the
 * kept station"), a station of the collision's line as the one on that line
 * now ("the 0 that is on Side now": "Side's own 0" fit the station coming
 * back too, RS-FX8 N5), any other by its home line.
 */
export function collisionAnswerText(
  suggestion: RenameSuggestion,
  collision: NameCollision,
  subject: CollisionSubject | undefined,
  lineName: (lineId: number) => string,
): string {
  return answerWords(suggestion, collision, subject, {
    identity: (identity) => identityLabelOf(identity, lineName),
    line: lineName,
    own: (identity) => displayStationName(identity.name),
  });
}

/**
 * {@link collisionAnswerText} as prose (RS-PROSE): the same words, the
 * station it renames and the collision's line as pills. The new name stays
 * text, and so does the Station ID, which the words say.
 */
export function collisionAnswerProse(
  suggestion: RenameSuggestion,
  collision: NameCollision,
  subject: CollisionSubject | undefined,
  prose: Pick<KeptProse, 'identityNoId' | 'line'>,
): string {
  return answerWords(suggestion, collision, subject, {
    identity: (identity) => prose.identityNoId(identity),
    line: (lineId) => prose.line(lineId),
    own: (identity) => prose.identityNoId(identity, false),
  });
}

/**
 * A dialog's action row pinned to the bottom of `DialogWrapper`'s scrolling
 * body (RS-FX6, P3-18): on a short window (1024x640) a long Detach or Merge
 * body scrolls, and Cancel and the action stay in view. Relies on the
 * wrapper's default `p-4` body padding, as the import dialogs' footers do.
 */
export function StickyDialogFooter({ children, testId }: { children: ReactNode; testId?: string }) {
  return (
    <div
      className="sticky -bottom-4 z-10 -mx-4 -mb-4 flex justify-end gap-2 px-4 py-3 border-t border-border bg-panel"
      data-testid={testId}
    >
      {children}
    </div>
  );
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
 * two answers as radios (RS 7), and under them why the chosen answers cannot
 * be sent when a name is one Edit Station refuses ({@link renamesProblem}). `testIdPrefix` roots the test ids:
 * `{prefix}-collision-{i}`, `{prefix}-collision-{i}-letter`,
 * `{prefix}-collision-{i}-alternative`.
 */
export function CollisionRows({
  collisions,
  answers,
  onAnswer,
  testIdPrefix,
  subject,
  disabled = false,
}: {
  /** The preview's collisions (an older preview without them: none). */
  collisions: readonly NameCollision[] | undefined;
  answers: readonly CollisionAnswer[];
  onAnswer: (index: number, answer: CollisionAnswer) => void;
  testIdPrefix: string;
  /** The dialog's own station, named by its role in an answer. */
  subject?: CollisionSubject | undefined;
  disabled?: boolean;
}) {
  const { t } = useTranslation();
  const prose = useKeptProse();
  if (!collisions || collisions.length === 0) return null;
  const problem = renamesProblem(renamesFromAnswers(collisions, answers));
  return (
    <div className="space-y-2 text-sm" data-testid={`${testIdPrefix}-collisions`}>
      <p className="text-text-secondary">{t('dialogs.keptStations.collisionsLead', { count: collisions.length })}</p>
      {collisions.map((collision, i) => {
        const answer = answers[i] ?? 'letter';
        const name = `${testIdPrefix}-collision-${i}`;
        const legend = t('dialogs.keptStations.collision', { line: prose.line(collision.line_id), name: collision.name });
        // A name a label gives (the group's, each radio's) is the plain
        // sentence: computed from the pills, a browser spaces their words
        // apart ("( Side / 0 , Station ID 40)").
        return (
          <fieldset
            key={`${collision.line_id}:${collision.name}:${i}`}
            className="p-2 rounded border border-border space-y-1"
            aria-label={proseToPlain(legend)}
            data-testid={name}
          >
            <legend className="px-1 text-text-primary">
              <Prose text={legend} />
            </legend>
            {(['letter', 'alternative'] as const).map((which) => {
              const suggestion = collision[which];
              const text = collisionAnswerProse(suggestion, collision, subject, prose);
              return (
                <label key={which} className="flex items-center gap-2 p-1 rounded hover:bg-background-50">
                  <input
                    type="radio"
                    name={name}
                    value={which}
                    checked={answer === which}
                    disabled={disabled}
                    onChange={() => onAnswer(i, which)}
                    aria-label={proseToPlain(text)}
                    data-testid={`${name}-${which}`}
                  />
                  {/* A sentence beside the radio: its names as pills. */}
                  <span>
                    <Prose text={text} />
                  </span>
                </label>
              );
            })}
          </fieldset>
        );
      })}
      {problem !== null && (
        <p role="alert" className="text-xs text-error" data-testid={`${testIdPrefix}-rename-problem`}>
          {problem}
        </p>
      )}
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
  const prose = useKeptProse();
  if (!hidden || hidden.length === 0) return null;
  return (
    <ul className="text-xs text-text-secondary space-y-0.5" data-testid={testId}>
      {hidden.map((h) => (
        <li key={h.id} data-testid={`${testId}-${h.id}`}>
          <Prose text={t('dialogs.keptStations.commentHidden', { station: prose.identity(h), comment: h.comment ?? '' })} />
        </li>
      ))}
    </ul>
  );
}
