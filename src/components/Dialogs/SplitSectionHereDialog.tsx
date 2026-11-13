import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { isEditingBlockedNow } from '../../hooks/useEditingBlocked';
import { getErrorMessage } from '../../utils/ipc';
import { trackDialogComplete } from '../../utils/telemetry';
import { formatLength } from '../../utils/formatStats';
import { MIN_SPLIT_CHILD_LENGTH_M } from '../../utils/shotSplit';
import { previewSplitSection } from '../../utils/splitSection';
import { defaultConfig } from '../../types/config';
import type { SplitAt } from '../../types/ipc/SplitAt';
import type { SplitPreview } from '../../types/ipc/SplitPreview';
import type { SplitSide } from '../../types/ipc/SplitSide';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';

/**
 * "Split Section Here" (#335): cut one section into two at a station, or at a
 * point along a shot, without deleting anything.
 */
export default function SplitSectionHereDialog() {
  const { t } = useTranslation();
  const { closeDialog, dialogData, clearSelection } = useUIStore();
  const { splitSection, project, stations, config } = useProjectStore();
  const lengthUnit = config?.units?.length ?? defaultConfig.units.length;

  const sectionId = dialogData.sectionId as number | undefined;
  const at = dialogData.at as SplitAt | undefined;
  const label = (dialogData.label as string | undefined) ?? '';

  const section = project?.sections.find((s) => s.id === sectionId);
  const sectionName = section?.name ?? `Section ${sectionId ?? ''}`;
  const sectionColor = section?.color ?? '#3b82f6';

  const shotIndex = at?.type === 'shot' ? (section?.shots.findIndex((s) => s.id === at.shot_id) ?? -1) : -1;
  const shot = shotIndex >= 0 ? section?.shots[shotIndex] : undefined;
  const isStale = !section || !at || (at.type === 'shot' && !shot);

  // The existing station the cut lands on, if any: a mid-shot cut makes a fresh
  // station that carries no comment or datum.
  const splitStationIndex = (() => {
    if (at?.type === 'station') return at.station_index;
    if (!shot || at?.type !== 'shot') return null;
    // A cut this close to a shot end snaps to that end's station, as the backend does.
    if (at.distance < MIN_SPLIT_CHILD_LENGTH_M) return shotIndex;
    if (shot.length - at.distance < MIN_SPLIT_CHILD_LENGTH_M) return shotIndex + 1;
    return null;
  })();
  const splitStation =
    splitStationIndex === null
      ? undefined
      : stations.find(
          (s) => s.station_ref.section_id === sectionId && s.station_ref.station_index === splitStationIndex
        );
  const hasComment = Boolean(splitStation?.comment);
  const isDatum = splitStation?.is_zero_datum ?? false;

  const [nameA, setNameA] = useState(`${sectionName}-A`);
  const [nameB, setNameB] = useState(`${sectionName}-B`);
  const [colorA, setColorA] = useState(sectionColor);
  const [colorB, setColorB] = useState(sectionColor);
  const [commentSide, setCommentSide] = useState<SplitSide>('a');
  const [datumSide, setDatumSide] = useState<SplitSide>('a');
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const isValid = nameA.trim().length > 0 && nameB.trim().length > 0;

  // A loop closure from before the cut to past it, with nothing else tying the
  // halves, becomes a merge and moves one half: say so before the user confirms.
  // Only the datum's side changes the answer (it decides which half is anchored).
  const [preview, setPreview] = useState<SplitPreview | null>(null);
  const previewDatumSide = isDatum ? datumSide : 'a';
  useEffect(() => {
    if (isStale || sectionId === undefined || !at) return;
    let live = true;
    const options = {
      name_a: 'A',
      name_b: 'B',
      color_a: sectionColor,
      color_b: sectionColor,
      comment_side: 'a' as const,
      datum_side: previewDatumSide,
    };
    previewSplitSection(sectionId, at, options)
      .then((answer) => live && setPreview(answer))
      .catch(() => live && setPreview(null));
    return () => {
      live = false;
    };
  }, [isStale, sectionId, at, sectionColor, previewDatumSide]);
  const movedName = preview?.moved_side === 'a' ? nameA.trim() || 'A' : nameB.trim() || 'B';
  const otherName = preview?.moved_side === 'a' ? nameB.trim() || 'B' : nameA.trim() || 'A';
  const closures = preview?.closures_to_merges ?? 0;

  const summary =
    at?.type === 'shot' && shot
      ? `Splitting ${sectionName} ${formatLength(at.distance, lengthUnit, 2)} (${Math.round(
          (at.distance / shot.length) * 100
        )}%) along shot ${label}`
      : `Splitting ${sectionName} at station ${label}`;

  const handleConfirm = async () => {
    if (isSubmitting || isStale || !isValid || sectionId === undefined || !at) return;
    if (isEditingBlockedNow()) {
      setError('Editing is disabled in the current view.');
      return;
    }
    setIsSubmitting(true);
    setError(null);
    const dialogKey = dialogData;
    try {
      const result = await splitSection(sectionId, at, {
        name_a: nameA.trim(),
        name_b: nameB.trim(),
        color_a: colorA,
        color_b: colorB,
        comment_side: hasComment ? commentSide : 'a',
        datum_side: isDatum ? datumSide : 'a',
      });
      if (!result.success) {
        setError(result.error ?? 'Failed to split section');
        setIsSubmitting(false);
        return;
      }
      clearSelection();
      if (useUIStore.getState().dialogData !== dialogKey) return;
      trackDialogComplete('split-section-here');
      closeDialog();
    } catch (e) {
      setError(getErrorMessage(e, 'Failed to split section'));
      setIsSubmitting(false);
    }
  };

  const sideChoice = (
    name: string,
    legend: string,
    value: SplitSide,
    onChange: (side: SplitSide) => void,
    sides: SplitSide[]
  ) => (
    <fieldset className="space-y-1" data-testid={`split-here-${name}-side`}>
      <legend className="text-sm text-text-secondary mb-1">{legend}</legend>
      <div className="flex gap-4">
        {sides.map((side) => (
          <label key={side} className="flex items-center gap-1 text-sm cursor-pointer">
            <input
              type="radio"
              name={`split-here-${name}-side`}
              checked={value === side}
              onChange={() => onChange(side)}
              disabled={isSubmitting}
              data-testid={`split-here-${name}-side-${side}`}
            />
            {side === 'both' ? 'Both' : side.toUpperCase()}
          </label>
        ))}
      </div>
    </fieldset>
  );

  const half = (
    key: 'a' | 'b',
    title: string,
    name: string,
    setName: (v: string) => void,
    color: string,
    setColor: (v: string) => void
  ) => (
    <div className="flex items-end gap-2">
      <div className="flex-1">
        <label className="block text-sm text-text-secondary mb-1">{title}</label>
        <input
          type="text"
          value={name}
          onChange={(e) => setName(e.target.value)}
          className="w-full"
          disabled={isSubmitting}
          data-testid={`split-here-name-${key}`}
        />
      </div>
      <input
        type="color"
        value={color}
        onChange={(e) => setColor(e.target.value)}
        className="w-8 h-8 rounded cursor-pointer border border-border"
        aria-label={`${title} colour`}
        disabled={isSubmitting}
        data-testid={`split-here-color-${key}`}
      />
    </div>
  );

  return (
    <DialogWrapper
      title="Split Section Here"
      onClose={closeDialog}
      size="md"
      closeOnBackdropClick={!isSubmitting}
      data-testid="split-section-here-dialog"
    >
      <div className="space-y-4">
        <p className="text-text-secondary" data-testid="split-here-summary">{summary}</p>
        <p className="text-sm text-text-secondary" data-testid="split-here-credit-note">
          Each shot keeps its own surveyors, explorers and survey date.
        </p>
        <p className="text-sm text-text-tertiary" data-testid="split-here-unconnected-hint">
          {t('dialogs.splitSectionHere.halvesNotConnected')}
        </p>

        <div className="space-y-3">
          {half('a', 'Section A name (first half)', nameA, setNameA, colorA, setColorA)}
          {half('b', 'Section B name (second half)', nameB, setNameB, colorB, setColorB)}
        </div>

        {hasComment &&
          sideChoice('comment', 'Keep the station comment on', commentSide, setCommentSide, ['a', 'b', 'both'])}
        {isDatum && sideChoice('datum', 'Keep the zero datum on', datumSide, setDatumSide, ['a', 'b'])}

        {preview?.moved_side && (
          <div
            className="p-3 bg-warning-10 border border-warning-20 rounded text-warning text-sm"
            data-testid="split-here-closure-warning"
          >
            {movedName} is tied to {otherName} only by {closures === 1 ? 'a loop closure' : `${closures} loop closures`},
            so the split turns {closures === 1 ? 'it into a merge' : 'them into merges'} and moves {movedName}{' '}
            {formatLength(preview.moved_m, lengthUnit, 2)} to close {closures === 1 ? 'it' : 'them'}.
          </div>
        )}

        {isStale && (
          <div className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm" data-testid="split-here-stale">
            This section or shot no longer exists. Close this and pick the split point again.
          </div>
        )}

        {error && (
          <div className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm" data-testid="split-here-error">
            {error}
          </div>
        )}

        <div className="flex justify-end gap-2 pt-4 border-t border-border">
          <Button onClick={closeDialog} disabled={isSubmitting} data-testid="split-here-cancel">
            Cancel
          </Button>
          <Button
            variant="primary"
            onClick={handleConfirm}
            disabled={isSubmitting || isStale || !isValid}
            data-primary-action=""
            data-testid="split-here-confirm"
          >
            {isSubmitting ? 'Splitting...' : 'Split'}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
