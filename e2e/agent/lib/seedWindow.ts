/**
 * [#576 fuzz farm] The seeds a seeded vitest property suite runs: its own fixed
 * range by default, or a window the knobs move, so a long run can roll it over fresh
 * seeds. `STALAGMIGHT_VITEST_SEED_FIRST` moves each loop's range to start at the knob
 * plus the loop's own offset (`first - 1`), so loops that run disjoint ranges by
 * default stay disjoint while the count fits the gap between them ([#576 RVF-c-M2]);
 * `STALAGMIGHT_VITEST_SEED_COUNT` is how many. Either knob alone moves only its half.
 * The same knobs, read the same way, live in `src/test/seedWindow.ts` for the suites
 * under `src/`; docs/testing-recipes.md lists every seeded suite's knobs.
 */
export const SEED_FIRST_ENV = 'STALAGMIGHT_VITEST_SEED_FIRST';
export const SEED_COUNT_ENV = 'STALAGMIGHT_VITEST_SEED_COUNT';

function envInt(name: string, min: number): number | undefined {
  const raw = process.env[name];
  if (raw === undefined || raw === '') return undefined;
  const n = Number(raw);
  if (!Number.isSafeInteger(n) || n < min) throw new Error(`${name}=${raw}: expected an integer >= ${min}`);
  return n;
}

/** `count` consecutive seeds from `first`, unless the knobs move the window (see above). */
export function seedWindow(first: number, count: number): number[] {
  const knob = envInt(SEED_FIRST_ENV, 0);
  const from = knob === undefined ? first : knob + first - 1;
  const length = envInt(SEED_COUNT_ENV, 1) ?? count;
  return Array.from({ length }, (_unused, index) => from + index);
}
