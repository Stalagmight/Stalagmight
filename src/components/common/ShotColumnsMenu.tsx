/**
 * The eye (show/hide columns) menu, shared by every shot table in the app.
 *
 * There used to be three hand-rolled copies - the data table's, the new-section
 * dialog's and the shared shot editor's - and they had drifted into three
 * different menus: only the data table's rows could go red for a missing value,
 * only two of them had the azimuth avg / in-out / off switch, and only one had
 * an inclination row at all. This is the data table's version, which was the
 * superset, with the parts that genuinely differ per host lifted into props.
 *
 * What is NOT a prop, on purpose: the row set and its grouping. Depth In and
 * Depth Out share a line, LRUD is one checkbox over four columns, and the two
 * angle families each get a segmented control. That layout IS the convention
 * this component exists to enforce, so a host picks which rows apply, never how
 * they look.
 *
 * A host omits a row for a column its table does not have (the new-section
 * dialog has no warnings or id columns - the section does not exist yet), rather
 * than rendering a toggle for nothing.
 */
import { useEffect, useRef, useState, type RefObject } from 'react';
import { useTranslation } from 'react-i18next';
import { EyeIcon } from './icons';
import { useAnchoredFixedMenu } from '../../hooks/useAnchoredFixedMenu';
import { useDismissableLayer } from '../../hooks/useDismissableLayer';

export type AngleDisplayMode = 'avg' | 'inout' | 'off';

/** One angle family's segmented control. Omit the family whose columns the host
 *  table does not define. */
export interface AngleModeControl {
  mode: AngleDisplayMode;
  onPick: (mode: AngleDisplayMode) => void;
}

/** A plain check row, for a host whose columns aren't the shot set (the data
 *  table's stations view). */
export interface ColumnCheckRow {
  keys: string[];
  label: string;
}

export interface ShotColumnsMenuProps {
  /** Currently hidden column keys. A row is checked when NONE of its keys are in
   *  here, so a grouped row (LRUD) unchecks as soon as one member hides. */
  hidden: ReadonlySet<string>;
  /** Toggle every key of a row together. */
  onToggle: (keys: string[]) => void;
  /** Segmented avg / in-out / off controls, per family. */
  angleModes?: Partial<Record<'azimuth' | 'inclination', AngleModeControl>>;
  /** Rows for columns not every shot table has. */
  extraRows?: { warnings?: boolean; sectionId?: boolean; shotId?: boolean; credit?: boolean };
  /** Row identities whose label goes red and which tint the eye button: a
   *  required value is missing in a column that is currently HIDDEN, so there is
   *  no cell on screen to point at. Values are the row identities below
   *  ('length', 'azimuth', 'depthIn', 'depthOut'), not column keys. */
  problems?: ReadonlySet<string>;
  /** Replaces the shot rows entirely (stations view). */
  rowsOverride?: ColumnCheckRow[];
  /** Prefixes every emitted test ID. The data table passes nothing, so its
   *  long-standing bare `col-vis-*` / `col-azimuth-*` IDs are unchanged. */
  testIdPrefix?: string;
  /** Test ID for the trigger button, which predates this component per host. */
  buttonTestId: string;
  /** Test ID for the dropdown, likewise. */
  dropdownTestId: string;
  /** A scroll anywhere inside this element dismisses the menu. See
   *  `useDismissableLayer` - needed by hosts whose trigger sits in a scroller. */
  anchorScope?: RefObject<HTMLElement | null>;
}

