import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { isEditingBlockedNow } from '../../hooks/useEditingBlocked';
import { getErrorMessage } from '../../utils/ipc';
import { trackDialogComplete } from '../../utils/telemetry';
import { formatStationIdWithName } from '../../utils/formatters';
import { formatLength } from '../../utils/formatStats';
import { defaultConfig } from '../../types/config';
import { combinePlanFor, defaultCombinedName } from '../../utils/combineSections';
import type { CombineCandidate } from '../../types/ipc/CombineCandidate';
import type { CombinePick } from '../../types/ipc/CombinePick';
import type { Section, SectionId, StationRef } from '../../types/project';
import type { CreditedPerson, Shot } from '../../types/ipc.generated';
import { formatCreditDateRange, shotCredit, summarizeCredits } from '../../utils/sectionCredit';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import { CombinePlanNotes } from './CombinePlanNotes';

const sectionLabel = (section: Section | undefined, id: number) =>
  section?.name?.trim() ? section.name : `Section ${id}`;

/**
 * Who the combined section credits, and when: every shot keeps its own
 * surveyors, explorers and date, so this is the two sections' shots taken
 * together, in the combined section's order (the other section reversed when
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
        <div data-testid="combine-credit-none">No surveyors, explorers or dates recorded on either section.</div>
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
 * Combine Sections: join two sections that meet end to end (merged,
 * co-located, or across a loop closure) into one. Other sections there stay
 * attached as branches. Opened with the backend's `CombineCandidate` for one
 * pair: a two-section selection, or the pair picked in the station menu
 * ("Combine Section With <name>...", or the "Combine Sections..." submenu at a
 * junction), with `keep` the section the menu named first. What else the
 * combine changes for the kept section shown (a side that moves, ties that
 * become loop closures, converted bearings...) is listed, with everything
 * else informational, under a folded "Details".
 */
