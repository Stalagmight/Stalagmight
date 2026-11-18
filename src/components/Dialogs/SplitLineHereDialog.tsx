import { useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { isEditingBlockedNow } from '../../hooks/useEditingBlocked';
import { getErrorMessage } from '../../utils/ipc';
import { trackDialogComplete } from '../../utils/telemetry';
import { formatLength } from '../../utils/formatStats';
import { defaultSplitNames, lineNameProblemMessage, splitNameProblems } from '../../utils/nameValidation';
import { identityLabels, lineDisplayName } from '../../utils/stationIdentity';
import { getNextLineColor } from '../../utils/colors';
import { MIN_SPLIT_CHILD_LENGTH_M } from '../../utils/shotSplit';
import { previewSplitLine } from '../../utils/splitLine';
import { defaultConfig } from '../../types/config';
import type { SplitAt } from '../../types/ipc/SplitAt';
import type { SplitPreview } from '../../types/ipc/SplitPreview';
import type { SplitSide } from '../../types/ipc/SplitSide';
import type { StationRef } from '../../types/ipc/StationRef';
import { Button } from '../common/Button';
import { EntityBlob } from '../common/EntityBlob';
import { DialogWrapper } from './DialogWrapper';

/** A station by its line and name (`Main Line / 17`), as a pill at the Full level. */
function StationLabel({ station, label }: { station: StationRef; label: (ref: StationRef) => string }) {
  return <EntityBlob entity="station">{label(station)}</EntityBlob>;
}

/**
 * "Split Line Here" (#335): cut one line into two at a station, or at a
 * point along a shot, without deleting anything.
 */
export default function SplitLineHereDialog() {
  const { t } = useTranslation();
  const { closeDialog, dialogData, clearSelection, resolvedTheme } = useUIStore();
  const { splitLine, project, stations, surveyData, config } = useProjectStore();
  const lengthUnit = config?.units?.length ?? defaultConfig.units.length;
  const labels = useMemo(
    () => identityLabels({ surveyData, stations, lines: project?.lines ?? null }),
    [surveyData, stations, project?.lines]
  );

  const lineId = dialogData.lineId as number | undefined;
  const at = dialogData.at as SplitAt | undefined;

  const line = project?.lines.find((s) => s.id === lineId);
  const lineName = lineDisplayName(line?.name);
  const lineColor = line?.color ?? '#3b82f6';

  const shotIndex = at?.type === 'shot' ? (line?.shots.findIndex((s) => s.id === at.shot_id) ?? -1) : -1;
  const shot = shotIndex >= 0 ? line?.shots[shotIndex] : undefined;
  const isStale = !line || !at || (at.type === 'shot' && !shot);

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
          (s) => s.station_ref.line_id === lineId && s.station_ref.station_index === splitStationIndex
        );
  const hasComment = Boolean(splitStation?.comment);
  const isDatum = splitStation?.is_zero_datum ?? false;

  // `<name> A` / `<name> B`, neither taken by another line nor by each other.
  const [defaults] = useState(() => defaultSplitNames(line?.name, project?.lines, lineId));
  const [nameA, setNameA] = useState(defaults.a);
  const [nameB, setNameB] = useState(defaults.b);
  const [colorA, setColorA] = useState(lineColor);
  // Half B is a new line, so it defaults to the colour a new line gets
  // (the next palette colour no line uses), which is never half A's.
  // Compared lowercase: the palette is, a stored colour may not be.
  const [colorB, setColorB] = useState(() =>
    getNextLineColor(
      (project?.lines ?? []).map((s) => s.color.toLowerCase()).concat(lineColor.toLowerCase()),
      resolvedTheme
    )
  );
  const [commentSide, setCommentSide] = useState<SplitSide>('a');
  const [datumSide, setDatumSide] = useState<SplitSide>('a');
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Half A keeps the line (and may keep its name); B becomes a new line. Both
  // must be named, not after another line, and not after each other.
  const problems = splitNameProblems(nameA, nameB, project?.lines, lineId);
  const errorA = problems.a ? lineNameProblemMessage(problems.a, nameA) : null;
  const errorB = problems.b ? lineNameProblemMessage(problems.b, nameB) : null;
  const isValid = !errorA && !errorB;

  // A loop closure from before the cut to past it, with nothing else tying the
  // halves, becomes a merge and moves one half: say so before the user confirms.
  // Only the datum's side changes the answer (it decides which half is anchored).
  const [preview, setPreview] = useState<SplitPreview | null>(null);
  const previewDatumSide = isDatum ? datumSide : 'a';
  useEffect(() => {
    if (isStale || lineId === undefined || !at) return;
    let live = true;
    const options = {
      name_a: 'A',
      name_b: 'B',
      color_a: lineColor,
      color_b: lineColor,
      comment_side: 'a' as const,
      datum_side: previewDatumSide,
    };
    previewSplitLine(lineId, at, options)
      .then((answer) => live && setPreview(answer))
      .catch(() => live && setPreview(null));
    return () => {
      live = false;
    };
  }, [isStale, lineId, at, lineColor, previewDatumSide]);
  const movedName = preview?.moved_side === 'a' ? nameA.trim() || 'A' : nameB.trim() || 'B';
  const otherName = preview?.moved_side === 'a' ? nameB.trim() || 'B' : nameA.trim() || 'A';
  const closures = preview?.closures_to_merges ?? 0;
  // A closure that tied the moved half to the same point as the one that
  // becomes the merge is then a closure between merged stations, which the
  // split removes (#738): the halves were tied by more than the merged ones,
  // and the note lists the rest, by the stations they were created with. One
  // that already tied a point to itself (an undo can restore one from an old
  // history) goes too, but tied nothing, so it is listed apart and does not
  // change how the halves are described.
  const removed = preview?.removed_closures ?? [];
  const alreadyOnePoint = preview?.removed_already_one_point ?? [];
  const tiedBy =
    removed.length > 0 ? 'loop closures' : closures === 1 ? 'a loop closure' : `${closures} loop closures`;
  const turned =
    removed.length > 0
      ? closures === 1
        ? 'one of them into a merge'
        : `${closures} of them into merges`
      : closures === 1
        ? 'it into a merge'
        : 'them into merges';
  const moveNote = preview?.moved_side
    ? `${movedName} is tied to ${otherName} only by ${tiedBy}, so the split turns ${turned} and moves ` +
      `${movedName} ${formatLength(preview.moved_m, lengthUnit, 2)} to close ${closures === 1 ? 'it' : 'them'}.`
    : null;

  // The split point by name: the shot `4 → 5`, or the station `17`.
  const summary =
    at?.type === 'shot' && shot && lineId !== undefined
      ? `Splitting ${lineName} ${formatLength(at.distance, lengthUnit, 2)} (${Math.round(
          (at.distance / shot.length) * 100
        )}%) along shot ${labels.shotAt(lineId, shotIndex)}`
      : `Splitting ${lineName} at station ${
          at?.type === 'station' && lineId !== undefined
            ? labels.station({ line_id: lineId, station_index: at.station_index })
            : ''
        }`;
  const stationWithLine = (ref: StationRef) => labels.station(ref, true);

  const handleConfirm = async () => {
    if (isSubmitting || isStale || !isValid || lineId === undefined || !at) return;
    if (isEditingBlockedNow()) {
      setError('Editing is disabled in the current view.');
      return;
    }
    setIsSubmitting(true);
    setError(null);
    const dialogKey = dialogData;
    try {
      const result = await splitLine(lineId, at, {
        name_a: nameA.trim(),
        name_b: nameB.trim(),
        color_a: colorA,
        color_b: colorB,
        comment_side: hasComment ? commentSide : 'a',
        datum_side: isDatum ? datumSide : 'a',
      });
      if (!result.success) {
        setError(result.error ?? 'Failed to split line');
        setIsSubmitting(false);
        return;
      }
      clearSelection();
      if (useUIStore.getState().dialogData !== dialogKey) return;
      trackDialogComplete('split-line-here');
      closeDialog();
    } catch (e) {
      setError(getErrorMessage(e, 'Failed to split line'));
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
    setColor: (v: string) => void,
    problem: string | null
  ) => (
    <div>
      <div className="flex items-end gap-2">
        <div className="flex-1">
          <label className="block text-sm text-text-secondary mb-1" htmlFor={`split-here-name-${key}`}>
            {title}
          </label>
          <input
            id={`split-here-name-${key}`}
            type="text"
            value={name}
            onChange={(e) => setName(e.target.value)}
            className="w-full"
            disabled={isSubmitting}
            aria-invalid={problem !== null}
            {...(problem ? { 'aria-describedby': `split-here-name-${key}-error` } : {})}
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
      {problem && (
        <p
          id={`split-here-name-${key}-error`}
          className="mt-1 text-sm text-error"
          data-testid={`split-here-name-${key}-error`}
        >
          {problem}
        </p>
      )}
    </div>
  );

  return (
    <DialogWrapper
      title="Split Line Here"
      onClose={closeDialog}
      size="md"
      closeOnBackdropClick={!isSubmitting}
      data-testid="split-line-here-dialog"
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
          {half('a', 'Line A name (first half)', nameA, setNameA, colorA, setColorA, errorA)}
          {half('b', 'Line B name (second half)', nameB, setNameB, colorB, setColorB, errorB)}
        </div>

        {hasComment &&
          sideChoice('comment', 'Keep the station comment on', commentSide, setCommentSide, ['a', 'b', 'both'])}
        {isDatum && sideChoice('datum', 'Keep the zero datum on', datumSide, setDatumSide, ['a', 'b'])}

        {(moveNote || removed.length > 0 || alreadyOnePoint.length > 0) && (
          <div
            className="p-3 bg-warning-10 border border-warning-20 rounded text-warning text-sm space-y-1"
            data-testid="split-here-closure-warning"
          >
            {moveNote && <p data-testid="split-here-closure-move">{moveNote}</p>}
            {removed.map((closure, i) => (
              <p key={`${closure.closure_id.toString()}-${i}`} data-testid={`split-here-closure-removed-${i}`}>
                Loop closure <StationLabel station={closure.station_a} label={stationWithLine} /> to{' '}
                <StationLabel station={closure.station_b} label={stationWithLine} /> is removed: its two stations
                become one point.
              </p>
            ))}
            {alreadyOnePoint.map((closure, i) => (
              <p key={`${closure.closure_id.toString()}-${i}`} data-testid={`split-here-closure-already-${i}`}>
                Loop closure <StationLabel station={closure.station_a} label={stationWithLine} /> to{' '}
                <StationLabel station={closure.station_b} label={stationWithLine} /> is removed: its two stations
                are already one point.
              </p>
            ))}
          </div>
        )}

        {isStale && (
          <div className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm" data-testid="split-here-stale">
            This line or shot no longer exists. Close this and pick the split point again.
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
            title={errorA ?? errorB ?? undefined}
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
