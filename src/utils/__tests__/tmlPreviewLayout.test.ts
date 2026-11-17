import { describe, it, expect } from 'vitest';
import { computeTmlPreviewLayout, isOfferableCoordinate } from '../tmlPreviewLayout';
import type { TmlFile, TmlSection, TmlShot } from '../../types/commands';

/**
 * The preview layout has one job the dialog depends on: agreeing with the Rust
 * converter about TOPOLOGY under a line selection. These drive the same
 * cases as `import_options.rs`'s tests, from the other side of the wire.
 */

function shot(id: number, fromId: number, over: Partial<TmlShot> = {}): TmlShot {
  return {
    id: BigInt(id),
    from_id: BigInt(fromId),
    shot_type: 'Real',
    azimuth: 90,
    length: 10,
    depth: 0,
    depth_in: 0,
    inclination: 0,
    left: 0,
    right: 0,
    up: 0,
    down: 0,
    comment: null,
    explorer: null,
    date: null,
    color: '#ff0000',
    excluded: false,
    latitude: 0,
    longitude: 0,
    closure_to_id: -1n,
    line: '',
    ...over,
  };
}

function line(name: string, startArianeId: number, shots: TmlShot[]): TmlSection {
  return {
    name,
    shots,
    surveyor: null,
    date: null,
    color: '#ff0000',
    is_branch_split: false,
    start_ariane_id: BigInt(startArianeId),
    end_ariane_id: shots.length > 0 ? shots[shots.length - 1]!.id : -1n,
    start_depth: 0,
  };
}

function file(lines: TmlSection[]): TmlFile {
  return {
    cave_name: 'T',
    unit: 'Metric',
    use_magnetic_azimuth: false,
    first_start_absolute_elevation: 0,
    geo_coding: null,
    lines,
    closures: [],
  };
}

/** A -> B -> C, each hanging off the previous line's last shot. Every shot
 *  runs due east, 10 m, so offsets are exact whole numbers. */
function chainOfThree(): TmlFile {
  return file([
    line('A', 0, [shot(1, 0), shot(2, 1)]),
    line('B', 2, [shot(3, 2), shot(4, 3)]),
    line('C', 4, [shot(5, 4), shot(6, 5)]),
  ]);
}

const all = { includeExcluded: false, selected: null };

/** Offsets to whole millimetres. Every fixture runs due east, where `cos(90 deg)`
 *  is 6e-17 rather than 0, and that noise is not what these tests are about. */
const mm = (v: number) => Math.round(v * 1000) / 1000;
const offsets = (layout: { lines: Array<{ offsetX: number; offsetY: number }> }) =>
  layout.lines.map((s) => [mm(s.offsetX), mm(s.offsetY)]);

