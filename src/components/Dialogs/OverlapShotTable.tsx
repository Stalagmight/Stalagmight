import { useTranslation } from 'react-i18next';
import { Button } from '../common/Button';
import { Select } from '../common/Select';
import { useProjectStore } from '../../store/projectStore';
import { useDecimalPrecision } from '../../hooks/useDecimalPrecision';
import { defaultConfig } from '../../types/config';
import { formatFixed } from '../../i18n/format';
import { convertLength } from '../../utils/formatStats';
import type { TargetShotLike } from '../../utils/overlapHelpers';
import {
  OVERLAP_CHOICES,
  exceedsRepeatTolerance,
  readingDiff,
  repeatDiffFlags,
  shotAzimuth,
  shotInclination,
  type OverlapChoice,
} from '../../utils/overlapChoices';

/** One repeated shot: the existing reading and the new one, oriented alike. */
export interface OverlapShotRow {
  orphanShotIndex: number;
  existingLabel: string;
  newLabel: string;
  existing: TargetShotLike;
  incoming: TargetShotLike;
}

interface OverlapShotTableProps {
  rows: OverlapShotRow[];
  /** The effective choice per row, keyed by `orphanShotIndex`. */
  choices: ReadonlyMap<number, OverlapChoice>;
  onChoiceChange: (orphanShotIndex: number, choice: OverlapChoice) => void;
  onSetAll: (choice: OverlapChoice) => void;
  /** The existing survey is anchored at both ends: only Keep existing is allowed. */
  replaceDisabled: boolean;
}

// No `whitespace-nowrap`: in a narrow dialog, or with WebKit's minimum font size
// enlarging the text at a small UI scale, headers and the depth range wrap
// rather than push the table past its column (#681).
const cell = 'px-1 py-0.5 text-right';

/** Kebab-case ids for the set-all buttons, as the test tooling expects. */
const SET_ALL_TESTID: Record<OverlapChoice, string> = {
  keep_existing: 'overlap-set-all-keep-existing',
  use_new: 'overlap-set-all-use-new',
  average: 'overlap-set-all-average',
};

/**
 * The per-shot decision table for repeated survey (#681): one row per repeated
 * shot, existing over new with their difference, and the reading to keep.
 *
 * A plain table rather than `SpreadsheetGrid`: it holds at most a handful of
 * read-only comparisons, and the grid's editing model would only get in the way.
 */