export default function CombineSectionsDialog() {
  const { t } = useTranslation();
  const { closeDialog, dialogData, setSelection } = useUIStore();
  const { combineSections, project, stations, config } = useProjectStore();
  const lengthUnit = config?.units?.length ?? defaultConfig.units.length;
  const candidate = dialogData.candidate as CombineCandidate | undefined;

  const sectionA = project?.sections.find((s) => s.id === candidate?.section_a);
  const sectionB = project?.sections.find((s) => s.id === candidate?.section_b);
  const isStale = !candidate || !sectionA || !sectionB;

  // `a` (the lower id) is kept unless the user swaps.
  // Start on `b` only when `a` cannot be kept but `b` can (its shot numbers
  // are used up for the session).
  // The section the menu named first (`keep`) is kept, as long as it can be:
  // otherwise `a`, the lower id, unless `a` cannot be kept but `b` can (its
  // shot numbers are used up for the session).
  const [keepB, setKeepB] = useState(() => {
    const keep = dialogData.keep as SectionId | undefined;
    if (keep === candidate?.section_b && !candidate?.blocked_keeping_b) return true;
    if (keep === candidate?.section_a && !candidate?.blocked_keeping_a) return false;
    return Boolean(candidate?.blocked_keeping_a) && !candidate?.blocked_keeping_b;
  });
  const kept = keepB ? sectionB : sectionA;
  const other = keepB ? sectionA : sectionB;
  const keptId = (keepB ? candidate?.section_b : candidate?.section_a) ?? 0;
  const otherId = (keepB ? candidate?.section_a : candidate?.section_b) ?? 0;
  const keptName = sectionLabel(kept, keptId);
  const otherName = sectionLabel(other, otherId);

  // From the RAW names: an unnamed kept section defaults to unnamed (the field
  // shows the "Section N" label as its placeholder), not to that label.
  const defaultName = defaultCombinedName(kept?.name ?? '', other?.name ?? '');
  const [name, setName] = useState<string | null>(null);
  const [color, setColor] = useState<string | null>(null);
  const [swapLr, setSwapLr] = useState(true);
  const [commentPick, setCommentPick] = useState<CombinePick>('kept');
  const [namePick, setNamePick] = useState<CombinePick>('kept');
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Untouched fields follow the kept section when the user swaps.
  const shownName = name ?? defaultName;
  const shownColor = color ?? kept?.color ?? '#3b82f6';

  const join = keepB ? candidate?.station_b : candidate?.station_a;
  const joinName = join
    ? stations.find(
        (s) => s.station_ref.section_id === join.section_id && s.station_ref.station_index === join.station_index
      )?.name
    : undefined;
  const joinLabel = join ? formatStationIdWithName(join.section_id, join.station_index, joinName) : '';
  const stationLabel = (ref: StationRef) =>
    formatStationIdWithName(
      ref.section_id,
      ref.station_index,
      stations.find(
        (s) => s.station_ref.section_id === ref.section_id && s.station_ref.station_index === ref.station_index
      )?.name
    );
  const plan = candidate ? combinePlanFor(candidate, keptId) : undefined;
  const keptShots = (keepB ? candidate?.shot_count_b : candidate?.shot_count_a) ?? 0;
  const otherShots = (keepB ? candidate?.shot_count_a : candidate?.shot_count_b) ?? 0;
  const summary = t('dialogs.combineSections.summary', {
    kept: keptName,
    keptShots: t('dialogs.combineSections.shots', { count: keptShots }),
    other: otherName,
    otherShots: t('dialogs.combineSections.shots', { count: otherShots }),
    station: joinLabel,
  });
  // Where the other section's shots go (wording from #713): the kept section
  // keeps its station 0 unless the join is at its start.
  const keptEnd = keepB ? candidate?.end_b : candidate?.end_a;
  const otherEnd = keepB ? candidate?.end_a : candidate?.end_b;
  const placement = `${otherName} will be ${candidate?.reverses ? 'reversed and ' : ''}added ${
    keptEnd === 'start' ? 'before' : 'after'
  } ${keptName}.`;

  const keptText = (a: string | null | undefined, b: string | null | undefined) => (keepB ? b : a) ?? '';
  const otherText = (a: string | null | undefined, b: string | null | undefined) => (keepB ? a : b) ?? '';
  const commentConflict = Boolean(candidate?.comment_a && candidate?.comment_b);
  const nameConflict = Boolean(candidate?.station_name_a && candidate?.station_name_b);
  const blocked =
    candidate?.blocked_reason ?? (keepB ? candidate?.blocked_keeping_b : candidate?.blocked_keeping_a) ?? null;
  const sectionName = (id: SectionId) => sectionLabel(project?.sections.find((s) => s.id === id), id);
  const listNames = (ids: SectionId[]) => ids.map(sectionName).join(', ');
  const handleConfirm = async () => {
    if (isSubmitting || isStale || blocked) return;
    if (isEditingBlockedNow()) {
      setError('Editing is disabled in the current view.');
      return;
    }
    setIsSubmitting(true);
    setError(null);
    const dialogKey = dialogData;
    try {
      const result = await combineSections(keptId, otherId, {
        name: shownName.trim(),
        color: shownColor,
        swap_lr: swapLr,
        comment_pick: commentConflict ? commentPick : 'kept',
        name_pick: nameConflict ? namePick : 'kept',
        // The join this dialog shows: two sections can meet at more than one
        // place (a loop of two), and the backend must not pick another.
        ...(keptEnd && otherEnd ? { join: { kept: keptEnd, other: otherEnd } } : {}),
      });
      if (!result.success) {
        setError(result.error ?? 'Failed to combine sections');
        setIsSubmitting(false);
        return;
      }
      setSelection({ type: 'section', sectionId: keptId });
      if (useUIStore.getState().dialogData !== dialogKey) return;
      trackDialogComplete('combine-sections');
      closeDialog();
    } catch (e) {
      setError(getErrorMessage(e, 'Failed to combine sections'));
      setIsSubmitting(false);
    }
  };

  const pickChoice = (
    key: 'comment' | 'name',
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
      title="Combine Sections"
      onClose={closeDialog}
      size="md"
      closeOnBackdropClick={!isSubmitting}
      data-testid="combine-sections-dialog"
    >
      <div className="space-y-4">
        <p className="text-text-secondary" data-testid="combine-summary">{summary}</p>
        {!isStale && <p className="text-sm" data-testid="combine-plan">{placement}</p>}
        {!isStale && candidate?.join_kind === 'closure' && (
          <p className="text-sm" data-testid="combine-closure-join">
            {t('dialogs.combineSections.closureJoin', {
              kept: keptName,
              other: otherName,
              distance: formatLength(plan?.shift_m ?? 0, lengthUnit, 2),
            })}
          </p>
        )}

        <div className="flex items-center justify-between gap-2 text-sm">
          <span data-testid="combine-kept-label">
            Keep <strong>{keptName}</strong> (its id, shot numbers and direction)
          </span>
          <Button onClick={() => setKeepB((v) => !v)} disabled={isSubmitting || isStale} data-testid="combine-swap-kept">
            Keep {otherName} instead
          </Button>
        </div>

        <div className="flex items-end gap-2">
          <div className="flex-1">
            <label className="block text-sm text-text-secondary mb-1" htmlFor="combine-name">
              Combined section name
            </label>
            <input
              id="combine-name"
              type="text"
              value={shownName}
              placeholder={`${keptName} (leave empty to keep it unnamed)`}
              onChange={(e) => setName(e.target.value)}
              className="w-full"
              disabled={isSubmitting}
              data-testid="combine-name"
            />
          </div>
          <input
            type="color"
            value={shownColor}
            onChange={(e) => setColor(e.target.value)}
            className="w-8 h-8 rounded cursor-pointer border border-border"
            aria-label="Combined section colour"
            disabled={isSubmitting}
            data-testid="combine-color"
          />
        </div>

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
        {nameConflict &&
          pickChoice('name', 'The join station has two names. Keep:', namePick, setNamePick, [
            ['kept', keptText(candidate?.station_name_a, candidate?.station_name_b)],
            ['other', otherText(candidate?.station_name_a, candidate?.station_name_b)],
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
                sectionName={sectionName}
                stationLabel={stationLabel}
              />
            )}

            <p className="text-text-secondary" data-testid="combine-direction-note">
              The combined section keeps {keptName}'s survey direction ({kept?.direction === 'out' ? 'Out' : 'In'}).
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
            One of these sections no longer exists. Close this and pick the join again.
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
            disabled={isSubmitting || isStale || Boolean(blocked)}
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
