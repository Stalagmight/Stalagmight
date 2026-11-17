/**
 * Edit Line's survey credit: a PENDING edit of every shot of the line,
 * shown as the line aggregate it would produce and applied on Save as ONE
 * `editShotCredit` (one undo step), never per keystroke.
 *
 * The fields show the union of the shots' people: someone on only some shots
 * is a partial pill ("12 of 30 shots"). Adding a pill puts that person on
 * every shot, its x takes them off every shot, and clicking a partial pill
 * puts them on the rest. The date is the shared date, or the read-only range
 * with "Set for all shots" when the shots disagree.
 */
import { useCallback, useMemo, useRef, useState } from 'react';
import type { Line } from '../../types/project';
import type { CreditEdit } from '../../types/ipc.generated';
import type { LinePeopleProps } from '../common/LineMetadataForm';
import {
  NO_PENDING,
  aggregateCredits,
  pendAdd,
  pendDate,
  pendRemove,
  previewCreditEdit,
  toCreditEdit,
  type CreditDateAggregate,
  type PendingCredit,
} from '../../utils/creditEdit';
import { shotCredit } from '../../utils/lineCredit';
import { useStablePeopleOrder } from '../../hooks/useStablePeopleOrder';

export interface LineCreditDraft {
  people: LinePeopleProps;
  dateAggregate: CreditDateAggregate;
  onDateChange: (value: string) => void;
  onDateReset?: () => void;
  /** The pending edit changes at least one shot. */
  changed: boolean;
  /**
   * The line has shots to carry a credit. Without any, an edit would
   * change nothing and be dropped on Save, so the fields are disabled.
   */
  available: boolean;
  /** A people field holds typed text that is not a pill yet. */
  hasPendingText: boolean;
  /**
   * The edit Save should send, or null for none. Commits whatever the focused
   * field holds first (by blurring it, which adds a typed name / sets a typed
   * date the way leaving the field always does), so a value typed and then
   * saved with the keyboard is kept.
   */
  takeEdit: () => CreditEdit | null;
  /** Forget the pending edit (the line was saved or reloaded). */
  reset: () => void;
}

export function useLineCreditDraft(line: Pick<Line, 'shots'> | undefined): LineCreditDraft {
  // State for rendering, a ref for Save: the blur in `takeEdit` adds the typed
  // name synchronously, before React re-renders.
  const [pending, setPendingState] = useState<PendingCredit>(NO_PENDING);
  const pendingRef = useRef<PendingCredit>(NO_PENDING);
  const setPending = useCallback((next: (prev: PendingCredit) => PendingCredit) => {
    pendingRef.current = next(pendingRef.current);
    setPendingState(pendingRef.current);
  }, []);
  const [hasPendingText, setHasPendingText] = useState(false);
  const pendingTextRef = useRef(false);

  const credits = useMemo(() => (line?.shots ?? []).map(shotCredit), [line]);
  const original = useMemo(() => aggregateCredits(credits), [credits]);
  const edit = useMemo(() => toCreditEdit(pending), [pending]);
  const { aggregate, changed } = useMemo(() => previewCreditEdit(credits, edit), [credits, edit]);

  // A pill just added sits at the end, beside the input it was typed in, for
  // as long as the dialog is open (the hook's memory lives with it).
  const surveyors = useStablePeopleOrder(aggregate.surveyors, 'dialog');
  const explorers = useStablePeopleOrder(aggregate.explorers, 'dialog');
  const people: LinePeopleProps = {
    surveyors,
    explorers,
    onAdd: (role, person) => setPending((p) => pendAdd(p, role, person)),
    onRemove: (role, person) => setPending((p) => pendRemove(p, role, person)),
    onPromote: (role, person) => setPending((p) => pendAdd(p, role, person)),
    onCommitPending: (has) => {
      pendingTextRef.current = has;
      setHasPendingText(has);
    },
  };

  const takeEdit = useCallback((): CreditEdit | null => {
    // Leaving the focused field commits what it holds, exactly as clicking
    // Save would: a name being typed becomes a pill, a date being typed is set.
    if (document.activeElement instanceof HTMLElement) document.activeElement.blur();
    const next = toCreditEdit(pendingRef.current);
    if (Object.keys(next).length === 0) return null;
    return previewCreditEdit(credits, next).changed ? next : null;
  }, [credits]);

  const reset = useCallback(() => setPending(() => NO_PENDING), [setPending]);

  return {
    people,
    dateAggregate: aggregate.date,
    onDateChange: (value) => setPending((p) => pendDate(p, value === '' ? null : value)),
    ...(pending.date !== undefined && original.date.kind === 'mixed'
      ? { onDateReset: () => setPending((p) => pendDate(p, undefined)) }
      : {}),
    changed,
    available: credits.length > 0,
    hasPendingText,
    takeEdit,
    reset,
  };
}
