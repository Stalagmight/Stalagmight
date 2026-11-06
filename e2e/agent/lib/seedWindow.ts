/**
 * [#576 fuzz farm] The seeds a seeded vitest property suite runs: its own fixed
 * range by default, or the window `STALAGMIGHT_VITEST_SEED_FIRST` (the first seed)
 * and `STALAGMIGHT_VITEST_SEED_COUNT` (how many) ask for, so a long run can roll
 * the window over fresh seeds. Either knob alone moves only its half. The same
 * knobs, read the same way, live in `src/test/seedWindow.ts` for the suites under
 * `src/`; docs/testing-recipes.md lists every seeded suite's knobs.
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

/** `count` consecutive seeds from `first`, unless the knobs ask for another window. */
export function seedWindow(first: number, count: number): number[] {
  const from = envInt(SEED_FIRST_ENV, 0) ?? first;
  const length = envInt(SEED_COUNT_ENV, 1) ?? count;
  return Array.from({ length }, (_unused, index) => from + index);
}
