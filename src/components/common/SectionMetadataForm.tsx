/**
 * Shared form component for editing section metadata (name, surveyors,
 * explorers, date, color, direction). Used by EditSectionDialog and the import
 * flows (zero-datum config, the mNemo wizard's sections).
 *
 * The people fields are pill fields ({@link CreditPeopleFields}); the host
 * decides what an edit means. An import dialog holds plain lists (see
 * {@link listPeopleProps}); Edit Section holds a pending edit of every shot and
 * passes each person with their shot count, plus `dateAggregate` for a date
 * that may differ per shot.
 */
import { DatePicker } from './DatePicker';
import { HelpTooltip } from './HelpTooltip';
import { Select } from './Select';
import { CreditDateInput, CreditPeopleFields, type CreditPeopleFieldsProps } from './CreditFields';
import type { PeopleInputValue } from './PeopleInput';
import { useTranslation } from 'react-i18next';
import type { Person } from '../../types/ipc.generated';
import type { CreditDateAggregate, CreditRole } from '../../utils/creditEdit';
import { dedupePeople, personKey, type PersonLike } from '../../utils/people';
import { toPerson } from '../../store/recentPeople';

/** The people half of the form: values plus what an add / remove / promote does. */
export type SectionPeopleProps = Pick<
  CreditPeopleFieldsProps,
  'surveyors' | 'explorers' | 'onAdd' | 'onRemove' | 'onPromote' | 'onCommitPending'
>;

/** Plain surveyor / explorer lists, as an import dialog keeps them. */
export interface CreditLists {
  surveyors: Person[];
  explorers: Person[];
}

export const EMPTY_CREDIT_LISTS: CreditLists = Object.freeze({ surveyors: [], explorers: [] }) as unknown as CreditLists;

/** People as pills with no shot counts. */
export function peopleValues(people: readonly Person[]): PeopleInputValue[] {
  return people.map((person) => ({ person }));
}

/** `list` with `person` appended (once, by identity). */
export function withPerson(list: readonly Person[], person: PersonLike): Person[] {
  return dedupePeople([...list, toPerson(person)]);
}

/** `list` without `person` (by identity). */
export function withoutPerson(list: readonly Person[], person: PersonLike): Person[] {
  const key = personKey(person);
  return list.filter((p) => personKey(p) !== key);
}

/**
 * The people props of a form backed by plain lists. `onChange` gets the next
 * list of one role; a pasted list calls it once per name in one event, so the
 * host should update functionally (`setLists((prev) => ...)`) - `update`
 * receives the role and a reducer for exactly that.
 */
export function listPeopleProps(
  lists: CreditLists,
  update: (role: CreditRole, next: (prev: Person[]) => Person[]) => void,
  onCommitPending?: (hasPending: boolean) => void,
): SectionPeopleProps {
  return {
    surveyors: peopleValues(lists.surveyors),
    explorers: peopleValues(lists.explorers),
    onAdd: (role, person) => update(role, (prev) => withPerson(prev, person)),
    onRemove: (role, person) => update(role, (prev) => withoutPerson(prev, person)),
    ...(onCommitPending ? { onCommitPending } : {}),
  };
}

export interface SectionMetadataFormProps {
  name: string;
  people: SectionPeopleProps;
  /** Lay the two people fields side by side (a wide dialog). */
  peopleColumns?: boolean;
  /** The date as one value (an import dialog). Ignored when `dateAggregate` is set. */
  date: string;
  /** The date across the section's shots (Edit Section): one value, or a per-shot range. */
  dateAggregate?: CreditDateAggregate;
  /** With `dateAggregate`: back to per-shot dates after a date was set over a range. */
  onDateReset?: () => void;
  color: string;
  direction?: 'in' | 'out';
  quality?: number;
  /** Import-time reverse toggle (#24). Only meaningful when `showReverse`. */
  reverse?: boolean;
  onNameChange: (value: string) => void;
  /** The new date ('' when cleared). */
  onDateChange: (value: string) => void;
  onColorChange: (value: string) => void;
  onQualityChange?: (value: number) => void;
  onDirectionChange?: (value: 'in' | 'out') => void;
  onReverseChange?: (value: boolean) => void;
  showName?: boolean;
  showDirection?: boolean;
  showQuality?: boolean;
  /**
   * Show the per-section reverse checkbox (#24). Import wizards only - an
   * existing section is reversed through the Reverse Section action instead,
   * which can ask about left/right and is undoable on its own.
   */
  showReverse?: boolean;
  disabled?: boolean;
  /**
   * The credit has nowhere to go (Edit Section on a section with no shots:
   * credit lives on shots). Disables the people and date fields and says
   * why, instead of taking input Save would drop.
   */
  creditUnavailable?: boolean;
}

