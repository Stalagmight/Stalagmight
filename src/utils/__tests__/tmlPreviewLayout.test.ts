import { describe, it, expect } from 'vitest';
import { computeTmlPreviewLayout, isOfferableCoordinate } from '../tmlPreviewLayout';
import type { TmlFile, TmlSection, TmlShot } from '../../types/commands';

/**
 * The preview layout has one job the dialog depends on: agreeing with the Rust
 * converter about TOPOLOGY under a section selection. These drive the same
 * cases as `import_options.rs`'s tests, from the other side of the wire.
 */

function shot(id: number, fromId: number, over: Partial<TmlShot> = {}): TmlShot {
  return {
    id,
    from_id: fromId,
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
    closure_to_id: -1,
    section: '',
    ...over,
  };
}

function section(name: string, startArianeId: number, shots: TmlShot[]): TmlSection {
  return {
    name,
    shots,
    surveyor: null,
    date: null,
    color: '#ff0000',
    is_branch_split: false,
    start_ariane_id: startArianeId,
    end_ariane_id: shots.length > 0 ? shots[shots.length - 1]!.id : -1,
    start_depth: 0,
  };
}

function file(sections: TmlSection[]): TmlFile {
  return {
    cave_name: 'T',
    unit: 'Metric',
    use_magnetic_azimuth: false,
    first_start_absolute_elevation: 0,
    geo_coding: null,
    sections,
    closures: [],
  };
}

/** A -> B -> C, each hanging off the previous section's last shot. Every shot
 *  runs due east, 10 m, so offsets are exact whole numbers. */
function chainOfThree(): TmlFile {
  return file([
    section('A', 0, [shot(1, 0), shot(2, 1)]),
    section('B', 2, [shot(3, 2), shot(4, 3)]),
    section('C', 4, [shot(5, 4), shot(6, 5)]),
  ]);
}

const all = { includeExcluded: false, selected: null };

/** Offsets to whole millimetres. Every fixture runs due east, where `cos(90 deg)`
 *  is 6e-17 rather than 0, and that noise is not what these tests are about. */
const mm = (v: number) => Math.round(v * 1000) / 1000;
const offsets = (layout: { sections: Array<{ offsetX: number; offsetY: number }> }) =>
  layout.sections.map((s) => [mm(s.offsetX), mm(s.offsetY)]);

describe('computeTmlPreviewLayout', () => {
  it('chains the sections end to end when nothing is filtered', () => {
    const layout = computeTmlPreviewLayout(chainOfThree(), all);

    expect(layout.sections.map((s) => s.index)).toEqual([0, 1, 2]);
    // A at the origin; B starts where A's 2 shots ended (20 m east); C at 40.
    expect(offsets(layout)).toEqual([
      [0, 0],
      [20, 0],
      [40, 0],
    ]);
    expect(layout.reanchoredCount).toBe(0);
  });

  it('drops a deselected section and re-anchors the first survivor to the origin', () => {
    const layout = computeTmlPreviewLayout(chainOfThree(), {
      includeExcluded: false,
      selected: new Set([1, 2]),
    });

    expect(layout.sections.map((s) => s.index)).toEqual([1, 2]);
    // B is the new root, so it moves to the origin and C follows it there.
    expect(offsets(layout)).toEqual([
      [0, 0],
      [20, 0],
    ]);
    // The new root belongs at the origin - that is not a lost anchor.
    expect(layout.reanchoredCount).toBe(0);
    expect(layout.sections.every((s) => !s.reanchored)).toBe(true);
  });

  it('counts a section whose parent was deselected as re-anchored', () => {
    const layout = computeTmlPreviewLayout(chainOfThree(), {
      includeExcluded: false,
      selected: new Set([0, 2]),
    });

    expect(layout.sections.map((s) => s.index)).toEqual([0, 2]);
    const c = layout.sections.find((s) => s.index === 2)!;
    expect(c.reanchored).toBe(true);
    expect([mm(c.offsetX), mm(c.offsetY)]).toEqual([0, 0]);
    expect(layout.reanchoredCount).toBe(1);
  });

  it('resolves a parent that appears later in the file', () => {
    // Section 1 hangs off the last shot of section 2: a FORWARD reference,
    // which a single resolution pass in file order would leave at the origin
    // (section 2 is not placed yet when section 1 is visited). Section 0 is the
    // root, so this is not the root rule doing the work.
    const tml = file([
      section('A', 0, [shot(1, 0), shot(2, 1)]),
      section('Forward child', 6, [shot(7, 6)]),
      section('C', 2, [shot(5, 2), shot(6, 5)]),
    ]);
    const layout = computeTmlPreviewLayout(tml, all);

    const c = layout.sections.find((s) => s.index === 2)!;
    expect([mm(c.offsetX), mm(c.offsetY)]).toEqual([20, 0]);
    const child = layout.sections.find((s) => s.index === 1)!;
    // C sits 20 m east and runs 20 m further, so its station 2 is at 40.
    expect([mm(child.offsetX), mm(child.offsetY)]).toEqual([40, 0]);
    expect(child.reanchored).toBe(false);
  });

  it('leaves a self-referential cycle at the origin instead of looping', () => {
    // Two sections that each claim to start at the other's last shot.
    const tml = file([
      section('Loop A', 30, [shot(20, 30)]),
      section('Loop B', 20, [shot(30, 20)]),
    ]);
    const layout = computeTmlPreviewLayout(tml, all);
    expect(layout.sections).toHaveLength(2);
    for (const s of layout.sections) {
      expect(Number.isFinite(s.offsetX)).toBe(true);
      expect(Number.isFinite(s.offsetY)).toBe(true);
    }
  });

  it('omits a section left with no shots by the exclusion filter', () => {
    const tml = file([
      section('A', 0, [shot(1, 0)]),
      section('Excluded', 1, [shot(2, 1, { excluded: true })]),
    ]);

    expect(computeTmlPreviewLayout(tml, all).sections.map((s) => s.index)).toEqual([0]);
    expect(
      computeTmlPreviewLayout(tml, { includeExcluded: true, selected: null }).sections.map(
        (s) => s.index
      )
    ).toEqual([0, 1]);
  });

  it('lays out nothing for an empty selection', () => {
    const layout = computeTmlPreviewLayout(chainOfThree(), {
      includeExcluded: false,
      selected: new Set(),
    });
    expect(layout.sections).toEqual([]);
    expect(layout.reanchoredCount).toBe(0);
  });

  it('keeps CLOSURE rows out of the drawn geometry', () => {
    const tml = file([
      section('A', 0, [shot(1, 0), shot(9, 1, { shot_type: 'Closure', closure_to_id: 1 })]),
    ]);
    const layout = computeTmlPreviewLayout(tml, all);
    expect(layout.sections[0]!.section.shots).toHaveLength(1);
  });
});