export default function OverlapShotTable({
  rows,
  choices,
  onChoiceChange,
  onSetAll,
  replaceDisabled,
}: OverlapShotTableProps) {
  const { t } = useTranslation();
  const units = useProjectStore((s) => s.config?.units) ?? defaultConfig.units;
  const precision = useDecimalPrecision();

  const lengthUnit = units.length === 'feet' ? t('common.units.feet') : t('common.units.meters');
  const depthUnit = units.depth === 'feet' ? t('common.units.feet') : t('common.units.meters');
  const length = (m: number) => formatFixed(convertLength(m, units.length), undefined, precision.length);
  const depth = (m: number) => formatFixed(convertLength(m, units.depth), undefined, precision.depth);
  const azimuth = (deg: number) => formatFixed(deg, undefined, precision.azimuth);
  const inclination = (deg: number | null) =>
    deg == null ? '--' : formatFixed(deg, undefined, precision.inclination);

  const choiceLabel = (choice: OverlapChoice): string => {
    switch (choice) {
      case 'keep_existing': return t('dialogs.orphanPlacement.repeat.choices.keep_existing');
      case 'use_new': return t('dialogs.orphanPlacement.repeat.choices.use_new');
      case 'average': return t('dialogs.orphanPlacement.repeat.choices.average');
    }
  };
  const isDisabled = (choice: OverlapChoice) => replaceDisabled && choice !== 'keep_existing';

  const choiceOf = (row: OverlapShotRow): OverlapChoice => choices.get(row.orphanShotIndex) ?? 'keep_existing';
  const firstChoice = rows[0] ? choiceOf(rows[0]) : null;
  const sharedChoice = rows.every((row) => choiceOf(row) === firstChoice) ? firstChoice : null;

  const reading = (shot: TargetShotLike) => (
    <>
      <td className={cell}>{length(shot.length)}</td>
      <td className={cell}>{azimuth(shotAzimuth(shot))}</td>
      <td className={cell}>{inclination(shotInclination(shot))}</td>
      <td className={cell}>{depth(shot.depth_in)} &rarr; {depth(shot.depth_out)}</td>
    </>
  );
  const flag = (over: boolean) => (over ? 'text-warning-text font-medium' : '');

  return (
    <div className="space-y-2" data-testid="overlap-shot-section">
      <div className="flex items-center justify-between gap-2 flex-wrap">
        <span className="text-xs font-medium text-text-secondary">
          {t('dialogs.orphanPlacement.repeat.heading', { count: rows.length })}
        </span>
        <div
          className="flex items-center gap-1"
          role="group"
          aria-label={t('dialogs.orphanPlacement.repeat.setAll')}
        >
          <span className="text-xs text-text-tertiary">{t('dialogs.orphanPlacement.repeat.setAll')}</span>
          {OVERLAP_CHOICES.map((choice) => (
            <Button
              key={choice}
              size="sm"
              variant="ghost"
              className={`border ${sharedChoice === choice ? 'border-accent bg-accent-20' : 'border-border'}`}
              aria-pressed={sharedChoice === choice}
              disabled={isDisabled(choice)}
              onClick={() => onSetAll(choice)}
              data-testid={SET_ALL_TESTID[choice]}
            >
              {choiceLabel(choice)}
            </Button>
          ))}
        </div>
      </div>

      <p className="text-xs text-text-tertiary">
        {replaceDisabled
          ? t('dialogs.orphanPlacement.repeat.anchored')
          : t('dialogs.orphanPlacement.repeat.hint')}
      </p>

      <table className="w-full text-xs tabular-nums border-collapse" data-testid="overlap-shot-table">
        <thead>
          <tr className="text-text-tertiary border-b border-border">
            <th className="px-1 py-1 text-left font-medium">{t('dialogs.orphanPlacement.repeat.columns.shot')}</th>
            <th className={`${cell} font-medium`}>{t('dialogs.orphanPlacement.repeat.columns.length', { unit: lengthUnit })}</th>
            <th className={`${cell} font-medium`}>{t('dialogs.orphanPlacement.repeat.columns.azimuth')}</th>
            <th className={`${cell} font-medium`}>{t('dialogs.orphanPlacement.repeat.columns.inclination')}</th>
            <th className={`${cell} font-medium`}>{t('dialogs.orphanPlacement.repeat.columns.depth', { unit: depthUnit })}</th>
          </tr>
        </thead>
        {rows.map((row) => {
          const diff = readingDiff(row.existing, row.incoming);
          const differs = exceedsRepeatTolerance(diff);
          const over = repeatDiffFlags(diff);
          return (
            <tbody
              key={row.orphanShotIndex}
              className={`border-b border-border ${differs ? 'bg-warning-10' : ''}`}
              data-testid={`overlap-row-${row.orphanShotIndex}`}
              data-differs={differs ? 'true' : 'false'}
            >
              <tr>
                <td className="px-1 pt-1 text-text-secondary">
                  {t('dialogs.orphanPlacement.repeat.existing', { shot: row.existingLabel })}
                </td>
                {reading(row.existing)}
              </tr>
              <tr>
                <td className="px-1 text-text-secondary">
                  {t('dialogs.orphanPlacement.repeat.new', { shot: row.newLabel })}
                </td>
                {reading(row.incoming)}
              </tr>
              <tr className="text-text-tertiary">
                <td className="px-1">{t('dialogs.orphanPlacement.repeat.diff')}</td>
                <td className={`${cell} ${flag(over.length)}`}>{length(diff.length)}</td>
                <td className={`${cell} ${flag(over.azimuth)}`}>{azimuth(diff.azimuth)}</td>
                <td className={`${cell} ${flag(over.inclination)}`}>
                  {inclination(diff.inclination)}
                </td>
                <td className={`${cell} ${flag(over.depth)}`}>{depth(diff.depth)}</td>
              </tr>
              {/* The choice on its own line, not a last column: as a column it
                  was the first thing pushed out of sight in a narrow dialog. */}
              <tr>
                <td colSpan={5} className="px-1 pt-1 pb-2">
                  <div className="flex items-center gap-2 flex-wrap">
                    <span className="text-text-secondary">{t('dialogs.orphanPlacement.repeat.choiceLabel')}</span>
                    <Select
                      value={choiceOf(row)}
                      onChange={(v) => onChoiceChange(row.orphanShotIndex, v as OverlapChoice)}
                      options={OVERLAP_CHOICES.map((choice) => ({
                        value: choice,
                        label: choiceLabel(choice),
                        disabled: isDisabled(choice),
                      }))}
                      size="sm"
                      aria-label={t('dialogs.orphanPlacement.repeat.choiceAria', { shot: row.existingLabel })}
                      data-testid={`overlap-choice-${row.orphanShotIndex}`}
                    />
                  </div>
                </td>
              </tr>
            </tbody>
          );
        })}
      </table>
    </div>
  );
}