export function SectionMetadataForm({
  name,
  people,
  peopleColumns = false,
  date,
  dateAggregate,
  onDateReset,
  color,
  direction = 'in',
  quality = 1.0,
  reverse = false,
  onNameChange,
  onDateChange,
  onColorChange,
  onQualityChange,
  onDirectionChange,
  onReverseChange,
  showName = true,
  showDirection = false,
  showQuality = false,
  showReverse = false,
  disabled = false,
  creditUnavailable = false,
}: SectionMetadataFormProps) {
  const { t } = useTranslation();
  const creditDisabled = disabled || creditUnavailable;
  return (
    <div className="space-y-3" data-testid="section-metadata-form">
      {showName && (
        <div>
          <label className="block text-sm text-text-secondary mb-1">{t('common.labels.name')}</label>
          <input
            type="text"
            value={name}
            onChange={(e) => onNameChange(e.target.value)}
            className="w-full px-3 py-2 bg-background border border-border rounded"
            placeholder={t('sectionMetadata.placeholders.name')}
            disabled={disabled}
            data-testid="section-name-input"
          />
        </div>
      )}

      {creditUnavailable && (
        <p className="text-sm text-text-tertiary" data-testid="section-credit-no-shots">
          {t('credit.noShotsHint')}
        </p>
      )}
      <CreditPeopleFields
        {...people}
        layout={peopleColumns ? 'columns' : 'stacked'}
        disabled={creditDisabled}
        testIdPrefix="section"
      />

      {showQuality && (
        <div>
          <label className="block text-sm text-text-secondary mb-1">
            <span className="inline-flex items-center gap-1">
              {t('sectionMetadata.labels.quality')}
              <HelpTooltip
                text={t('sectionMetadata.help.quality')}
                iconTestId="section-quality-help-icon"
                tooltipTestId="section-quality-help-tooltip"
              />
            </span>
          </label>
          <input
            type="number" inputMode="decimal"
            min={0.1}
            step={0.1}
            value={quality}
            onChange={(e) => onQualityChange?.(parseFloat(e.target.value) || 1.0)}
            className="w-32 px-3 py-2 bg-background border border-border rounded"
            disabled={disabled}
            data-testid="section-quality-input"
          />
        </div>
      )}

      {showReverse && (
        <div>
          <label className="flex items-start gap-2 text-sm cursor-pointer">
            <input
              type="checkbox"
              checked={reverse}
              onChange={(e) => onReverseChange?.(e.target.checked)}
              className="mt-0.5"
              disabled={disabled}
              data-testid="section-reverse-checkbox"
            />
            <span>
              <span className="text-text-primary">
                {t('sectionMetadata.labels.reverse')}
              </span>
              <span className="block text-text-tertiary">
                {t('sectionMetadata.help.reverse')}
              </span>
            </span>
          </label>
        </div>
      )}

      {/* Date, Direction, and Color row */}
      <div className="flex gap-3 items-end justify-between">
        {/* Date and Direction - left aligned */}
        <div className="flex gap-3 items-end">
          <div className={dateAggregate ? 'min-w-32' : 'w-32'}>
            <label className="block text-sm text-text-secondary mb-1">{t('credit.dateLabel')}</label>
            {dateAggregate ? (
              <CreditDateInput
                value={dateAggregate}
                onChange={(next) => onDateChange(next ?? '')}
                {...(onDateReset ? { onReset: onDateReset } : {})}
                disabled={creditDisabled}
                testId="section-date-input"
              />
            ) : (
              <DatePicker
                value={date}
                onChange={onDateChange}
                disabled={creditDisabled}
                data-testid="section-date-input"
              />
            )}
          </div>

          {showDirection && (
            <div className="w-16">
              <label className="block text-sm text-text-secondary mb-1">{t('sectionMetadata.labels.directionShort')}</label>
              <Select
                value={direction}
                onChange={(v) => onDirectionChange?.(v as 'in' | 'out')}
                options={[
                  { value: 'in', label: t('sectionMetadata.direction.in') },
                  { value: 'out', label: t('sectionMetadata.direction.out') },
                ]}
                disabled={disabled}
                data-testid="section-direction-select"
              />
            </div>
          )}
        </div>

        {/* Color - right aligned */}
        <div className="flex-shrink-0">
          <label className="block text-sm text-text-secondary mb-1">{t('sectionMetadata.labels.color')}</label>
          <div className="flex items-center gap-2">
            <input
              type="color"
              value={color}
              onChange={(e) => onColorChange(e.target.value)}
              className="w-8 h-8 rounded cursor-pointer border border-border"
              disabled={disabled}
              data-testid="section-color-picker"
            />
            <input
              type="text"
              value={color}
              onChange={(e) => onColorChange(e.target.value)}
              className="w-20 px-2 py-1.5 bg-background border border-border rounded text-sm"
              placeholder={t('sectionMetadata.placeholders.color')}
              disabled={disabled}
              data-testid="section-color-hex-input"
            />
          </div>
        </div>
      </div>
    </div>
  );
}