/**
 * Review round 2. The preview is the only thing telling the user where the
 * import will put a section, so every claim it makes has to be one the backend
 * will honour (`import_options.rs`, `determine_placement`).
 */
describe('computeTmlPreviewLayout agrees with the backend about placement', () => {
  /** Section 0 is emptied by the exclusion filter, so it is never created. */
  function firstSectionAllExcluded(): TmlFile {
    return file([
      section('Dropped', 0, [shot(1, 0, { excluded: true })]),
      section('Survivor', 1, [shot(2, 1), shot(3, 2)]),
    ]);
  }

  it('makes the first CREATED section the root, not the lowest index', () => {
    const layout = computeTmlPreviewLayout(firstSectionAllExcluded(), all);

    expect(layout.sections.map((s) => s.index)).toEqual([1]);
    // The survivor IS the root - the dialog labels it as such and anchors the
    // placement choice to it - so calling it re-anchored contradicts the very
    // panel beside it, and the backend now puts the typed datum here.
    expect(layout.sections[0]!.reanchored).toBe(false);
    expect(layout.reanchoredCount).toBe(0);
  });

  it('does not claim the origin for a section that carries its own GPS', () => {
    // C hangs off B; B is deselected, so C loses its connection - but its first
    // shot carries coordinates, and `determine_placement` prefers those over
    // the origin fallback.
    const tml = chainOfThree();
    tml.sections[2]!.shots[0] = shot(5, 4, { latitude: 20.5, longitude: -87.1 });

    const layout = computeTmlPreviewLayout(tml, {
      includeExcluded: false,
      selected: new Set([0, 2]),
    });
    const c = layout.sections.find((s) => s.index === 2)!;
    expect(c.reanchored).toBe(false);
    expect(layout.reanchoredCount).toBe(0);
  });

  it('reads the GPS off the raw first shot even when that shot is excluded (#576 D39(j))', () => {
    // An unconnected entrance whose first leg carries the propagated START GPS
    // and is excluded. The backend keeps the GPS and carries the datum past the
    // excluded leg (`gps_placement`), so the section is placed, not
    // re-anchored - the preview used to test the first IMPORTED shot instead
    // and badge it.
    const tml = file([
      section('Root', 0, [shot(1, 0)]),
      section('Entrance', 50, [
        shot(51, 50, { excluded: true, latitude: 20.5, longitude: -87.3 }),
        shot(52, 51),
      ]),
    ]);
    const layout = computeTmlPreviewLayout(tml, all);

    const entrance = layout.sections.find((s) => s.index === 1)!;
    expect(entrance.section.shots).toHaveLength(1);
    expect(entrance.reanchored).toBe(false);
    expect(layout.reanchoredCount).toBe(0);
  });

  it('ignores GPS the backend would refuse, so the two never disagree', () => {
    // NaN slips past a null-island guard written as `Math.abs(x) > 0`, exactly
    // as it did in Rust. `GpsCoordinates::new` refuses it, so the section really
    // does land at the origin and the preview must say so.
    for (const bad of [{ latitude: Number.NaN, longitude: 10 }, { latitude: 1e300, longitude: 10 }]) {
      const tml = chainOfThree();
      tml.sections[2]!.shots[0] = shot(5, 4, bad);
      const layout = computeTmlPreviewLayout(tml, {
        includeExcluded: false,
        selected: new Set([0, 2]),
      });
      expect(layout.sections.find((s) => s.index === 2)!.reanchored).toBe(true);
    }
  });

  it('flags a file orphan whose parent ids are all negative', () => {
    // `start_ariane_id: -1` with a negative `from_id` names no shot at all. The
    // backend still warns and places it at the origin; the preview used to stay
    // silent because it had no candidate id to fail to resolve.
    const tml = file([
      section('A', 0, [shot(1, 0)]),
      section('Orphan', -1, [shot(2, -1)]),
    ]);
    const layout = computeTmlPreviewLayout(tml, all);

    expect(layout.sections.find((s) => s.index === 1)!.reanchored).toBe(true);
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
    expect(layout.sections.map((s) => s.index)).toEqual([0, 1, 2]);
  });

  it('resolves a long chain of forward references in one sweep', () => {
    // 5k sections in a single chain: S0 is the head and comes FIRST in the file
    // (so it is the root), and the rest are written in reverse, which makes
    // almost every parent link point forwards. The fixed-point relaxation this
    // replaced needed one full pass per link and took ~0.8s here; the sweep is
    // one visit per section.
    const count = 5000;
    const sections: TmlSection[] = [section('S0', -1, [shot(1, -1)])];
    for (let i = count - 1; i >= 1; i--) {
      // S_i hangs off S_{i-1}'s only shot, which has id `i`.
      sections.push(section(`S${i}`, i, [shot(i + 1, i)]));
    }
    const layout = computeTmlPreviewLayout(file(sections), all);

    expect(layout.sections).toHaveLength(count);
    const at = (name: string) => {
      const entry = layout.sections.find((s) => s.name === name)!;
      return [mm(entry.offsetX), mm(entry.offsetY)];
    };
    // Every shot runs 10 m due east, so section i starts at 10i.
    expect(at('S0')).toEqual([0, 0]);
    expect(at('S1')).toEqual([10, 0]);
    expect(at('S4999')).toEqual([49_990, 0]);
    // A chain with one head has no orphan: every link resolved.
    expect(layout.reanchoredCount).toBe(0);
  });
});

