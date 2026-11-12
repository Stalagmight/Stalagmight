/**
 * The survey-credit fields every form shares: a Surveyors and an Explorers
 * pill field ({@link CreditPeopleFields}) and the survey date
 * ({@link CreditDateInput}). The host owns the values and decides what an
 * edit means - a list in an import dialog, a pending edit of every shot in
 * Edit Section, an immediate edit in the Detail panel - so these only render
 * and report.
 *
 * People entered here are remembered for the autocomplete of every later
 * field, in any project (`store/recentPeople`), and the suggestions are the
 * project's own people, the linked SpeleoDB project's collaborators, then the
 * remembered ones (`usePeopleSuggestions`).
 */
import { useCallback, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { PeopleInput, type PeopleInputValue } from './PeopleInput';
import { usePeopleSuggestions } from '../../hooks/usePeopleSuggestions';
import { rememberPeople } from '../../store/recentPeople';
import type { CreditDateAggregate, CreditRole } from '../../utils/creditEdit';
import type { PersonLike } from '../../utils/people';

export interface CreditPeopleFieldsProps {
  surveyors: PeopleInputValue[];
  explorers: PeopleInputValue[];
  onAdd: (role: CreditRole, person: PersonLike) => void;
  onRemove: (role: CreditRole, person: PersonLike) => void;
  /** Clicking a partial pill (on some of the shots): put that person on all of them. */
  onPromote?: (role: CreditRole, person: PersonLike) => void;
  disabled?: boolean;
  /** Test IDs `<prefix>-people` (both fields), `<prefix>-surveyors` and `<prefix>-explorers` (see PeopleInput for the children). */
  testIdPrefix: string;
  /** Typed-but-uncommitted text in either field appears (true) or goes away (false). */
  onCommitPending?: (hasPending: boolean) => void;
  /** Side by side (a wide dialog) or one above the other (a panel). */
  layout?: 'stacked' | 'columns';
  autoFocus?: boolean;
}

export function CreditPeopleFields({
  surveyors,
  explorers,
  onAdd,
  onRemove,
  onPromote,
  disabled = false,
  testIdPrefix,
  onCommitPending,
  layout = 'stacked',
  autoFocus = false,
}: CreditPeopleFieldsProps) {
  const { t } = useTranslation();
  // Someone just entered in one field is offered in the other one too.
  const listed = useMemo(() => [...surveyors, ...explorers].map((v) => v.person), [surveyors, explorers]);
  const suggestions = usePeopleSuggestions(listed);

  const pendingRef = useRef<Record<CreditRole, boolean>>({ surveyors: false, explorers: false });
  const reportPending = useCallback(
    (role: CreditRole, has: boolean) => {
      const before = pendingRef.current.surveyors || pendingRef.current.explorers;
      pendingRef.current = { ...pendingRef.current, [role]: has };
      const after = pendingRef.current.surveyors || pendingRef.current.explorers;
      if (before !== after) onCommitPending?.(after);
    },
    [onCommitPending],
  );

  const field = (role: CreditRole, value: PeopleInputValue[]) => (
    <PeopleInput
      label={t(`credit.${role}`)}
      value={value}
      onAdd={(person) => {
        rememberPeople([person]);
        onAdd(role, person);
      }}
      onRemove={(person) => onRemove(role, person)}
      {...(onPromote
        ? {
            onPromote: (person: PersonLike) => {
              rememberPeople([person]);
              onPromote(role, person);
            },
          }
        : {})}
      suggestions={suggestions}
      placeholder={t(`credit.placeholders.${role}`)}
      disabled={disabled}
      testId={`${testIdPrefix}-${role}`}
      autoFocus={autoFocus && role === 'surveyors'}
      onCommitPending={(has) => reportPending(role, has)}
    />
  );

  return (
    <div
      className={layout === 'columns' ? 'grid grid-cols-2 gap-3' : 'space-y-3'}
      data-testid={`${testIdPrefix}-people`}
    >
      {field('surveyors', surveyors)}
      {field('explorers', explorers)}
    </div>
  );
}

export interface CreditDateInputProps {
  value: CreditDateAggregate;
  /** A date sets it on every shot the field covers; `null` clears it. Called on commit only. */
  onChange: (date: string | null) => void;
  /** Offered after a date was set over shots that had different ones: back to per-shot dates. */
  onReset?: () => void;
  disabled?: boolean;
  /** Test IDs: `<testId>` (the date input), `-mixed`, `-set-all`, `-set-all-cancel`, `-reset`. */
  testId: string;
}

interface DraftDateInputProps {
  /** The committed date ('' for none). */
  value: string;
  onCommit: (date: string | null) => void;
  /** Escape on an unchanged input. */
  onCancel?: () => void;
  disabled: boolean;
  autoFocus?: boolean;
  testId: string;
  label: string;
}

/**
 * A native date input (the control New Section uses) that COMMITS only on
 * Enter or leaving the field. A date input reports a value every time the
 * typed segments make a valid date, so typing 2026-04-11 over 2026-03-02 passes
 * through 2026-04-01; committing each of those wrote one undo step per segment.
 * Escape puts the committed date back (and, when nothing was changed, cancels
 * through `onCancel`). A half-typed date is never committed as "no date".
 */
function DraftDateInput({ value, onCommit, onCancel, disabled, autoFocus = false, testId, label }: DraftDateInputProps) {
  const [draft, setDraft] = useState(value);
  // The committed value moved (a commit landed, or another view changed it).
  const [seen, setSeen] = useState(value);
  if (seen !== value) {
    setSeen(value);
    setDraft(value);
  }
  const commit = (input: HTMLInputElement) => {
    // An incomplete date reads as '' with badInput set: keep what is committed.
    if (input.validity?.badInput) {
      setDraft(value);
      return;
    }
    if (draft !== value) onCommit(draft === '' ? null : draft);
  };
  return (
    <input
      type="date"
      value={draft}
      onChange={(event) => setDraft(event.target.value)}
      onBlur={(event) => commit(event.currentTarget)}
      onKeyDown={(event) => {
        if (event.key === 'Enter') {
          event.preventDefault();
          event.stopPropagation();
          commit(event.currentTarget);
        } else if (event.key === 'Escape') {
          if (draft !== value) {
            event.preventDefault();
            event.stopPropagation();
            setDraft(value);
          } else if (onCancel) {
            event.preventDefault();
            event.stopPropagation();
            onCancel();
          }
        }
      }}
      disabled={disabled}
      autoFocus={autoFocus}
      aria-label={label}
      className="h-9 w-40 px-2"
      data-testid={testId}
    />
  );
}

/**
 * The survey date of one shot or of many. One shared date (or none) is a date
 * input. Shots with different dates show their range, read-only, and an
 * explicit "Set for all shots" that opens the input, so one click can never
 * flatten per-shot dates by accident. Either way the date is committed on Enter
 * or when the field is left, never while typing.
 */
export function CreditDateInput({ value, onChange, onReset, disabled = false, testId }: CreditDateInputProps) {
  const { t } = useTranslation();
  const [settingAll, setSettingAll] = useState(false);

  if (value.kind === 'uniform' || settingAll) {
    return (
      <div className="flex min-w-0 items-center gap-2">
        <DraftDateInput
          value={value.kind === 'uniform' ? (value.date ?? '') : ''}
          onCommit={(next) => {
            setSettingAll(false);
            onChange(next);
          }}
          {...(settingAll ? { onCancel: () => setSettingAll(false) } : {})}
          disabled={disabled}
          autoFocus={settingAll}
          testId={testId}
          label={value.kind === 'mixed' ? t('credit.date.pickForAll') : t('credit.dateLabel')}
        />
        {settingAll && (
          <button
            type="button"
            className="text-xs text-text-secondary underline-offset-2 hover:text-text-primary hover:underline"
            // Before the input's blur, which would commit what it holds.
            onMouseDown={(event) => event.preventDefault()}
            onClick={() => setSettingAll(false)}
            data-testid={`${testId}-set-all-cancel`}
          >
            {t('common.actions.cancel')}
          </button>
        )}
        {!settingAll && onReset && (
          <button
            type="button"
            className="text-xs text-text-secondary underline-offset-2 hover:text-text-primary hover:underline"
            onClick={onReset}
            data-testid={`${testId}-reset`}
          >
            {t('credit.date.keepPerShot')}
          </button>
        )}
      </div>
    );
  }

  const range = value.min === value.max ? (value.min ?? '') : `${value.min ?? ''} - ${value.max ?? ''}`;
  return (
    <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1">
      <span className="text-sm text-text-primary" data-testid={`${testId}-mixed`}>
        {value.undated > 0
          ? t('credit.date.mixedUndated', { range, count: value.undated })
          : t('credit.date.mixed', { range })}
      </span>
      {!disabled && (
        <button
          type="button"
          className="text-xs text-accent underline-offset-2 hover:underline"
          onClick={() => setSettingAll(true)}
          data-testid={`${testId}-set-all`}
        >
          {t('credit.date.setForAll')}
        </button>
      )}
    </div>
  );
}
