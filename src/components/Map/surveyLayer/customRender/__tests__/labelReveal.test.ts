import { describe, expect, it } from 'vitest';
import {
  computeRevealZooms,
  revealScreenBox,
  labelPriority,
  emitSlackFor,
  REVEAL_NEVER,
  EMIT_SLACK,
  REVEAL_CEILING,
  ADAPTIVE_EMIT_MAX,
  labelVirtualRank,
  VIRTUAL_RANK_REAL,
  VIRTUAL_RANK_JUMP,
  VIRTUAL_RANK_REFERENCE,
  anchorHull,
  type RevealInput,
  type ScreenBox,
} from '../labelReveal';
import type { LabelDescriptor } from '../labelTessellate';
import type { ExtentEm } from '../labelLayout';
import { LABEL_KIND, LABEL_FLAG, type LabelKind } from '../labelShaders';
import { LABEL_KIND_STYLE, interpolateStops } from '../labelStyle';

/**
 * Per-label reveal-zoom computation (#customRender labels, reveal-zoom rework).
 * Pure, node env. Locks: the fold of decimation-admission + collision into one
 * reveal zoom, the zoom-independent priority order (coarser tier / higher kind
 * priority wins), determinism (same input → same output, and stable across
 * rebuilds whose membership shifts), the candidate filter (toggled-off /
 * reference-hidden / empty labels), and - the headline property - MONOTONICITY:
 * the visible set only grows with zoom and never overlaps at any zoom.
 */

const UNIT_EXT: ExtentEm = { minX: 0, maxX: 1, minY: 0, maxY: 1 };

/** buildZoom 18, a large world-pixel scale so a small mercator delta is many px.
 *  slack default 2 → ladder covers up to z20 unless overridden. */
function input(over: Partial<RevealInput> = {}): RevealInput {
  return {
    buildZoom: 18,
    worldPixelScale: 1e6,
    kindVisible: [true, true, true, true, true],
    hideReference: false,
    hideJump: false,
    ...over,
  };
}

function desc(kind: LabelKind, over: Partial<LabelDescriptor> = {}): LabelDescriptor {
  return {
    kind,
    text: 'x',
    anchorX: 0,
    anchorY: 0,
    color: [0, 0, 0, 255],
    ordinal: 0,
    flags: 0,
    ...over,
  };
}

describe('labelPriority', () => {
  it('orders section > first-in-section station > station > shot > comments', () => {
    expect(labelPriority(LABEL_KIND.SECTION_NAME, false)).toBe(0);
    expect(labelPriority(LABEL_KIND.STATION_LABEL, true)).toBe(1);
    expect(labelPriority(LABEL_KIND.STATION_LABEL, false)).toBe(2);
    expect(labelPriority(LABEL_KIND.SHOT_LABEL, false)).toBe(3);
    expect(labelPriority(LABEL_KIND.STATION_COMMENT, false)).toBe(4);
    expect(labelPriority(LABEL_KIND.SHOT_COMMENT, false)).toBe(4);
  });
});

describe('computeRevealZooms decimation-admission fold', () => {
  it('an isolated shot reveals at its nested tier floor: %4 -> 14, %2 -> 16, odd -> 18', () => {
    // Far-apart shots never collide, so reveal == the decimation admission zoom.
    const descs = [
      desc(LABEL_KIND.SHOT_LABEL, { anchorX: 0, ordinal: 4 }), // tier 0
      desc(LABEL_KIND.SHOT_LABEL, { anchorX: 0.01, ordinal: 2 }), // tier 1
      desc(LABEL_KIND.SHOT_LABEL, { anchorX: 0.02, ordinal: 1 }), // tier 2
    ];
    const reveal = computeRevealZooms(descs, [UNIT_EXT, UNIT_EXT, UNIT_EXT], input());
    expect(Array.from(reveal)).toEqual([14, 16, 18]);
  });

  it('a first-in-section station reveals at its minzoom regardless of ordinal', () => {
    const descs = [desc(LABEL_KIND.STATION_LABEL, { anchorX: 0, ordinal: 7, flags: LABEL_FLAG.FIRST_IN_SECTION })];
    expect(computeRevealZooms(descs, [UNIT_EXT], input())[0]).toBe(14);
  });
});

describe('computeRevealZooms priority (coincident labels)', () => {
  it('a section name reveals; a coincident shot is pushed past the horizon (never)', () => {
    // Same anchor → boxes overlap at every zoom (0 separation), so only the
    // higher-priority section can ever reveal.
    const descs = [desc(LABEL_KIND.SHOT_LABEL, { ordinal: 0 }), desc(LABEL_KIND.SECTION_NAME)];
    const reveal = computeRevealZooms(descs, [UNIT_EXT, UNIT_EXT], input());
    expect(reveal[1]).toBe(10); // section at its minzoom
    expect(reveal[0]).toBe(REVEAL_NEVER); // coincident shot never clears
  });

  it('a first-in-section station beats a coincident ordinary station', () => {
    const descs = [
      desc(LABEL_KIND.STATION_LABEL, { ordinal: 2 }), // ordinary (tier 1, admit 16)
      desc(LABEL_KIND.STATION_LABEL, { ordinal: 7, flags: LABEL_FLAG.FIRST_IN_SECTION }), // admit 14, priority 1
    ];
    const reveal = computeRevealZooms(descs, [UNIT_EXT, UNIT_EXT], input());
    expect(reveal[1]).toBe(14);
    expect(reveal[0]).toBe(REVEAL_NEVER);
  });
});

