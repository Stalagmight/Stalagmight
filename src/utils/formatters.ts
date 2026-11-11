/**
 * Formatting utilities for consistent display of survey data.
 */

/**
 * Format a shot ID for user display.
 *
 * @param sectionId - Section ID (1-indexed, from database)
 * @param shotPosition - Shot position within section (1-indexed)
 * @returns Formatted ID string in "sectionId:shotPosition" format
 *
 * @example
 * formatShotId(5, 3) // returns "5:3"
 * formatShotId(1, 1) // returns "1:1"
 */
export function formatShotId(sectionId: number, shotPosition: number): string {
  return `${sectionId}:${shotPosition}`;
}

/**
 * Format a station ID for user display.
 *
 * @param sectionId - Section ID (1-indexed, from database)
 * @param stationIndex - Station index within section (0-indexed)
 * @returns Formatted ID string in "sectionId:stationIndex" format
 *
 * @example
 * formatStationId(5, 0) // returns "5:0"
 * formatStationId(1, 3) // returns "1:3"
 */
export function formatStationId(sectionId: number, stationIndex: number): string {
  return `${sectionId}:${stationIndex}`;
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
 * A station name as it may be shown. Names come from the file - a shared or
 * downloaded `.stalx` - so a right-to-left override or a newline in one must not
 * be able to reorder or disguise the positional ID displayed beside it.
 */
export function displayStationName(name: string | null | undefined): string {
  if (!name) return '';
  let out = '';
  for (const ch of name) {
    if (!isUnsafeForDisplay(ch.codePointAt(0) ?? 0)) out += ch;
  }
  return out;
}

/**
 * A station ID with its name, when it has one (e.g. the mNemo station number
 * a DMP import recorded). The name is not unique - mNemo restarts at 0 in
 * every section - so here it is shown beside the positional ID. Only the
 * opt-in Station names map label (exclusive with the Station IDs label) shows
 * a name instead of the ID.
 *
 * @example
 * formatStationIdWithName(5, 3, '17') // returns "5:3 · 17"
 * formatStationIdWithName(5, 3)       // returns "5:3"
 */
export function formatStationIdWithName(
  sectionId: number,
  stationIndex: number,
  name?: string | null,
): string {
  const id = formatStationId(sectionId, stationIndex);
  const shown = displayStationName(name);
  return shown ? `${id} · ${shown}` : id;
}
