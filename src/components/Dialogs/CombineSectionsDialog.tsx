import { useEffect, useState } from 'react';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { isEditingBlockedNow } from '../../hooks/useEditingBlocked';
import { getErrorMessage } from '../../utils/ipc';
import { trackDialogComplete } from '../../utils/telemetry';
import { formatLength } from '../../utils/formatStats';
import {
  combineCandidate,
  combineCandidateAtStation,
  defaultCombinedName,
  prependsOther,
  swapCandidate,
} from '../../utils/combineSections';
import { defaultConfig } from '../../types/config';
import type { StationRef } from '../../types/project';
import type { CombineCandidate } from '../../types/ipc/CombineCandidate';
import type { SplitSide } from '../../types/ipc/SplitSide';
import { Button } from '../common/Button';
import { EntityBlob } from '../common/EntityBlob';
import { DialogWrapper } from './DialogWrapper';

/**
 * "Combine Sections" (#707): join two sections whose ends meet into one, the
 * inverse of Split Section Here. Opened from a section's end station
 * (`{ station }`) or from two selected sections (`{ a, b }`).
 */
export default function CombineSectionsDialog() {
  const { closeDialog, dialogData, setSelection } = useUIStore();
  const { combineSections, project, config } = useProjectStore();
  const lengthUnit = config?.units?.length ?? defaultConfig.units.length;

  const station = dialogData.station as StationRef | undefined;
  const pairA = dialogData.a as number | undefined;
  const pairB = dialogData.b as number | undefined;

  // undefined while loading, null when the two don't meet end to end.
  const [fetched, setFetched] = useState<CombineCandidate | null | undefined>(undefined);
  const [swapped, setSwapped] = useState(false);
  const [name, setName] = useState('');
  const [color, setColor] = useState('#3b82f6');
  const [commentSide, setCommentSide] = useState<SplitSide>('a');
  const [nameSide, setNameSide] = useState<SplitSide>('a');
  const [offsetSide, setOffsetSide] = useState<SplitSide>('a');
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const sectionName = (id: number) => project?.sections.find((s) => s.id === id)?.name ?? `Section ${id}`;
  const sectionColor = (id: number) => project?.sections.find((s) => s.id === id)?.color ?? '#3b82f6';

  const resetDefaults = (c: CombineCandidate) => {
    setName(defaultCombinedName(sectionName(c.kept), sectionName(c.other)));
    setColor(sectionColor(c.kept));
    setCommentSide('a');
    setNameSide('a');
    setOffsetSide('a');
  };

  useEffect(() => {
    let live = true;
    const query = station ? combineCandidateAtStation(station) : pairA !== undefined && pairB !== undefined ? combineCandidate(pairA, pairB) : Promise.resolve(null);
    query
      .then((c) => {
        if (!live) return;
        setFetched(c);
        if (c) resetDefaults(c);
      })
      .catch((e: unknown) => {
        if (!live) return;
        setFetched(null);
        setError(getErrorMessage(e, 'Failed to check the sections'));
      });
    return () => {
      live = false;
    };
    // The candidate is fetched once per opening; dialogData identifies it.
  }, [dialogData]);

  const candidate = fetched && swapped ? swapCandidate(fetched) : fetched;
  const keptName = candidate ? sectionName(candidate.kept) : '';
  const otherName = candidate ? sectionName(candidate.other) : '';
  const canConfirm = Boolean(candidate) && !candidate?.blocked && !isSubmitting;

  const handleSwap = () => {
    if (!fetched) return;
    const next = !swapped;
    setSwapped(next);
    resetDefaults(next ? swapCandidate(fetched) : fetched);
  };

  const handleConfirm = async () => {
    if (!candidate || candidate.blocked || isSubmitting) return;
    if (isEditingBlockedNow()) {
      setError('Editing is disabled in the current view.');
      return;
    }
    setIsSubmitting(true);
    setError(null);
    const dialogKey = dialogData;
    try {
      const result = await combineSections(candidate.kept, candidate.other, {
        name: name.trim(),
        color,
        comment_side: candidate.comment_conflict ? commentSide : 'a',
        name_side: candidate.name_conflict ? nameSide : 'a',
        offset_side: candidate.offset_conflict ? offsetSide : 'a',
      });
      if (!result.success) {
        setError(result.error ?? 'Failed to combine sections');
        setIsSubmitting(false);
        return;
      }
      setSelection({ type: 'section', sectionId: candidate.kept });
      if (useUIStore.getState().dialogData !== dialogKey) return;
      trackDialogComplete('combine-sections');
      closeDialog();
    } catch (e) {
      setError(getErrorMessage(e, 'Failed to combine sections'));
      setIsSubmitting(false);
    }
  };

  const sideChoice = (
    key: string,
    legend: string,
    value: SplitSide,
    onChange: (side: SplitSide) => void,
    sides: SplitSide[]
  ) => (
    <fieldset className="space-y-1" data-testid={`combine-${key}-side`}>
      <legend className="text-sm text-text-secondary mb-1">{legend}</legend>
      <div className="flex gap-4">
        {sides.map((side) => (
          <label key={side} className="flex items-center gap-1 text-sm cursor-pointer">
            <input
              type="radio"
              name={`combine-${key}-side`}
              checked={value === side}
              onChange={() => onChange(side)}
              disabled={isSubmitting}
              data-testid={`combine-${key}-side-${side}`}
            />
            {side === 'a' ? keptName : side === 'b' ? otherName : 'Both'}
          </label>
        ))}
      </div>
    </fieldset>
  );

  const body = () => {
    if (fetched === undefined) {
      return (
        <p className="text-text-secondary" data-testid="combine-loading">
          Checking the sections...
        </p>
      );
    }
    if (!candidate) {
      return (
        <div className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm" data-testid="combine-not-joined">
          {station ? "This station does not join exactly one other section's end." : "These sections don't meet end to end."}
        </div>
      );
    }
    const plan = `${otherName} will be ${candidate.reverse_other ? 'reversed and ' : ''}added ${
      prependsOther(candidate.pairing) ? 'before' : 'after'
    } ${keptName}.`;
    return (
      <>
        <div className="flex items-center gap-2 flex-wrap" data-testid="combine-sections-chips">
          <span className="text-text-secondary text-sm">Keep</span>
          <EntityBlob entity="section" testId="combine-kept">{keptName}</EntityBlob>
          <span className="text-text-secondary text-sm">and add</span>
          <EntityBlob entity="section" testId="combine-other">{otherName}</EntityBlob>
          <Button size="sm" onClick={handleSwap} disabled={isSubmitting} data-testid="combine-swap">
            Keep the other
          </Button>
        </div>
        <p className="text-text-secondary" data-testid="combine-plan">
          {plan} The direction of {keptName} is kept.
        </p>

        {candidate.blocked && (
          <div className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm" data-testid="combine-blocked">
            {candidate.blocked}
          </div>
        )}

        <div className="flex items-end gap-2">
          <div className="flex-1">
            <label className="block text-sm text-text-secondary mb-1">Combined section name</label>
            <input
              type="text"
              value={name}
              onChange={(e) => setName(e.target.value)}
              className="w-full"
              disabled={isSubmitting}
              data-testid="combine-name"
            />
          </div>
          <input
            type="color"
            value={color}
            onChange={(e) => setColor(e.target.value)}
            className="w-8 h-8 rounded cursor-pointer border border-border"
            aria-label="Combined section colour"
            disabled={isSubmitting}
            data-testid="combine-color"
          />
        </div>

        {candidate.credit.length > 0 && (
          <div className="space-y-1" data-testid="combine-credit">
            <div className="text-sm text-text-secondary">Credit</div>
            {candidate.credit.map((run, i) => (
              <div key={i} className="text-sm" data-testid={`combine-credit-${i}`}>
                {run.surveyor ?? 'Unknown'} · {run.date ?? 'Unknown'} · {formatLength(run.length_m, lengthUnit, 1)}
              </div>
            ))}
          </div>
        )}

        {candidate.comment_conflict &&
          sideChoice('comment', 'Keep the joined station comment from', commentSide, setCommentSide, ['a', 'b', 'both'])}
        {candidate.name_conflict &&
          sideChoice('name', 'Keep the joined station name from', nameSide, setNameSide, ['a', 'b'])}
        {candidate.offset_conflict &&
          sideChoice('offset', 'Keep the joined station position offset from', offsetSide, setOffsetSide, ['a', 'b'])}
      </>
    );
  };

  return (
    <DialogWrapper
      title="Combine Sections"
      onClose={closeDialog}
      size="md"
      closeOnBackdropClick={!isSubmitting}
      data-testid="combine-sections-dialog"
    >
      <div className="space-y-4">
        {body()}

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
            disabled={!canConfirm}
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
