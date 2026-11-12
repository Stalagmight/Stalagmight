/**
 * The Ariane import's section list, with a checkbox per row (#425).
 *
 * Its own component so `TmlImportDialog` stays readable now that it also hosts a
 * map column and a placement step. The selection SHAPE mirrors
 * `WallsImportDialog`'s (a `Set<number>` of file indices in the UI, sent as
 * `number[] | null`), so the two importers' section filters behave the same way.
 */
import { useTranslation } from 'react-i18next';
import type { TmlFile } from '../../types/commands';
import { importableShots } from '../../utils/tmlImportStats';
import { tmlSectionCredit } from '../../utils/tmlSectionCredit';
import type { AggregatedPerson } from '../../utils/creditEdit';
import { PeopleInput } from '../common/PeopleInput';

const NO_SUGGESTIONS: never[] = [];
const noop = () => {};

/**
 * One read-only cell of people: compact pills (partial ones on only some
 * shots), WRAPPED onto as many lines as the cell needs so every name shows
 * whole - a truncated "Cy..." told nobody anything - with full detail on hover.
 */
function PeopleCell({ label, people, testId }: { label: string; people: AggregatedPerson[]; testId: string }) {
  return (
    <div className="min-w-0">
      <PeopleInput
        label={label}
        value={people}
        onAdd={noop}
        onRemove={noop}
        suggestions={NO_SUGGESTIONS}
        disabled
        compact
        wrap
        testId={testId}
      />
    </div>
  );
}

/**
 * A date or a `first - last` range, each date unbroken. A range may wrap after
 * its dash, so the column never forces the table wider than its panel (which
 * clipped it behind a horizontal scrollbar).
 */
function DateText({ text }: { text: string }) {
  const [first, last] = text.split(' - ');
  if (last === undefined) return <span className="whitespace-nowrap">{text}</span>;
  return (
    <>
      <span className="whitespace-nowrap">{first} -</span> <span className="whitespace-nowrap">{last}</span>
    </>
  );
}

export interface TmlSectionTableProps {
  tmlFile: TmlFile;
  includeExcluded: boolean;
  /** Indices (into `tmlFile.sections`) the user has kept. */
  selected: ReadonlySet<number>;
  onToggle: (index: number) => void;
  onToggleAll: () => void;
  /** Indices that will be placed at the origin because the section they hang
   *  off is not selected - flagged inline, where the cause is. */
  reanchored: ReadonlySet<number>;
  disabled: boolean;
}

export function TmlSectionTable({
  tmlFile,
  includeExcluded,
  selected,
  onToggle,
  onToggleAll,
  reanchored,
  disabled,
}: TmlSectionTableProps) {
  const { t } = useTranslation();
  const allSelected =
    tmlFile.sections.length > 0 && tmlFile.sections.every((_, i) => selected.has(i));

  return (
    <div className="space-y-1">
      <div className="flex items-center justify-between text-xs text-text-tertiary">
        <span>{t('dialogs.tmlImport.sectionsToImport')}</span>
        <span data-testid="tml-selected-count">
          {t('dialogs.tmlImport.selectedCount', {
            selected: selected.size,
            total: tmlFile.sections.length,
          })}
        </span>
      </div>
      <div className="max-h-64 overflow-y-auto border border-border rounded">
        <table className="w-full text-sm">
          <thead className="bg-surface-alt sticky top-0">
            <tr className="text-left text-text-tertiary text-xs">
              <th className="px-2 py-1.5 w-8">
                <input
                  type="checkbox"
                  checked={allSelected}
                  disabled={disabled}
                  onChange={onToggleAll}
                  aria-label={t('dialogs.tmlImport.selectAllLabel')}
                  data-testid="tml-select-all"
                />
              </th>
              {/* Width hints, so the people columns get the room (and their pills
                  sit side by side) instead of the auto layout's even spread. */}
              <th className="px-2 py-1.5">{t('dialogs.tmlImport.table.section')}</th>
              <th className="px-2 py-1.5">{t('dialogs.tmlImport.table.shots')}</th>
              <th className="px-2 py-1.5 w-[38%]">{t('credit.surveyors')}</th>
              <th className="px-2 py-1.5 w-[24%]">{t('credit.explorers')}</th>
              <th className="px-2 py-1.5">{t('dialogs.tmlImport.table.date')}</th>
              {/* No colour column: the swatch rides in the Section cell, so the
                  people and date columns have the panel's width to themselves. */}
            </tr>
          </thead>
          <tbody>
            {tmlFile.sections.map((section, i) => {
              const credit = tmlSectionCredit(section, includeExcluded);
              return (
              <tr
                key={i}
                className={`border-t border-border ${selected.has(i) ? '' : 'opacity-50'}`}
                data-testid={`tml-section-row-${i}`}
              >
                <td className="px-2 py-1.5">
                  <input
                    type="checkbox"
                    checked={selected.has(i)}
                    disabled={disabled}
                    onChange={() => onToggle(i)}
                    aria-label={t('dialogs.tmlImport.importSectionLabel', { name: section.name })}
                    data-testid={`tml-section-${i}`}
                  />
                </td>
                <td className="px-2 py-1.5">
                  <span
                    className="mr-1.5 inline-block h-2.5 w-2.5 rounded-sm align-baseline"
                    style={{ backgroundColor: section.color }}
                    title={t('dialogs.tmlImport.table.color')}
                    data-testid={`tml-section-${i}-color`}
                  />
                  {section.name}
                  {section.is_branch_split && (
                    <span className="ml-1 text-xs text-text-tertiary">
                      {t('dialogs.tmlImport.table.branch')}
                    </span>
                  )}
                  {reanchored.has(i) && (
                    <span
                      className="ml-1 text-xs text-warning"
                      data-testid={`tml-section-reanchored-${i}`}
                    >
                      {t('dialogs.tmlImport.table.reanchored')}
                    </span>
                  )}
                </td>
                <td className="px-2 py-1.5">{importableShots(section, includeExcluded).length}</td>
                <td className="px-2 py-1">
                  <PeopleCell
                    label={t('dialogs.tmlImport.table.surveyorsOf', { name: section.name })}
                    people={credit.surveyors}
                    testId={`tml-section-${i}-surveyors`}
                  />
                </td>
                <td className="px-2 py-1">
                  <PeopleCell
                    label={t('dialogs.tmlImport.table.explorersOf', { name: section.name })}
                    people={credit.explorers}
                    testId={`tml-section-${i}-explorers`}
                  />
                </td>
                <td className="px-2 py-1.5 text-text-secondary" data-testid={`tml-section-${i}-date`}>
                  {credit.date ? <DateText text={credit.date} /> : t('dialogs.tmlImport.stats.none')}
                </td>
              </tr>
              );
            })}
          </tbody>
        </table>
      </div>
    </div>
  );
}