/**
 * The shape the layout receives in the shipped app (#566, #576 C6).
 *
 * `parse_tml` crosses Tauri's JSON IPC, and serde writes an `i64` as a JSON
 * number, so the Ariane ids arrive as plain numbers. The generated types used to
 * declare them `bigint` and every fixture here built `BigInt` ids - a shape the
 * wire never delivers - so a lookup that worked only for one of them stayed green
 * here and was dead in the app. The types and the fixtures now say `number`.
 */
describe('computeTmlPreviewLayout reads the ids the wire actually delivers', () => {
  it('links the chain when the ids are plain numbers', () => {
    const wire = chainOfThree();

    // Premise: this really is the wire's runtime shape.
    expect(typeof wire.sections[1]!.start_ariane_id).toBe('number');
    expect(typeof wire.sections[0]!.shots[0]!.id).toBe('number');

    const layout = computeTmlPreviewLayout(wire, all);
    // Each section starts where the previous one ended. A dropped lookup shows
    // up as every section stacked at the origin and flagged as connecting to
    // nothing.
    expect(offsets(layout)).toEqual([
      [0, 0],
      [20, 0],
      [40, 0],
    ]);
    expect(layout.reanchoredCount).toBe(0);
  });

  it('keeps the selection semantics on wire-shaped ids', () => {
    // The re-anchor rules are what the dialog draws its badges from:
    // deselecting the middle section strands the one that hung off it.
    const layout = computeTmlPreviewLayout(chainOfThree(), {
      includeExcluded: false,
      selected: new Set([0, 2]),
    });
    expect(layout.sections.find((s) => s.index === 2)!.reanchored).toBe(true);
    expect(layout.reanchoredCount).toBe(1);

    // ...and deselecting the head promotes its child without stranding anything.
    const promoted = computeTmlPreviewLayout(chainOfThree(), {
      includeExcluded: false,
      selected: new Set([1, 2]),
    });
    expect(promoted.reanchoredCount).toBe(0);
    expect(offsets(promoted)).toEqual([
      [0, 0],
      [20, 0],
    ]);
  });

  it('treats a negative wire id as "names nothing"', () => {
    const orphan = file([section('A', 0, [shot(1, 0)]), section('Orphan', -1, [shot(2, -1)])]);
    const layout = computeTmlPreviewLayout(orphan, all);
    expect(layout.sections.find((s) => s.index === 1)!.reanchored).toBe(true);
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
