/**
 * GridColumn<Station> factory for the data-table stations view (issue #61).
 *
 * Mirrors the previous EditableTable columns (order, widths, formatting) as
 * SpreadsheetGrid column configs. Stations are derived from shots, so X/Y/Depth
 * are read-only, and so is the name (an imported MNemo station number, for
 * now); only the comment is editable (and never in the read-only
 * closure-adjusted geometry mode). The index pill moved to the row gutter. The
 * GPS (zero-datum) cell is a click target that opens the Edit Station dialog.
 */
import type { ReactNode } from 'react';
import type { TFunction } from 'i18next';
import type { Station } from '../../types/project';
import type { GridColumn } from '../common/SpreadsheetGrid/types';
import { ChevronDownIcon, DotIcon, LinkIcon } from '../common/icons';
import { formatFixed } from '../../i18n/format';
import { xyToGps } from '../../utils/coordinates';
import { displayStationName } from '../../utils/formatters';
import type { GpsCoordinates } from '../../types/project';

export interface StationColumnContext {
  t: TFunction;
  precision: { coordinates: number; depth: number; gps: number };
  geometryReadOnly: boolean;
  /** The survey's GPS anchor (first zero datum), as the map uses it; null when
   *  the project has none - the GPS columns then stay blank. */
  gpsOrigin: { latitude: number; longitude: number } | null;
  /** Click on the GPS (zero-datum) cell - opens the Edit Station dialog in
   *  the mode matching the station's zero-datum state. */
  onGpsCellClick: (station: Station) => void;
  /** Click on a connections cell with connections - opens the merged/closure
   *  station picker at the pointer. */
  onConnectionsCellClick: (station: Station, pos: { x: number; y: number }) => void;
}

const fixed = (precision: number) => (value: unknown): string =>
  typeof value === 'number' && Number.isFinite(value) ? formatFixed(value, undefined, precision) : '';

export function buildStationColumns(ctx: StationColumnContext): GridColumn<Station>[] {
  const { t, precision, geometryReadOnly } = ctx;

  const coordCol = (key: '_x' | '_y' | '_depth', header: string, prec: number): GridColumn<Station> => ({
    key,
    header,
    width: 90,
    align: 'right',
    getValue: (r) => (key === '_x' ? r.coordinates.x : key === '_y' ? r.coordinates.y : r.coordinates.z),
    format: fixed(prec),
  });

  // GPS per station: the backend only stores coordinates on zero-datum
  // stations, so derive the rest from local meters through the same
  // conversion and origin the map uses (utils/coordinates.xyToGps).
  const stationGps = (r: Station): GpsCoordinates | null => {
    if (r.gps_coordinates) return r.gps_coordinates;
    if (!ctx.gpsOrigin) return null;
    return xyToGps(r.coordinates, { ...ctx.gpsOrigin, depth_reference: 0 });
  };
  const gpsCol = (key: '_lat' | '_long', header: string): GridColumn<Station> => ({
    key,
    header,
    width: 100,
    align: 'right',
    getValue: (r) => {
      const gps = stationGps(r);
      return gps === null ? null : key === '_lat' ? gps.latitude : gps.longitude;
    },
    format: fixed(precision.gps),
  });

  return [
    {
      // Next to the gutter's positional S:I pill: both identify the station.
      // Hidden by default (STATION_DEFAULT_HIDDEN / the backend panels default).
      key: 'name',
      header: t('dataTable.stationColumns.name'),
      width: 64,
      kind: 'text',
      getValue: (r) => displayStationName(r.name),
    },
    coordCol('_x', t('dataTable.stationColumns.x'), precision.coordinates),
    coordCol('_y', t('dataTable.stationColumns.y'), precision.coordinates),
    gpsCol('_lat', t('dataTable.stationColumns.lat')),
    gpsCol('_long', t('dataTable.stationColumns.long')),
    coordCol('_depth', t('dataTable.stationColumns.depth'), precision.depth),
    {
      key: '_zeroDatum',
      header: t('dataTable.stationColumns.zeroDatum'),
      width: 56,
      align: 'center',
      getValue: (r) => r.is_zero_datum,
      format: (v) => (v ? 'true' : 'false'),
      // The clickable dot follows the shots view's J/R pattern: the set state
      // renders a dot, the unset one a hover affordance; either opens the
      // Edit Station dialog (add-zero-datum or edit mode). Disabled in the
      // closure-adjusted (read-only) view - the dialog can set/remove the zero
      // datum, which must not mutate geometry while viewing the adjusted network
      // (same gate as the shot reference toggle and the station comment edit).
      render: (station): ReactNode => (
        <span
          className={`flex items-center justify-center ${geometryReadOnly ? 'cursor-default opacity-60' : 'cursor-pointer'}`}
          onClick={(e) => {
            e.stopPropagation();
            if (!geometryReadOnly) ctx.onGpsCellClick(station);
          }}
          title={geometryReadOnly ? undefined : t('dataTable.stationGps.edit')}
          data-testid={`station-gps-${station.station_ref.section_id}-${station.station_ref.station_index}`}
        >
          {station.is_zero_datum ? (
            <DotIcon size={20} />
          ) : geometryReadOnly ? null : (
            <span className="opacity-0 hover:opacity-30">
              <DotIcon size={20} />
            </span>
          )}
        </span>
      ),
    },
    {
      key: '_connections',
      header: t('dataTable.stationColumns.connections'),
      width: 60,
      align: 'center',
      getValue: (r) => (r.merged_with?.length ?? 0) + (r.closures_with?.length ?? 0) || '',
      render: (station): ReactNode => {
        const mergeCount = station.merged_with?.length ?? 0;
        const closureCount = station.closures_with?.length ?? 0;
        const total = mergeCount + closureCount;
        if (total === 0) return null;
        const tooltipParts: string[] = [];
        if (mergeCount > 0) {
          tooltipParts.push(`Merges: ${station.merged_with.map((m) => `${m.section_id}.${m.station_index}`).join(', ')}`);
        }
        if (closureCount > 0) {
          tooltipParts.push(`Closures: ${closureCount}`);
        }
        return (
          <span
            className="inline-flex cursor-pointer items-center justify-center gap-1"
            title={tooltipParts.join('\n')}
            onClick={(e) => {
              e.stopPropagation();
              ctx.onConnectionsCellClick(station, { x: e.clientX, y: e.clientY });
            }}
            data-testid={`station-conn-${station.station_ref.section_id}-${station.station_ref.station_index}`}
          >
            <LinkIcon size={12} />
            {total}
            {/* The arrowhead signals the picker behind the cell. */}
            <ChevronDownIcon size={10} />
          </span>
        );
      },
    },
    {
      key: 'comment',
      header: t('dataTable.stationColumns.comment'),
      width: 240,
      grow: true, // absorb leftover width so the grid fills the panel
      kind: 'text',
      multiline: true,
      getValue: (r) => r.comment ?? '',
      editable: !geometryReadOnly,
      parse: (raw) => (raw.trim() === '' ? null : raw), // empty/whitespace clears to null
    },
  ];
}
