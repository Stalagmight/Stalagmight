import { useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { isEditingBlockedNow } from '../../hooks/useEditingBlocked';
import { getErrorMessage } from '../../utils/ipc';
import { trackDialogComplete } from '../../utils/telemetry';
import { identityLabels, lineDisplayName } from '../../utils/stationIdentity';
import { lineNameProblem, lineNameProblemMessage, nextFreeLineName } from '../../utils/nameValidation';
import { formatLength } from '../../utils/formatStats';
import { defaultConfig } from '../../types/config';
import { combinePlanFor, defaultCombinedName } from '../../utils/combineLines';
import type { CombineCandidate } from '../../types/ipc/CombineCandidate';
import type { CombineLineRenames } from '../../types/ipc/CombineLineRenames';
import type { CombineStationRename } from '../../types/ipc/CombineStationRename';
import type { CombineStationRenames } from '../../types/ipc/CombineStationRenames';
import type { CombinePick } from '../../types/ipc/CombinePick';
import type { LineId, StationRef } from '../../types/project';
import type { CreditedPerson, Shot } from '../../types/ipc.generated';
import { formatCreditDateRange, shotCredit, summarizeCredits } from '../../utils/lineCredit';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import { CombinePlanNotes } from './CombinePlanNotes';

/**
 * Who the combined line credits, and when: every shot keeps its own
 * surveyors, explorers and date, so this is the two lines' shots taken
 * together, in the combined line's order (the other line reversed when
 * it is, and before or after the kept one).
 */
function CombinedCreditSummary({ shots }: { shots: Array<Pick<Shot, 'credit'>> }) {
  const { t } = useTranslation();
  const summary = summarizeCredits(shots.map(shotCredit));
  const shotCount = (count: number) => t('dialogs.combineSections.shots', { count });
  const people = (list: CreditedPerson[]) =>
    list
      .map((p) => t('dialogs.combineSections.personShots', { name: p.person.name, shots: shotCount(p.shots) }))
      .join(', ');
  const dates = formatCreditDateRange(summary);
  const empty = summary.surveyors.length === 0 && summary.explorers.length === 0 && !dates;
  return (
    <div className="space-y-1 text-sm" data-testid="combine-credit">
      <div className="text-text-secondary">
        {t('dialogs.combineSections.creditTitle', { shots: shotCount(summary.shot_count) })}
      </div>
      {empty ? (
        <div data-testid="combine-credit-none">No surveyors, explorers or dates recorded on either line.</div>
      ) : (
        <>
          {summary.surveyors.length > 0 && (
            <div data-testid="combine-credit-surveyors">Surveyors: {people(summary.surveyors)}</div>
          )}
          {summary.explorers.length > 0 && (
            <div data-testid="combine-credit-explorers">Explorers: {people(summary.explorers)}</div>
          )}
          {dates && <div data-testid="combine-credit-dates">Surveyed: {dates}</div>}
        </>
      )}
      <div className="text-text-secondary">Every shot keeps its own surveyors, explorers and date.</div>
    </div>
  );
}

/**
 * One line's lettering as the note reads it: `Main 0..3 → A0..A3`. The range
 * spans the lettered stations only. When some of the line's stations are not
 * lettered (a custom `Junction` between `1` and `5` keeps its name, a `Sump`
 * both lines hold is listed on its own), the counts say so, so the range never
 * reads as renaming them: `Main 0..10 → A0..A10 (9 renamed, 2 keep their
 * names)`.
 */
function lineRenameText(lineName: string, renames: CombineLineRenames, suffixed: number): string {
  const span = (first: string, last: string) => (renames.count > 1 ? `${first}..${last}` : first);
  const text = `${lineName} ${span(renames.first_from, renames.last_from)} \u2192 ${span(renames.first_to, renames.last_to)}`;
  if (renames.unchanged === 0 && suffixed === 0) return text;
  if (renames.unchanged === 0) return `${text} (${renames.count} renamed)`;
  const kept = renames.unchanged === 1 ? '1 keeps its name' : `${renames.unchanged} keep their names`;
  return `${text} (${renames.count} renamed, ${kept})`;
}

/** At most this many Mid-suffixed renames are spelled out; the rest are counted. */
const SUFFIXED_SHOWN = 3;

/** The other line's names made unique by a letter: `Side Sump → Sump-a`. */
function suffixedRenameText(lineName: string, suffixed: readonly CombineStationRename[]): string {
  const shown = suffixed.slice(0, SUFFIXED_SHOWN).map((r) => `${r.from} \u2192 ${r.to}`);
  const more = suffixed.length - shown.length;
  return `${lineName} ${shown.join(', ')}${more > 0 ? ` and ${more} more` : ''}`;
}

/**
 * The note that tells the user, before they confirm, that the combine renames
 * stations (`CombinePlan.station_renames`): why, and each line's first and
 * last renamed station before and after.
 */
function stationRenameNote(renames: CombineStationRenames, keptName: string, otherName: string): string {
  const suffixed = renames.suffixed ?? [];
  const parts = [
    renames.kept ? lineRenameText(keptName, renames.kept, 0) : null,
    renames.other ? lineRenameText(otherName, renames.other, suffixed.length) : null,
    suffixed.length > 0 ? suffixedRenameText(otherName, suffixed) : null,
  ].filter((part): part is string => part !== null);
  const why =
    renames.reason === 'collision'
      ? 'Station names collide'
      : `${keptName} is lettered by an earlier combine`;
  return `${why}, so stations will be renamed: ${parts.join(', ')}`;
}

/**
 * Combine Lines: join two lines that meet end to end (merged,
 * co-located, or across a loop closure) into one. Other lines there stay
 * attached as branches. Opened with the backend's `CombineCandidate` for one
 * pair: a two-line selection, or the pair picked in the station menu
 * ("Combine Line With <name>...", or the "Combine Lines..." submenu at a
 * junction), with `keep` the line the menu named first. What else the
 * combine changes for the kept line shown (a side that moves, ties that
 * become loop closures, converted bearings...) is listed, with everything
 * else informational, under a folded "Details".
 */
export default function CombineLinesDialog() {
  const { t } = useTranslation();
  const { closeDialog, dialogData, setSelection } = useUIStore();
  const { combineLines, project, stations, surveyData, config } = useProjectStore();
  const lengthUnit = config?.units?.length ?? defaultConfig.units.length;
  const candidate = dialogData.candidate as CombineCandidate | undefined;

  const lineA = project?.lines.find((s) => s.id === candidate?.line_a);
  const lineB = project?.lines.find((s) => s.id === candidate?.line_b);
  const isStale = !candidate || !lineA || !lineB;

  // `a` (the lower id) is kept unless the user swaps.
  // Start on `b` only when `a` cannot be kept but `b` can (its shot numbers
  // are used up for the session).
  // The line the menu named first (`keep`) is kept, as long as it can be:
  // otherwise `a`, the lower id, unless `a` cannot be kept but `b` can (its
  // shot numbers are used up for the session).
  const [keepB, setKeepB] = useState(() => {
    const keep = dialogData.keep as LineId | undefined;
    if (keep === candidate?.line_b && !candidate?.blocked_keeping_b) return true;
    if (keep === candidate?.line_a && !candidate?.blocked_keeping_a) return false;
    return Boolean(candidate?.blocked_keeping_a) && !candidate?.blocked_keeping_b;
  });
  const kept = keepB ? lineB : lineA;
  const other = keepB ? lineA : lineB;
  const keptId = (keepB ? candidate?.line_b : candidate?.line_a) ?? 0;
  const otherId = (keepB ? candidate?.line_a : candidate?.line_b) ?? 0;
  const labels = useMemo(
    () => identityLabels({ surveyData, stations, lines: project?.lines ?? null }),
    [surveyData, stations, project?.lines]
  );
  const keptName = lineDisplayName(kept?.name);
  const otherName = lineDisplayName(other?.name);

  // The two lines become the combined one, so neither of their names is a
  // clash; every other line's is.
  const otherLines = (project?.lines ?? []).filter((l) => l.id !== keptId && l.id !== otherId);
  // The kept line's name, or the split's base name when these are its two
  // halves - unless another line already holds that; then the kept line's.
  const derivedName = defaultCombinedName(kept?.name ?? '', other?.name ?? '');
  const defaultName = !lineNameProblem(derivedName, otherLines)
    ? derivedName
    : !lineNameProblem(kept?.name ?? '', otherLines)
      ? (kept?.name ?? '').trim()
      : nextFreeLineName(project?.lines);
  const [name, setName] = useState<string | null>(null);
  const [color, setColor] = useState<string | null>(null);
  const [swapLr, setSwapLr] = useState(true);
  const [commentPick, setCommentPick] = useState<CombinePick>('kept');
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Untouched fields follow the kept line when the user swaps.
  const shownName = name ?? defaultName;
  const shownColor = color ?? kept?.color ?? '#3b82f6';
  const nameProblem = lineNameProblem(shownName, otherLines);
  const nameError = nameProblem ? lineNameProblemMessage(nameProblem, shownName) : null;

  const join = keepB ? candidate?.station_b : candidate?.station_a;
  // A tie's station beside its line pill: the name alone (`17`); with
  // `withLine`, `Main Line / 17`.
  const stationLabel = (ref: StationRef, withLine = false) => labels.station(ref, withLine);
  const plan = candidate ? combinePlanFor(candidate, keptId) : undefined;
  // The stations the combine renames for the line shown as kept, if any.
  const renames = plan?.station_renames ?? null;
  // The join station by its line and name: `Main Line / 17`; by the name it
  // will have when the combine renames it (`Main Line / A3`).
  const joinLabel = renames ? `${keptName} / ${renames.join_name}` : join ? labels.station(join, true) : '';
  const keptShots = (keepB ? candidate?.shot_count_b : candidate?.shot_count_a) ?? 0;
  const otherShots = (keepB ? candidate?.shot_count_a : candidate?.shot_count_b) ?? 0;
  const summary = t('dialogs.combineSections.summary', {
    kept: keptName,
    keptShots: t('dialogs.combineSections.shots', { count: keptShots }),
    other: otherName,
    otherShots: t('dialogs.combineSections.shots', { count: otherShots }),
    station: joinLabel,
  });
  // Where the other line's shots go (wording from #713): the kept line
  // keeps its station 0 unless the join is at its start.
  const keptEnd = keepB ? candidate?.end_b : candidate?.end_a;
  const otherEnd = keepB ? candidate?.end_a : candidate?.end_b;
  const placement = `${otherName} will be ${candidate?.reverses ? 'reversed and ' : ''}added ${
    keptEnd === 'start' ? 'before' : 'after'
  } ${keptName}.`;

  const keptText = (a: string | null | undefined, b: string | null | undefined) => (keepB ? b : a) ?? '';
  const otherText = (a: string | null | undefined, b: string | null | undefined) => (keepB ? a : b) ?? '';
  const commentConflict = Boolean(candidate?.comment_a && candidate?.comment_b);
  const blocked =
    candidate?.blocked_reason ?? (keepB ? candidate?.blocked_keeping_b : candidate?.blocked_keeping_a) ?? null;
  const lineName = (id: LineId) => labels.lineName(id);
  const listNames = (ids: LineId[]) => ids.map(lineName).join(', ');
  const handleConfirm = async () => {
    if (isSubmitting || isStale || blocked || nameError) return;
    if (isEditingBlockedNow()) {
      setError('Editing is disabled in the current view.');
      return;
    }
    setIsSubmitting(true);
    setError(null);
    const dialogKey = dialogData;
    try {
      const result = await combineLines(keptId, otherId, {
        name: shownName.trim(),
        color: shownColor,
        swap_lr: swapLr,
        comment_pick: commentConflict ? commentPick : 'kept',
        // Ignored: the joined station keeps the kept line's name (names are
        // unique per line); still sent, the options' shape is unchanged.
        name_pick: 'kept',
        // The join this dialog shows: two lines can meet at more than one
        // place (a loop of two), and the backend must not pick another.
        ...(keptEnd && otherEnd ? { join: { kept: keptEnd, other: otherEnd } } : {}),
      });
      if (!result.success) {
        setError(result.error ?? 'Failed to combine lines');
        setIsSubmitting(false);
        return;
      }
      setSelection({ type: 'line', lineId: keptId });
      if (useUIStore.getState().dialogData !== dialogKey) return;
      trackDialogComplete('combine-lines');
      closeDialog();
    } catch (e) {
      setError(getErrorMessage(e, 'Failed to combine lines'));
      setIsSubmitting(false);
    }
  };

  const pickChoice = (
    key: 'comment',
    legend: string,
    value: CombinePick,
    onChange: (pick: CombinePick) => void,
    options: Array<[CombinePick, string]>
  ) => (
    <fieldset className="space-y-1" data-testid={`combine-${key}-pick`}>
      <legend className="text-sm text-text-secondary mb-1">{legend}</legend>
      {options.map(([pick, label]) => (
        <label key={pick} className="flex items-center gap-2 text-sm cursor-pointer">
          <input
            type="radio"
            name={`combine-${key}-pick`}
            checked={value === pick}
            onChange={() => onChange(pick)}
            disabled={isSubmitting}
            data-testid={`combine-${key}-pick-${pick}`}
          />
          <span className="truncate">{label}</span>
        </label>
      ))}
    </fieldset>
  );

  return (
    <DialogWrapper
      title="Combine Lines"
      onClose={closeDialog}
      size="md"
      closeOnBackdropClick={!isSubmitting}
      data-testid="combine-lines-dialog"
    >
      <div className="space-y-4">
        <p className="text-text-secondary" data-testid="combine-summary">{summary}</p>
        {!isStale && <p className="text-sm" data-testid="combine-plan">{placement}</p>}
        {/* Outside the folded Details: renamed stations are a change the user
            sees everywhere afterwards, so they are told before confirming. */}
        {!isStale && renames && (
          <p className="text-sm" data-testid="combine-rename-note">
            {stationRenameNote(renames, keptName, otherName)}
          </p>
        )}
        {!isStale && candidate?.join_kind === 'closure' && (
          <p className="text-sm" data-testid="combine-closure-join">
            {t('dialogs.combineSections.closureJoin', {
              kept: keptName,
              other: otherName,
              distance: formatLength(plan?.shift_m ?? 0, lengthUnit, 2),
            })}
          </p>
        )}
        {/* Outside the folded Details: the user is told before confirming that
            a merge or closure of theirs goes (#746); Details names each one. */}
        {!isStale && (plan?.closures_removed_across_one_shot?.length ?? 0) > 0 && (
          <p className="text-sm" data-testid="combine-removed-summary">
            {t('dialogs.combineSections.removedSummary', {
              count: plan?.closures_removed_across_one_shot?.length ?? 0,
            })}
          </p>
        )}

        <div className="flex items-center justify-between gap-2 text-sm">
          <span data-testid="combine-kept-label">
            Keep <strong>{keptName}</strong> (the combined line continues it, in its direction)
          </span>
          <Button onClick={() => setKeepB((v) => !v)} disabled={isSubmitting || isStale} data-testid="combine-swap-kept">
            Keep {otherName} instead
          </Button>
        </div>

        <div className="flex items-end gap-2">
          <div className="flex-1">
            <label className="block text-sm text-text-secondary mb-1" htmlFor="combine-name">
              Combined line name
            </label>
            <input
              id="combine-name"
              type="text"
              value={shownName}
              onChange={(e) => setName(e.target.value)}
              className="w-full"
              disabled={isSubmitting}
              aria-invalid={nameError !== null}
              {...(nameError ? { 'aria-describedby': 'combine-name-error' } : {})}
              data-testid="combine-name"
            />
          </div>
          <input
            type="color"
            value={shownColor}
            onChange={(e) => setColor(e.target.value)}
            className="w-8 h-8 rounded cursor-pointer border border-border"
            aria-label="Combined line colour"
            disabled={isSubmitting}
            data-testid="combine-color"
          />
        </div>
        {nameError && (
          <p id="combine-name-error" className="-mt-2 text-sm text-error" data-testid="combine-name-error">
            {nameError}
          </p>
        )}

        {candidate?.reverses && (
          <div className="text-sm" data-testid="combine-reverse-notice">
            <label className="flex items-center gap-2 cursor-pointer">
              <input
                type="checkbox"
                checked={swapLr}
                onChange={(e) => setSwapLr(e.target.checked)}
                disabled={isSubmitting}
                data-testid="combine-swap-lr"
              />
              Swap {otherName}'s left and right walls
            </label>
          </div>
        )}

        {commentConflict &&
          pickChoice('comment', 'The join station has two comments. Keep:', commentPick, setCommentPick, [
            ['kept', keptText(candidate?.comment_a, candidate?.comment_b)],
            ['other', otherText(candidate?.comment_a, candidate?.comment_b)],
            ['both', 'Both'],
          ])}

        {/* Everything informational, folded: what moves, what becomes a loop
            closure, branches, depth, direction and credit. */}
        <details className="text-sm" data-testid="combine-details">
          <summary className="cursor-pointer text-text-secondary hover:text-text-primary" data-testid="combine-details-toggle">
            Details
          </summary>
          <div className="mt-3 space-y-4">
            {candidate?.reverses && (
              <p data-testid="combine-reverse-explain">
                Reversing {otherName} reciprocates its bearings and swaps its depths, so every station stays where
                it is.
              </p>
            )}

            {candidate && candidate.branches.length > 0 && (
              <p className="text-text-secondary" data-testid="combine-branches-note">
                {t('dialogs.combineSections.branchesNote', {
                  names: listNames(candidate.branches),
                  count: candidate.branches.length,
                })}
              </p>
            )}
            {candidate && candidate.loose_ends.length > 0 && (
              <p className="text-text-secondary" data-testid="combine-loose-ends-note">
                {t('dialogs.combineSections.looseEndsNote', {
                  names: listNames(candidate.loose_ends),
                  count: candidate.loose_ends.length,
                })}
              </p>
            )}
            {candidate?.depth_difference_m != null && (
              <p className="text-warning" data-testid="combine-depth-note">
                {t('dialogs.combineSections.depthNote', {
                  gap: formatLength(candidate.depth_difference_m, lengthUnit, 2),
                  half: formatLength(candidate.depth_difference_m / 2, lengthUnit, 2),
                })}
              </p>
            )}

            {!isStale && candidate && plan && (
              <CombinePlanNotes
                candidate={candidate}
                plan={plan}
                keptId={keptId}
                keptName={keptName}
                otherName={otherName}
                keptQuality={kept?.quality}
                otherQuality={other?.quality}
                lengthUnit={lengthUnit}
                lineName={lineName}
                stationLabel={stationLabel}
              />
            )}

            <p className="text-text-secondary" data-testid="combine-direction-note">
              The combined line keeps {keptName}'s survey direction ({kept?.direction === 'out' ? 'Out' : 'In'}).
            </p>

            {!isStale && kept && other && (
              <CombinedCreditSummary
                shots={(() => {
                  const moved = candidate?.reverses ? [...other.shots].reverse() : other.shots;
                  return keptEnd === 'start' ? [...moved, ...kept.shots] : [...kept.shots, ...moved];
                })()}
              />
            )}
          </div>
        </details>

        {blocked && (
          <div className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm" data-testid="combine-blocked">
            {blocked}
          </div>
        )}
        {isStale && (
          <div className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm" data-testid="combine-stale">
            One of these lines no longer exists. Close this and pick the join again.
          </div>
        )}
        {error && (
          <div className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm" data-testid="combine-error">
            {error}
          </div>
        )}

        <div className="flex justify-end gap-2 pt-4 border-t border-border">
          <Button onClick={closeDialog} disabled={isSubmitting} data-testid="combine-cancel">
            Cancel
          </Button>
          <Button
            variant="primary"
            onClick={handleConfirm}
            disabled={isSubmitting || isStale || Boolean(blocked) || nameError !== null}
            title={nameError ?? undefined}
            data-primary-action=""
            data-testid="combine-confirm"
          >
            {isSubmitting ? 'Combining...' : 'Combine'}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
