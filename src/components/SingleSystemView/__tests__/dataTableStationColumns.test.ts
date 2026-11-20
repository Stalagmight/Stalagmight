/**
 * The stations view's identity columns (SPEC 4.1): the Name cell is editable
 * (set_station_name) with the same rules the backend applies - trimmed, never
 * empty, unique on every line the station is on (a junction's lines too, plan
 * 5.3) - and the Station ID column is read-only.
 */
import { describe, expect, it } from 'vitest';
import i18n from '../../../i18n';
import { buildStationColumns, stationRenameToSend, type StationColumnContext } from '../dataTableStationColumns';
import { identityLabels } from '../../../utils/stationIdentity';
import type { Station } from '../../../types/project';

function station(index: number, name: string, id: number): Station {
  return {
    station_ref: { line_id: 4, station_index: index },
    coordinates: { x: 0, y: 0, z: 0 },
    is_zero_datum: false,
    gps_coordinates: null,
    merged_with: index === 1 ? [{ line_id: 9, station_index: 0 }] : [],
    id,
    name,
  };
}

function columns(overrides: Partial<StationColumnContext> = {}) {
  return buildStationColumns({
    t: i18n.t.bind(i18n),
    precision: { coordinates: 2, depth: 1, gps: 6 },
    geometryReadOnly: false,
    gpsOrigin: null,
    onGpsCellClick: () => {},
    onConnectionsCellClick: () => {},
    labels: {
      station: (ref, withLine) => (withLine ? `Side / ${ref.station_index}` : String(ref.station_index)),
      stationName: (ref) => String(ref.station_index),
      stationId: () => null,
      lineName: () => 'Main Line',
      // Line 1 names its stations 4, 5, 6 by index.
      stationNameTakenOn: (ref, name) =>
        ['4', '5', '6'].some((n, i) => n === name && i !== ref.station_index) ? ref.line_id : null,
    },
    ...overrides,
  });
}

const col = (key: string, ctx?: Partial<StationColumnContext>) => {
  const found = columns(ctx).find((c) => c.key === key);
  if (!found) throw new Error(`no column ${key}`);
  return found;
};

describe('stations view identity columns', () => {
  it('shows the Station ID read-only', () => {
    const id = col('_stationId');
    expect(id.header).toBe('Station ID');
    expect(id.getValue(station(1, '5', 41), 1)).toBe(41);
    expect(id.editable).toBeFalsy();
  });

  it('makes the name editable, except in a read-only view', () => {
    expect(col('name').editable).toBe(true);
    expect(col('name', { geometryReadOnly: true }).editable).toBe(false);
  });

  it('trims a typed name and skips an empty one', () => {
    const name = col('name');
    expect(name.parse?.(' 5a ', station(1, '5', 41))).toBe('5a');
    expect(name.parse?.('   ', station(1, '5', 41))).toBeUndefined();
  });

  it("refuses a name another station of the line holds, but not the station's own", () => {
    const name = col('name');
    expect(name.validate?.('6', station(1, '5', 41))).toBe(
      'Main Line already has a station named "6". Station names must be unique within their line.',
    );
    expect(name.validate?.('5', station(1, '5', 41))).toBeNull();
    expect(name.validate?.('5a', station(1, '5', 41))).toBeNull();
  });

  it('refuses control and invisible formatting characters', () => {
    const name = col('name');
    const msg = 'A name cannot contain control characters or invisible formatting characters.';
    for (const bad of ['5\u0007', '5\u202Eb', 'a\u200Bb', '\u2066x\u2069', '5\u200F', '\uFEFF5', 'a\u0085b']) {
      expect(name.validate?.(bad, station(1, '5', 41))).toBe(msg);
    }
    expect(name.validate?.('5 b', station(1, '5', 41))).toBeNull();
  });

  it('sends a rename only when the trimmed name differs from the stored one', () => {
    expect(stationRenameToSend('5', station(1, '5', 41))).toBeNull();
    expect(stationRenameToSend(' 5 ', station(1, '5', 41))).toBeNull();
    expect(stationRenameToSend('  ', station(1, '5', 41))).toBeNull();
    expect(stationRenameToSend(null, station(1, '5', 41))).toBeNull();
    expect(stationRenameToSend(' 5a', station(1, '5', 41))).toBe('5a');
  });

  it('names merged stations in the connections tooltip, never line.index', () => {
    const conn = col('_connections');
    const rendered = conn.render?.(station(1, '5', 41), 1) as { props: { title: string } };
    expect(rendered.props.title).toBe('Merges: Side / 0');
  });

  describe('a junction (one station two lines pass, plan 5.3)', () => {
    // Main Line (4) names its stations 4, 5, 6; Side (9) starts at Main's
    // station 5 - the same station, Station ID 41 on both slots - then 7, X.
    const slot = (line_id: number, station_index: number, name: string, id: number, merged: Station['merged_with'] = []): Station => ({
      station_ref: { line_id, station_index },
      coordinates: { x: 0, y: 0, z: 0 },
      is_zero_datum: false,
      gps_coordinates: null,
      merged_with: merged,
      id,
      name,
    });
    const stations: Station[] = [
      slot(4, 0, '4', 40),
      slot(4, 1, '5', 41, [{ line_id: 9, station_index: 0 }]),
      slot(4, 2, '6', 42),
      slot(9, 0, '5', 41, [{ line_id: 4, station_index: 1 }]),
      slot(9, 1, '7', 43),
      slot(9, 2, 'X', 44),
    ];
    const labels = identityLabels({
      stations,
      lines: [
        { id: 4, name: 'Main Line' },
        { id: 9, name: 'Side' },
      ],
    });
    const name = col('name', { labels });

    it("refuses a name the junction's OTHER line holds, naming that line", () => {
      expect(name.validate?.('X', stations[1]!)).toBe(
        'Side already has a station named "X". Station names must be unique within their line.',
      );
      expect(name.validate?.(' 7 ', stations[1]!)).toBe(
        'Side already has a station named "7". Station names must be unique within their line.',
      );
    });

    it('checks every line from either slot of the junction', () => {
      // From Side's row: Main Line's station 6 holds the name.
      expect(name.validate?.('6', stations[3]!)).toBe(
        'Main Line already has a station named "6". Station names must be unique within their line.',
      );
      expect(name.validate?.('4', stations[3]!)).toContain('Main Line already has a station named "4"');
    });

    it("accepts the junction's own name on either slot and a name free on both lines", () => {
      expect(name.validate?.('5', stations[1]!)).toBeNull();
      expect(name.validate?.('5', stations[3]!)).toBeNull();
      expect(name.validate?.('5a', stations[3]!)).toBeNull();
    });

    it('checks a station on one line against that line only', () => {
      // Main's station 6 is on Main only: Side's X is no clash for it.
      expect(name.validate?.('X', stations[2]!)).toBeNull();
      expect(name.validate?.('4', stations[2]!)).toContain('Main Line already has a station named "4"');
    });
  });
});
