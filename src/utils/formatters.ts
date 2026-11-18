/**
 * Formatting utilities for consistent display of survey data.
 */

/** The arrow between a shot's two station names (`4 → 5`, U+2192). */
export const SHOT_ARROW = '\u2192';

/**
 * A station as the user sees it: its name (unique within its line), or with
 * line context `Main Line / 17`. Never the positional `line:index` handle,
 * which is an internal session address (SPEC 4.1). Both names pass through
 * {@link displayStationName}, so a hostile file cannot reorder the text.
 *
 * @example
 * formatStationLabel('17')              // "17"
 * formatStationLabel('17', 'Main Line') // "Main Line / 17"
 */
export function formatStationLabel(stationName: string, lineName?: string | null): string {
  const station = displayStationName(stationName);
  const line = displayStationName(lineName);
  return line ? `${line} / ${station}` : station;
}

/**
 * A shot as the user sees it: its from and to station names, or with line
 * context `Main Line: 4 → 5` (SPEC 4.2). Never the shot's id or position.
 *
 * @example
 * formatShotLabel('4', '5')              // "4 → 5"
 * formatShotLabel('4', '5', 'Main Line') // "Main Line: 4 → 5"
 */
export function formatShotLabel(fromName: string, toName: string, lineName?: string | null): string {
  const shot = `${displayStationName(fromName)} ${SHOT_ARROW} ${displayStationName(toName)}`;
  const line = displayStationName(lineName);
  return line ? `${line}: ${shot}` : shot;
}

/**
 * Whether a code point would reorder, hide or break the text around it: C0/C1
 * controls, the bidi marks, embeddings, overrides and isolates, and the two
 * noncharacters U+FFFE/U+FFFF (illegal in XML, so the image export drops them).
 */
function isUnsafeForDisplay(codePoint: number): boolean {
  return (
    codePoint <= 0x1f ||
    (codePoint >= 0x7f && codePoint <= 0x9f) ||
    codePoint === 0xfffe ||
    codePoint === 0xffff ||
    codePoint === 0x061c ||
    codePoint === 0x200e ||
    codePoint === 0x200f ||
    (codePoint >= 0x202a && codePoint <= 0x202e) ||
    (codePoint >= 0x2066 && codePoint <= 0x2069)
  );
}

/**
 * A station or line name as it may be shown. Names come from the file - a
 * shared or downloaded `.stalx` - so a right-to-left override or a newline in
 * one must not be able to reorder or disguise the text displayed beside it.
 */
export function displayStationName(name: string | null | undefined): string {
  if (!name) return '';
  let out = '';
  for (const ch of name) {
    if (!isUnsafeForDisplay(ch.codePointAt(0) ?? 0)) out += ch;
  }
  return out;
}
