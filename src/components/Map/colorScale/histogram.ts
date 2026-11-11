/**
 * Value histograms for the Levels slider: how the survey's depths (or
 * distances) spread across the data extent, in equal-width bins.
 */

export const HISTOGRAM_BINS = 64;

/** A strided view of the values to bin (e.g. station z: stride 3, offset 2). */
export interface HistogramSource {
  readonly data: ArrayLike<number | null>;
  readonly stride?: number;
  readonly offset?: number;
}

/**
 * Bin the finite values of `source` over `[min, max]`. Values outside the range
 * land in the end bins (they are coloured as the ends, so that is where they
 * belong visually). A flat range puts everything in one bin.
 */
export function computeHistogram(
  source: HistogramSource,
  min: number,
  max: number,
  bins = HISTOGRAM_BINS,
): number[] {
  const counts = new Array<number>(bins).fill(0);
  const stride = source.stride ?? 1;
  const span = max - min;
  const { data } = source;
  for (let i = source.offset ?? 0; i < data.length; i += stride) {
    const v = data[i];
    if (v === null || v === undefined || !Number.isFinite(v)) continue;
    const k = span > 0 ? Math.floor(((v - min) / span) * bins) : 0;
    counts[k < 0 ? 0 : k >= bins ? bins - 1 : k]! += 1;
  }
  return counts;
}