describe('computeTmlPreviewLayout', () => {
  it('chains the lines end to end when nothing is filtered', () => {
    const layout = computeTmlPreviewLayout(chainOfThree(), all);

    expect(layout.lines.map((s) => s.index)).toEqual([0, 1, 2]);
    // A at the origin; B starts where A's 2 shots ended (20 m east); C at 40.
    expect(offsets(layout)).toEqual([
      [0, 0],
      [20, 0],
      [40, 0],
    ]);
    expect(layout.reanchoredCount).toBe(0);
  });

  it('drops a deselected line and re-anchors the first survivor to the origin', () => {
    const layout = computeTmlPreviewLayout(chainOfThree(), {
      includeExcluded: false,
      selected: new Set([1, 2]),
    });

    expect(layout.lines.map((s) => s.index)).toEqual([1, 2]);
    // B is the new root, so it moves to the origin and C follows it there.
    expect(offsets(layout)).toEqual([
      [0, 0],
      [20, 0],
    ]);
    // The new root belongs at the origin - that is not a lost anchor.
    expect(layout.reanchoredCount).toBe(0);
    expect(layout.lines.every((s) => !s.reanchored)).toBe(true);
  });

  it('counts a line whose parent was deselected as re-anchored', () => {
    const layout = computeTmlPreviewLayout(chainOfThree(), {
      includeExcluded: false,
      selected: new Set([0, 2]),
    });

    expect(layout.lines.map((s) => s.index)).toEqual([0, 2]);
    const c = layout.lines.find((s) => s.index === 2)!;
    expect(c.reanchored).toBe(true);
    expect([mm(c.offsetX), mm(c.offsetY)]).toEqual([0, 0]);
    expect(layout.reanchoredCount).toBe(1);
  });

  it('resolves a parent that appears later in the file', () => {
    // Line 1 hangs off the last shot of line 2: a FORWARD reference,
    // which a single resolution pass in file order would leave at the origin
    // (line 2 is not placed yet when line 1 is visited). Line 0 is the
    // root, so this is not the root rule doing the work.
    const tml = file([
      line('A', 0, [shot(1, 0), shot(2, 1)]),
      line('Forward child', 6, [shot(7, 6)]),
      line('C', 2, [shot(5, 2), shot(6, 5)]),
    ]);
    const layout = computeTmlPreviewLayout(tml, all);

    const c = layout.lines.find((s) => s.index === 2)!;
    expect([mm(c.offsetX), mm(c.offsetY)]).toEqual([20, 0]);
    const child = layout.lines.find((s) => s.index === 1)!;
    // C sits 20 m east and runs 20 m further, so its station 2 is at 40.
    expect([mm(child.offsetX), mm(child.offsetY)]).toEqual([40, 0]);
    expect(child.reanchored).toBe(false);
  });

  it('leaves a self-referential cycle at the origin instead of looping', () => {
    // Two lines that each claim to start at the other's last shot.
    const tml = file([
      line('Loop A', 30, [shot(20, 30)]),
      line('Loop B', 20, [shot(30, 20)]),
    ]);
    const layout = computeTmlPreviewLayout(tml, all);
    expect(layout.lines).toHaveLength(2);
    for (const s of layout.lines) {
      expect(Number.isFinite(s.offsetX)).toBe(true);
      expect(Number.isFinite(s.offsetY)).toBe(true);
    }
  });

  it('omits a line left with no shots by the exclusion filter', () => {
    const tml = file([
      line('A', 0, [shot(1, 0)]),
      line('Excluded', 1, [shot(2, 1, { excluded: true })]),
    ]);

    expect(computeTmlPreviewLayout(tml, all).lines.map((s) => s.index)).toEqual([0]);
    expect(
      computeTmlPreviewLayout(tml, { includeExcluded: true, selected: null }).lines.map(
        (s) => s.index
      )
    ).toEqual([0, 1]);
  });

  it('lays out nothing for an empty selection', () => {
    const layout = computeTmlPreviewLayout(chainOfThree(), {
      includeExcluded: false,
      selected: new Set(),
    });
    expect(layout.lines).toEqual([]);
    expect(layout.reanchoredCount).toBe(0);
  });

  it('keeps CLOSURE rows out of the drawn geometry', () => {
    const tml = file([
      line('A', 0, [shot(1, 0), shot(9, 1, { shot_type: 'Closure', closure_to_id: 1n })]),
    ]);
    const layout = computeTmlPreviewLayout(tml, all);
    expect(layout.lines[0]!.line.shots).toHaveLength(1);
  });
});

/**
 * Review round 2. The preview is the only thing telling the user where the
 * import will put a line, so every claim it makes has to be one the backend
 * will honour (`import_options.rs`, `determine_placement`).
 */
