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
import type { ReactNode, MouseEvent as ReactMouseEvent } from 'react';
import type { TFunction } from 'i18next';
import type { Shot, Marker } from '../../types/project';
import type { GridColumn } from '../common/SpreadsheetGrid/types';
import { AlertCircleIcon, AlertTriangleIcon, DotIcon, LockIcon } from '../common/icons';
import { circularMeanDegrees } from '../../utils/angles';
import { formatFixed } from '../../i18n/format';
import type { ProjectWarning } from '../../types/commands';
import { AZIMUTH_AVG_KEY, PITCH_AVG_KEY } from './dataTableEdits';

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
  sectionId: number | null;
  geometryReadOnly: boolean;
  isShotFrozen: (sectionId: number, shotIndex: number) => boolean;
  getShotWarnings: (sectionId: number, shotId: number) => ProjectWarning[];
  /** Map a grid row index to its index in the section's shots array — `null`
   *  for draft rows (grid rows interleave drafts with the real shots). */
  shotIndexAt: (gridRowIndex: number) => number | null;
  /** Toggle a shot's reference flag (the inline dot control). */
  onToggleReference: (shot: Shot) => void;
  /** Open the Warnings tab and start resolving the given shot's issue (the
   *  warning indicator becomes a button when the shot has unresolved warnings). */
  onResolveWarning?: (shotId: number) => void;
  /** Markers for all shots in the project (used to show the marker indicator column). */
  markers?: Marker[];
  /** Called when the user clicks the marker indicator cell; triggers auto-open of the marker form. */
  onMarkerColumnClick?: (shot: Shot) => void;
}

const fixed = (precision: number) => (value: unknown): string =>
  typeof value === 'number' && Number.isFinite(value) ? formatFixed(value, undefined, precision) : '';

export function buildShotColumns(ctx: ShotColumnContext): GridColumn<Shot>[] {
  const { t, precision, sectionId, geometryReadOnly, isShotFrozen, getShotWarnings, shotIndexAt, onResolveWarning } = ctx;

  // Editable iff the view is writable AND (non-geometry, or a non-frozen shot).
  // Draft rows (no shot index) are always editable.
  const editableUnlessFrozenGeometry =
    (key: string) =>
    (_row: Shot, rowIndex: number): boolean => {
      if (geometryReadOnly) return false;
      if (FROZEN_LOCKED.has(key) && sectionId !== null) {
        const shotIndex = shotIndexAt(rowIndex);
        return shotIndex === null || !isShotFrozen(sectionId, shotIndex);
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
        if (sectionId === null) return null;
        const shotIndex = shotIndexAt(index);
        if (shotIndex === null) return null; // draft rows carry no warnings/frozen state
        const frozen = isShotFrozen(sectionId, shotIndex);
        const shotWarnings = getShotWarnings(sectionId, shot.id).filter((w) => !w.isResolved);
        if (shotWarnings.length === 0 && !frozen) return null;
        const hasError = shotWarnings.some((w) => w.severity === 'error');
        const tooltip = [
          ...(frozen ? [t('dataTable.frozen')] : []),
          ...shotWarnings.map((w) => w.message),
        ].join('\n');
        const testId = frozen ? `frozen-${sectionId}-${shot.id}` : `warning-${sectionId}-${shot.id}`;
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
    // Raw identifiers (read-only, hidden by default) — for cross-referencing and
    // copying out alongside the data.
    {
      key: 'section_id',
      header: t('dataTable.columns.sectionIdShort'),
      width: 60,
      align: 'right',
      getValue: () => sectionId ?? '',
    },
    {
      key: 'shot_id',
      header: t('dataTable.columns.shotIdShort'),
      width: 60,
      align: 'right',
      // The shot sub-portion of the displayed id — the 1-based position after
      // the ':' in the pill, not the internal row id. Drafts have none yet.
      getValue: (_r, rowIndex) => {
        const shotIndex = shotIndexAt(rowIndex);
        return shotIndex === null ? '' : shotIndex + 1;
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
      getValue: (r) => circularMeanDegrees(r.azimuth_in, r.azimuth_out),
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
      key: 'is_reference',
      header: t('dataTable.columns.referenceShort'),
      width: 40,
      align: 'center',
      kind: 'text',
      getValue: (r) => r.is_reference,
      format: (v) => (v ? 'true' : 'false'),
      // Editable as a boolean so fill/paste/clear work like any other column
      // (drag a J/R cell's fill handle to stamp the flag down a range); the
      // rendered dot stays the pointer affordance for single toggles.
      editable: true,
      isEditable: editableUnlessFrozenGeometry('is_reference'), // not locked on frozen
      parse: (raw) => {
        const v = raw.trim().toLowerCase();
        if (v === '' || v === 'false' || v === '0' || v === 'no') return false;
        if (v === 'true' || v === '1' || v === 'yes') return true;
        return undefined; // anything else is rejected
      },
      render: (shot): ReactNode => (
        <span
          className={`flex items-center justify-center ${geometryReadOnly ? 'cursor-default opacity-60' : 'cursor-pointer'}`}
          onClick={(e) => {
            e.stopPropagation();
            if (!geometryReadOnly) ctx.onToggleReference(shot);
          }}
          title={shot.is_reference ? t('detailPanel.shot.markPermanent') : t('detailPanel.shot.markReference')}
        >
          {shot.is_reference ? (
            <DotIcon size={20} />
          ) : (
            <span className="opacity-0 hover:opacity-30">
              <DotIcon size={20} />
            </span>
          )}
        </span>
      ),
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
    {
      key: 'marker',
      header: '●', // filled circle glyph
      width: 32,
      align: 'center',
      getValue: () => '',
      render: (row) => {
        const marker = ctx.markers?.find(
          (m) => m.section_id === ctx.sectionId && m.shot_id === row.id
        );
        const handleClick = (e: ReactMouseEvent) => {
          e.stopPropagation();
          if (!geometryReadOnly) ctx.onMarkerColumnClick?.(row);
        };
        if (!marker) {
          return (
            <span
              role={geometryReadOnly ? undefined : 'button'}
              tabIndex={geometryReadOnly ? undefined : -1}
              onClick={handleClick}
              style={{
                fontSize: 10,
                color: 'var(--cc-text-secondary, #a0a0a0)',
                opacity: 0.4,
                cursor: geometryReadOnly ? 'default' : 'pointer',
              }}
              title={geometryReadOnly ? undefined : 'Add marker'}
            >
              &#9711;
            </span>
          );
        }
        return (
          <span
            role={geometryReadOnly ? undefined : 'button'}
            tabIndex={geometryReadOnly ? undefined : -1}
            onClick={handleClick}
            style={{
              display: 'inline-block',
              width: 10,
              height: 10,
              borderRadius: '50%',
              background: marker.color,
              border: '1.5px solid rgba(255,255,255,.2)',
              verticalAlign: 'middle',
              cursor: geometryReadOnly ? 'default' : 'pointer',
              opacity: geometryReadOnly ? 0.6 : 1,
            }}
            title={`${marker.marker_type === 'Arrow' ? 'Arrow' : 'NDM'}${marker.text ? ' — ' + marker.text : ''} @ ${Math.round(marker.position)}%`}
          />
        );
      },
    },
  ];
}