export function ShotColumnsMenu({
  hidden,
  onToggle,
  angleModes,
  extraRows,
  problems,
  rowsOverride,
  testIdPrefix = '',
  buttonTestId,
  dropdownTestId,
  anchorScope,
}: ShotColumnsMenuProps) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  // Wraps both the trigger and its portaled dropdown, so an outside-mousedown
  // check treats the whole control as "inside".
  const wrapRef = useRef<HTMLDivElement | null>(null);
  const btnRef = useRef<HTMLButtonElement | null>(null);
  const listRef = useRef<HTMLDivElement | null>(null);

  // `position:fixed` so the dropdown escapes any overflow-hidden clip box when it
  // flips up - the data table sits at the bottom of the window, so it usually does.
  const menuStyle = useAnchoredFixedMenu(open, btnRef, listRef, {
    align: 'right',
    gap: 4,
    width: 224, // px; the hook also applies this as the dropdown width
  });

  useDismissableLayer({
    open,
    onDismiss: () => setOpen(false),
    contains: [wrapRef],
    // Anchored to the eye button, so an unrelated scroll leaves it correct;
    // `anchorScope` is the scroller that DOES move it.
    ...(anchorScope ? { anchorScope, survivesUnrelatedScroll: true } : {}),
  });
  useEffect(() => {
    if (!open) return undefined;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setOpen(false);
    };
    window.addEventListener('keydown', onKey);
    return () => {
      window.removeEventListener('keydown', onKey);
    };
  }, [open]);

  const checkRow = (keys: string[], label: string, className = '', problem = false) => (
    <label
      key={keys[0]}
      className={`flex cursor-pointer items-center gap-2 rounded px-2 py-0.5 hover:bg-accent-10 ${
        problem ? 'text-error' : ''
      } ${className}`}
    >
      <input
        type="checkbox"
        checked={keys.every((k) => !hidden.has(k))}
        onChange={() => onToggle(keys)}
        data-testid={`${testIdPrefix}col-vis-${keys[0]}`}
      />
      {label}
    </label>
  );

  const angleRow = (
    label: string,
    control: AngleModeControl,
    testId: string,
    problem = false
  ) => (
    <div key={testId} className="flex items-center justify-between gap-2 px-2 py-0.5">
      <span className={problem ? 'text-error' : undefined}>{label}</span>
      <span className="flex overflow-hidden rounded border border-border">
        {(['avg', 'inout', 'off'] as const).map((m) => (
          <button
            key={m}
            type="button"
            className={`px-1.5 py-0.5 ${
              control.mode === m
                ? 'bg-accent-20 text-text'
                : 'text-text-secondary hover:bg-accent-10'
            }`}
            onClick={() => control.onPick(m)}
            data-testid={`${testIdPrefix}${testId}-${m}`}
          >
            {t(`dataTable.columnsMenu.${m === 'inout' ? 'inOut' : m}`)}
          </button>
        ))}
      </span>
    </div>
  );

  const has = (p: string) => problems?.has(p) === true;

  return (
    <div ref={wrapRef} className="relative" onMouseDown={(e) => e.stopPropagation()}>
      <button
        ref={btnRef}
        type="button"
        onClick={() => setOpen((v) => !v)}
        className={
          problems && problems.size > 0
            ? 'text-error'
            : open
              ? 'text-text'
              : 'text-text-secondary hover:text-text'
        }
        title={t('dataTable.columnsMenu.button')}
        data-testid={buttonTestId}
      >
        <EyeIcon size={14} />
      </button>
      {open && (
        <div
          ref={listRef}
          className="overflow-auto rounded border border-border bg-panel p-1 text-xs shadow-lg"
          style={menuStyle}
          data-dropdown-open
          data-testid={dropdownTestId}
        >
          {rowsOverride ? (
            rowsOverride.map(({ keys, label }) => checkRow(keys, label))
          ) : (
            <>
              {extraRows?.warnings && checkRow(['_warnings'], t('dataTable.columnsMenu.warnings'))}
              {extraRows?.sectionId && checkRow(['section_id'], t('dataTable.columnsMenu.sectionId'))}
              {extraRows?.shotId && checkRow(['shot_id'], t('dataTable.columnsMenu.shotId'))}
              {checkRow(['length'], t('dataTable.columnsMenu.length'), '', has('length'))}
              {angleModes?.azimuth &&
                angleRow(
                  t('dataTable.columnsMenu.azimuth'),
                  angleModes.azimuth,
                  'col-azimuth',
                  has('azimuth')
                )}
              {angleModes?.inclination &&
                angleRow(
                  t('dataTable.columnsMenu.inclination'),
                  angleModes.inclination,
                  'col-inclination'
                )}
              <div className="flex">
                {checkRow(['depth_in'], t('dataTable.columnsMenu.depthIn'), 'flex-1', has('depthIn'))}
                {checkRow(
                  ['depth_out'],
                  t('dataTable.columnsMenu.depthOut'),
                  'flex-1',
                  has('depthOut')
                )}
              </div>
              {checkRow(['left', 'right', 'up', 'down'], t('dataTable.columnsMenu.lrud'))}
              {checkRow(['kind'], t('dataTable.columnsMenu.reference'))}
              {checkRow(['comment'], t('dataTable.columnsMenu.comment'))}
              {extraRows?.credit && (
                <div className="mt-1 border-t border-border pt-1">
                  {checkRow(['surveyors'], t('credit.surveyors'))}
                  {checkRow(['explorers'], t('credit.explorers'))}
                  {checkRow(['survey_date'], t('credit.dateLabel'))}
                </div>
              )}
            </>
          )}
        </div>
      )}
    </div>
  );
}