describe('computeRevealZooms determinism', () => {
  it('is deterministic: identical inputs → identical output', () => {
    const descs = [
      desc(LABEL_KIND.SHOT_LABEL, { anchorX: 0, ordinal: 0, shotGlobal: 0 }),
      desc(LABEL_KIND.SHOT_LABEL, { anchorX: 0, ordinal: 0, shotGlobal: 1 }),
    ];
    const a = Array.from(computeRevealZooms(descs, [UNIT_EXT, UNIT_EXT], input()));
    const b = Array.from(computeRevealZooms(descs, [UNIT_EXT, UNIT_EXT], input()));
    expect(a).toEqual(b);
    // Exactly one of the coincident pair reveals (the lower stable id wins).
    expect(a.filter((r) => r !== REVEAL_NEVER)).toHaveLength(1);
  });

  it('resolves an unchanged neighbourhood identically when far-away membership changes', () => {
    // Determinism across rebuilds: adding a distant label must not perturb the
    // reveal zooms of an existing cluster (padding absorbs edge effects).
    const cluster = [
      desc(LABEL_KIND.SHOT_LABEL, { anchorX: 0, ordinal: 0, shotGlobal: 10 }),
      desc(LABEL_KIND.SHOT_LABEL, { anchorX: 0.000004, ordinal: 4, shotGlobal: 11 }),
      desc(LABEL_KIND.SHOT_LABEL, { anchorX: 0.000008, ordinal: 8, shotGlobal: 12 }),
    ];
    const exts = cluster.map(() => UNIT_EXT);
    const before = Array.from(computeRevealZooms(cluster, exts, input()));
    const withFarLabel = [
      ...cluster,
      desc(LABEL_KIND.SHOT_LABEL, { anchorX: 5, ordinal: 0, shotGlobal: 99 }),
    ];
    const after = Array.from(
      computeRevealZooms(withFarLabel, [...exts, UNIT_EXT], input()),
    );
    expect(after.slice(0, 3)).toEqual(before);
  });
});

describe('computeRevealZooms candidate filter (matches the shader gates)', () => {
  it('a toggled-off kind is NOT contested - it reveals at its floor and blocks nobody', () => {
    // Shot toggled off: it is emitted at its admission floor (so a later toggle-on
    // shows it without a rebuild) and does not cull the coincident section.
    const descs = [desc(LABEL_KIND.SECTION_NAME), desc(LABEL_KIND.SHOT_LABEL, { ordinal: 0 })];
    const kindVisible = [false, true, true, true, true]; // shot off
    const reveal = computeRevealZooms(descs, [UNIT_EXT, UNIT_EXT], input({ kindVisible }));
    expect(reveal[0]).toBe(10); // section still reveals at its minzoom
    expect(reveal[1]).toBe(14); // shot emitted uncontested at its floor
  });

  it('reference/all-reference labels are not contested when both virtual kinds are hidden', () => {
    const descs = [
      desc(LABEL_KIND.SECTION_NAME, { flags: LABEL_FLAG.ALL_REFERENCE }),
      desc(LABEL_KIND.SHOT_LABEL, { flags: LABEL_FLAG.REFERENCE, ordinal: 0 }),
    ];
    const reveal = computeRevealZooms(
      descs,
      [UNIT_EXT, UNIT_EXT],
      input({ hideReference: true, hideJump: true })
    );
    // Both are emitted at their floors (the shader hides them via the uniforms).
    expect(reveal[0]).toBe(10);
    expect(reveal[1]).toBe(14);
  });

  it('each virtual kind stops being a candidate under ITS OWN toggle (#27)', () => {
    // Regression: one shared "virtual" bit meant hiding References also
    // suppressed a Jump's label (and vice versa), while the line shader kept
    // drawing that jump - text over collapsed geometry, or text missing from a
    // drawn leg. Contest each against a plain shot label at the same anchor:
    // a non-candidate yields its floor, a candidate loses and reveals later.
    const jump = desc(LABEL_KIND.SHOT_LABEL, { flags: LABEL_FLAG.JUMP, ordinal: 1 });
    const winner = desc(LABEL_KIND.SHOT_LABEL, { ordinal: 0 });

    // Jumps hidden -> non-candidate: emitted uncontested at its decimation
    // floor. References hidden -> still a candidate: it contests the shared
    // anchor, loses to the lower ordinal, and so reveals strictly later.
    const jumpsHidden = computeRevealZooms(
      [winner, jump],
      [UNIT_EXT, UNIT_EXT],
      input({ hideJump: true })
    );
    const refsHidden = computeRevealZooms(
      [winner, jump],
      [UNIT_EXT, UNIT_EXT],
      input({ hideReference: true })
    );
    expect(jumpsHidden[1]).toBeLessThan(refsHidden[1]!);
    // And hiding references leaves it exactly where hiding nothing does.
    const nothingHidden = computeRevealZooms([winner, jump], [UNIT_EXT, UNIT_EXT], input());
    expect(refsHidden[1]).toBe(nothingHidden[1]);
  });

  it('a station label follows the kinds that touch ITS station (#27)', () => {
    // Station labels carry a bit PAIR, not one flag: a station can be touched by
    // both virtual kinds, so it stays a candidate while either of its own kinds
    // is drawn - and stops being one as soon as all of them are hidden. A single
    // both-off rule left a reference-only station's label drawn over a marker
    // the circle shader had already collapsed.
    const winner = desc(LABEL_KIND.SHOT_LABEL, { ordinal: 0 });
    const refOnly = desc(LABEL_KIND.SHOT_LABEL, { flags: LABEL_FLAG.ALL_REFERENCE, ordinal: 1 });
    const both = desc(LABEL_KIND.SHOT_LABEL, {
      flags: LABEL_FLAG.ALL_REFERENCE | LABEL_FLAG.ALL_JUMP,
      ordinal: 1,
    });
    const at = (d: typeof refOnly, over: Parameters<typeof input>[0]) =>
      computeRevealZooms([winner, d], [UNIT_EXT, UNIT_EXT], input(over))[1];

    const contested = at(refOnly, {});
    // References hidden -> a reference-ONLY station's label drops out.
    expect(at(refOnly, { hideReference: true })).toBeLessThan(contested!);
    // Jumps hidden is irrelevant to it - no jump touches that station.
    expect(at(refOnly, { hideJump: true })).toBe(contested);
    // A station touched by BOTH needs both hidden before its label goes.
    expect(at(both, { hideReference: true })).toBe(contested);
    expect(at(both, { hideJump: true })).toBe(contested);
    expect(at(both, { hideReference: true, hideJump: true })).toBeLessThan(contested!);
  });

  it('an empty (whitespace) label never reveals - no ink, never emitted', () => {
    const descs = [desc(LABEL_KIND.SECTION_NAME)];
    expect(computeRevealZooms(descs, [null], input())[0]).toBe(REVEAL_NEVER);
  });
});

