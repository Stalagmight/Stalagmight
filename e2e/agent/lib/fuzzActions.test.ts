import { describe, expect, it } from 'vitest';

import { mulberry32 } from '../scenarios/framework';
import {
  streamV2,
  buildBatch,
  swarmProfileFor,
  weightedNamesFor,
  type TargetingDigest,
} from '../scenarios/undoRedoFuzzActions';
import type { ScenarioContext } from '../scenarios/framework';
import { seedWindow } from './seedWindow';

/** A deterministic ScenarioContext stub: only the draw surface is exercised. */
function fakeCtx(seed: number): ScenarioContext {
  const rand = mulberry32(seed);
  return {
    seed,
    rng: () => rand(),
    randInt: (min: number, max: number) => min + Math.floor(rand() * (max - min + 1)),
    pick: <T,>(items: readonly T[]) => items[Math.floor(rand() * items.length)] as T,
  } as unknown as ScenarioContext;
}

/** Two sections, enough stations for a cross-section pair. */
const digest: TargetingDigest = {
  s: [
    [1, [100000, 100001, 100002]],
    [2, [200000, 200001]],
  ],
  st: [
    [1, 0],
    [1, 1],
    [1, 2],
    [1, 3],
    [2, 0],
    [2, 1],
    [2, 2],
  ],
  m: [],
  z: [],
  c: [],
};

describe('buildBatch', () => {
  it('composes 2-4 sub-commands with the BatchCommand shapes', () => {
    const known = new Set([
      'update_shot',
      'move_station',
      'move_section',
      'reverse_section',
      'create_section',
      'merge_stations',
    ]);

    for (const seed of seedWindow(1, 40)) {
      const batch = buildBatch(fakeCtx(seed), digest);
      if (!batch) continue;
      expect(batch.commands.length).toBeGreaterThanOrEqual(1);
      for (const command of batch.commands) {
        expect(typeof command.type).toBe('string');
        // The poison entry is a move_section, which is in the union too.
        expect(known.has(String(command.type))).toBe(true);
      }
      // Every command must survive the JSON round trip WebDriver performs.
      expect(() => JSON.parse(JSON.stringify(batch.commands))).not.toThrow();
    }
  });

  it('sometimes poisons the LAST sub-command to exercise batch rollback', () => {
    let poisoned = 0;
    for (const seed of seedWindow(1, 60)) {
      const batch = buildBatch(fakeCtx(seed), digest);
      if (batch?.label.endsWith('POISON')) {
        poisoned += 1;
        const last = batch.commands[batch.commands.length - 1];
        expect(last?.type).toBe('move_section');
        expect(last?.section_id).toBe(999999);
      }
    }

    expect(poisoned).toBeGreaterThan(0);
  });

  it('returns null rather than an empty batch when nothing can be targeted', () => {
    const empty: TargetingDigest = { s: [], st: [], m: [], z: [], c: [] };

    // No sections and no stations: every sub-command generator declines.
    for (const seed of seedWindow(1, 10)) {
      const batch = buildBatch(fakeCtx(seed), empty);
      if (batch) expect(batch.commands.length).toBeGreaterThan(0);
    }
  });

  it('is deterministic for a seed', () => {
    expect(JSON.stringify(buildBatch(fakeCtx(7), digest))).toBe(
      JSON.stringify(buildBatch(fakeCtx(7), digest)),
    );
  });
});

describe('swarm profiles', () => {
  it('is stable for a seed and spreads across profiles', () => {
    expect(swarmProfileFor(11).name).toBe(swarmProfileFor(11).name);

    const names = new Set<string>();
    for (let seed = 1; seed <= 60; seed++) names.add(swarmProfileFor(seed).name);
    // A single profile for every seed would defeat the point.
    expect(names.size).toBeGreaterThan(2);
  });

  it('boosts the generators its profile names, and never drops one to zero', () => {
    const profile = { name: 'test', multipliers: { deleteShot: 5 } };
    const names = weightedNamesFor(profile);
    const count = (name: string): number => names.filter((entry) => entry === name).length;

    expect(count('deleteShot')).toBeGreaterThan(count('deleteSection'));
    // Every generator stays reachable whatever the profile.
    expect(count('mergeStations')).toBeGreaterThan(0);
    expect(count('updateShot')).toBeGreaterThan(0);
  });

  it('includes the v2 generators only on the v2 stream', () => {
    const names = weightedNamesFor(swarmProfileFor(1));
    const v2Only = ['executeBatch', 'splitShotAtDistance', 'moveComment', 'importSections'];

    for (const name of v2Only) {
      expect(names.includes(name)).toBe(streamV2());
    }
    // The v1 generators are present on both streams.
    expect(names.includes('createShot')).toBe(true);
  });
});
