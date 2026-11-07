/**
 * GridColumn factory for the editable-shot grid behind `ShotTableEditor`.
 *
 * The rows here are `EditableShot` - a shot that has not been committed yet, so
 * it carries `isNew` / `isDeleted` / `tempId` on top of the real fields. That is
 * why this cannot just reuse `dataTableShotColumns`: those read a committed
 * `Shot` and take their warnings from the project's warning set, while these
 * derive warnings locally from the validation thresholds.
 *
 * Frozen policy matches the data table's: azimuth/length/depth are
 * network-adjusted and locked on a frozen shot; LRUD and comment stay editable.
 */
import type { ReactNode } from 'react';
import type { TFunction } from 'i18next';
import type { GridColumn } from './SpreadsheetGrid/types';
import { AlertCircleIcon, AlertTriangleIcon, LockIcon } from './icons';
import { formatFixed } from '../../i18n/format';
import { meanOrLoneLeg } from '../../utils/angles';
import {
  SHOT_KIND_CLASS,
  SHOT_KIND_LABEL_KEY,
  nextShotKind,
  shotKindGlyph,
} from '../../utils/shotKind';
import type { ShotKind } from '../../types/ipc/ShotKind';
import type { EditableShot } from './editableShot';

/** Virtual column key for the displayed average azimuth (not a real field). */
export const EDITABLE_AZIMUTH_AVG_KEY = '_azimuth_avg';
/** Virtual column key for the displayed average inclination. Same string as the
 *  data table's `PITCH_AVG_KEY`, deliberately: `buildShotUpdates` dispatches on
 *  it, and both grids now route their averaged edits through that one function. */
export const EDITABLE_PITCH_AVG_KEY = '_pitch_avg';

/** Virtual column key for the per-row warning indicator. */
export const EDITABLE_WARNINGS_KEY = '_warnings';

/** Columns whose edits touch the adjusted centerline -> locked on a frozen shot.
 *  Matches the data table's set: the azimuth legs and the inclination family are
 *  as network-adjusted as the averages they feed. */
const FROZEN_LOCKED = new Set<string>([
  'length',
  EDITABLE_AZIMUTH_AVG_KEY,
  'azimuth_in',
  'azimuth_out',
  EDITABLE_PITCH_AVG_KEY,
  'pitch_in',
  'pitch_out',
  'depth_in',
  'depth_out',
]);

export interface EditableShotColumnContext {
  t: TFunction;
  precision: { length: number; azimuth: number; depth: number; inclination: number };
  readOnly: boolean;
  /** Blocking errors per VISIBLE row index (length/azimuth validity). */
  errorsAt: (visibleIndex: number) => string[];
  /** Threshold warnings per VISIBLE row index (azimuth/pitch discrepancy). */
  warningsAt: (visibleIndex: number) => string[];
  /** Whether the shot at this VISIBLE row index is frozen. */
  isFrozenAt: (visibleIndex: number) => boolean;
  /** Advance the shot's kind around Normal -> Jump -> Reference; `-1` walks it
   *  backwards (shift-click). */
  onCycleKindAt: (visibleIndex: number, direction: 1 | -1) => void;
  /** The trailing blank row, which is an invitation rather than a shot: it gets
   *  no delete control and no kind glyph until something is typed into it. */
  isPhantomAt: (visibleIndex: number) => boolean;
}

const fixed = (precision: number) => (value: unknown): string =>
  typeof value === 'number' && Number.isFinite(value)
    ? formatFixed(value, undefined, precision)
    : '';

