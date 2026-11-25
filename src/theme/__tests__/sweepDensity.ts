/**
 * How densely the colour sweeps sample (pillColors, prosePillColors,
 * twoTonePills). The normal run samples a deterministic subset that always
 * keeps every hard case (near-white, near-black, yellow, blue, grey, every
 * app palette and CSS name), so the vitest run stays well inside its worker
 * RPC timeout on a slow CI runner: the full sweeps took about 50 s of
 * unbroken CPU and CI's `check` job started failing with
 * `Timeout calling "onTaskUpdate"` although every test passed.
 *
 * Set `STALAGMIGHT_FULL_COLOUR_SWEEP=1` to run every colour and every pair,
 * as before. Do that whenever pillColors.ts, pillBlend.ts or proseGlow.ts
 * changes: `STALAGMIGHT_FULL_COLOUR_SWEEP=1 pnpm exec vitest run src/theme`.
 */
export const FULL_COLOUR_SWEEP = process.env.STALAGMIGHT_FULL_COLOUR_SWEEP === '1';

/** The hue step for a sweep ring: `full` in the full sweep, three times wider otherwise. */
export function hueStep(full: number): number {
  return FULL_COLOUR_SWEEP ? full : full * 3;
}

/**
 * Every item in the full sweep; otherwise the items `keep` accepts plus
 * every `stride`-th of the rest, in order.
 */
export function sampled<T>(items: readonly T[], stride: number, keep: (item: T) => boolean = () => false): T[] {
  if (FULL_COLOUR_SWEEP) return [...items];
  return items.filter((item, i) => keep(item) || i % stride === 0);
}