describe('computeRevealZooms comment overlap-exempt (C1)', () => {
  it('exempt comment kinds reveal uncontested at their minzoom, blocking nobody', () => {
    // A shot comment coincident (0 separation) with a section name is normally
    // culled - lowest priority, always overlapping. Overlap-exempt: it reveals at
    // its floor (comment minzoom 14) and the section is unaffected.
    const descs = [desc(LABEL_KIND.SECTION_NAME), desc(LABEL_KIND.SHOT_COMMENT)];
    const culled = computeRevealZooms(descs, [UNIT_EXT, UNIT_EXT], input());
    expect(culled[1]).toBe(REVEAL_NEVER); // normally culled by the coincident section
    const exempt = computeRevealZooms(
      descs, [UNIT_EXT, UNIT_EXT], input({ commentOverlapExempt: true }),
    );
    expect(exempt[0]).toBe(10); // section unchanged (its minzoom)
    expect(exempt[1]).toBe(14); // comment now revealed uncontested at its floor
  });

  it('exempts ONLY comment kinds - a coincident non-comment label still collides', () => {
    // A shot LABEL coincident with a section name stays culled even with the
    // exemption on: the flag must not touch non-comment kinds.
    const descs = [desc(LABEL_KIND.SECTION_NAME), desc(LABEL_KIND.SHOT_LABEL, { ordinal: 0 })];
    const reveal = computeRevealZooms(
      descs, [UNIT_EXT, UNIT_EXT], input({ commentOverlapExempt: true }),
    );
    expect(reveal[0]).toBe(10);
    expect(reveal[1]).toBe(REVEAL_NEVER); // non-comment: unchanged, still culled
  });

  it('both comment kinds reveal even when coincident with each other', () => {
    const descs = [desc(LABEL_KIND.STATION_COMMENT), desc(LABEL_KIND.SHOT_COMMENT)];
    const reveal = computeRevealZooms(
      descs, [UNIT_EXT, UNIT_EXT], input({ commentOverlapExempt: true }),
    );
    expect(reveal[0]).toBe(14);
    expect(reveal[1]).toBe(14);
  });
});

describe('emitSlackFor (adaptive emit horizon)', () => {
  it('returns the bounded EMIT_SLACK when the candidate set exceeds the threshold', () => {
    expect(emitSlackFor(ADAPTIVE_EMIT_MAX + 1, 15)).toBe(EMIT_SLACK);
  });

  it('runs the ladder to the ceiling when the set is at/below the threshold', () => {
    expect(emitSlackFor(ADAPTIVE_EMIT_MAX, 15)).toBe(REVEAL_CEILING - 15);
    expect(emitSlackFor(10, 17)).toBe(REVEAL_CEILING - 17);
  });

  it('never returns below EMIT_SLACK, even at/above the ceiling zoom', () => {
    expect(emitSlackFor(10, REVEAL_CEILING)).toBe(EMIT_SLACK);
    expect(emitSlackFor(10, REVEAL_CEILING + 3)).toBe(EMIT_SLACK);
  });

  it('threshold override forces either branch (used by the controller tests)', () => {
    expect(emitSlackFor(5, 15, 0)).toBe(EMIT_SLACK); // 5 > 0 → bounded
    expect(emitSlackFor(5, 15, 10)).toBe(REVEAL_CEILING - 15); // 5 <= 10 → adaptive
  });
});

describe('computeRevealZooms adaptive ceiling emits high-reveal labels', () => {
  it('a band-edge label past the bounded horizon is emitted when the ladder runs to the ceiling', () => {
    // Built at z15. An isolated odd (tier 2) shot admits at z18 - past the bounded
    // horizon 17 (15 + EMIT_SLACK) but within the adaptive ladder. Bounded →
    // REVEAL_NEVER (dropped, appears only after a rebuild); adaptive → reveal 18
    // (emitted, admitted per-frame by the shader as the live zoom reaches 18).
    const descs = [desc(LABEL_KIND.SHOT_LABEL, { ordinal: 1, anchorX: 0 })];
    const bounded = computeRevealZooms(descs, [UNIT_EXT], input({ buildZoom: 15, slack: EMIT_SLACK }));
    expect(bounded[0]).toBe(REVEAL_NEVER);
    const adaptive = computeRevealZooms(descs, [UNIT_EXT], input({ buildZoom: 15, slack: emitSlackFor(1, 15) }));
    expect(adaptive[0]).toBe(18);
  });
});

describe('computeRevealZooms emit horizon (slack)', () => {
  it('drops a non-candidate whose floor is past buildZoom + slack (not emitted)', () => {
    // buildZoom 14, slack 1 → horizon 15. An odd (tier 2, admit 18) toggled-off
    // shot is past the horizon → REVEAL_NEVER.
    const descs = [desc(LABEL_KIND.SHOT_LABEL, { ordinal: 1 })];
    const reveal = computeRevealZooms(
      descs,
      [UNIT_EXT],
      input({ buildZoom: 14, slack: 1, kindVisible: [false, false, false, false, false] }),
    );
    expect(reveal[0]).toBe(REVEAL_NEVER);
  });
});

// ─── Monotonicity property (the headline correctness guarantee) ───────────────

/** Deterministic LCG so the randomized property test is reproducible. */
function lcg(seed: number): () => number {
  let s = seed >>> 0;
  return () => {
    s = (Math.imul(s, 1664525) + 1013904223) >>> 0;
    return s / 0xffffffff;
  };
}

function boxAt(d: LabelDescriptor, ext: ExtentEm, z: number, buildZoom: number, wpsBuild: number): ScreenBox {
  const wps = wpsBuild * 2 ** (z - buildZoom);
  const style = LABEL_KIND_STYLE[d.kind];
  const size = interpolateStops(z, style.size);
  return revealScreenBox(d, ext, wps, size, style.haloWidth);
}

function boxesOverlap(a: ScreenBox, b: ScreenBox): boolean {
  return a.minX < b.maxX && a.maxX > b.minX && a.minY < b.maxY && a.maxY > b.minY;
}

