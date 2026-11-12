import { useState } from 'react';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { isEditingBlockedNow } from '../../hooks/useEditingBlocked';
import { getErrorMessage } from '../../utils/ipc';
import { trackDialogComplete } from '../../utils/telemetry';
import { formatStationIdWithName } from '../../utils/formatters';
import { defaultCombinedName } from '../../utils/combineSections';
import type { CombineCandidate } from '../../types/ipc/CombineCandidate';
import type { CombinePick } from '../../types/ipc/CombinePick';
import type { Section } from '../../types/project';
import type { CreditedPerson, Shot } from '../../types/ipc.generated';
import { formatCreditDateRange, shotCredit, summarizeCredits } from '../../utils/sectionCredit';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';

const sectionLabel = (section: Section | undefined, id: number) =>
  section?.name?.trim() ? section.name : `Section ${id}`;

/**
 * Who the combined section credits, and when: every shot keeps its own
 * surveyors, explorers and date, so this is the two sections' shots taken
 * together, in the combined section's order (the other section reversed when
 * it is, and before or after the kept one).
 */
function CombinedCreditSummary({ shots }: { shots: Array<Pick<Shot, 'credit'>> }) {
  const summary = summarizeCredits(shots.map(shotCredit));
  const people = (list: CreditedPerson[]) =>
    list.map((p) => `${p.person.name} (${p.shots} ${p.shots === 1 ? 'shot' : 'shots'})`).join(', ');
  const dates = formatCreditDateRange(summary);
  const empty = summary.surveyors.length === 0 && summary.explorers.length === 0 && !dates;
  return (
    <div className="space-y-1 text-sm" data-testid="combine-credit">
      <div className="text-text-secondary">Survey credit of the combined section ({summary.shot_count} shots)</div>
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
 * Combine Sections: join two sections that meet end to end into one, the
 * inverse of Split Section Here. Opened with the backend's `CombineCandidate`
 * (from the station context menu or a two-section selection).
 */
export default function CombineSectionsDialog() {
  const { closeDialog, dialogData, setSelection } = useUIStore();
  const { combineSections, project, stations } = useProjectStore();
  const candidate = dialogData.candidate as CombineCandidate | undefined;

  const sectionA = project?.sections.find((s) => s.id === candidate?.section_a);
  const sectionB = project?.sections.find((s) => s.id === candidate?.section_b);
  const isStale = !candidate || !sectionA || !sectionB;

  // `a` (the lower id) is kept unless the user swaps.
  const [keepB, setKeepB] = useState(false);
  const kept = keepB ? sectionB : sectionA;
  const other = keepB ? sectionA : sectionB;
  const keptId = (keepB ? candidate?.section_b : candidate?.section_a) ?? 0;
  const otherId = (keepB ? candidate?.section_a : candidate?.section_b) ?? 0;
  const keptName = sectionLabel(kept, keptId);
  const otherName = sectionLabel(other, otherId);

  const defaultName = defaultCombinedName(keptName, otherName);
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
  const keptShots = (keepB ? candidate?.shot_count_b : candidate?.shot_count_a) ?? 0;
  const otherShots = (keepB ? candidate?.shot_count_a : candidate?.shot_count_b) ?? 0;
  const summary = `Combining ${keptName} (${keptShots} shots) and ${otherName} (${otherShots} shots) at station ${joinLabel}`;
  // Where the other section's shots go (wording from #713): the kept section
  // keeps its station 0 unless the join is at its start.
  const keptEnd = keepB ? candidate?.end_b : candidate?.end_a;
  const plan = `${otherName} will be ${candidate?.reverses ? 'reversed and ' : ''}added ${
    keptEnd === 'start' ? 'before' : 'after'
  } ${keptName}.`;

  const keptText = (a: string | null | undefined, b: string | null | undefined) => (keepB ? b : a) ?? '';
  const otherText = (a: string | null | undefined, b: string | null | undefined) => (keepB ? a : b) ?? '';
  const commentConflict = Boolean(candidate?.comment_a && candidate?.comment_b);
  const nameConflict = Boolean(candidate?.station_name_a && candidate?.station_name_b);
  const blocked = candidate?.blocked_reason ?? null;
  const isValid = shownName.trim().length > 0;

  const handleConfirm = async () => {
    if (isSubmitting || isStale || blocked || !isValid) return;
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
        {!isStale && <p className="text-sm" data-testid="combine-plan">{plan}</p>}

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
          <div className="space-y-2 text-sm" data-testid="combine-reverse-notice">
            <p>
              Reversing {otherName} reciprocates its bearings and swaps its depths, so every station stays where
              it is.
            </p>
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

        <p className="text-sm text-text-secondary" data-testid="combine-direction-note">
          The combined section keeps {keptName}'s survey direction ({kept?.direction === 'out' ? 'Out' : 'In'}).
        </p>

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

        {!isStale && kept && other && (
          <CombinedCreditSummary
            shots={(() => {
              const moved = candidate?.reverses ? [...other.shots].reverse() : other.shots;
              const prepends = (keepB ? candidate?.end_b : candidate?.end_a) === 'start';
              return prepends ? [...moved, ...kept.shots] : [...kept.shots, ...moved];
            })()}
          />
        )}

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
            disabled={isSubmitting || isStale || Boolean(blocked) || !isValid}
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
