/**
 * [#576 NF-W7F-1] Editable text for a reading the store holds as an f32.
 *
 * The binary envelope carries every shot reading as an f32 (`core::load_binary`,
 * the PRECISION note): a length typed as 1.2 reaches the store as
 * 1.2000000476837158, and an editor seeded with `String(value)` showed those
 * digits. The text here is the decimal with the fewest fraction digits that
 * rounds to the SAME f32 - what was typed, for anything typed to fewer than
 * eight significant digits - so it reads as entered and, parsed back, is the same
 * reading ({@link sameF32}).
 */

/** More fraction digits than any survey reading carries; the f64 editors' cap too. */
const MAX_FRACTION_DIGITS = 12;

/**
 * The fraction digits of the shortest decimal that rounds to `value`'s f32, or
 * null when `value` is not exactly an f32 (an f64 the frontend computed, a unit
 * conversion) or not finite: such a value has no f32 to round to.
 */
export function f32FractionDigits(value: number): number | null {
  if (!Number.isFinite(value) || Math.fround(value) !== value) return null;
  for (let digits = 0; digits <= MAX_FRACTION_DIGITS; digits += 1) {
    if (Math.fround(Number(value.toFixed(digits))) === value) return digits;
  }
  return null;
}

/** `value` as plain editable text: the shortest f32 decimal, else `String(value)`. */
export function f32Text(value: number): string {
  const digits = f32FractionDigits(value);
  return digits === null ? String(value) : String(Number(value.toFixed(digits)));
}

/** The same reading once both are rounded to f32, the precision the store holds. */
export function sameF32(a: number, b: number): boolean {
  return Math.fround(a) === Math.fround(b);
}
