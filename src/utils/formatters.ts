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

/** Longest station name an edit stores, in code points: Rust
 *  `MAX_STATION_NAME_CHARS` (src-tauri/crates/models/src/station_name.rs). */
export const MAX_STATION_NAME_CHARS = 256;

/** Rust `char::is_whitespace` (the Unicode White_Space property). Not JS `\s`
 *  or `String.prototype.trim`, which also take U+FEFF and Rust does not. */
function isRustWhitespace(codePoint: number): boolean {
  return (
    (codePoint >= 0x09 && codePoint <= 0x0d) ||
    codePoint === 0x20 ||
    codePoint === 0x85 ||
    codePoint === 0xa0 ||
    codePoint === 0x1680 ||
    (codePoint >= 0x2000 && codePoint <= 0x200a) ||
    codePoint === 0x2028 ||
    codePoint === 0x2029 ||
    codePoint === 0x202f ||
    codePoint === 0x205f ||
    codePoint === 0x3000
  );
}

/** Drop Rust-whitespace code points from both ends (`str::trim`), or only the
 *  end (`str::trim_end`). */
function trimRustWhitespace(cps: readonly string[], start = true): readonly string[] {
  let lo = 0;
  let hi = cps.length;
  const ws = (ch: string | undefined) => ch !== undefined && isRustWhitespace(ch.codePointAt(0) ?? 0);
  if (start) while (lo < hi && ws(cps[lo])) lo++;
  while (hi > lo && ws(cps[hi - 1])) hi--;
  return lo === 0 && hi === cps.length ? cps : cps.slice(lo, hi);
}

/**
 * A station name as the backend stores it from an edit - an exact mirror of
 * Rust `sanitize_station_name`, so the UI can tell a no-op edit before sending
 * it - or null when nothing is left (the edit removes the name).
 *
 * Unsafe code points (`isUnsafeForDisplay`, the set Rust
 * `is_unsafe_in_station_name` drops) go, except that a whitespace control (a
 * tab or newline from a pasted cell) becomes a space. Then surrounding
 * whitespace is trimmed and the result is cut to MAX_STATION_NAME_CHARS code
 * points, dropping whitespace left at the cut.
 */
export function sanitizeStationName(raw: string): string | null {
  const cleaned: string[] = [];
  for (const ch of raw) {
    const cp = ch.codePointAt(0) ?? 0;
    if (!isUnsafeForDisplay(cp)) cleaned.push(ch);
    else if (isRustWhitespace(cp)) cleaned.push(' ');
  }
  let name = trimRustWhitespace(cleaned);
  if (name.length > MAX_STATION_NAME_CHARS) name = trimRustWhitespace(name.slice(0, MAX_STATION_NAME_CHARS), false);
  return name.length > 0 ? name.join('') : null;
}

/**
 * A station ID with its name, when it has one (e.g. the MNemo station number
 * a DMP import recorded). The name is not unique - MNemo restarts at 0 in
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