describe('computeRevealZooms MONOTONICITY (randomized zoom sweep)', () => {
  it('for a fixed build the visible set only GROWS with zoom and never overlaps', () => {
    const rnd = lcg(0xc0ffee);
    const kinds: LabelKind[] = [
      LABEL_KIND.SECTION_NAME,
      LABEL_KIND.STATION_LABEL,
      LABEL_KIND.SHOT_LABEL,
      LABEL_KIND.STATION_COMMENT,
    ];
    const descs: LabelDescriptor[] = [];
    const exts: ExtentEm[] = [];
    for (let i = 0; i < 120; i++) {
      const kind = kinds[Math.floor(rnd() * kinds.length)]!;
      const over: Partial<LabelDescriptor> = {
        // A tight cluster (small mercator span) so there is REAL collision at low
        // zoom that clears as the camera zooms in.
        anchorX: rnd() * 0.0008,
        anchorY: rnd() * 0.0008,
        ordinal: Math.floor(rnd() * 40),
        flags: rnd() < 0.15 ? LABEL_FLAG.FIRST_IN_SECTION : 0,
        sectionId: i,
      };
      // A distinct stable id per label (viewport-independent), set on the field
      // that matches the kind - avoids passing `undefined` under exactOptionalPropertyTypes.
      if (kind === LABEL_KIND.SHOT_LABEL) over.shotGlobal = i;
      else if (kind === LABEL_KIND.STATION_LABEL) over.stationGlobal = i;
      descs.push(desc(kind, over));
      exts.push(UNIT_EXT);
    }
    // buildZoom high + generous slack so the ladder computes reveals across the
    // whole sweep range (10..22), and a small wps forces low-zoom collision.
    const buildZoom = 22;
    const wps = 4e6;
    const reveal = computeRevealZooms(descs, exts, input({ buildZoom, worldPixelScale: wps, slack: 2 }));

    let prevVisible = new Set<number>();
    for (let z = 10; z <= 22 + 1e-9; z += 0.5) {
      const visible: number[] = [];
      for (let i = 0; i < descs.length; i++) {
        if (reveal[i]! !== REVEAL_NEVER && reveal[i]! <= z + 1e-9) visible.push(i);
      }
      // (a) MONOTONE: every label visible at a lower zoom stays visible.
      for (const i of prevVisible) expect(visible).toContain(i);
      // (b) NO OVERLAP among the visible set at this exact zoom.
      const boxes = visible.map((i) => boxAt(descs[i]!, exts[i]!, z, buildZoom, wps));
      for (let a = 0; a < boxes.length; a++) {
        for (let b = a + 1; b < boxes.length; b++) {
          expect(boxesOverlap(boxes[a]!, boxes[b]!)).toBe(false);
        }
      }
      prevVisible = new Set(visible);
    }
    // The sweep actually admitted labels over time (not all-at-once / none).
    expect(prevVisible.size).toBeGreaterThan(0);
  });
});

// ─── Real line first: jump / reference labels never take a real label's slot ─

describe('labelVirtualRank', () => {
  const rank = (flags: number) => labelVirtualRank(desc(LABEL_KIND.STATION_LABEL, { flags }));

  it('ranks real 0, jump 1, reference 2 from the shot kind bits', () => {
    expect(rank(0)).toBe(VIRTUAL_RANK_REAL);
    expect(rank(LABEL_FLAG.FIRST_IN_SECTION)).toBe(VIRTUAL_RANK_REAL);
    expect(rank(LABEL_FLAG.JUMP)).toBe(VIRTUAL_RANK_JUMP);
    expect(rank(LABEL_FLAG.REFERENCE)).toBe(VIRTUAL_RANK_REFERENCE);
  });

  it('ranks an all-virtual station by the better kind touching it', () => {
    expect(rank(LABEL_FLAG.ALL_JUMP)).toBe(VIRTUAL_RANK_JUMP);
    expect(rank(LABEL_FLAG.ALL_REFERENCE)).toBe(VIRTUAL_RANK_REFERENCE);
    // On both a jump and a reference (or on no leg at all): a jump label.
    expect(rank(LABEL_FLAG.ALL_JUMP | LABEL_FLAG.ALL_REFERENCE)).toBe(VIRTUAL_RANK_JUMP);
  });
});

describe('computeRevealZooms real line first (coincident labels)', () => {
  it('a real shot label wins over a coincident jump or reference one from a coarser tier', () => {
    // Before: the virtual label (ordinal 0, tier 0, admitted at z14) placed first
    // and kept the slot, so the real one (ordinal 1, admitted at z18) never showed.
    for (const virtual of [LABEL_FLAG.JUMP, LABEL_FLAG.REFERENCE]) {
      const descs = [
        desc(LABEL_KIND.SHOT_LABEL, { ordinal: 0, flags: virtual, shotGlobal: 0 }),
        desc(LABEL_KIND.SHOT_LABEL, { ordinal: 1, shotGlobal: 1 }),
      ];
      const reveal = computeRevealZooms(descs, [UNIT_EXT, UNIT_EXT], input());
      expect(reveal[1]).toBe(18);
      expect(reveal[0]).toBe(REVEAL_NEVER);
    }
  });

  it('a real station label wins over a coincident all-jump or all-reference station', () => {
    for (const virtual of [LABEL_FLAG.ALL_JUMP, LABEL_FLAG.ALL_REFERENCE]) {
      const descs = [
        desc(LABEL_KIND.STATION_LABEL, { ordinal: 0, flags: virtual, stationGlobal: 0 }),
        desc(LABEL_KIND.STATION_LABEL, { ordinal: 1, stationGlobal: 1 }),
      ];
      const reveal = computeRevealZooms(descs, [UNIT_EXT, UNIT_EXT], input());
      expect(reveal[1]).toBe(18);
      expect(reveal[0]).toBe(REVEAL_NEVER);
    }
  });

  it('an all-reference section name yields to a coincident real station label', () => {
    // Kind priority alone would put the section name (priority 0, z10) first.
    const descs = [
      desc(LABEL_KIND.SECTION_NAME, { flags: LABEL_FLAG.ALL_REFERENCE, sectionId: 5 }),
      desc(LABEL_KIND.STATION_LABEL, { ordinal: 3, flags: LABEL_FLAG.FIRST_IN_SECTION, stationGlobal: 3 }),
    ];
    const reveal = computeRevealZooms(descs, [UNIT_EXT, UNIT_EXT], input());
    expect(reveal[1]).toBe(14);
    expect(reveal[0]).toBe(REVEAL_NEVER);
  });

  it('a real section name still beats a coincident real shot (kind priority inside a rank)', () => {
    const descs = [desc(LABEL_KIND.SHOT_LABEL, { ordinal: 0 }), desc(LABEL_KIND.SECTION_NAME)];
    const reveal = computeRevealZooms(descs, [UNIT_EXT, UNIT_EXT], input());
    expect(reveal[1]).toBe(10);
    expect(reveal[0]).toBe(REVEAL_NEVER);
  });

  it('a jump label wins over a coincident reference label', () => {
    const descs = [
      desc(LABEL_KIND.SHOT_LABEL, { ordinal: 0, flags: LABEL_FLAG.REFERENCE, shotGlobal: 0 }),
      desc(LABEL_KIND.SHOT_LABEL, { ordinal: 1, flags: LABEL_FLAG.JUMP, shotGlobal: 1 }),
    ];
    const reveal = computeRevealZooms(descs, [UNIT_EXT, UNIT_EXT], input());
    expect(reveal[1]).toBe(18);
    expect(reveal[0]).toBe(REVEAL_NEVER);
  });

  it('a hidden kind frees the slot: the losing virtual label is emitted at its floor', () => {
    // Jumps hidden: the jump label is no candidate (the shader hides it), and
    // the real label is unaffected either way.
    const descs = [
      desc(LABEL_KIND.SHOT_LABEL, { ordinal: 0, flags: LABEL_FLAG.JUMP, shotGlobal: 0 }),
      desc(LABEL_KIND.SHOT_LABEL, { ordinal: 1, shotGlobal: 1 }),
    ];
    const reveal = computeRevealZooms(descs, [UNIT_EXT, UNIT_EXT], input({ hideJump: true }));
    expect(reveal[1]).toBe(18);
    expect(reveal[0]).toBe(14);
  });
});

