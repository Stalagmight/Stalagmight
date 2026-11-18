/**
 * GridColumn<Shot> factory for the data-table shots view (issue #61).
 *
 * Mirrors the previous EditableTable columns (order, widths, formatting) but as
 * SpreadsheetGrid column configs with parse/validate/kind/isEditable. Frozen-shot
 * policy: azimuth/length/depth are network-adjusted and locked on a frozen shot;
 * LRUD + comment stay editable (they don't touch the centerline geometry, and the
 * Rust `update_shot` permits them on frozen shots). The whole view is read-only in
 * the closure-adjusted geometry mode (`geometryReadOnly`).
 */
import type { ReactNode } from 'react';
import type { TFunction } from 'i18next';
import type { Shot } from '../../types/project';
import type { GridColumn } from '../common/SpreadsheetGrid/types';
import { AlertCircleIcon, AlertTriangleIcon, LockIcon } from '../common/icons';
import { meanOrLoneLeg } from '../../utils/angles';
import { formatFixed } from '../../i18n/format';
import type { ProjectWarning } from '../../types/commands';
import { AZIMUTH_AVG_KEY, PITCH_AVG_KEY } from './dataTableEdits';
import type { ShotKind } from '../../types/ipc/ShotKind';
import { type IdentityLabels } from '../../utils/stationIdentity';
import {
  SHOT_KIND_CLASS,
  SHOT_KIND_LABEL_KEY,
  nextShotKind,
  shotKindGlyph,
} from '../../utils/shotKind';

/** Columns whose edits touch the adjusted centerline geometry → locked on a frozen
 *  shot (matches the Rust `ShotUpdates::touches_geometry` set, minus LRUD). */
const FROZEN_LOCKED = new Set<string>([
  'length',
  AZIMUTH_AVG_KEY,
  'azimuth_in',
  'azimuth_out',
  PITCH_AVG_KEY,
  'pitch_in',
  'pitch_out',
  'depth_in',
  'depth_out',
]);

export interface ShotColumnContext {
  t: TFunction;
  precision: { length: number; azimuth: number; depth: number; inclination: number };
  lineId: number | null;
  geometryReadOnly: boolean;
  isShotFrozen: (lineId: number, shotIndex: number) => boolean;
  getShotWarnings: (lineId: number, shotId: number) => ProjectWarning[];
  /** Map a grid row index to its index in the line's shots array - `null`
   *  for draft rows (grid rows interleave drafts with the real shots). */
  shotIndexAt: (gridRowIndex: number) => number | null;
  /** Advance a shot's kind around the Normal -> Jump -> Reference ring (the
   *  inline J/R cell). `direction` of -1 walks it backwards (shift-click);
   *  `rowIndex` is the grid row, for a host whose edit funnel addresses rows by
   *  position rather than by id. */
  onCycleShotKind: (shot: Shot, direction: 1 | -1, rowIndex: number) => void;
  /** Open the Warnings tab and start resolving the given shot's issue (the
   *  warning indicator becomes a button when the shot has unresolved warnings). */
  onResolveWarning?: (shotId: number) => void;
  /** Names for the Line and Shot columns and the warning tooltips. Without
   *  them those columns stay blank and a warning shows its raw text. */
  labels?: Pick<IdentityLabels, 'lineName' | 'shotAt'>;
}

const fixed = (precision: number) => (value: unknown): string =>
  typeof value === 'number' && Number.isFinite(value) ? formatFixed(value, undefined, precision) : '';