describe('computeTmlPreviewLayout agrees with the backend about placement', () => {
  /** Line 0 is emptied by the exclusion filter, so it is never created. */
  function firstLineAllExcluded(): TmlFile {
    return file([
      line('Dropped', 0, [shot(1, 0, { excluded: true })]),
      line('Survivor', 1, [shot(2, 1), shot(3, 2)]),
    ]);
  }

  it('makes the first CREATED line the root, not the lowest index', () => {
    const layout = computeTmlPreviewLayout(firstLineAllExcluded(), all);

    expect(layout.lines.map((s) => s.index)).toEqual([1]);
    // The survivor IS the root - the dialog labels it as such and anchors the
    // placement choice to it - so calling it re-anchored contradicts the very
    // panel beside it, and the backend now puts the typed datum here.
    expect(layout.lines[0]!.reanchored).toBe(false);
    expect(layout.reanchoredCount).toBe(0);
  });

  it('does not claim the origin for a line that carries its own GPS', () => {
    // C hangs off B; B is deselected, so C loses its connection - but its first
    // shot carries coordinates, and `determine_placement` prefers those over
    // the origin fallback.
    const tml = chainOfThree();
    tml.lines[2]!.shots[0] = shot(5, 4, { latitude: 20.5, longitude: -87.1 });

    const layout = computeTmlPreviewLayout(tml, {
      includeExcluded: false,
      selected: new Set([0, 2]),
    });
    const c = layout.lines.find((s) => s.index === 2)!;
    expect(c.reanchored).toBe(false);
    expect(layout.reanchoredCount).toBe(0);
  });

  it('ignores GPS the backend would refuse, so the two never disagree', () => {
    // NaN slips past a null-island guard written as `Math.abs(x) > 0`, exactly
    // as it did in Rust. `GpsCoordinates::new` refuses it, so the line really
    // does land at the origin and the preview must say so.
    for (const bad of [{ latitude: Number.NaN, longitude: 10 }, { latitude: 1e300, longitude: 10 }]) {
      const tml = chainOfThree();
      tml.lines[2]!.shots[0] = shot(5, 4, bad);
      const layout = computeTmlPreviewLayout(tml, {
        includeExcluded: false,
        selected: new Set([0, 2]),
      });
      expect(layout.lines.find((s) => s.index === 2)!.reanchored).toBe(true);
    }
  });

  it('flags a file orphan whose parent ids are all negative', () => {
    // `start_ariane_id: -1` with a negative `from_id` names no shot at all. The
    // backend still warns and places it at the origin; the preview used to stay
    // silent because it had no candidate id to fail to resolve.
    const tml = file([
      line('A', 0, [shot(1, 0)]),
      line('Orphan', -1, [shot(2, -1)]),
    ]);
    const layout = computeTmlPreviewLayout(tml, all);

    expect(layout.lines.find((s) => s.index === 1)!.reanchored).toBe(true);
    expect(layout.reanchoredCount).toBe(1);
  });

  it('lays out a huge selection without spreading it into an argument list', () => {
    // `Math.min(...set)` throws RangeError past ~1e5 arguments, and this runs
    // inside a useMemo on every checkbox click - i.e. it took the dialog down.
    const many = 200_000;
    const selectedAll = new Set<number>();
    for (let i = 0; i < many; i++) selectedAll.add(i);

    const layout = computeTmlPreviewLayout(chainOfThree(), {
      includeExcluded: false,
      selected: selectedAll,
    });
    expect(layout.lines.map((s) => s.index)).toEqual([0, 1, 2]);
  });

  it('resolves a long chain of forward references in one sweep', () => {
    // 5k lines in a single chain: S0 is the head and comes FIRST in the file
    // (so it is the root), and the rest are written in reverse, which makes
    // almost every parent link point forwards. The fixed-point relaxation this
    // replaced needed one full pass per link and took ~0.8s here; the sweep is
    // one visit per line.
    const count = 5000;
    const lines: TmlSection[] = [line('S0', -1, [shot(1, -1)])];
    for (let i = count - 1; i >= 1; i--) {
      // S_i hangs off S_{i-1}'s only shot, which has id `i`.
      lines.push(line(`S${i}`, i, [shot(i + 1, i)]));
    }
    const layout = computeTmlPreviewLayout(file(lines), all);

    expect(layout.lines).toHaveLength(count);
    const at = (name: string) => {
      const entry = layout.lines.find((s) => s.name === name)!;
      return [mm(entry.offsetX), mm(entry.offsetY)];
    };
    // Every shot runs 10 m due east, so line i starts at 10i.
    expect(at('S0')).toEqual([0, 0]);
    expect(at('S1')).toEqual([10, 0]);
    expect(at('S4999')).toEqual([49_990, 0]);
    // A chain with one head has no orphan: every link resolved.
    expect(layout.reanchoredCount).toBe(0);
  });
});

/**
 * The shape the layout ACTUALLY receives in the shipped app.
 *
 * `parse_tml` crosses Tauri's JSON IPC, and serde writes an `i64` as a JSON
 * number - so every field ts-rs types as `bigint` arrives as a plain `number`
 * at runtime. The e2e mock encodes the same truth in its own `TmlWire<T>`
 * helper (e2e-playwright/mocks/tauri-ipc.ts), and `loadProjectBinary.ts`
 * normalizes `bigint | number` for the one path where bigints are real.
 *
 * Every fixture above builds `BigInt` ids, which is the shape the TYPES promise
 * and not the shape the wire delivers - so a lookup that only works for one of
 * them stayed green here and was dead in the app.
 */