describe('computeRevealZooms reserves the slots of real labels that reveal later', () => {
  it('a jump label does not take, at z14, a slot a real label needs at z18', () => {
    // Real shot R: tier 2, admitted at z18. Jump J: tier 0, admitted at z14,
    // 10 px to R's right at z18 - an overlap (a shot box is 16 px wide there)
    // that clears at z19 (20 px apart). J alone would show from z14; if it did,
    // R would have to wait for z19, or both would overlap at z18.
    const dx = 10 / 1e6; // 10 px at the build zoom 18 (worldPixelScale 1e6)
    const real = desc(LABEL_KIND.SHOT_LABEL, { anchorX: 0, ordinal: 1, shotGlobal: 1 });
    const jump = desc(LABEL_KIND.SHOT_LABEL, { anchorX: dx, ordinal: 0, flags: LABEL_FLAG.JUMP, shotGlobal: 0 });
    const reveal = computeRevealZooms([real, jump], [UNIT_EXT, UNIT_EXT], input());
    expect(reveal[0]).toBe(18);
    expect(reveal[1]).toBe(19);
    // Never drawn on top of each other once both show.
    const both = Math.max(reveal[0]!, reveal[1]!);
    expect(boxesOverlap(boxAt(real, UNIT_EXT, both, 18, 1e6), boxAt(jump, UNIT_EXT, both, 18, 1e6))).toBe(false);
  });

  it('a jump label that clears the real one by the time it reveals shows from its own floor', () => {
    // 100 px apart at z18 (6 px at z14, where both boxes are 13 px wide): the
    // real label only shows from z18 and they are clear there, so nothing stops
    // the jump label from showing at z14.
    const dx = 100 / 1e6;
    const real = desc(LABEL_KIND.SHOT_LABEL, { anchorX: 0, ordinal: 1, shotGlobal: 1 });
    const jump = desc(LABEL_KIND.SHOT_LABEL, { anchorX: dx, ordinal: 0, flags: LABEL_FLAG.JUMP, shotGlobal: 0 });
    const reveal = computeRevealZooms([real, jump], [UNIT_EXT, UNIT_EXT], input());
    expect(reveal[0]).toBe(18);
    expect(reveal[1]).toBe(14);
  });
});

/** A random tight cluster of every label kind, a share of them virtual. */
function randomMixedLabels(seed: number, count: number): { descs: LabelDescriptor[]; exts: ExtentEm[] } {
  const rnd = lcg(seed);
  const kinds: LabelKind[] = [
    LABEL_KIND.SECTION_NAME,
    LABEL_KIND.STATION_LABEL,
    LABEL_KIND.SHOT_LABEL,
    LABEL_KIND.STATION_COMMENT,
    LABEL_KIND.SHOT_COMMENT,
  ];
  const shotBits = [0, 0, LABEL_FLAG.JUMP, LABEL_FLAG.REFERENCE];
  const stationBits = [0, 0, LABEL_FLAG.ALL_JUMP, LABEL_FLAG.ALL_REFERENCE, LABEL_FLAG.ALL_JUMP | LABEL_FLAG.ALL_REFERENCE];
  const descs: LabelDescriptor[] = [];
  const exts: ExtentEm[] = [];
  for (let i = 0; i < count; i++) {
    const kind = kinds[Math.floor(rnd() * kinds.length)]!;
    const onShot = kind === LABEL_KIND.SHOT_LABEL || kind === LABEL_KIND.SHOT_COMMENT;
    const bits = onShot ? shotBits : stationBits;
    let flags = bits[Math.floor(rnd() * bits.length)]!;
    if (kind === LABEL_KIND.STATION_LABEL && rnd() < 0.15) flags |= LABEL_FLAG.FIRST_IN_SECTION;
    const over: Partial<LabelDescriptor> = {
      anchorX: rnd() * 0.0008,
      anchorY: rnd() * 0.0008,
      ordinal: kind === LABEL_KIND.SHOT_LABEL || kind === LABEL_KIND.STATION_LABEL ? Math.floor(rnd() * 40) : 0,
      flags,
      sectionId: i,
    };
    if (onShot) over.shotGlobal = i;
    else if (kind !== LABEL_KIND.SECTION_NAME) over.stationGlobal = i;
    descs.push(desc(kind, over));
    exts.push({ minX: 0, maxX: 1 + rnd() * 3, minY: 0, maxY: 1 });
  }
  return { descs, exts };
}

const HIDE_CASES: { hideJump: boolean; hideReference: boolean }[] = [
  { hideJump: false, hideReference: false },
  { hideJump: true, hideReference: false },
  { hideJump: false, hideReference: true },
  { hideJump: true, hideReference: true },
];

