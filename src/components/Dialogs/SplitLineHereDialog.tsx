import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { isEditingBlockedNow } from '../../hooks/useEditingBlocked';
import { getErrorMessage } from '../../utils/ipc';
import { trackDialogComplete } from '../../utils/telemetry';
import { formatLength } from '../../utils/formatStats';
import { defaultSplitNames, lineNameProblemMessage, splitNameProblems } from '../../utils/nameValidation';
import { useProse } from '../../hooks/useProse';
import { lineDisplayName } from '../../utils/stationIdentity';
import { splitSecondHalfColor } from '../../utils/colors';
import { defaultConfig } from '../../types/config';
import type { SplitAt } from '../../types/ipc/SplitAt';
import { Button } from '../common/Button';
import { Prose } from '../common/Prose';
import { DialogWrapper } from './DialogWrapper';

/**
 * "Split Line Here" (#335): cut one line into two at a station, or at a
 * point along a shot, without deleting anything.
 *
 * On the station graph the two halves SHARE the split station (plan 5.4): the
 * second half starts on it, so the halves stay connected, nothing moves, and
 * the station keeps its one comment, datum and Station ID. So there is no side
 * to choose for the comment or the datum, and no loop closure that becomes a
 * merge (the preview had nothing to report); the dialog says what the shared
 * station means instead.
 */
export default function SplitLineHereDialog() {
  const { t } = useTranslation();
  const { closeDialog, dialogData, clearSelection, resolvedTheme } = useUIStore();
  const { splitLine, project, config } = useProjectStore();
  const lengthUnit = config?.units?.length ?? defaultConfig.units.length;
  // The line, the shot and the station the summary names, as prose pills (RS-PROSE).
  const prose = useProse();

  const lineId = dialogData.lineId as number | undefined;
  const at = dialogData.at as SplitAt | undefined;

  const line = project?.lines.find((s) => s.id === lineId);
  // A line gone since the dialog opened reads plain (`Unnamed`); the dialog says it is stale.
  const lineName = line ? prose.line(line.id) : lineDisplayName(undefined);
  const lineColor = line?.color ?? '#3b82f6';

  const shotIndex = at?.type === 'shot' ? (line?.shots.findIndex((s) => s.id === at.shot_id) ?? -1) : -1;
  const shot = shotIndex >= 0 ? line?.shots[shotIndex] : undefined;
  const isStale = !line || !at || (at.type === 'shot' && !shot);

  // `<name> A` / `<name> B`, neither taken by another line nor by each other.
  const [defaults] = useState(() => defaultSplitNames(line?.name, project?.lines, lineId));
  const [nameA, setNameA] = useState(defaults.a);
  const [nameB, setNameB] = useState(defaults.b);
  const [colorA, setColorA] = useState(lineColor);
  // Half B is a new line, so it defaults to the colour a new line gets
  // (the next palette colour no line uses), which is never half A's.
  const [colorB, setColorB] = useState(() => splitSecondHalfColor(project?.lines, lineColor, resolvedTheme));
  const [isSubmitting, setIsSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Half A keeps the line (and may keep its name); B becomes a new line. Both
  // must be named, not after another line, and not after each other.
  const problems = splitNameProblems(nameA, nameB, project?.lines, lineId);
  const errorA = problems.a ? lineNameProblemMessage(problems.a, nameA) : null;
  const errorB = problems.b ? lineNameProblemMessage(problems.b, nameB) : null;
  const isValid = !errorA && !errorB;

  // The split point by name: the shot `4 → 5`, or the station `17`.
  const summary =
    at?.type === 'shot' && shot && lineId !== undefined
      ? `Splitting ${lineName} ${formatLength(at.distance, lengthUnit, 2)} (${Math.round(
          (at.distance / shot.length) * 100
        )}%) along shot ${prose.shotAt(lineId, shotIndex)}`
      : `Splitting ${lineName} at station ${
          at?.type === 'station' && lineId !== undefined
            ? prose.station({ line_id: lineId, station_index: at.station_index })
            : ''
        }`;

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
        // The halves share the split station, so its comment and datum stay
        // on it: there is no side to choose (S9).
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
        <p className="text-text-secondary" data-testid="split-here-summary">
          <Prose text={summary} />
        </p>
        <p className="text-sm text-text-secondary" data-testid="split-here-credit-note">
          Each shot keeps its own surveyors, explorers and survey date.
        </p>
        <p className="text-sm text-text-tertiary" data-testid="split-here-shared-station-hint">
          {t('dialogs.splitSectionHere.halvesShareStation')}
        </p>

        <div className="space-y-3">
          {half('a', 'Line A name (first half)', nameA, setNameA, colorA, setColorA, errorA)}
          {half('b', 'Line B name (second half)', nameB, setNameB, colorB, setColorB, errorB)}
        </div>

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