export function buildShotColumns(ctx: ShotColumnContext): GridColumn<Shot>[] {
  const { t, precision, lineId, geometryReadOnly, isShotFrozen, getShotWarnings, shotIndexAt, onResolveWarning, labels } = ctx;

  // Editable iff the view is writable AND (non-geometry, or a non-frozen shot).
  // Draft rows (no shot index) are always editable.
  const editableUnlessFrozenGeometry =
    (key: string) =>
    (_row: Shot, rowIndex: number): boolean => {
      if (geometryReadOnly) return false;
      if (FROZEN_LOCKED.has(key) && lineId !== null) {
        const shotIndex = shotIndexAt(rowIndex);
        return shotIndex === null || !isShotFrozen(lineId, shotIndex);
      }
      return true;
    };

  const numberCol = (
    key: 'length' | 'depth_in' | 'depth_out',
    header: string,
    width: number,
    prec: number,
    validate?: (v: unknown) => string | null,
  ): GridColumn<Shot> => ({
    key,
    header,
    width,
    align: 'right',
    kind: 'number',
    getValue: (r) => r[key],
    format: fixed(prec),
    editable: true,
    isEditable: editableUnlessFrozenGeometry(key),
    ...(validate ? { validate } : {}),
  });

  const lrudCol = (key: 'left' | 'right' | 'up' | 'down', header: string): GridColumn<Shot> => ({
    key,
    header,
    width: 50,
    align: 'right',
    kind: 'number-nullable',
    getValue: (r) => r[key],
    format: fixed(precision.length),
    editable: true,
    isEditable: editableUnlessFrozenGeometry(key), // not in FROZEN_LOCKED → editable on frozen
    validate: (v) => (typeof v === 'number' && v < 0 ? t('dataTable.validation.negativeLrud') : null),
  });

  const azimuthLegCol = (key: 'azimuth_in' | 'azimuth_out', header: string): GridColumn<Shot> => ({
    key,
    header,
    width: 70,
    align: 'right',
    kind: 'number',
    getValue: (r) => r[key],
    format: fixed(precision.azimuth),
    editable: true,
    isEditable: editableUnlessFrozenGeometry(key),
    validate: (v) => (typeof v === 'number' && (v < 0 || v >= 360) ? t('dataTable.validation.azimuthRange') : null),
  });

  const pitchLegCol = (key: 'pitch_in' | 'pitch_out', header: string): GridColumn<Shot> => ({
    key,
    header,
    width: 70,
    align: 'right',
    kind: 'number-nullable',
    getValue: (r) => r[key],
    format: fixed(precision.inclination),
    editable: true,
    isEditable: editableUnlessFrozenGeometry(key),
    validate: (v) => (typeof v === 'number' && (v < -90 || v > 90) ? t('dataTable.validation.pitchRange') : null),
  });

  return [
    {
      key: '_warnings',
      header: t('dataTable.columns.warningsShort'),
      width: 30,
      align: 'center',
      getValue: () => '',
      render: (shot, index): ReactNode => {
        if (lineId === null) return null;
        const shotIndex = shotIndexAt(index);
        if (shotIndex === null) return null; // draft rows carry no warnings/frozen state
        const frozen = isShotFrozen(lineId, shotIndex);
        const shotWarnings = getShotWarnings(lineId, shot.id).filter((w) => !w.isResolved);
        if (shotWarnings.length === 0 && !frozen) return null;
        const hasError = shotWarnings.some((w) => w.severity === 'error');
        const tooltip = [
          ...(frozen ? [t('dataTable.frozen')] : []),
          ...shotWarnings.map((w) => w.message),
        ].join('\n');
        const testId = frozen ? `frozen-${lineId}-${shot.id}` : `warning-${lineId}-${shot.id}`;
        const colorClass = hasError
          ? 'text-error'
          : frozen && shotWarnings.length === 0
            ? 'text-text-secondary'
            : 'text-warning';
        const icon =
          frozen && shotWarnings.length === 0 ? (
            <LockIcon size={13} />
          ) : hasError ? (
            <AlertCircleIcon size={14} />
          ) : (
            <AlertTriangleIcon size={14} />
          );
        // With unresolved warnings, the indicator is a button that opens the
        // Warnings tab and jumps straight to resolving this shot's issue.
        if (shotWarnings.length > 0 && onResolveWarning) {
          return (
            <button
              type="button"
              data-testid={testId}
              className={`${colorClass} cursor-pointer rounded hover:opacity-80 focus:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-inset`}
              title={tooltip}
              aria-label={t('detailPanel.warnings.resolve')}
              onMouseDown={(e) => e.stopPropagation()}
              onClick={(e) => {
                e.stopPropagation();
                onResolveWarning(shot.id);
              }}
            >
              {icon}
            </button>
          );
        }
        return (
          <span data-testid={testId} className={colorClass} title={tooltip}>
            {icon}
          </span>
        );
      },
    },
    // The shot's identity as text (read-only, hidden by default) - for
    // cross-referencing and copying out alongside the data: its line's name
    // and `from → to` (SPEC 4.2). The keys keep their persisted names
    // (`line_id` / `shot_id`); no numeric id is shown. Drafts have no shot yet.
    {
      key: 'line_id',
      header: t('dataTable.columns.sectionIdShort'),
      width: 90,
      getValue: () => (lineId !== null && labels ? labels.lineName(lineId) : ''),
    },
    {
      key: 'shot_id',
      header: t('dataTable.columns.shotIdShort'),
      width: 80,
      getValue: (_r, rowIndex) => {
        const shotIndex = shotIndexAt(rowIndex);
        return shotIndex === null || lineId === null || !labels ? '' : labels.shotAt(lineId, shotIndex);
      },
    },
    numberCol('length', t('dataTable.columns.length', { unit: t('common.units.meters') }), 100, precision.length, (v) =>
      typeof v === 'number' && v <= 0 ? t('dataTable.validation.nonPositiveLength') : null,
    ),
    {
      key: AZIMUTH_AVG_KEY,
      header: t('dataTable.columns.azimuthShort'),
      width: 70,
      align: 'right',
      kind: 'number',
      // Mean of the measured legs; a single measured leg IS the mean, and a
      // draft row accepts one leg - averaging it against the unset (NaN) leg
      // rendered a real, saveable bearing as an empty cell.
      getValue: (r) => meanOrLoneLeg(r.azimuth_in, r.azimuth_out),
      format: fixed(precision.azimuth),
      editable: true,
      isEditable: editableUnlessFrozenGeometry(AZIMUTH_AVG_KEY),
      validate: (v) => (typeof v === 'number' && (v < 0 || v >= 360) ? t('dataTable.validation.azimuthRange') : null),
    },
    azimuthLegCol('azimuth_in', t('dataTable.columns.azimuthInShort')),
    azimuthLegCol('azimuth_out', t('dataTable.columns.azimuthOutShort')),
    {
      key: PITCH_AVG_KEY,
      header: t('dataTable.columns.inclinationShort'),
      width: 70,
      align: 'right',
      // Nullable: inclination is optional, so clearing the Avg cell must clear
      // BOTH legs (number-nullable emits `null`, which buildShotUpdates routes to
      // a clear) rather than being swallowed as a required-but-missing edit.
      kind: 'number-nullable',
      // Mean of the present legs; a single present leg IS the mean, no legs → blank.
      getValue: (r) =>
        typeof r.pitch_in === 'number' && typeof r.pitch_out === 'number'
          ? (r.pitch_in + r.pitch_out) / 2
          : r.pitch_in ?? r.pitch_out ?? null,
      format: fixed(precision.inclination),
      editable: true,
      isEditable: editableUnlessFrozenGeometry(PITCH_AVG_KEY),
      // Reject means outside ±90, and means whose delta would push a leg past ±90
      // (pitch doesn't wrap like azimuth).
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
    numberCol('depth_in', t('dataTable.columns.depthIn'), 80, precision.depth),
    numberCol('depth_out', t('dataTable.columns.depthOut'), 95, precision.depth),
    lrudCol('left', t('dataTable.columns.leftShort')),
    lrudCol('right', t('dataTable.columns.rightShort')),
    lrudCol('up', t('dataTable.columns.upShort')),
    lrudCol('down', t('dataTable.columns.downShort')),
    {
      key: 'kind',
      header: t('dataTable.columns.referenceShort'),
      width: 40,
      align: 'center',
      kind: 'text',
      getValue: (r) => r.kind,
      // Blank for a permanent shot so the column stays visually quiet on a
      // normal survey and the virtual shots stand out (#27).
      format: (v) => shotKindGlyph(v as ShotKind),
      // Editable as text so fill/paste/clear work like any other column: drag a
      // cell's fill handle to stamp one kind down a whole run, which is the
      // bulk-reclassify path now that classification never re-runs.
      editable: true,
      isEditable: editableUnlessFrozenGeometry('kind'), // not locked on frozen
      // ...but no cell EDITOR. The cell is a three-state button, so a click is
      // the whole interaction; a text box opening over one glyph on double-click
      // was a second, worse way to do the same thing. The bulk paths above are
      // unaffected - this only closes the inline editor.
      noInlineEdit: true,
      // The keyboard equivalent of that click (F2 / Space, Shift to walk the
      // ring backwards), so the cell is reachable without a mouse.
      activate: (shot, index, dir) => {
        if (!geometryReadOnly) ctx.onCycleShotKind(shot, dir, index);
      },
      parse: (raw) => {
        const v = raw.trim().toLowerCase();
        // Accept the rendered glyphs, the full words, and the legacy booleans
        // so a sheet copied out of an older build still pastes back in.
        if (v === '' || v === 'n' || v === 'normal' || v === 'permanent') return 'Normal';
        if (v === 'j' || v === 'jump') return 'Jump';
        if (v === 'r' || v === 'ref' || v === 'reference') return 'Reference';
        if (v === 'false' || v === '0' || v === 'no') return 'Normal';
        if (v === 'true' || v === '1' || v === 'yes') return 'Jump';
        return undefined; // anything else is rejected
      },
      render: (shot, rowIndex): ReactNode => {
        const glyph = shotKindGlyph(shot.kind);
        return (
          <span
            data-testid="shot-kind-cell"
            data-kind={shot.kind}
            role={geometryReadOnly ? undefined : 'button'}
            tabIndex={geometryReadOnly ? undefined : -1}
            aria-label={t(SHOT_KIND_LABEL_KEY[shot.kind])}
            // Fills the cell so ANY click in it cycles, not just one that lands
            // on the glyph - which on a Normal shot is an invisible target.
            className={`group flex h-full w-full items-center justify-center font-semibold ${
              SHOT_KIND_CLASS[shot.kind]
            } ${geometryReadOnly ? 'cursor-default opacity-60' : 'cursor-pointer'}`}
            onClick={(e) => {
              e.stopPropagation();
              // Shift-click walks the ring backwards, so overshooting a
              // three-state cycle costs one click instead of two.
              if (!geometryReadOnly) ctx.onCycleShotKind(shot, e.shiftKey ? -1 : 1, rowIndex);
            }}
            title={t('dataTable.columns.kindCycleHint', {
              current: t(SHOT_KIND_LABEL_KEY[shot.kind]),
              next: t(SHOT_KIND_LABEL_KEY[nextShotKind(shot.kind, 1)]),
            })}
          >
            {glyph === '' ? (
              // Keep the hover affordance the old dot had: an empty cell still
              // has to advertise that it is clickable. group-hover, not hover,
              // so it appears from anywhere in the cell rather than only over
              // the hidden glyph itself.
              <span className="opacity-0 group-hover:opacity-40">J</span>
            ) : (
              glyph
            )}
          </span>
        );
      },
    },
    {
      key: 'comment',
      header: t('dataTable.columns.comment'),
      width: 240,
      grow: true, // absorb leftover width so the grid fills the panel
      kind: 'text',
      multiline: true,
      getValue: (r) => r.comment ?? '',
      editable: true,
      isEditable: editableUnlessFrozenGeometry('comment'), // not locked on frozen
      parse: (raw) => (raw.trim() === '' ? null : raw), // empty/whitespace clears to null
    },
  ];
}