describe('computeRevealZooms real line first (randomized properties)', () => {
  it('real labels reveal exactly as if no virtual label existed; jump labels as if no reference one did', () => {
    for (const seed of [1, 2, 3, 0xbeef]) {
      const { descs, exts } = randomMixedLabels(seed, 150);
      for (const hide of HIDE_CASES) {
        const opts = input({ buildZoom: 22, worldPixelScale: 4e6, slack: 2, ...hide });
        const full = computeRevealZooms(descs, exts, opts);
        for (const maxRank of [VIRTUAL_RANK_REAL, VIRTUAL_RANK_JUMP]) {
          const keep = descs.map((_, i) => i).filter((i) => labelVirtualRank(descs[i]!) <= maxRank);
          const alone = computeRevealZooms(
            keep.map((i) => descs[i]!),
            keep.map((i) => exts[i]!),
            opts,
          );
          expect(keep.map((i) => full[i])).toEqual(Array.from(alone));
        }
      }
    }
  });

  it('with virtual labels mixed in, the visible set still only GROWS with zoom and never overlaps', () => {
    const { descs, exts } = randomMixedLabels(0xc0ffee, 150);
    const buildZoom = 22;
    const wps = 4e6;
    for (const hide of HIDE_CASES) {
      const reveal = computeRevealZooms(descs, exts, input({ buildZoom, worldPixelScale: wps, slack: 2, ...hide }));
      // Hidden kinds are emitted uncontested and the shader hides them, so only
      // labels the shader draws take part in the overlap check.
      const drawn = (d: LabelDescriptor): boolean => {
        if (hide.hideJump && (d.flags & LABEL_FLAG.JUMP) !== 0) return false;
        if (hide.hideReference && (d.flags & LABEL_FLAG.REFERENCE) !== 0) return false;
        const allJump = (d.flags & LABEL_FLAG.ALL_JUMP) !== 0;
        const allRef = (d.flags & LABEL_FLAG.ALL_REFERENCE) !== 0;
        return !(allJump || allRef) || (allJump && !hide.hideJump) || (allRef && !hide.hideReference);
      };
      let prevVisible = new Set<number>();
      let sawVirtual = false;
      for (let z = 10; z <= 22 + 1e-9; z += 0.5) {
        const visible: number[] = [];
        for (let i = 0; i < descs.length; i++) {
          if (drawn(descs[i]!) && reveal[i]! !== REVEAL_NEVER && reveal[i]! <= z + 1e-9) visible.push(i);
        }
        for (const i of prevVisible) expect(visible).toContain(i);
        const boxes = visible.map((i) => boxAt(descs[i]!, exts[i]!, z, buildZoom, wps));
        for (let a = 0; a < boxes.length; a++) {
          for (let b = a + 1; b < boxes.length; b++) {
            expect(boxesOverlap(boxes[a]!, boxes[b]!)).toBe(false);
          }
        }
        if (visible.some((i) => labelVirtualRank(descs[i]!) !== VIRTUAL_RANK_REAL)) sawVirtual = true;
        prevVisible = new Set(visible);
      }
      expect(prevVisible.size).toBeGreaterThan(0);
      // The fixture really exercises the virtual passes: some virtual label
      // shows whenever a virtual kind is drawn at all.
      expect(sawVirtual).toBe(!(hide.hideJump && hide.hideReference));
    }
  });
});