function toWire<T>(value: T): T {
  if (typeof value === 'bigint') return Number(value) as unknown as T;
  if (Array.isArray(value)) return value.map(toWire) as unknown as T;
  if (value && typeof value === 'object') {
    return Object.fromEntries(
      Object.entries(value as Record<string, unknown>).map(([k, v]) => [k, toWire(v)])
    ) as T;
  }
  return value;
}

describe('computeTmlPreviewLayout reads the ids the wire actually delivers', () => {
  it('links the chain when the ids are plain numbers', () => {
    const wire = toWire(chainOfThree());

    // Premise: this really is the other runtime shape.
    expect(typeof wire.lines[1]!.start_ariane_id).toBe('number');
    expect(typeof wire.lines[0]!.shots[0]!.id).toBe('number');

    const layout = computeTmlPreviewLayout(wire, all);
    // Identical to the bigint fixture's expectations: each line starts where
    // the previous one ended. A dropped lookup shows up as every line stacked
    // at the origin and flagged as connecting to nothing.
    expect(offsets(layout)).toEqual([
      [0, 0],
      [20, 0],
      [40, 0],
    ]);
    expect(layout.reanchoredCount).toBe(0);
  });

  it('still links the chain when the ids are bigints', () => {
    // The types' shape, kept pinned so a normalization that only handles the
    // wire shape cannot silently break the other one.
    const layout = computeTmlPreviewLayout(chainOfThree(), all);
    expect(offsets(layout)).toEqual([
      [0, 0],
      [20, 0],
      [40, 0],
    ]);
    expect(layout.reanchoredCount).toBe(0);
  });

  it('keeps the selection semantics on wire-shaped ids', () => {
    // The re-anchor rules are what the dialog draws its badges from, so they
    // have to survive the id shape too: deselecting the middle line strands
    // the one that hung off it.
    const layout = computeTmlPreviewLayout(toWire(chainOfThree()), {
      includeExcluded: false,
      selected: new Set([0, 2]),
    });
    expect(layout.lines.find((s) => s.index === 2)!.reanchored).toBe(true);
    expect(layout.reanchoredCount).toBe(1);

    // ...and deselecting the head promotes its child without stranding anything.
    const promoted = computeTmlPreviewLayout(toWire(chainOfThree()), {
      includeExcluded: false,
      selected: new Set([1, 2]),
    });
    expect(promoted.reanchoredCount).toBe(0);
    expect(offsets(promoted)).toEqual([
      [0, 0],
      [20, 0],
    ]);
  });

  it('treats a negative wire id as "names nothing", like a negative bigint', () => {
    const orphan = toWire(
      file([line('A', 0, [shot(1, 0)]), line('Orphan', -1, [shot(2, -1)])])
    );
    const layout = computeTmlPreviewLayout(orphan, all);
    expect(layout.lines.find((s) => s.index === 1)!.reanchored).toBe(true);
  });
});

describe('isOfferableCoordinate mirrors the backend null-island guard', () => {
  it('treats a denormal as absent, the way f64::EPSILON does', () => {
    // The backend's `gps_placement` says "no anchor" when BOTH components are
    // `<= f64::EPSILON`. A `> 0` test disagrees for a denormal: it offers the
    // coordinate, the dialog preselects "use the file's position", and the
    // survey is georeferenced next to Null Island by an explicit zero datum the
    // backend would never have chosen on its own.
    expect(Number.EPSILON).toBe(2 ** -52); // === f64::EPSILON
    expect(isOfferableCoordinate(1e-300, 0)).toBe(false);
    expect(isOfferableCoordinate(0, 1e-300)).toBe(false);
    expect(isOfferableCoordinate(Number.EPSILON, 0)).toBe(false);
  });

  it('still offers a real coordinate, however small', () => {
    // 1e-10 degrees is ~0.01 mm, far below survey precision, but it is a value
    // a file states rather than a rounding artefact - and the backend accepts
    // it, so refusing it here would put the two out of step again.
    expect(isOfferableCoordinate(1e-10, 0)).toBe(true);
    expect(isOfferableCoordinate(20.5, -87.1)).toBe(true);
  });

  it('refuses what GpsCoordinates::new refuses', () => {
    expect(isOfferableCoordinate(Number.NaN, 10)).toBe(false);
    expect(isOfferableCoordinate(91, 10)).toBe(false);
    expect(isOfferableCoordinate(10, 181)).toBe(false);
    expect(isOfferableCoordinate(0, 0)).toBe(false);
  });
});