export function buildEditableShotColumns(
  ctx: EditableShotColumnContext
): GridColumn<EditableShot>[] {
  const {
    t,
    precision,
    readOnly,
    errorsAt,
    warningsAt,
    isFrozenAt,
    onCycleKindAt,
    isPhantomAt,
  } = ctx;

  const editableUnlessFrozen =
    (key: string) =>
    (_row: EditableShot, rowIndex: number): boolean => {
      if (readOnly) return false;
      return !(FROZEN_LOCKED.has(key) && isFrozenAt(rowIndex));
    };

  const numberCol = (
    key: 'length' | 'depth_in' | 'depth_out',
    header: string,
    width: number,
    prec: number
  ): GridColumn<EditableShot> => ({
    key,
    header,
    width,
    align: 'right',
    kind: 'number',
    getValue: (r) => r[key],
    format: fixed(prec),
    editable: true,
    isEditable: editableUnlessFrozen(key),
  });

  const azimuthLegCol = (
    key: 'azimuth_in' | 'azimuth_out',
    header: string
  ): GridColumn<EditableShot> => ({
    key,
    header,
    width: 70,
    align: 'right',
    kind: 'number',
    getValue: (r) => r[key],
    format: fixed(precision.azimuth),
    editable: true,
    isEditable: editableUnlessFrozen(key),
    validate: (v) =>
      typeof v === 'number' && (v < 0 || v >= 360)
        ? t('dataTable.validation.azimuthRange')
        : null,
  });

  const pitchLegCol = (
    key: 'pitch_in' | 'pitch_out',
    header: string
  ): GridColumn<EditableShot> => ({
    key,
    header,
    width: 70,
    align: 'right',
    // Nullable: inclination is optional, so a cleared cell must clear the field
    // rather than read as a required value the user failed to supply.
    kind: 'number-nullable',
    getValue: (r) => r[key],
    format: fixed(precision.inclination),
    editable: true,
    isEditable: editableUnlessFrozen(key),
    validate: (v) =>
      typeof v === 'number' && (v < -90 || v > 90)
        ? t('dataTable.validation.pitchRange')
        : null,
  });

  const lrudCol = (
    key: 'left' | 'right' | 'up' | 'down',
    header: string
  ): GridColumn<EditableShot> => ({
    key,
    header,
    width: 50,
    align: 'right',
    kind: 'number-nullable',
    getValue: (r) => r[key],
    format: fixed(precision.length),
    editable: true,
    isEditable: editableUnlessFrozen(key), // not frozen-locked
    validate: (v) =>
      typeof v === 'number' && v < 0 ? t('dataTable.validation.negativeLrud') : null,
  });

  return [
    {
      key: EDITABLE_WARNINGS_KEY,
      header: '',
      width: 30,
      align: 'center',
      getValue: () => '',
      render: (_shot, index): ReactNode => {
        if (isPhantomAt(index)) return null;
        const errors = errorsAt(index);
        const warnings = warningsAt(index);
        const frozen = isFrozenAt(index);
        if (errors.length === 0 && warnings.length === 0 && !frozen) return null;
        const hasErrors = errors.length > 0;
        const quiet = frozen && errors.length === 0 && warnings.length === 0;
        // The data table's indicator, icon for icon and precedence for
        // precedence: a lock on a frozen shot with nothing else to say, a filled
        // circle for an error, a triangle for a warning, with everything in the
        // tooltip. A frozen row's geometry cells are locked, so without this the
        // greying had no explanation.
        return (
          <span
            data-testid={quiet ? `shot-frozen-${index}` : `shot-warning-${index}`}
            className={hasErrors ? 'text-error' : quiet ? 'text-text-secondary' : 'text-warning'}
            title={[...(frozen ? [t('dataTable.frozen')] : []), ...errors, ...warnings].join('\n')}
            aria-label={
              quiet
                ? t('dataTable.frozen')
                : hasErrors
                  ? t('shotTable.warnings.error')
                  : t('shotTable.warnings.warning')
            }
          >
            {quiet ? (
              <LockIcon size={13} />
            ) : hasErrors ? (
              <AlertCircleIcon size={14} />
            ) : (
              <AlertTriangleIcon size={14} />
            )}
          </span>
        );
      },
    },
    numberCol('length', t('shotTable.columns.length'), 100, precision.length),
    {
      key: EDITABLE_AZIMUTH_AVG_KEY,
      header: t('shotTable.columns.azimuth'),
      width: 70,
      align: 'right',
      kind: 'number',
      // One displayed azimuth: the average of the two legs, with a commit
      // fanning back out to both (`buildShotUpdates`, dataTableEdits.ts). A
      // non-finite leg must never be coerced to 0 here - that made an unset
      // azimuth render as a confident "0.0" the user never typed.
      //
      // Blank only when BOTH legs are unset. With exactly one measured leg -
      // which the completeness rules allow and Save accepts - that reading IS
      // the mean, so show it; `circularMeanDegrees(x, NaN)` is NaN, which
      // rendered a real bearing as an empty cell. Same rule the edit path uses.
      getValue: (r) => meanOrLoneLeg(r.azimuth_in, r.azimuth_out),
      format: fixed(precision.azimuth),
      editable: true,
      isEditable: editableUnlessFrozen(EDITABLE_AZIMUTH_AVG_KEY),
      validate: (v) =>
        typeof v === 'number' && (v < 0 || v >= 360)
          ? t('dataTable.validation.azimuthRange')
          : null,
    },
    azimuthLegCol('azimuth_in', t('dataTable.columns.azimuthInShort')),
    azimuthLegCol('azimuth_out', t('dataTable.columns.azimuthOutShort')),
    {
      key: EDITABLE_PITCH_AVG_KEY,
      header: t('dataTable.columns.inclinationShort'),
      width: 70,
      align: 'right',
      // Nullable for the same reason as the legs: clearing the Avg must clear
      // BOTH legs, which `number-nullable` expresses by emitting `null`.
      kind: 'number-nullable',
      // Mean of the present legs; one present leg IS the mean, no legs -> blank.
      getValue: (r) =>
        typeof r.pitch_in === 'number' && typeof r.pitch_out === 'number'
          ? (r.pitch_in + r.pitch_out) / 2
          : r.pitch_in ?? r.pitch_out ?? null,
      format: fixed(precision.inclination),
      editable: true,
      isEditable: editableUnlessFrozen(EDITABLE_PITCH_AVG_KEY),
      // Reject means outside +/-90, and means whose delta would push a leg past
      // +/-90 - pitch does not wrap the way azimuth does.
      validate: (v, r) => {
        if (typeof v !== 'number') return null;
        if (v < -90 || v > 90) return t('dataTable.validation.pitchRange');
        if (typeof r.pitch_in === 'number' && typeof r.pitch_out === 'number') {
          const delta = v - (r.pitch_in + r.pitch_out) / 2;
          const movedIn = r.pitch_in + delta;
          const movedOut = r.pitch_out + delta;
          if (movedIn < -90 || movedIn > 90 || movedOut < -90 || movedOut > 90) {
            return t('dataTable.validation.pitchRange');
          }
        }
        return null;
      },
    },
    pitchLegCol('pitch_in', t('dataTable.columns.pitchInShort')),
    pitchLegCol('pitch_out', t('dataTable.columns.pitchOutShort')),
    numberCol('depth_in', t('shotTable.columns.depthIn'), 80, precision.depth),
    numberCol('depth_out', t('shotTable.columns.depthOut'), 95, precision.depth),
    lrudCol('left', t('shotTable.columns.leftShort')),
    lrudCol('right', t('shotTable.columns.rightShort')),
    lrudCol('up', t('shotTable.columns.upShort')),
    lrudCol('down', t('shotTable.columns.downShort')),
    {
      key: 'kind',
      header: t('dataTable.columns.referenceShort'),
      width: 40,
      align: 'center',
      kind: 'text',
      getValue: (r) => r.kind,
      format: (v) => shotKindGlyph(v as ShotKind),
      editable: true,
      isEditable: () => !readOnly,
      // A three-state button, so no cell editor - same as the other two grids.
      noInlineEdit: true,
      // The keyboard equivalent of that button's click (F2 / Space, Shift to go
      // back), so the cell is not mouse-only.
      activate: (_shot, index, dir) => {
        if (!readOnly && !isPhantomAt(index)) onCycleKindAt(index, dir);
      },
      parse: (raw) => {
        const v = raw.trim().toLowerCase();
        if (v === '' || v === 'n' || v === 'normal' || v === 'permanent') return 'Normal';
        if (v === 'j' || v === 'jump') return 'Jump';
        if (v === 'r' || v === 'ref' || v === 'reference') return 'Reference';
        return undefined;
      },
      render: (shot, index): ReactNode => {
        if (isPhantomAt(index)) return null;
        const glyph = shotKindGlyph(shot.kind);
        return (
          <span
            data-testid="shot-kind-cell"
            data-kind={shot.kind}
            role={readOnly ? undefined : 'button'}
            tabIndex={readOnly ? undefined : -1}
            aria-label={t(SHOT_KIND_LABEL_KEY[shot.kind])}
            className={`group flex h-full w-full items-center justify-center font-semibold ${
              SHOT_KIND_CLASS[shot.kind]
            } ${readOnly ? 'cursor-default' : 'cursor-pointer'}`}
            onClick={(e) => {
              e.stopPropagation();
              if (!readOnly) onCycleKindAt(index, e.shiftKey ? -1 : 1);
            }}
            title={t('dataTable.columns.kindCycleHint', {
              current: t(SHOT_KIND_LABEL_KEY[shot.kind]),
              next: t(SHOT_KIND_LABEL_KEY[nextShotKind(shot.kind, 1)]),
            })}
          >
            {glyph === '' ? <span className="opacity-0 group-hover:opacity-40">J</span> : glyph}
          </span>
        );
      },
    },
    {
      key: 'comment',
      header: t('shotTable.columns.comment'),
      width: 140,
      grow: true,
      kind: 'text',
      multiline: true,
      getValue: (r) => r.comment ?? '',
      editable: !readOnly,
      parse: (raw) => (raw.trim() === '' ? null : raw),
    },
  ];
}