describe('computeRevealZooms reserved labels (section-only build below the label band)', () => {
  const reservedStation = (flags = 0) =>
    desc(LABEL_KIND.STATION_LABEL, { ordinal: 4, flags, stationGlobal: 4 });
  const sectionOnly = (names: LabelDescriptor[], reserved: LabelDescriptor[]) =>
    computeRevealZooms(
      names,
      names.map(() => UNIT_EXT),
      input({ buildZoom: 12, reserved: { descriptors: reserved, extents: reserved.map(() => UNIT_EXT) } }),
    );

  it('a virtual section name leaves free the slot of a real station label built only from z13.5', () => {
    const jumpName = desc(LABEL_KIND.SECTION_NAME, { flags: LABEL_FLAG.ALL_JUMP, sectionId: 2 });
    // Not reserved: the name shows from z10 (and would vanish at the full build).
    expect(sectionOnly([jumpName], [])[0]).toBe(10);
    // Reserved: hidden here, as it is in the full build.
    expect(sectionOnly([jumpName], [reservedStation()])[0]).toBe(REVEAL_NEVER);
    expect(computeRevealZooms([jumpName, reservedStation()], [UNIT_EXT, UNIT_EXT], input({ buildZoom: 14 }))[0])
      .toBe(REVEAL_NEVER);
  });

  it('a reservation blocks only worse-rank labels, and only while its kind is shown', () => {
    const realName = desc(LABEL_KIND.SECTION_NAME, { sectionId: 1 });
    const jumpName = desc(LABEL_KIND.SECTION_NAME, { flags: LABEL_FLAG.ALL_JUMP, sectionId: 2 });
    const refName = desc(LABEL_KIND.SECTION_NAME, { flags: LABEL_FLAG.ALL_REFERENCE, sectionId: 3 });
    // Real names never yield to a reserved station label (a full build places them first).
    expect(sectionOnly([realName], [reservedStation()])[0]).toBe(10);
    // A jump station label outranks a reference name, not a jump one.
    expect(sectionOnly([refName], [reservedStation(LABEL_FLAG.ALL_JUMP)])[0]).toBe(REVEAL_NEVER);
    expect(sectionOnly([jumpName], [reservedStation(LABEL_FLAG.ALL_JUMP)])[0]).toBe(10);
    // Station labels toggled off: nothing to leave free.
    const off = computeRevealZooms([jumpName], [UNIT_EXT], input({
      buildZoom: 12,
      kindVisible: [true, false, true, true, true],
      reserved: { descriptors: [reservedStation()], extents: [UNIT_EXT] },
    }));
    expect(off[0]).toBe(10);
  });

  it('a name held back by a reservation keeps its slot, so no other name shows only to vanish', () => {
    // Q (first-in-section) and R share an anchor; R's box is wider, so R never
    // clears Q and a full build never shows it. V2 overlaps R but not Q; V1
    // overlaps V2 but neither station. Full build: V2 shows (R never does) and
    // V1 never clears V2. Below the band R is only a reservation, so V2 is held
    // back - and without keeping its slot V1 would show there, then vanish.
    const ext = (minX: number, maxX: number, minY: number, maxY: number): ExtentEm => ({ minX, maxX, minY, maxY });
    const q = desc(LABEL_KIND.STATION_LABEL, { ordinal: 0, flags: LABEL_FLAG.FIRST_IN_SECTION, stationGlobal: 0 });
    const r = desc(LABEL_KIND.STATION_LABEL, { ordinal: 4, stationGlobal: 4 });
    const v2 = desc(LABEL_KIND.SECTION_NAME, { flags: LABEL_FLAG.ALL_JUMP, sectionId: 1 });
    const v1 = desc(LABEL_KIND.SECTION_NAME, { flags: LABEL_FLAG.ALL_JUMP, sectionId: 2 });
    const exts = { q: ext(0, 1, 0, 1), r: ext(0, 6, 0, 1), v2: ext(4, 5, 0.5, 1.7), v1: ext(4.5, 5.5, 1.2, 2.2) };

    const full = computeRevealZooms([v2, v1, q, r], [exts.v2, exts.v1, exts.q, exts.r], input({ buildZoom: 14 }));
    expect(Array.from(full)).toEqual([10, REVEAL_NEVER, 14, REVEAL_NEVER]);

    const below = computeRevealZooms([v2, v1], [exts.v2, exts.v1], input({
      buildZoom: 12,
      reserved: { descriptors: [q, r], extents: [exts.q, exts.r] },
    }));
    // Both hidden below the band: V2 appears at the full build, V1 never shows.
    expect(Array.from(below)).toEqual([REVEAL_NEVER, REVEAL_NEVER]);
  });

  it('a name shown below the band is still shown by the full build (randomized)', () => {
    // Names spread across the screen, each with real and virtual station / shot
    // labels clustered around it - the shape a jump section tied in at a real
    // station has. Real pixel scale: 512 px per world unit at z0.
    const wps12 = 512 * 2 ** 12;
    let virtualShownBelow = 0;
    let virtualHeldBack = 0;
    for (const seed of [11, 12, 13, 14, 0xfeed]) {
      const rnd = lcg(seed);
      const names: LabelDescriptor[] = [];
      const others: LabelDescriptor[] = [];
      const nameBits = [0, LABEL_FLAG.ALL_JUMP, LABEL_FLAG.ALL_REFERENCE];
      const shotBits = [0, 0, LABEL_FLAG.JUMP, LABEL_FLAG.REFERENCE];
      const stationBits = [0, 0, LABEL_FLAG.ALL_JUMP, LABEL_FLAG.ALL_REFERENCE];
      let id = 0;
      for (let i = 0; i < 20; i++) {
        // ~0..900 px apart at z12; cluster members within ~40 px at z14.
        const ax = (rnd() * 900) / wps12;
        const ay = (rnd() * 600) / wps12;
        names.push(desc(LABEL_KIND.SECTION_NAME, { anchorX: ax, anchorY: ay, flags: nameBits[i % 3]!, sectionId: i }));
        const members = 1 + Math.floor(rnd() * 4);
        for (let k = 0; k < members; k++) {
          const shot = rnd() < 0.5;
          const bits = shot ? shotBits : stationBits;
          const n = id++;
          others.push(desc(shot ? LABEL_KIND.SHOT_LABEL : LABEL_KIND.STATION_LABEL, {
            anchorX: ax + ((rnd() - 0.5) * 80) / (wps12 * 4),
            anchorY: ay + ((rnd() - 0.5) * 80) / (wps12 * 4),
            ordinal: n,
            flags: bits[Math.floor(rnd() * bits.length)]!,
            ...(shot ? { shotGlobal: n } : { stationGlobal: n }),
          }));
        }
      }
      const nameExt = names.map(() => ({ minX: -2, maxX: 2, minY: 0.5, maxY: 1.5 }));
      const otherExt = others.map(() => ({ minX: 0.3, maxX: 3, minY: -0.5, maxY: 0.5 }));
      const common = { worldPixelScale: wps12, slack: 22 - 12 };
      const below = computeRevealZooms(names, nameExt, input({
        buildZoom: 12, ...common, reserved: { descriptors: others, extents: otherExt },
      }));
      const full = computeRevealZooms([...names, ...others], [...nameExt, ...otherExt], input({
        buildZoom: 12, ...common,
      }));
      for (let i = 0; i < names.length; i++) {
        const isVirtual = labelVirtualRank(names[i]!) !== VIRTUAL_RANK_REAL;
        if (below[i]! > 13.5) {
          if (isVirtual && full[i]! > 13.5) virtualHeldBack++;
          continue; // never drawn below the band
        }
        if (isVirtual) virtualShownBelow++;
        expect(full[i]!).toBeLessThanOrEqual(below[i]!);
      }
    }
    // The fixture exercises both outcomes for virtual names.
    expect(virtualShownBelow).toBeGreaterThan(0);
    expect(virtualHeldBack).toBeGreaterThan(0);
  });
});

/** Em extents shaped like `labelLayout.ts` places each kind: a shot label and a
 *  comment hang below their anchor, a station label sits up-right of it, a
 *  section name above it - so boxes are OFFSET from their anchors. */
function layoutExtent(kind: LabelKind, widthEm: number): ExtentEm {
  switch (kind) {
    case LABEL_KIND.SHOT_LABEL:
      return { minX: -widthEm / 2, maxX: widthEm / 2, minY: -1.5, maxY: -0.5 };
    case LABEL_KIND.STATION_LABEL:
      return { minX: 0.8, maxX: 0.8 + widthEm, minY: 0.8, maxY: 1.8 };
    case LABEL_KIND.SECTION_NAME:
      return { minX: -widthEm / 2, maxX: widthEm / 2, minY: 1.2, maxY: 2.2 };
    default:
      return { minX: -widthEm / 2, maxX: widthEm / 2, minY: -2.5, maxY: -1.5 };
  }
}

