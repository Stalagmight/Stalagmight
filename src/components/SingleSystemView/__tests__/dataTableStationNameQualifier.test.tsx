/**
 * RS-FX6, P2-5 (shots 04, 05, 11 of the RS capture): a line may show one
 * name twice (RS-FX5), the junction showing another line's station with its
 * home line's dot, and the line's own station. The Name column read "12"
 * twice; only the gutter pill's 5 px dot told them apart. A dotted row now
 * reads "12 (Main)", the qualifier muted, and a loop end "8 (loop end)". The
 * edit, the copy and the sort still read the bare name. Injection: drop the
 * Name column's `render`.
 */
import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';
import i18n from '../../../i18n';
import { buildStationColumns, NAME_COLUMN_WIDTH, nameQualifier } from '../dataTableStationColumns';
import type { Station } from '../../../types/project';

const MAIN = 1;
const SIDE = 2;
const lineName = (id: number) => (id === MAIN ? 'Main' : 'Side');
const t = i18n.t.bind(i18n);

function station(index: number, name: string, id: number, shown?: Station['shown']): Station {
  return {
    station_ref: { line_id: SIDE, station_index: index },
    coordinates: { x: 0, y: 0, z: 0 },
    is_zero_datum: false,
    gps_coordinates: null,
    merged_with: [],
    id,
    name,
    ...(shown ? { shown } : {}),
  };
}

const dotted = station(0, '12', 12, { id: 12, home_line: MAIN, foreign: true, loop_end: false });
const own = station(12, '12', 27);
const loopEnd = station(8, '8', 38, { id: 8, home_line: MAIN, foreign: true, loop_end: true, loop_closure: 1n });

describe('nameQualifier', () => {
  it('says the home line of a dotted row, "loop end" of a loop end, nothing for a native row', () => {
    const labels = { lineName };
    expect(nameQualifier(dotted, labels, t)).toBe('(Main)');
    expect(nameQualifier(loopEnd, labels, t)).toBe('(loop end)');
    expect(nameQualifier(own, labels, t)).toBeNull();
    expect(nameQualifier(dotted, undefined, t)).toBeNull();
  });
});

describe('the Name column', () => {
  const nameColumn = () => {
    const found = buildStationColumns({
      t,
      precision: { coordinates: 2, depth: 1, gps: 6 },
      geometryReadOnly: false,
      gpsOrigin: null,
      onGpsCellClick: () => {},
      onConnectionsCellClick: () => {},
      labels: {
        station: (ref) => String(ref.station_index),
        stationName: (ref) => String(ref.station_index),
        stationId: () => null,
        lineName,
        stationNameTakenOn: () => null,
      },
    }).find((c) => c.key === 'name');
    if (!found?.render) throw new Error('the Name column renders its cell');
    return found;
  };

  it("adds the muted home line after a dotted row's name and keeps the value bare", () => {
    const column = nameColumn();
    render(<>{column.render!(dotted, 0)}</>);
    const qualifier = screen.getByTestId(`station-name-qualifier-${SIDE}-0`);
    expect(qualifier).toHaveTextContent('(Main)');
    expect(qualifier).toHaveClass('text-text-secondary');
    expect(qualifier.parentElement).toHaveTextContent(/^12 \(Main\)$/);
    expect(column.getValue(dotted, 0)).toBe('12');
  });

  it("leaves the line's own station of that name bare", () => {
    const column = nameColumn();
    const { container } = render(<>{column.render!(own, 12)}</>);
    expect(container).toHaveTextContent(/^12$/);
    expect(screen.queryByTestId(`station-name-qualifier-${SIDE}-12`)).toBeNull();
  });
});

/**
 * RS-FX8, N3 (shot 29 of the RS capture): at the 72 px default the Name
 * cell read "8 (loop..."; the column is sized for a three-digit name and the
 * "(loop end)" qualifier at the grid's 12 px text, and the qualifier, not the
 * name, is what truncates when a long home line name does not fit.
 * Injection: the 72 px width again.
 */
describe('the Name column fits its qualifier (RS-FX8)', () => {
  // About 6.5 px a character at 12 px, plus the cell's 16 px padding.
  const fits = (text: string) => Math.ceil(text.length * 6.5) + 16;

  it('is wide enough for "888 (loop end)"', () => {
    expect(NAME_COLUMN_WIDTH).toBeGreaterThanOrEqual(fits('888 (loop end)'));
    const column = buildStationColumns({
      t,
      precision: { coordinates: 2, depth: 1, gps: 6 },
      geometryReadOnly: false,
      gpsOrigin: null,
      onGpsCellClick: () => {},
      onConnectionsCellClick: () => {},
    }).find((c) => c.key === 'name');
    expect(column?.width).toBe(NAME_COLUMN_WIDTH);
  });

  it('truncates the qualifier, never the name', () => {
    const column = buildStationColumns({
      t,
      precision: { coordinates: 2, depth: 1, gps: 6 },
      geometryReadOnly: false,
      gpsOrigin: null,
      onGpsCellClick: () => {},
      onConnectionsCellClick: () => {},
      labels: {
        station: (ref) => String(ref.station_index),
        stationName: (ref) => String(ref.station_index),
        stationId: () => null,
        lineName,
        stationNameTakenOn: () => null,
      },
    }).find((c) => c.key === 'name')!;
    render(<>{column.render!(loopEnd, 8)}</>);
    const qualifier = screen.getByTestId(`station-name-qualifier-${SIDE}-8`);
    expect(qualifier).toHaveTextContent('(loop end)');
    expect(qualifier).toHaveClass('truncate');
    expect(qualifier.previousElementSibling).toHaveClass('shrink-0');
    expect(qualifier.parentElement).toHaveAttribute('title', '8 (loop end)');
  });
});