describe('computeRevealZooms with boxes offset from their anchors', () => {
  it('a far-away label does not change whether a jump name may take a real label\'s slot', () => {
    // The jump name's box sits above its anchor; the real shot label, 10 px (at
    // z14) north, hangs below its own - the two boxes face each other. Clear at
    // z10, they overlap around z15-16 and clear again above. Unrelated real
    // labels far away must not change that (a one-level overlap test did).
    const jump = desc(LABEL_KIND.SECTION_NAME, { flags: LABEL_FLAG.ALL_JUMP, sectionId: 9 });
    const real = desc(LABEL_KIND.SHOT_LABEL, { anchorY: -10, ordinal: 0, shotGlobal: 1 });
    const far = (i: number) => desc(LABEL_KIND.SHOT_LABEL, { anchorX: 5000 + i * 500, ordinal: 0, shotGlobal: 10 + i });
    const ext = { jump: layoutExtent(LABEL_KIND.SECTION_NAME, 4), shot: layoutExtent(LABEL_KIND.SHOT_LABEL, 4) };
    const opts = input({ buildZoom: 14, worldPixelScale: 1, slack: 8 });
    const alone = computeRevealZooms([jump, real], [ext.jump, ext.shot], opts);
    const withFar = computeRevealZooms(
      [jump, real, far(0), far(1)],
      [ext.jump, ext.shot, ext.shot, ext.shot],
      opts,
    );
    expect(withFar[0]).toBe(alone[0]);
    expect(alone[1]).toBe(14);
    // And the two never overlap once both are drawn.
    for (let z = Math.max(alone[0]!, alone[1]!); z <= 22 + 1e-9; z += 0.5) {
      expect(boxesOverlap(boxAt(jump, ext.jump, z, 14, 1), boxAt(real, ext.shot, z, 14, 1))).toBe(false);
    }
  });

  it('every box at a higher zoom stays inside the anchor hull at a lower one (the filter premise)', () => {
    // Text grows far slower than the map (labelStyle stops), so in map units a
    // box only moves towards its anchor as the zoom rises.
    const kinds: LabelKind[] = [0, 1, 2, 3, 4] as LabelKind[];
    for (const kind of kinds) {
      const d = desc(kind, { anchorX: 3, anchorY: -7 });
      const ext = layoutExtent(kind, 6);
      for (let lo = 10; lo <= 22; lo += 0.5) {
        const wpsLo = 2 ** (lo - 14);
        const hull = anchorHull(d, boxAt(d, ext, lo, 14, 1), wpsLo);
        for (let hi = lo; hi <= 22; hi += 0.5) {
          const b = boxAt(d, ext, hi, 14, 1);
          const k = wpsLo / 2 ** (hi - 14); // px at `hi` -> px at `lo`
          expect(b.minX * k).toBeGreaterThanOrEqual(hull.minX - 1e-9);
          expect(b.maxX * k).toBeLessThanOrEqual(hull.maxX + 1e-9);
          expect(b.minY * k).toBeGreaterThanOrEqual(hull.minY - 1e-9);
          expect(b.maxY * k).toBeLessThanOrEqual(hull.maxY + 1e-9);
        }
      }
    }
  });

  it('labels of different ranks never overlap at any zoom, nor do real labels depend on virtual ones (randomized)', () => {
    for (const seed of [21, 22, 23, 0xabc]) {
      const rnd = lcg(seed);
      const kinds: LabelKind[] = [
        LABEL_KIND.SECTION_NAME, LABEL_KIND.STATION_LABEL, LABEL_KIND.SHOT_LABEL,
        LABEL_KIND.STATION_COMMENT, LABEL_KIND.SHOT_COMMENT,
      ];
      const shotBits = [0, 0, LABEL_FLAG.JUMP, LABEL_FLAG.REFERENCE];
      const stationBits = [0, 0, LABEL_FLAG.ALL_JUMP, LABEL_FLAG.ALL_REFERENCE];
      const descs: LabelDescriptor[] = [];
      const exts: ExtentEm[] = [];
      for (let i = 0; i < 140; i++) {
        const kind = kinds[Math.floor(rnd() * kinds.length)]!;
        const onShot = kind === LABEL_KIND.SHOT_LABEL || kind === LABEL_KIND.SHOT_COMMENT;
        const bits = onShot ? shotBits : stationBits;
        const over: Partial<LabelDescriptor> = {
          anchorX: rnd() * 0.0008,
          anchorY: rnd() * 0.0008,
          ordinal: kind === LABEL_KIND.SHOT_LABEL || kind === LABEL_KIND.STATION_LABEL ? Math.floor(rnd() * 40) : 0,
          flags: bits[Math.floor(rnd() * bits.length)]!,
          sectionId: i,
        };
        if (onShot) over.shotGlobal = i;
        else if (kind !== LABEL_KIND.SECTION_NAME) over.stationGlobal = i;
        descs.push(desc(kind, over));
        exts.push(layoutExtent(kind, 1 + rnd() * 5));
      }
      const buildZoom = 22;
      const wps = 4e6;
      const opts = input({ buildZoom, worldPixelScale: wps, slack: 2 });
      const reveal = computeRevealZooms(descs, exts, opts);
      const realOnly = descs.map((_, i) => i).filter((i) => labelVirtualRank(descs[i]!) === VIRTUAL_RANK_REAL);
      const alone = computeRevealZooms(realOnly.map((i) => descs[i]!), realOnly.map((i) => exts[i]!), opts);
      expect(realOnly.map((i) => reveal[i])).toEqual(Array.from(alone));
      let crossPairs = 0;
      let prevVisible = new Set<number>();
      for (let z = 10; z <= 22 + 1e-9; z += 0.5) {
        const visible: number[] = [];
        for (let i = 0; i < descs.length; i++) if (reveal[i]! <= z + 1e-9) visible.push(i);
        for (const i of prevVisible) expect(visible).toContain(i);
        for (let a = 0; a < visible.length; a++) {
          for (let b = a + 1; b < visible.length; b++) {
            const ia = visible[a]!;
            const ib = visible[b]!;
            if (labelVirtualRank(descs[ia]!) === labelVirtualRank(descs[ib]!)) continue;
            crossPairs++;
            const hit = boxesOverlap(boxAt(descs[ia]!, exts[ia]!, z, buildZoom, wps), boxAt(descs[ib]!, exts[ib]!, z, buildZoom, wps));
            expect(hit).toBe(false);
          }
        }
        prevVisible = new Set(visible);
      }
      expect(crossPairs).toBeGreaterThan(0);
    }
  });
});

describe('computeRevealZooms held-back names (section-only build)', () => {
  it('a name held back only by a real label - not a reservation - still shows once clear', () => {
    // A real section name holds the jump name back until they clear; an
    // unrelated reservation far away must not turn that into "hidden".
    const realName = desc(LABEL_KIND.SECTION_NAME, { sectionId: 1 });
    const jumpName = desc(LABEL_KIND.SECTION_NAME, { anchorX: 200, flags: LABEL_FLAG.ALL_JUMP, sectionId: 2 });
    const farStation = desc(LABEL_KIND.STATION_LABEL, { anchorX: 90000, ordinal: 4, stationGlobal: 4 });
    const ext = layoutExtent(LABEL_KIND.SECTION_NAME, 4);
    const run = (reserved: LabelDescriptor[]) =>
      computeRevealZooms([realName, jumpName], [ext, ext], input({
        buildZoom: 12, worldPixelScale: 0.25, slack: 10,
        reserved: { descriptors: reserved, extents: reserved.map(() => layoutExtent(LABEL_KIND.STATION_LABEL, 1)) },
      }));
    const without = run([]);
    expect(without[1]).toBeGreaterThan(10); // really held back by the real name
    expect(without[1]).toBeLessThan(13.5);
    expect(run([farStation])[1]).toBe(without[1]);
  });
});
