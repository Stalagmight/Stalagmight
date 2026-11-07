/**
 * GPS coordinate parsing utilities.
 * Supports 7 formats: DD, DMS, DDM, UTM, MGRS, Plus Codes, What3words.
 * Also extracts coordinates from URLs (Google Maps, OpenStreetMap, Bing Maps).
 *
 * NOTE: UTM and MGRS conversions use simplified algorithms that are accurate
 * for most common use cases but may have small errors in edge cases (polar regions,
 * zone boundaries, special zones like Norway/Svalbard). For survey-grade accuracy,
 * consider using dedicated geodetic libraries.
 *
 * NOTE: What3words requires API access and cannot be converted offline.
 */

import type { GpsCoordinates } from '../types/project';
import i18n from '../i18n';
import { formatFixed } from '../i18n/format';

/**
 * Supported GPS coordinate formats.
 */
export type GpsFormat =
  | 'DD' // Decimal Degrees: 48.8584, 2.2945
  | 'DMS' // Degrees Minutes Seconds: 48°51'30"N 2°17'40"E
  | 'DDM' // Degrees Decimal Minutes: 48°51.5'N 2°17.67'E
  | 'UTM' // Universal Transverse Mercator: 31U 448251 5411932
  | 'MGRS' // Military Grid Reference: 31UDQ4825111932
  | 'PlusCodes' // Google Plus Codes: 8FW4V75V+8Q
  | 'What3words'; // What3words: ///filled.count.soap

/**
 * Result from parsing GPS coordinates.
 */
export interface GpsParseResult {
  /** Successfully parsed coordinates (DD format) */
  coordinates: GpsCoordinates;
  /** Detected format */
  format: GpsFormat;
  /** Original input string */
  original: string;
  /** How an ambiguous part of the input was read (localized). Shown next to the
   *  parsed preview so the reading is never silent. */
  note?: string;
  /** The input itself says which value is the latitude (hemisphere letters, a
   *  grid reference, a Plus Code or a map link), so the "may be swapped" hint
   *  does not apply. See `gpsSwapHint`. */
  orderExplicit?: boolean;
}

/**
 * Error from GPS parsing.
 */
export interface GpsParseError {
  /** Error message */
  message: string;
  /** Original input string */
  original: string;
  /** Detected format (if any) */
  format?: GpsFormat;
}

/**
 * Validation result for GPS coordinates.
 */
export interface GpsValidationResult {
  valid: boolean;
  errors: string[];
}

// Constants for validation
const LAT_MIN = -90;
const LAT_MAX = 90;
const LON_MIN = -180;
const LON_MAX = 180;

// UTM zone letters (C-X, excluding I and O)
const UTM_ZONE_LETTERS = 'CDEFGHJKLMNPQRSTUVWX';

// MGRS band letters (same as UTM)
const MGRS_BAND_LETTERS = UTM_ZONE_LETTERS;

// Open Location Code (Plus Codes) alphabet - 20 characters
// Note: '0' is used for padding in short codes but not for encoding
const OLC_ALPHABET = '23456789CFGHJMPQRVWX';

/**
 * The longest text, other than a map link, that is read as coordinates. Real
 * coordinate text, even a labelled paste, stays far below it. The parsers that
 * find coordinates inside other text search with unanchored patterns whose time
 * grows with the square of the length, and the dialogs parse on every
 * keystroke, so longer text is refused before any pattern runs.
 */
const MAX_COORDINATE_TEXT_LENGTH = 1024;

/**
 * The longest map link that is read. A map link gets its own cap because a real
 * one can outgrow coordinate text: a Google Maps place link with its full
 * `data=` block runs to one or two thousand characters. 8192 is about the
 * longest URL the web serves at all (Apache's default `LimitRequestLine` is 8190
 * bytes, nginx's `large_client_header_buffers` defaults to 8k), so no working
 * link is refused. Longer text is refused before any pattern runs, as for
 * `MAX_COORDINATE_TEXT_LENGTH`: link parsing also runs the unanchored
 * coordinate parsers (on a Google Maps place name, see `parseUrl`).
 */
const MAX_LINK_TEXT_LENGTH = 8192;

/** The length cap for `text`: `MAX_LINK_TEXT_LENGTH` for a map link, else `MAX_COORDINATE_TEXT_LENGTH` (checked without a pattern). */
function maxCoordinateTextLength(text: string): number {
  const link =
    text.includes('http') || (text.includes('maps') && text.includes('/')) || text.slice(0, 4).toLowerCase() === 'geo:';
  return link ? MAX_LINK_TEXT_LENGTH : MAX_COORDINATE_TEXT_LENGTH;
}

/** Text over its cap (`maxCoordinateTextLength`), checked without a pattern. */
function tooLongForCoordinates(input: string): boolean {
  const text = input.trim();
  if (text.length <= MAX_COORDINATE_TEXT_LENGTH) return false;
  return text.length > maxCoordinateTextLength(text);
}

/**
 * Normalize user-entered GPS text:
 * - collapse tabs/newlines/non-breaking spaces
 * - collapse repeated whitespace
 * - trim leading/trailing whitespace
 */
function normalizeGpsWhitespace(input: string): string {
  return input
    .replace(/\u00A0/g, ' ')
    .replace(/[\t\r\n]+/g, ' ')
    .replace(/\s+/g, ' ')
    .trim();
}

/**
 * A dash-like character (hyphen, non-breaking hyphen, figure, en or em dash,
 * horizontal bar, small or fullwidth hyphen-minus) where a minus sign stands:
 * right before a number, after a space, punctuation or a lone hemisphere
 * letter, never inside a word or a number. Read as a minus sign, never dropped
 * (dropping it flips the hemisphere), and never silently: it could also have
 * been meant as a separator. The Unicode minus sign (U+2212) is a minus sign
 * and needs no note.
 */
const DASH_AS_MINUS = /(^|[^A-Za-z0-9.]|(?:^|[^A-Za-z])[NSEW])([\u2010-\u2015\uFE63\uFF0D])(?=\d)/gi;

/**
 * Map the look-alike characters keyboards and word processors produce onto the
 * ASCII marks the parsers read: masculine ordinal (U+00BA, Spanish and Mexican
 * keyboards) and ring above (U+02DA, macOS Option+K) for the degree sign;
 * primes, acute accents and curly quotes for minutes and seconds; two
 * apostrophes for a seconds mark; and the Unicode minus and hyphen/dash
 * characters that copy-paste turns a '-' into. A dash where a minus sign
 * stands (`DASH_AS_MINUS`) becomes one; `parseGps` says so in a note. With
 * `dashesAsMinus` false that dash is left as a dash (a horizontal bar, which
 * no parser reads), so `parseGps` can tell whether reading it changed anything.
 */
function canonicalizeGpsSymbols(text: string, dashesAsMinus = true): string {
  return text
    .replace(/[\u00BA\u02DA]/g, '\u00B0')
    .replace(/[\u2018\u2019\u2032\u00B4\u02B9]/g, "'")
    .replace(/[\u201C\u201D\u2033\u02BA]/g, '"')
    .replace(/''/g, '"')
    .replace(DASH_AS_MINUS, dashesAsMinus ? '$1-' : '$1\u2015')
    .replace(/[\u2010-\u2013\u2212]/g, '-');
}

/**
 * Latitude beyond this is treated as "extreme" for the transposition heuristic
 * below. A lower threshold (60) false-positives on real diving/caving
 * latitudes - Iceland (~64.25 N), Norway (~66 N), Sweden (~63 N) all sit past
 * it - so this stays high enough that only near-polar or clearly-a-longitude
 * values trip it.
 */
const TRANSPOSITION_LAT_THRESHOLD = 75;

/**
 * Soft (non-blocking) heuristic: does this lat/lon pair look like the two
 * values were swapped? Fires only when the given latitude is implausibly
 * extreme AND the longitude would read as a plausible latitude if the two
 * were swapped - a real polar survey (both values genuinely extreme, or the
 * longitude also past the threshold) does not trip it.
 */
export function looksTransposed(lat: number, lon: number): boolean {
  // Belt-and-suspenders, not load-bearing: NaN never satisfies either
  // comparison below either, so this guard cannot be observed by a test -
  // it documents intent (never fire on invalid input) rather than changing it.
  if (!Number.isFinite(lat) || !Number.isFinite(lon)) return false;
  const givenLatExtreme = Math.abs(lat) > TRANSPOSITION_LAT_THRESHOLD;
  const swappedLatPlausible = Math.abs(lon) <= TRANSPOSITION_LAT_THRESHOLD;
  return givenLatExtreme && swappedLatPlausible;
}

/**
 * `looksTransposed`'s raw-pair sibling, for input that FAILED to parse - so
 * `parseDD` already rejected it with a range error and there is no
 * `GpsCoordinates` to run the coords-based check on. Without this, a swapped
 * "120, 45" (120 is not a latitude at all) got only the generic range error
 * and never the swap hint, because the hint was computed downstream of a
 * successful parse that this input can never produce.
 *
 * Deliberately narrow: matches only the same bare `lat, lon` / `lat lon`
 * shape `parseDD` accepts (no DMS/UTM/etc - a failed parse of one of those
 * needs a different fix, not a swap hint), and fires only when the first
 * number is an outright IMPOSSIBLE latitude (past +/-90, not merely
 * `looksTransposed`'s softer "extreme") that swapping would actually resolve
 * both ways - both `parseDD` would have accepted the swapped order too.
 */
export function looksTransposedRawDD(input: string): boolean {
  if (tooLongForCoordinates(input)) return false;
  const trimmed = normalizeGpsWhitespace(input);
  const match = trimmed.match(/^([+-]?\d+\.?\d*)[,\s]+([+-]?\d+\.?\d*)$/);
  if (!match || match[1] === undefined || match[2] === undefined) return false;
  const lat = parseFloat(match[1]);
  const lon = parseFloat(match[2]);
  if (!Number.isFinite(lat) || !Number.isFinite(lon)) return false;
  const givenLatImpossible = Math.abs(lat) > LAT_MAX;
  const swappedLatPlausible = Math.abs(lon) <= LAT_MAX;
  const swappedLonPlausible = Math.abs(lat) <= LON_MAX;
  return givenLatImpossible && swappedLatPlausible && swappedLonPlausible;
}

/**
 * Validate latitude and longitude values.
 */
export function validateCoordinates(lat: number, lon: number): GpsValidationResult {
  const errors: string[] = [];

  if (!Number.isFinite(lat)) {
    errors.push(i18n.t('validation.gps.latitudeInvalid'));
  } else if (lat < LAT_MIN || lat > LAT_MAX) {
    errors.push(i18n.t('validation.gps.latitudeRange', { min: LAT_MIN, max: LAT_MAX }));
  }

  if (!Number.isFinite(lon)) {
    errors.push(i18n.t('validation.gps.longitudeInvalid'));
  } else if (lon < LON_MIN || lon > LON_MAX) {
    errors.push(i18n.t('validation.gps.longitudeRange', { min: LON_MIN, max: LON_MAX }));
  }

  return {
    valid: errors.length === 0,
    errors,
  };
}

/**
 * Parse Decimal Degrees format.
 * Examples: "48.8584, 2.2945", "48.8584 2.2945", "-33.8688, 151.2093"
 *
 * `depthReference` (meters below the surface, +down - matches
 * `GpsCoordinates.depth_reference`; see docs/inclination.md) defaults to 0,
 * the app's long-standing "not recorded" value.
 */
export function parseDD(input: string, depthReference = 0): GpsParseResult | GpsParseError {
  const trimmed = normalizeGpsWhitespace(input);

  // Match patterns like "lat, lon" or "lat lon" with optional signs
  const ddRegex = /^(-?\d+\.?\d*)[,\s]+(-?\d+\.?\d*)$/;
  const match = trimmed.match(ddRegex);

  if (!match || match[1] === undefined || match[2] === undefined) {
    return {
      message: i18n.t('validation.gps.ddFormat'),
      original: input,
      format: 'DD',
    };
  }

  const lat = parseFloat(match[1]);
  const lon = parseFloat(match[2]);

  const validation = validateCoordinates(lat, lon);
  if (!validation.valid) {
    return {
      message: validation.errors.join('; '),
      original: input,
      format: 'DD',
    };
  }

  return {
    coordinates: { latitude: lat, longitude: lon, depth_reference: depthReference },
    format: 'DD',
    original: input,
  };
}

/**
 * Parse Degrees Minutes Seconds format.
 * Examples: "48°51'30\"N 2°17'40\"E", "48 51 30 N 2 17 40 E"
 *
 * Requires direction letters OR negative degrees for disambiguation.
 * Will not silently default to N/E for ambiguous inputs.
 *
 * See `parseDD` for `depthReference`.
 */
export function parseDMS(input: string, depthReference = 0): GpsParseResult | GpsParseError {
  const trimmed = normalizeGpsWhitespace(input);

  // We'll extract these values from whichever pattern matches
  let latDegStr: string | undefined;
  let latMinStr: string | undefined;
  let latSecStr: string | undefined;
  let latDir: string | undefined;
  let lonDegStr: string | undefined;
  let lonMinStr: string | undefined;
  let lonSecStr: string | undefined;
  let lonDir: string | undefined;
  // Patterns 1 and 2 carry direction letters; pattern 3 is signed degrees only.
  let lettered = true;
  // Notes beyond `signLetterNotes`, e.g. a letter next to the pair read as text,
  // and the hemispheres taken from a word (`hemisphereFromTextNote`).
  const extraNotes: string[] = [];
  const hemisphereFrom: { latitude?: string | undefined; longitude?: string | undefined } = {};

  // Patterns 1 and 2 end in the longitude letter: E or W, or the direction
  // word it starts (East, West; Este/Est), never the first letter of another
  // word ("2°17'40" WGS84" has no letter). The latitude letter is already
  // followed by a separator.
  //
  // Every pattern starts at a whole number: `(?<!\d\.?)` keeps a match from
  // starting inside "48.5" (and reading 5°51'30"); see `wholeDegreesError`.

  // Pattern 1: With degree/minute/second symbols AND direction letters
  // Matches: 48°51'30"N 2°17'40"E (or with spaces)
  const dmsSymbolWithDirRegex =
    /(?<!\d\.?)([+-]?\d+)[°]\s*(\d+)[′']\s*(\d+\.?\d*)[″"]\s*([NSns])[,\s]+([+-]?\d+)[°]\s*(\d+)[′']\s*(\d+\.?\d*)[″"]\s*(East|Este|Est|West|[EW])(?![A-Za-z])/i;

  // Pattern 2: Space-separated with direction letters (required for disambiguation)
  // Matches: 48 51 30 N 2 17 40 E
  const dmsSpaceRegex =
    /(?<!\d\.?)([+-]?\d+)\s+(\d+)\s+(\d+\.?\d*)\s*([NSns])\s+([+-]?\d+)\s+(\d+)\s+(\d+\.?\d*)\s*(East|Este|Est|West|[EW])(?![A-Za-z])/i;

  // Pattern 3: Signed degrees with symbols (no direction letters)
  // Matches: -33°51'54" 151°12'33" or 33°51'54" -151°12'33"
  const dmsSignedRegex =
    /(?<!\d\.?)(-?\d+)[°]\s*(\d+)[′']\s*(\d+\.?\d*)[″"][,\s]+(-?\d+)[°]\s*(\d+)[′']\s*(\d+\.?\d*)[″"]/i;

  // Patterns 1 and 2 without the longitude letter, tried last: the longitude's
  // hemisphere then comes from the signs (`lonDirFromSigns`), as in the strict
  // pair reader - `87°53'15"S, -106°46'0" WGS84` is W by its minus sign. No
  // number may follow, so a longitude is never cut short.
  const dmsSymbolNoLonLetterRegex =
    /(?<!\d\.?)([+-]?\d+)[°]\s*(\d+)[′']\s*(\d+\.?\d*)[″"]\s*([NSns])[,\s]+([+-]?\d+)[°]\s*(\d+)[′']\s*(\d+\.?\d*)[″"](?!\s*\d)/i;
  const dmsSpaceNoLonLetterRegex =
    /(?<!\d\.?)([+-]?\d+)\s+(\d+)\s+(\d+\.?\d*)\s*([NSns])\s+([+-]?\d+)\s+(\d+)\s+(\d+\.?\d*)(?![\d.])(?!\s*\d)/i;

  let match = trimmed.match(dmsSymbolWithDirRegex);
  if (match) {
    // Pattern 1: has direction letters
    latDegStr = match[1];
    latMinStr = match[2];
    latSecStr = match[3];
    latDir = match[4]?.toUpperCase();
    lonDegStr = match[5];
    lonMinStr = match[6];
    lonSecStr = match[7];
    lonDir = match[8]?.[0]?.toUpperCase();
    hemisphereFrom.longitude = wordDecides(match[8], lonDegStr, wordFollows(trimmed, match));
  } else {
    match = trimmed.match(dmsSpaceRegex);
    if (match) {
      // Pattern 2: space-separated with direction letters
      latDegStr = match[1];
      latMinStr = match[2];
      latSecStr = match[3];
      latDir = match[4]?.toUpperCase();
      lonDegStr = match[5];
      lonMinStr = match[6];
      lonSecStr = match[7];
      lonDir = match[8]?.[0]?.toUpperCase();
      hemisphereFrom.longitude = wordDecides(match[8], lonDegStr, wordFollows(trimmed, match));
    } else {
      match = trimmed.match(dmsSignedRegex);
      if (match) {
        // Pattern 3: signed degrees - at least one must be negative
        lettered = false;
        latDegStr = match[1];
        latMinStr = match[2];
        latSecStr = match[3];
        lonDegStr = match[4];
        lonMinStr = match[5];
        lonSecStr = match[6];

        // Derive direction from the sign as written. Read it off the text,
        // not the parsed number: parseInt('-0') is -0, which fails `< 0`.
        const latNegative = latDegStr?.startsWith('-') === true;
        const lonNegative = lonDegStr?.startsWith('-') === true;

        // A hemisphere letter or word right before the latitude or right
        // after the longitude (`adjacentHemisphere`): in
        // `Entrance: -20°30'15" 87°18'27"W` the W makes the longitude west.
        // A '+' between that letter and the latitude is the latitude's sign.
        // Groups: the end of a word before the letter (`Station N`), the letter, the
        // space between it and the number; and after the longitude the space,
        // the letter, and a word after it. Before the latitude a letter after a
        // word reads as part of a name; after the longitude a lone letter is
        // where the hemisphere goes (`87°18'27" W 12m`), so only a direction
        // word followed by another word does (`South entrance`).
        const end = (match.index ?? 0) + match[0].length;
        const before = trimmed
          .slice(0, match.index)
          .match(/(?:^|([A-Za-z])\s+|[^A-Za-z])(North|South|East|Este|Est|West|[NSEW])(\s*)(\+?)$/i);
        const after = trimmed
          .slice(end)
          .match(/^(\s*)(North|South|East|Este|Est|West|[NSEW])(?![A-Za-z])(\s+[A-Za-z])?/i);
        if (before?.[4] === '+') latDegStr = `+${latDegStr}`;
        const wordBefore = before?.[1] !== undefined;
        // Whether another word sits beside the direction word, for its note only
        // (RV3-FE-11): any word, so a name starting with a letter outside ASCII
        // (`Ñandú South 20...`) or with a digit counts. The label-word groups above
        // stay ASCII: they decide the reading, not just the note.
        const nameBefore = before !== null && WORD_THEN_DIRECTION.test(trimmed.slice(0, match.index));
        const nameAfter = after !== null && wordFollowsAt(trimmed, end + after[1]!.length + after[2]!.length);
        const latSide =
          before && adjacentHemisphere(before[2]!, before[3] !== '', wordBefore, nameBefore, latDegStr ?? '', 'NS');
        const lonSide =
          after &&
          adjacentHemisphere(
            after[2]!,
            after[1] !== '',
            after[2]!.length > 1 && after[3] !== undefined,
            nameAfter,
            lonDegStr ?? '',
            'EW'
          );
        // A wrong-axis letter on both sides says longitude first: no reading.
        const bothWrongAxis = latSide?.wrongAxis === true && lonSide?.wrongAxis === true;

        // Require at least one negative value for signed format to avoid ambiguity
        if ((!latNegative && !lonNegative) || latSide?.refuse || lonSide?.refuse || bothWrongAxis) {
          match = null; // Both positive = ambiguous, reject
        } else {
          latDir = latSide?.letter ?? (latNegative ? 'S' : 'N');
          lonDir = lonSide?.letter ?? (lonNegative ? 'W' : 'E');
          lettered = latSide?.letter !== undefined || lonSide?.letter !== undefined;
          for (const side of [latSide, lonSide]) {
            if (side?.asText !== undefined) {
              extraNotes.push(i18n.t('validation.gps.letterReadAsText', { text: side.asText }));
            }
          }
          hemisphereFrom.latitude = latSide?.decides;
          hemisphereFrom.longitude = lonSide?.decides;
        }
      }
    }
  }
  if (!match) {
    match = trimmed.match(dmsSymbolNoLonLetterRegex) ?? trimmed.match(dmsSpaceNoLonLetterRegex);
    if (match) {
      lettered = true;
      [, latDegStr, latMinStr, latSecStr, latDir, lonDegStr, lonMinStr, lonSecStr] = match;
      latDir = latDir?.toUpperCase();
      lonDir = lonDirFromSigns(latDegStr, latDir, lonDegStr);
    }
  }

  if (
    !match ||
    latDegStr === undefined ||
    latMinStr === undefined ||
    latSecStr === undefined ||
    lonDegStr === undefined ||
    lonMinStr === undefined ||
    lonSecStr === undefined ||
    latDir === undefined ||
    lonDir === undefined
  ) {
    return { message: wholeDegreesError(trimmed) ?? dmsFormatMessage(), original: input, format: 'DMS' };
  }

  const latDeg = parseInt(latDegStr, 10);
  const latMin = parseInt(latMinStr, 10);
  const latSec = parseFloat(latSecStr);

  const lonDeg = parseInt(lonDegStr, 10);
  const lonMin = parseInt(lonMinStr, 10);
  const lonSec = parseFloat(lonSecStr);

  // Validate minutes and seconds
  if (latMin < 0 || latMin >= 60 || lonMin < 0 || lonMin >= 60) {
    return {
      message: i18n.t('validation.gps.dmsMinutesRange'),
      original: input,
      format: 'DMS',
    };
  }
  if (latSec < 0 || latSec >= 60 || lonSec < 0 || lonSec >= 60) {
    return {
      message: i18n.t('validation.gps.secondsRange'),
      original: input,
      format: 'DMS',
    };
  }

  // Convert to decimal degrees
  let lat = Math.abs(latDeg) + latMin / 60 + latSec / 3600;
  let lon = Math.abs(lonDeg) + lonMin / 60 + lonSec / 3600;

  // Apply direction (negative for S and W). The '-' test is on the text so a
  // `-0` degree keeps its sign (parseInt gives -0, and -0 < 0 is false).
  if (latDir === 'S' || latDegStr.startsWith('-')) lat = -Math.abs(lat);
  if (lonDir === 'W' || lonDegStr.startsWith('-')) lon = -Math.abs(lon);

  const validation = validateCoordinates(lat, lon);
  if (!validation.valid) {
    return {
      message: validation.errors.join('; '),
      original: input,
      format: 'DMS',
    };
  }

  const note = joinNotes(
    signLetterNotes([latDegStr, latDir, lat], [lonDegStr, lonDir, lon]),
    hemisphereFromTextNote(hemisphereFrom.latitude, lat),
    hemisphereFromTextNote(hemisphereFrom.longitude, lon),
    ...extraNotes
  );
  return {
    coordinates: { latitude: lat, longitude: lon, depth_reference: depthReference },
    format: 'DMS',
    original: input,
    ...(lettered ? { orderExplicit: true } : {}),
    ...(note !== undefined ? { note } : {}),
  };
}

/**
 * A hemisphere letter or direction word found right outside a signed pair in
 * other text, before the latitude (`axis` 'NS') or after the longitude ('EW').
 * It is the hemisphere (`letter`), except when it is `inLabel`: set apart from
 * the number by a space and read together with a word on its other side
 * (`labelWord`, as the caller judges it: `Point E -20°...`, `Station N -20°...`,
 * `... South entrance`), where it may as well be part of that label. There it
 * is read as text (`asText`, said in a note) when it is on the wrong axis or
 * contradicts the minus sign next to it - the signs then give the value, as they
 * did before letters next to the pair were read. Elsewhere a letter on the wrong
 * axis is no reading and the input is refused (`refuse`). `nextToWord`: another
 * word sits right beside it on its far side (see `wordDecides`).
 */
function adjacentHemisphere(
  found: string,
  spaced: boolean,
  labelWord: boolean,
  nextToWord: boolean,
  degrees: string,
  axis: 'NS' | 'EW'
): { letter?: string; asText?: string; refuse?: true; wrongAxis?: true; decides?: string } {
  const letter = found.charAt(0).toUpperCase();
  const inLabel = spaced && labelWord;
  if (!axis.includes(letter)) return inLabel ? { asText: found, wrongAxis: true } : { refuse: true, wrongAxis: true };
  if (inLabel && degrees.startsWith('-') && (letter === 'N' || letter === 'E')) return { asText: found };
  // Used as the hemisphere. Where it alone gives the value (an unsigned
  // degree) and it is a word or reads with a label word, say so.
  const decides = inLabel ? (/^[+-]/.test(degrees) ? undefined : found) : wordDecides(found, degrees, nextToWord);
  return decides === undefined ? { letter } : { letter, decides };
}

/**
 * A direction word (East, West, Este, Est; North, South) that gives an
 * unsigned degree its hemisphere - a word, unlike a letter next to the number,
 * can also be part of a name ("West entrance"), so the reading is said
 * (`hemisphereFromTextNote`). Only where another word sits beside it on its far
 * side (`nextToWord`: `... West entrance`, `Cave South 20...`): a word that starts
 * or ends the text next to the number cannot be part of a name, so reading it
 * is no guess and a note there is noise (COORD-Q1). Undefined for a single
 * letter or a signed degree, whose hemisphere the sign gives or a contradiction
 * note covers.
 */
function wordDecides(found: string | undefined, degrees: string | undefined, nextToWord: boolean): string | undefined {
  return found !== undefined && found.length > 1 && nextToWord && degrees !== undefined && !/^[+-]/.test(degrees)
    ? found
    : undefined;
}

/**
 * True when another word follows `match` in `text`, after a space (`... West entrance`).
 * Any word (RV3-FE-11): one starting with a letter outside ASCII (`West Ñandú`) or with
 * a digit (`West 2nd`) can make the direction word part of a name just as well.
 */
function wordFollows(text: string, match: RegExpMatchArray): boolean {
  return wordFollowsAt(text, (match.index ?? 0) + match[0].length);
}

/** True when a word starts after a space at `at` in `text` - see `wordFollows`. */
function wordFollowsAt(text: string, at: number): boolean {
  return /^\s+[\p{L}\d]/u.test(text.slice(at));
}

/** A word, a space, then the direction word ending the text before a latitude
 *  (`Ñandú South `, `Pit 2 South`) - the before-side twin of `wordFollowsAt`. */
const WORD_THEN_DIRECTION = /[\p{L}\d]\s+(?:North|South|East|Este|Est|West|[NSEW])\s*\+?$/iu;

/** "Took the hemisphere from "West" - read as -87.307500.", or undefined. */
function hemisphereFromTextNote(text: string | undefined, value: number): string | undefined {
  if (text === undefined) return undefined;
  return i18n.t('validation.gps.hemisphereFromText', { text, value: formatFixed(value, undefined, 6) });
}

/** The notes that apply, joined; undefined when there are none. */
function joinNotes(...notes: Array<string | undefined>): string | undefined {
  const present = notes.filter((n): n is string => n !== undefined);
  return present.length > 0 ? present.join(' ') : undefined;
}

/**
 * The note for degrees whose sign and hemisphere letter disagree, one
 * [degrees as typed, letter, value read] per coordinate. A minus sign wins over
 * N or E (the long-standing grammar), a plus sign loses to S or W; either way
 * the reading is said, never silent. Undefined when nothing disagrees.
 */
function signLetterNotes(...coordinates: Array<[string, string | undefined, number]>): string | undefined {
  const notes = coordinates.flatMap(([degrees, letter, value]) => {
    const minusOverrides = degrees.startsWith('-') && (letter === 'N' || letter === 'E');
    const plusOverridden = degrees.startsWith('+') && (letter === 'S' || letter === 'W');
    if (!minusOverrides && !plusOverridden) return [];
    const key = minusOverrides ? 'validation.gps.signOverridesLetter' : 'validation.gps.plusOverriddenByLetter';
    return [i18n.t(key, { letter, value: formatFixed(value, undefined, 6) })];
  });
  return notes.length > 0 ? notes.join(' ') : undefined;
}

/**
 * The hemisphere of a longitude written with no letter after a lettered
 * latitude, from the signs as the strict pair reader takes them: W for its own
 * minus; E for its own plus, or for no sign once the latitude carries a minus
 * that agrees with S (signed degrees). Undefined when the signs do not say.
 */
function lonDirFromSigns(
  latDeg: string | undefined,
  latDir: string | undefined,
  lonDeg: string | undefined
): string | undefined {
  if (lonDeg?.startsWith('-')) return 'W';
  if (lonDeg?.startsWith('+') || (latDeg?.startsWith('-') && latDir === 'S')) return 'E';
  return undefined;
}

// Functions, not constants: the text is read per call, in the current language.
function dmsFormatMessage(): string {
  return i18n.t('validation.gps.dmsFormat');
}
function ddmFormatMessage(): string {
  return i18n.t('validation.gps.ddmFormat');
}

/**
 * Why parseDMS / parseDDM found no pair in text holding a fractional degree
 * followed by minutes (`48.5°51'30"`): the same refusal as the strict pair
 * reader's, instead of reading from the middle of the number. Undefined
 * otherwise.
 */
function wholeDegreesError(text: string): string | undefined {
  return /\d\.\d*\s*°\s*\d+(?:\.\d*)?\s*[′']/.test(text) ? i18n.t('validation.gps.degreesNotWhole') : undefined;
}

/**
 * Parse Degrees Decimal Minutes format.
 * Examples: "48°51.5'N 2°17.67'E"
 *
 * Requires degree symbol AND direction letters to disambiguate from DMS/DD.
 *
 * See `parseDD` for `depthReference`.
 */
export function parseDDM(input: string, depthReference = 0): GpsParseResult | GpsParseError {
  const trimmed = normalizeGpsWhitespace(input);

  // Stricter DDM regex - requires degree symbol and direction letters
  // The minutes must contain a decimal point to distinguish from DMS
  // The longitude letter is bounded as in parseDMS ("87°18.5' elev" has no E),
  // and a match starts at a whole number, as in parseDMS.
  const ddmRegex =
    /(?<!\d\.?)([+-]?\d+)[°]\s*(\d+\.\d+)[′']?\s*([NSns])[,\s]+([+-]?\d+)[°]\s*(\d+\.\d+)[′']?\s*(East|Este|Est|West|[EW])(?![A-Za-z])/i;

  // Also allow format without decimal if it has the prime symbol
  const ddmRegex2 =
    /(?<!\d\.?)([+-]?\d+)[°]\s*(\d+\.?\d*)[′']\s*([NSns])[,\s]+([+-]?\d+)[°]\s*(\d+\.?\d*)[′']\s*(East|Este|Est|West|[EW])(?![A-Za-z])/i;

  // Both without the longitude letter, tried last (see parseDMS).
  const ddmNoLonLetterRegex =
    /(?<!\d\.?)([+-]?\d+)[°]\s*(\d+\.\d+)[′']?\s*([NSns])[,\s]+([+-]?\d+)[°]\s*(\d+\.\d+)(?![\d.])(?![′']?\s*\d)/i;
  const ddmNoLonLetterRegex2 =
    /(?<!\d\.?)([+-]?\d+)[°]\s*(\d+\.?\d*)[′']\s*([NSns])[,\s]+([+-]?\d+)[°]\s*(\d+\.?\d*)[′'](?!\s*\d)/i;

  let match = trimmed.match(ddmRegex);
  if (!match) {
    match = trimmed.match(ddmRegex2);
  }
  let lonDir = match?.[6]?.charAt(0).toUpperCase();
  const lonHemisphereFrom = wordDecides(match?.[6], match?.[4], match !== null && wordFollows(trimmed, match));
  if (!match) {
    match = trimmed.match(ddmNoLonLetterRegex) ?? trimmed.match(ddmNoLonLetterRegex2);
    lonDir = lonDirFromSigns(match?.[1], match?.[3]?.toUpperCase(), match?.[4]);
  }

  if (
    !match ||
    match[1] === undefined ||
    match[2] === undefined ||
    match[3] === undefined ||
    match[4] === undefined ||
    match[5] === undefined ||
    lonDir === undefined
  ) {
    return { message: wholeDegreesError(trimmed) ?? ddmFormatMessage(), original: input, format: 'DDM' };
  }

  const latDeg = parseInt(match[1], 10);
  const latMin = parseFloat(match[2]);
  const latDir = match[3].toUpperCase();

  const lonDeg = parseInt(match[4], 10);
  const lonMin = parseFloat(match[5]);

  // Validate minutes
  if (latMin < 0 || latMin >= 60 || lonMin < 0 || lonMin >= 60) {
    return {
      message: i18n.t('validation.gps.ddmMinutesRange'),
      original: input,
      format: 'DDM',
    };
  }

  // Convert to decimal degrees
  let lat = Math.abs(latDeg) + latMin / 60;
  let lon = Math.abs(lonDeg) + lonMin / 60;

  // Apply direction. The '-' test is on the text, as in parseDMS, so a `-0`
  // degree keeps its sign (parseInt gives -0, and -0 < 0 is false).
  if (latDir === 'S' || match[1].startsWith('-')) lat = -Math.abs(lat);
  if (lonDir === 'W' || match[4].startsWith('-')) lon = -Math.abs(lon);

  const validation = validateCoordinates(lat, lon);
  if (!validation.valid) {
    return {
      message: validation.errors.join('; '),
      original: input,
      format: 'DDM',
    };
  }

  const note = joinNotes(
    signLetterNotes([match[1], latDir, lat], [match[4], lonDir, lon]),
    hemisphereFromTextNote(lonHemisphereFrom, lon)
  );
  return {
    coordinates: { latitude: lat, longitude: lon, depth_reference: depthReference },
    format: 'DDM',
    original: input,
    orderExplicit: true,
    ...(note !== undefined ? { note } : {}),
  };
}

// ---------------------------------------------------------------------------
// Degree pairs (DD, DDM, DMS) read strictly: the whole input must be one
// latitude and one longitude, each up to three numbers (degrees, minutes,
// seconds) with optional marks (°, ', ") and an optional hemisphere letter
// before or after it. `parseGps` tries this first; input it cannot read as a
// pair (for example coordinates inside other text) falls back to the per-format
// parsers above, which search the string for a match.
// ---------------------------------------------------------------------------

type DegreeToken =
  | { kind: 'num'; text: string }
  | { kind: 'mark'; position: number } // 0 = degrees, 1 = minutes, 2 = seconds
  | { kind: 'letter'; letter: string } // N, S, E or W
  | { kind: 'comma' };

/** One half of a pair: degrees[, minutes[, seconds]] as typed, plus its letter. */
interface DegreeComponent {
  parts: string[];
  letter?: string;
}

// A hemisphere letter is a single letter, never part of a word ("Entrance").
const DEGREE_TOKEN_RE = /(\s*)(?:([+-]?\d+(?:\.\d*)?)|([°'"])|([NSEW])(?![A-Z])|(,))/iy;

/** Split into tokens, or null when the text holds anything else (words, other marks). */
function tokenizeDegrees(text: string): DegreeToken[] | null {
  const tokens: DegreeToken[] = [];
  let pos = 0;
  while (pos < text.length) {
    DEGREE_TOKEN_RE.lastIndex = pos;
    const m = DEGREE_TOKEN_RE.exec(text);
    if (!m) return text.slice(pos).trim() === '' ? tokens : null;
    pos = DEGREE_TOKEN_RE.lastIndex;
    const [, space, num, mark, letter] = m;
    if (num !== undefined) {
      // Two numbers need a separator: "20.5-87.3" is not a pair.
      if (tokens[tokens.length - 1]?.kind === 'num' && !space) return null;
      tokens.push({ kind: 'num', text: num });
    } else if (mark !== undefined) {
      tokens.push({ kind: 'mark', position: '°\'"'.indexOf(mark) });
    } else if (letter !== undefined) {
      tokens.push({ kind: 'letter', letter: letter.toUpperCase() });
    } else {
      tokens.push({ kind: 'comma' });
    }
  }
  return tokens;
}

/** Where the latitude ends and the longitude starts, or null when that is unclear. */
function splitDegreePair(tokens: DegreeToken[]): [DegreeToken[], DegreeToken[]] | null {
  const indicesOf = (test: (t: DegreeToken) => boolean) =>
    tokens.flatMap((t, i) => (test(t) ? [i] : []));
  // 1. A comma (or a run of them) separates the halves.
  const commaRuns = indicesOf((t) => t.kind === 'comma').filter((i) => tokens[i - 1]?.kind !== 'comma');
  if (commaRuns.length > 1) return null;
  if (commaRuns[0] !== undefined) {
    let end = commaRuns[0];
    while (tokens[end]?.kind === 'comma') end++;
    return [tokens.slice(0, commaRuns[0]), tokens.slice(end)];
  }
  // 2. Two hemisphere letters, both leading ("N20 30.1 W87 18.4") or both trailing.
  const letters = indicesOf((t) => t.kind === 'letter');
  if (letters.length > 2) return null;
  if (letters.length === 2) {
    const [first, second] = letters as [number, number];
    const leading = first === 0;
    if (leading === (second === tokens.length - 1)) return null;
    const cut = leading ? second : first + 1;
    return [tokens.slice(0, cut), tokens.slice(cut)];
  }
  // 3. Otherwise the longitude starts at the number carrying the second degree
  //    sign or, with no degree signs at all, at the second of exactly two numbers.
  const degreeMarks = indicesOf((t) => t.kind === 'mark' && t.position === 0);
  const numbers = indicesOf((t) => t.kind === 'num');
  let cut: number;
  if (degreeMarks.length === 2) cut = degreeMarks[1]! - 1;
  else if (degreeMarks.length === 0 && numbers.length === 2) cut = numbers[1]!;
  else return null;
  if (cut <= 0 || tokens[cut]?.kind !== 'num') return null;
  // A lone letter right before the cut leads the longitude only if it is E or W.
  const before = tokens[cut - 1];
  if (before?.kind === 'letter' && (before.letter === 'E' || before.letter === 'W')) cut -= 1;
  return [tokens.slice(0, cut), tokens.slice(cut)];
}

/** One half: [letter] number [mark] (up to three numbers) [letter], marks in order. */
function readDegreeComponent(tokens: DegreeToken[]): DegreeComponent | null {
  let start = 0;
  let end = tokens.length;
  let letter: string | undefined;
  const first = tokens[0];
  if (first?.kind === 'letter') {
    letter = first.letter;
    start = 1;
  }
  const last = tokens[end - 1];
  if (end - 1 >= start && last?.kind === 'letter') {
    if (letter !== undefined) return null;
    letter = last.letter;
    end -= 1;
  }
  const parts: string[] = [];
  for (let i = start; i < end; i++) {
    const token = tokens[i]!;
    if (token.kind !== 'num' || parts.length === 3) return null;
    parts.push(token.text);
    const next = tokens[i + 1];
    if (i + 1 < end && next?.kind === 'mark') {
      if (next.position !== parts.length - 1) return null;
      i++;
    }
  }
  if (parts.length === 0) return null;
  return letter === undefined ? { parts } : { parts, letter };
}

const isLatitudeLetter = (c: DegreeComponent) => c.letter === 'N' || c.letter === 'S';
const hasSign = (text: string) => /^[+-]/.test(text);

/**
 * Read `text` as one latitude/longitude pair in degrees. Returns null when the
 * text is not shaped like a pair (the caller then tries the lenient parsers),
 * an error when it is a pair that cannot be read unambiguously, else the result.
 * A minus sign wins over a contradicting N or E, as in parseDMS, and a plus
 * sign loses to S or W, each with a note (`signLetterNotes`).
 */
function parseDegreePair(
  text: string,
  depthReference: number,
  original: string
): GpsParseResult | GpsParseError | null {
  const tokens = tokenizeDegrees(text);
  const halves = tokens && splitDegreePair(tokens);
  const first = halves && readDegreeComponent(halves[0]);
  const second = halves && readDegreeComponent(halves[1]);
  if (!first || !second) return null;

  const depth = Math.max(first.parts.length, second.parts.length);
  const format: GpsFormat = depth === 1 ? 'DD' : depth === 2 ? 'DDM' : 'DMS';
  const fail = (message: string): GpsParseError => ({ message, original, format });
  if (first.parts.length !== second.parts.length) return fail(i18n.t('validation.gps.mixedFormats'));

  // Which half is the latitude: the letters say so when both have one,
  // otherwise the order does (latitude first).
  let lat = first;
  let lon = second;
  if (first.letter !== undefined && second.letter !== undefined) {
    if (isLatitudeLetter(first) === isLatitudeLetter(second)) {
      return fail(i18n.t('validation.gps.hemisphereLetters'));
    }
    if (!isLatitudeLetter(first)) [lat, lon] = [second, first];
  } else if (first.letter !== undefined || second.letter !== undefined) {
    if ((first.letter !== undefined && !isLatitudeLetter(first)) || isLatitudeLetter(second)) {
      return fail(i18n.t('validation.gps.hemisphereLetters'));
    }
    const bare = first.letter === undefined ? first : second;
    const lettered = bare === first ? second : first;
    // Signed degrees read an unsigned degree as N or E once the other carries
    // a minus, and a minus that agrees with S or W is such a sign: this is how
    // `20°30'15" -87°18'27"W` and `S -20°30'15", 87°18'27"` have always read.
    const minusAgrees =
      lettered.parts[0]!.startsWith('-') && (lettered.letter === 'S' || lettered.letter === 'W');
    if (!hasSign(bare.parts[0]!) && !minusAgrees) return fail(i18n.t('validation.gps.letterMissing'));
  } else if (format !== 'DD' && !hasSign(first.parts[0]!) && !hasSign(second.parts[0]!)) {
    // Unsigned minutes/seconds with no letters: N/E would be a guess (as before).
    return fail(format === 'DMS' ? dmsFormatMessage() : ddmFormatMessage());
  }

  for (const c of [lat, lon]) {
    const [deg, min, sec] = c.parts;
    if ((min !== undefined && hasSign(min)) || (sec !== undefined && hasSign(sec))) {
      return fail(i18n.t('validation.gps.signOnMinutes'));
    }
    if (min !== undefined && deg!.includes('.')) return fail(i18n.t('validation.gps.degreesNotWhole'));
    if (sec !== undefined && min!.includes('.')) return fail(i18n.t('validation.gps.minutesNotWhole'));
  }
  // Same checks and messages as parseDMS / parseDDM.
  if ([lat, lon].some((c) => c.parts[1] !== undefined && parseFloat(c.parts[1]) >= 60)) {
    return fail(
      format === 'DMS' ? i18n.t('validation.gps.dmsMinutesRange') : i18n.t('validation.gps.ddmMinutesRange')
    );
  }
  if ([lat, lon].some((c) => c.parts[2] !== undefined && parseFloat(c.parts[2]) >= 60)) {
    return fail(i18n.t('validation.gps.secondsRange'));
  }

  const valueOf = (c: DegreeComponent): number => {
    const [deg, min, sec] = c.parts;
    let value = Math.abs(parseFloat(deg!));
    if (min !== undefined) value += parseFloat(min) / 60;
    if (sec !== undefined) value += parseFloat(sec) / 3600;
    // Read the sign off the text: parseFloat('-0') is -0, which fails `< 0`.
    return deg!.startsWith('-') || c.letter === 'S' || c.letter === 'W' ? -value : value;
  };
  const latitude = valueOf(lat);
  const longitude = valueOf(lon);

  const validation = validateCoordinates(latitude, longitude);
  if (!validation.valid) return fail(validation.errors.join('; '));

  const note = signLetterNotes([lat.parts[0]!, lat.letter, latitude], [lon.parts[0]!, lon.letter, longitude]);
  return {
    coordinates: { latitude, longitude, depth_reference: depthReference },
    format,
    original,
    orderExplicit: first.letter !== undefined || second.letter !== undefined,
    ...(note !== undefined ? { note } : {}),
  };
}

/**
 * Parse UTM (Universal Transverse Mercator) format.
 * Examples: "31U 448251 5411932", "31 U 448251 5411932", "31U, 448251, 5411932"
 *
 * Note: Uses simplified conversion formulas. For survey-grade accuracy,
 * use a dedicated geodetic library.
 *
 * See `parseDD` for `depthReference`.
 */
export function parseUTM(input: string, depthReference = 0): GpsParseResult | GpsParseError {
  const trimmed = normalizeGpsWhitespace(input);

  // More flexible UTM regex:
  // - Zone number (1-2 digits)
  // - Optional space
  // - Band letter
  // - Separator (space, comma, or both)
  // - Easting (can have decimals)
  // - Separator
  // - Northing (can have decimals)
  const utmRegex = /^(\d{1,2})\s*([A-Za-z])[,\s]+(\d+\.?\d*)[,\s]+(\d+\.?\d*)$/;
  const match = trimmed.match(utmRegex);

  if (
    !match ||
    match[1] === undefined ||
    match[2] === undefined ||
    match[3] === undefined ||
    match[4] === undefined
  ) {
    return {
      message: i18n.t('validation.gps.utmFormat'),
      original: input,
      format: 'UTM',
    };
  }

  const zone = parseInt(match[1], 10);
  const band = match[2].toUpperCase();
  const easting = parseFloat(match[3]);
  const northing = parseFloat(match[4]);

  // Validate zone
  if (zone < 1 || zone > 60) {
    return {
      message: i18n.t('validation.gps.utmZoneRange'),
      original: input,
      format: 'UTM',
    };
  }

  // Validate band letter
  if (!UTM_ZONE_LETTERS.includes(band)) {
    return {
      message: i18n.t('validation.gps.utmBandLetter'),
      original: input,
      format: 'UTM',
    };
  }

  // Validate easting range (typical valid range)
  if (easting < 100000 || easting > 900000) {
    return {
      message: i18n.t('validation.gps.utmEastingRange'),
      original: input,
      format: 'UTM',
    };
  }

  // Validate northing range
  if (northing < 0 || northing > 10000000) {
    return {
      message: i18n.t('validation.gps.utmNorthingRange'),
      original: input,
      format: 'UTM',
    };
  }

  const fail = (message: string): GpsParseError => ({ message, original: input, format: 'UTM' });
  const ok = (r: { lat: number; lon: number }, note?: string): GpsParseResult => ({
    coordinates: { latitude: r.lat, longitude: r.lon, depth_reference: depthReference },
    format: 'UTM',
    original: input,
    orderExplicit: true,
    ...(note !== undefined ? { note } : {}),
  });
  const fits = (r: { lat: number } | null, [from, to]: [number, number]) =>
    r !== null && r.lat >= from - UTM_BAND_TOLERANCE_DEG && r.lat <= to + UTM_BAND_TOLERANCE_DEG;

  // The band letter both picks the hemisphere and bounds the latitude, so the
  // northing must land inside the band: a reading impossible for the letter is
  // never accepted silently.
  const bandReading = utmToLatLon(zone, band >= 'N', easting, northing);
  const northingText = match[4];

  // "S" and "N" are also how GIS software writes the hemisphere ("zone 33S").
  if (band === 'S') {
    const asBand = 'message' in bandReading ? null : bandReading;
    const bandFits = fits(asBand, utmBandRange(band));
    const southReading = utmToLatLon(zone, false, easting, northing);
    const asSouth = 'message' in southReading ? null : southReading;
    const southFits = fits(asSouth, [-80, 0]);
    if (asBand && bandFits && asSouth && southFits) {
      // Band S covers real cave regions (Kentucky, Spain, Turkey): keep it, name the alternative.
      const suggestion = `${zone}${utmBandFor(asSouth.lat)} ${match[3]} ${northingText}`;
      return ok(asBand, i18n.t('validation.gps.utmBandSAmbiguous', { suggestion }));
    }
    if (asBand && bandFits) return ok(asBand);
    if (asSouth && southFits) return ok(asSouth, i18n.t('validation.gps.utmReadAsSouthernHemisphere'));
    return fail(i18n.t('validation.gps.utmNeitherSouth', { northing: northingText }));
  }
  if ('message' in bandReading) return fail(bandReading.message);
  const asBand = bandReading;
  const bandFits = fits(asBand, utmBandRange(band));
  if (band === 'N') {
    // Band N and the northern hemisphere convert identically; only the bounds differ.
    if (bandFits) return ok(asBand);
    if (fits(asBand, [0, 84])) return ok(asBand, i18n.t('validation.gps.utmReadAsNorthernHemisphere'));
    return fail(i18n.t('validation.gps.utmNeitherNorth', { northing: northingText }));
  }
  if (!bandFits) {
    const [from, to] = utmBandRange(band);
    return fail(
      i18n.t('validation.gps.utmBandMismatch', {
        northing: northingText,
        latitude: formatLatitude(asBand.lat, 2),
        band,
        from: formatLatitude(from, 0),
        to: formatLatitude(to, 0),
      })
    );
  }
  return ok(asBand);
}

/**
 * Slack on the band edges. A band letter is derived from the latitude, so a
 * correct input lies inside its band up to conversion and datum differences
 * (well under a kilometre); 0.05 degrees (about 5.5 km) absorbs those while a
 * wrong letter is at least a whole band (8 degrees) off.
 */
const UTM_BAND_TOLERANCE_DEG = 0.05;

/** Latitude range [south, north] of a UTM band letter. X is 12 degrees tall. */
function utmBandRange(band: string): [number, number] {
  const south = -80 + 8 * UTM_ZONE_LETTERS.indexOf(band);
  return [south, band === 'X' ? 84 : south + 8];
}

/** The band letter for a latitude inside the UTM range (80S to 84N). */
function utmBandFor(lat: number): string {
  return UTM_ZONE_LETTERS[Math.min(19, Math.max(0, Math.floor((lat + 80) / 8)))] ?? 'X';
}

/** "78.96°N", "8°S", "0°" - for messages. */
function formatLatitude(lat: number, precision: number): string {
  if (lat === 0) return '0°';
  return `${formatFixed(Math.abs(lat), undefined, precision)}°${lat < 0 ? 'S' : 'N'}`;
}

/**
 * Convert UTM coordinates to latitude/longitude.
 * Uses WGS84 ellipsoid parameters.
 */
function utmToLatLon(
  zone: number,
  northern: boolean,
  easting: number,
  northing: number
): { lat: number; lon: number } | { message: string } {
  // WGS84 parameters
  const a = 6378137; // Semi-major axis
  const f = 1 / 298.257223563; // Flattening
  const k0 = 0.9996; // Scale factor
  const e = Math.sqrt(2 * f - f * f); // First eccentricity
  const e1sq = (e * e) / (1 - e * e); // Second eccentricity squared

  // Remove false easting and northing
  const x = easting - 500000;
  const y = northern ? northing : northing - 10000000;

  // Calculate footprint latitude
  const m = y / k0;
  const mu = m / (a * (1 - (e * e) / 4 - (3 * Math.pow(e, 4)) / 64 - (5 * Math.pow(e, 6)) / 256));

  const e1 = (1 - Math.sqrt(1 - e * e)) / (1 + Math.sqrt(1 - e * e));
  const j1 = (3 * e1) / 2 - (27 * Math.pow(e1, 3)) / 32;
  const j2 = (21 * e1 * e1) / 16 - (55 * Math.pow(e1, 4)) / 32;
  const j3 = (151 * Math.pow(e1, 3)) / 96;
  const j4 = (1097 * Math.pow(e1, 4)) / 512;

  const fp =
    mu +
    j1 * Math.sin(2 * mu) +
    j2 * Math.sin(4 * mu) +
    j3 * Math.sin(6 * mu) +
    j4 * Math.sin(8 * mu);

  // Calculate latitude and longitude
  const sinFp = Math.sin(fp);
  const cosFp = Math.cos(fp);
  const tanFp = Math.tan(fp);

  const c1 = e1sq * cosFp * cosFp;
  const t1 = tanFp * tanFp;
  const r1 = (a * (1 - e * e)) / Math.pow(1 - e * e * sinFp * sinFp, 1.5);
  const n1 = a / Math.sqrt(1 - e * e * sinFp * sinFp);
  const d = x / (n1 * k0);

  const q1 = (n1 * tanFp) / r1;
  const q2 = (d * d) / 2;
  const q3 = ((5 + 3 * t1 + 10 * c1 - 4 * c1 * c1 - 9 * e1sq) * Math.pow(d, 4)) / 24;
  const q4 =
    ((61 + 90 * t1 + 298 * c1 + 45 * t1 * t1 - 252 * e1sq - 3 * c1 * c1) * Math.pow(d, 6)) / 720;

  const lat = fp - q1 * (q2 - q3 + q4);

  const q5 = d;
  const q6 = ((1 + 2 * t1 + c1) * Math.pow(d, 3)) / 6;
  const q7 =
    ((5 - 2 * c1 + 28 * t1 - 3 * c1 * c1 + 8 * e1sq + 24 * t1 * t1) * Math.pow(d, 5)) / 120;

  const centralMeridian = (zone - 1) * 6 - 180 + 3;
  const lon = centralMeridian + ((q5 - q6 + q7) / cosFp) * (180 / Math.PI);

  const latDeg = lat * (180 / Math.PI);

  const validation = validateCoordinates(latDeg, lon);
  if (!validation.valid) {
    return { message: validation.errors.join('; ') };
  }

  return { lat: latDeg, lon };
}

/**
 * Validate MGRS (Military Grid Reference System) format.
 * Examples: "31UDQ4825111932", "31U DQ 48251 11932"
 *
 * IMPORTANT: MGRS conversion to lat/lon requires a dedicated library (such as
 * the npm 'mgrs' package) to implement the full specification with proper band
 * minimum northing tables, 100km square rollover logic, and zone edge cases.
 *
 * This function validates the format only. Per the spec "Works offline (API
 * optional for Plus Codes/What3words)", offline format validation is the
 * expected behavior for complex grid systems like MGRS.
 *
 * Returns an error with format='MGRS' for valid formats (to indicate the format
 * was recognized but conversion requires a library) or an error with a different
 * message for invalid formats.
 */
export function parseMGRS(input: string): GpsParseResult | GpsParseError {
  const trimmed = normalizeGpsWhitespace(input).replace(/\s+/g, '').toUpperCase();

  // MGRS regex: zone, band, 100km square, easting, northing
  // Coordinates can be 2, 4, 6, 8, or 10 digits (1m to 10km precision)
  const mgrsRegex = /^(\d{1,2})([A-Za-z])([A-Za-z]{2})(\d+)$/;
  const match = trimmed.match(mgrsRegex);

  if (
    !match ||
    match[1] === undefined ||
    match[2] === undefined ||
    match[3] === undefined ||
    match[4] === undefined
  ) {
    return {
      message: i18n.t('validation.gps.mgrsFormat'),
      original: input,
      format: 'MGRS',
    };
  }

  const zone = parseInt(match[1], 10);
  const band = match[2].toUpperCase();
  const coords = match[4];

  // Validate zone
  if (zone < 1 || zone > 60) {
    return {
      message: i18n.t('validation.gps.mgrsZoneRange'),
      original: input,
      format: 'MGRS',
    };
  }

  // Validate band letter
  if (!MGRS_BAND_LETTERS.includes(band)) {
    return {
      message: i18n.t('validation.gps.mgrsBandLetter'),
      original: input,
      format: 'MGRS',
    };
  }

  // Coordinates must be even length (split equally between easting and northing)
  if (coords.length % 2 !== 0 || coords.length < 2 || coords.length > 10) {
    return {
      message: i18n.t('validation.gps.mgrsDigits'),
      original: input,
      format: 'MGRS',
    };
  }

  // MGRS requires a dedicated library for accurate conversion to lat/lon
  // The full spec involves complex lookup tables and zone edge case handling
  return {
    message: i18n.t('validation.gps.mgrsNeedsLibrary'),
    original: input,
    format: 'MGRS',
  };
}

/**
 * Parse Google Plus Codes (Open Location Code) format.
 * Examples: "8FW4V75V+8Q", "9F4MGCH4+QQ"
 *
 * Plus Codes are encoded lat/lon with the format: 8 digits + '+' + 2+ digits.
 * The first 10 digits (4 pairs before + and 1 pair after) use base-20 encoding.
 * Additional digits after the first 10 use a 4x5 grid refinement.
 *
 * See `parseDD` for `depthReference`.
 */
export function parsePlusCodes(input: string, depthReference = 0): GpsParseResult | GpsParseError {
  const trimmed = normalizeGpsWhitespace(input).replace(/\s+/g, '').toUpperCase();

  // Plus codes: 8 characters + '+' + at least 2 more characters
  // Characters must be from OLC alphabet (note: '0' is padding, not valid for encoding)
  const plusCodeRegex = /^([23456789CFGHJMPQRVWX]{8})\+([23456789CFGHJMPQRVWX]{2,})$/;
  const match = trimmed.match(plusCodeRegex);

  if (!match || match[1] === undefined || match[2] === undefined) {
    return {
      message: i18n.t('validation.gps.plusCodeFormat'),
      original: input,
      format: 'PlusCodes',
    };
  }

  const prefix = match[1];
  const suffix = match[2];
  const code = prefix + suffix;

  // Decode Plus Code using official OLC algorithm
  const result = decodeOpenLocationCode(code);
  if ('message' in result) {
    return {
      message: result.message,
      original: input,
      format: 'PlusCodes',
    };
  }

  return {
    coordinates: { latitude: result.lat, longitude: result.lon, depth_reference: depthReference },
    format: 'PlusCodes',
    original: input,
    orderExplicit: true,
  };
}

/**
 * Decode an Open Location Code (Plus Code) to latitude/longitude.
 * Implements the official OLC algorithm.
 *
 * The code is structured as:
 * - First 10 digits: 5 pairs using base-20 encoding
 * - Additional digits: Single characters using 4x5 grid refinement
 */
function decodeOpenLocationCode(code: string): { lat: number; lon: number } | { message: string } {
  const PAIR_CODE_LENGTH = 10; // First 10 digits are pairs
  const ENCODING_BASE = 20;
  const GRID_ROWS = 5;
  const GRID_COLS = 4;
  const LATITUDE_MAX = 90;
  const LONGITUDE_MAX = 180;

  // Initial resolution for the first digit pair
  const PAIR_FIRST_PLACE_VALUE = Math.pow(ENCODING_BASE, 2); // 400

  let latValue = 0;
  let lonValue = 0;
  let latPlaceValue = PAIR_FIRST_PLACE_VALUE;
  let lonPlaceValue = PAIR_FIRST_PLACE_VALUE;

  // Process pairs (first 10 characters, 5 pairs)
  const pairLength = Math.min(code.length, PAIR_CODE_LENGTH);
  for (let i = 0; i < pairLength; i += 2) {
    latPlaceValue /= ENCODING_BASE;
    lonPlaceValue /= ENCODING_BASE;

    const latChar = code[i];
    const lonChar = code[i + 1];

    if (latChar === undefined) {
      return { message: i18n.t('validation.gps.plusCodeMissingCharacter', { position: i }) };
    }

    const latIndex = OLC_ALPHABET.indexOf(latChar);
    if (latIndex < 0) {
      return { message: i18n.t('validation.gps.plusCodeInvalidCharacter', { character: latChar, position: i }) };
    }
    latValue += latIndex * latPlaceValue;

    if (lonChar === undefined) {
      // Odd length - just use lat value
      break;
    }
    const lonIndex = OLC_ALPHABET.indexOf(lonChar);
    if (lonIndex < 0) {
      return { message: i18n.t('validation.gps.plusCodeInvalidCharacter', { character: lonChar, position: i + 1 }) };
    }
    lonValue += lonIndex * lonPlaceValue;
  }

  // Process grid refinement (characters after position 10, if any)
  if (code.length > PAIR_CODE_LENGTH) {
    let gridLatPlaceValue = latPlaceValue / GRID_ROWS;
    let gridLonPlaceValue = lonPlaceValue / GRID_COLS;

    for (let i = PAIR_CODE_LENGTH; i < code.length; i++) {
      const char = code[i];
      if (char === undefined) break;

      const charIndex = OLC_ALPHABET.indexOf(char);
      if (charIndex < 0) {
        return { message: i18n.t('validation.gps.plusCodeInvalidCharacter', { character: char, position: i }) };
      }

      // Grid character encodes row * GRID_COLS + col
      const row = Math.floor(charIndex / GRID_COLS);
      const col = charIndex % GRID_COLS;

      latValue += row * gridLatPlaceValue;
      lonValue += col * gridLonPlaceValue;

      // Only divide place value if there are more characters to process
      if (i < code.length - 1) {
        gridLatPlaceValue /= GRID_ROWS;
        gridLonPlaceValue /= GRID_COLS;
      }
    }

    // Center of the final grid cell (use current place value, not over-divided)
    latValue += gridLatPlaceValue / 2;
    lonValue += gridLonPlaceValue / 2;
  } else {
    // Center of the pair cell
    latValue += latPlaceValue / 2;
    lonValue += lonPlaceValue / 2;
  }

  // Convert to actual lat/lon (offset from -90, -180)
  const lat = latValue - LATITUDE_MAX;
  const lon = lonValue - LONGITUDE_MAX;

  const validation = validateCoordinates(lat, lon);
  if (!validation.valid) {
    return { message: validation.errors.join('; ') };
  }

  return { lat, lon };
}

/**
 * Validate What3words format.
 * Examples: "///filled.count.soap", "filled.count.soap"
 *
 * IMPORTANT: What3words is a proprietary encoding that requires API access
 * for coordinate conversion. This function validates the format only.
 * Per the spec "Works offline (API optional for Plus Codes/What3words)",
 * offline format validation is the expected behavior.
 *
 * Returns an error with format='What3words' for valid formats (to indicate
 * the format was recognized but conversion requires API) or an error
 * with a different message for invalid formats.
 */
export function parseWhat3words(input: string): GpsParseResult | GpsParseError {
  const trimmed = normalizeGpsWhitespace(input)
    .replace(/\s*\.\s*/g, '.')
    .toLowerCase();

  // Remove leading slashes if present
  const words = trimmed.replace(/^\/+/, '');

  // What3words format: word.word.word
  const w3wRegex = /^[a-z]+\.[a-z]+\.[a-z]+$/;

  if (!w3wRegex.test(words)) {
    return {
      message: i18n.t('validation.gps.what3wordsFormat'),
      original: input,
      format: 'What3words',
    };
  }

  // What3words requires API for conversion - cannot decode offline
  // This is by design - the spec says "Works offline (API optional for Plus Codes/What3words)"
  // which means format validation works offline, but conversion requires API
  return {
    message: i18n.t('validation.gps.what3wordsNeedsApi'),
    original: input,
    format: 'What3words',
  };
}

/**
 * A geo: URI (RFC 5870) as Android apps share it: `geo:lat,lon[,alt][;u=..][?z=..]`.
 * A `q=lat,lon(label)` query is the shared pin and wins over the path, which
 * is then only where the map opens: Android's "pin at" form is
 * `geo:0,0?q=lat,lon(label)`. A `q=` that is a search (an address) carries no
 * coordinates of the point, so it is rejected rather than read as the path:
 * 0,0, or the spot the search map opens on. An empty `q=` leaves the path as
 * the point, except 0,0. Null when the input is not a geo: URI with coordinates.
 */
function parseGeoUri(
  trimmed: string,
  input: string,
  depthReference: number
): GpsParseResult | GpsParseError | null {
  const path = trimmed.match(/^geo:(-?\d+\.?\d*),(-?\d+\.?\d*)/i);
  if (path?.[1] === undefined || path[2] === undefined) return null;
  const query = trimmed.match(/[?&]q=([^&#]*)/i)?.[1];
  // "lat,lon" (comma possibly encoded, spaces as %20 or +), then the end or a "(label)".
  const pin = query?.match(
    /^(?:\s|%20|\+)*(-?\d+\.?\d*)(?:,|%2C)(?:\s|%20|\+)*(-?\d+\.?\d*)(?:\s|%20|\+)*(?:\(|%28|$)/i
  );
  let [latText, lonText] = [path[1], path[2]];
  const search = query !== undefined && query.replace(/\s|%20|\+/gi, '') !== '';
  if (pin?.[1] !== undefined && pin[2] !== undefined) {
    [latText, lonText] = [pin[1], pin[2]];
  } else if (search || (query !== undefined && parseFloat(latText) === 0 && parseFloat(lonText) === 0)) {
    return { message: i18n.t('validation.gps.geoQueryNotCoordinates'), original: input };
  }
  const lat = parseFloat(latText);
  const lon = parseFloat(lonText);
  const validation = validateCoordinates(lat, lon);
  if (!validation.valid) return { message: validation.errors.join('; '), original: input, format: 'DD' };
  return {
    coordinates: { latitude: lat, longitude: lon, depth_reference: depthReference },
    format: 'DD',
    original: input,
  };
}

/**
 * Extract GPS coordinates from a URL.
 * Supports: Google Maps, OpenStreetMap, Bing Maps.
 *
 * Note: Does not support shortened URLs (goo.gl) or Apple Maps.
 *
 * See `parseDD` for `depthReference`.
 */
export function parseUrl(input: string, depthReference = 0): GpsParseResult | GpsParseError {
  const trimmed = input.trim();

  const geo = parseGeoUri(trimmed, input, depthReference);
  if (geo) return geo;

  // Check if it looks like a URL
  if (!trimmed.includes('http') && !trimmed.includes('maps') && !trimmed.includes('@')) {
    return {
      message: i18n.t('validation.gps.notMapUrl'),
      original: input,
    };
  }

  // Google Maps patterns:
  // - https://www.google.com/maps/@48.8584,2.2945,15z
  // - https://www.google.com/maps/place/.../@48.8584,2.2945,15z
  // - https://www.google.com/maps/place/20%C2%B012'02.0%22N+87%C2%B029'59.8%22W/
  // - https://maps.google.com/?q=48.8584,2.2945
  //
  // IMPORTANT: In /place/ URLs, the @lat,lon is the VIEWPORT CENTER, not the
  // place coordinates. We must prefer the place name or !3d/!4d data params.

  const isGooglePlace = /google\.com\/maps\/place\//i.test(trimmed);

  // Pattern 1a: Google Maps /place/ URL - extract DMS/DDM/DD from the place name segment
  if (isGooglePlace) {
    const placeMatch = trimmed.match(/\/place\/([^/@]+)/);
    if (placeMatch && placeMatch[1]) {
      try {
        const decoded = decodeURIComponent(placeMatch[1].replace(/\+/g, ' '));
        // Try parsing the decoded place name as GPS coordinates. It is read as
        // coordinate text, so it gets that text's cap: these parsers search it with
        // patterns whose time grows with the square of its length. A longer place
        // name is no coordinate; the data parameters and `@` below still apply.
        if (decoded.trim().length <= MAX_COORDINATE_TEXT_LENGTH) {
          const placeResult = parseDMS(decoded, depthReference);
          if ('coordinates' in placeResult) return { ...placeResult, original: input };
          const ddmResult = parseDDM(decoded, depthReference);
          if ('coordinates' in ddmResult) return { ...ddmResult, original: input };
          const ddResult = parseDD(decoded, depthReference);
          if ('coordinates' in ddResult) return { ...ddResult, original: input };
        }
      } catch {
        // decodeURIComponent can throw on malformed input - fall through
      }
    }
  }

  // Pattern 1b: Google Maps data parameters !3d=lat !4d=lon (accurate pin coordinates)
  if (isGooglePlace) {
    const data3d = trimmed.match(/!3d(-?\d+\.?\d*)/);
    const data4d = trimmed.match(/!4d(-?\d+\.?\d*)/);
    if (data3d?.[1] && data4d?.[1]) {
      const lat = parseFloat(data3d[1]);
      const lon = parseFloat(data4d[1]);
      const validation = validateCoordinates(lat, lon);
      if (validation.valid) {
        return {
          coordinates: { latitude: lat, longitude: lon, depth_reference: depthReference },
          format: 'DD',
          original: input,
        };
      }
    }
  }

  // Pattern 1c: @lat,lon - only use for non-place URLs (where it IS the coordinate)
  // For /place/ URLs the @ is just the viewport center and should be a last resort.
  const googleAtPattern = /@(-?\d+\.?\d*),(-?\d+\.?\d*)/;
  const googleAtMatch = trimmed.match(googleAtPattern);
  if (googleAtMatch && googleAtMatch[1] !== undefined && googleAtMatch[2] !== undefined) {
    if (!isGooglePlace) {
      const lat = parseFloat(googleAtMatch[1]);
      const lon = parseFloat(googleAtMatch[2]);
      const validation = validateCoordinates(lat, lon);
      if (validation.valid) {
        return {
          coordinates: { latitude: lat, longitude: lon, depth_reference: depthReference },
          format: 'DD',
          original: input,
        };
      }
    }
  }

  // Pattern 2: ?q=lat,lon or ?ll=lat,lon, and the Google Maps URLs API's
  // query= / destination= (whose comma is often encoded as %2C)
  const queryPattern = /[?&](?:q|ll|query|destination)=(-?\d+\.?\d*)(?:,|%2C)(-?\d+\.?\d*)/i;
  const queryMatch = trimmed.match(queryPattern);
  if (queryMatch && queryMatch[1] !== undefined && queryMatch[2] !== undefined) {
    const lat = parseFloat(queryMatch[1]);
    const lon = parseFloat(queryMatch[2]);
    const validation = validateCoordinates(lat, lon);
    if (validation.valid) {
      return {
        coordinates: { latitude: lat, longitude: lon, depth_reference: depthReference },
        format: 'DD',
        original: input,
      };
    }
  }

  // Pattern 2b: For place URLs, fall back to @ as last resort
  if (isGooglePlace && googleAtMatch?.[1] !== undefined && googleAtMatch[2] !== undefined) {
    const lat = parseFloat(googleAtMatch[1]);
    const lon = parseFloat(googleAtMatch[2]);
    const validation = validateCoordinates(lat, lon);
    if (validation.valid) {
      return {
        coordinates: { latitude: lat, longitude: lon, depth_reference: depthReference },
        format: 'DD',
        original: input,
      };
    }
  }

  // OpenStreetMap marker: ?mlat=..&mlon=.. is the shared point; #map= is only
  // the view centre, which differs as soon as the map was panned.
  const osmMarkerLat = trimmed.match(/[?&]mlat=(-?\d+\.?\d*)/);
  const osmMarkerLon = trimmed.match(/[?&]mlon=(-?\d+\.?\d*)/);
  if (osmMarkerLat?.[1] !== undefined && osmMarkerLon?.[1] !== undefined) {
    const lat = parseFloat(osmMarkerLat[1]);
    const lon = parseFloat(osmMarkerLon[1]);
    if (validateCoordinates(lat, lon).valid) {
      return {
        coordinates: { latitude: lat, longitude: lon, depth_reference: depthReference },
        format: 'DD',
        original: input,
      };
    }
  }

  // OpenStreetMap pattern:
  // - https://www.openstreetmap.org/#map=15/48.8584/2.2945
  const osmPattern = /#map=\d+\/(-?\d+\.?\d*)\/(-?\d+\.?\d*)/;
  const osmMatch = trimmed.match(osmPattern);
  if (osmMatch && osmMatch[1] !== undefined && osmMatch[2] !== undefined) {
    const lat = parseFloat(osmMatch[1]);
    const lon = parseFloat(osmMatch[2]);
    const validation = validateCoordinates(lat, lon);
    if (validation.valid) {
      return {
        coordinates: { latitude: lat, longitude: lon, depth_reference: depthReference },
        format: 'DD',
        original: input,
      };
    }
  }

  // Bing Maps pattern:
  // - https://www.bing.com/maps?cp=48.8584~2.2945
  const bingPattern = /cp=(-?\d+\.?\d*)~(-?\d+\.?\d*)/;
  const bingMatch = trimmed.match(bingPattern);
  if (bingMatch && bingMatch[1] !== undefined && bingMatch[2] !== undefined) {
    const lat = parseFloat(bingMatch[1]);
    const lon = parseFloat(bingMatch[2]);
    const validation = validateCoordinates(lat, lon);
    if (validation.valid) {
      return {
        coordinates: { latitude: lat, longitude: lon, depth_reference: depthReference },
        format: 'DD',
        original: input,
      };
    }
  }

  return {
    message: i18n.t('validation.gps.mapUrlNoCoordinates'),
    original: input,
  };
}

/**
 * Detect the format of a GPS coordinate string.
 * Returns the most likely format or undefined if unknown.
 */
export function detectFormat(input: string): GpsFormat | undefined {
  // No format for text too long to be coordinates; a link has none either.
  if (input.trim().length > MAX_COORDINATE_TEXT_LENGTH) return undefined;
  const trimmed = normalizeGpsWhitespace(input);

  // URL detection (highest priority)
  if (trimmed.includes('http') || (trimmed.includes('maps') && trimmed.includes('/'))) {
    return undefined; // URLs handled separately
  }

  // What3words: word.word.word or ///word.word.word
  // Must be exactly 3 words with only letters
  if (/^(\/\/\/)?[a-z]+\.[a-z]+\.[a-z]+$/i.test(trimmed)) {
    return 'What3words';
  }

  // Plus Codes: 8 chars + '+' + 2+ chars from OLC alphabet
  if (/^[23456789CFGHJMPQRVWX]{8}\+[23456789CFGHJMPQRVWX]{2,}$/i.test(trimmed)) {
    return 'PlusCodes';
  }

  // MGRS: zone + band + 2 letters + even digits
  if (/^\d{1,2}[A-Za-z]\s*[A-Za-z]{2}\s*\d+$/i.test(trimmed.replace(/\s/g, ''))) {
    return 'MGRS';
  }

  // UTM: zone + band + easting + northing with separators (decimals allowed, as parseUTM does)
  if (/^\d{1,2}\s*[A-Za-z][,\s]+\d+\.?\d*[,\s]+\d+\.?\d*$/.test(trimmed)) {
    return 'UTM';
  }

  // DMS with symbols: has degree AND minute AND second markers
  if (/\d+[°]\s*\d+[′']\s*\d+\.?\d*[″"]/.test(trimmed)) {
    return 'DMS';
  }

  // DMS space-separated: 3 numbers + direction for both lat and lon
  if (/^\d+\s+\d+\s+\d+\.?\d*\s*[NSns]\s+\d+\s+\d+\s+\d+\.?\d*\s*[EWew]$/i.test(trimmed)) {
    return 'DMS';
  }

  // DDM: degree symbol + decimal minutes + direction (a letter, not the start of a word)
  if (/\d+[°]\s*\d+\.\d+[′']?\s*[NSEW](?![A-Za-z])/i.test(trimmed)) {
    return 'DDM';
  }

  // DD: two decimal numbers separated by comma or space
  if (/^-?\d+\.?\d*[,\s]+-?\d+\.?\d*$/.test(trimmed)) {
    return 'DD';
  }

  return undefined;
}

/**
 * Auto-detect format and parse GPS coordinates.
 * Tries each parser in order of likelihood.
 *
 * See `parseDD` for `depthReference`.
 */
export function parseGps(input: string, depthReference = 0): GpsParseResult | GpsParseError {
  if (tooLongForCoordinates(input)) {
    return {
      message: i18n.t('validation.gps.inputTooLong', {
        length: input.trim().length,
        max: maxCoordinateTextLength(input.trim()),
      }),
      original: input,
    };
  }
  const trimmed = normalizeGpsWhitespace(input);

  if (!trimmed) {
    return {
      message: i18n.t('validation.gps.emptyInput'),
      original: input,
    };
  }

  // Try URL extraction first (check for URL-like patterns)
  const isGeoUri = /^geo:/i.test(trimmed);
  if (
    trimmed.includes('http') ||
    (trimmed.includes('maps') && trimmed.includes('/')) ||
    trimmed.includes('@') ||
    isGeoUri
  ) {
    const urlResult = parseUrl(trimmed, depthReference);
    if ('coordinates' in urlResult) {
      // A link states latitude and longitude by name or position; never "swapped".
      return { ...urlResult, orderExplicit: true };
    }
    // Fall through to try other formats only if @ is not in a URL context
    if (trimmed.includes('http') || trimmed.includes('maps') || isGeoUri) {
      return urlResult; // Return URL error for actual URLs
    }
  }

  const result = parseGpsText(canonicalizeGpsSymbols(trimmed), depthReference, input);
  // A dash where a minus sign stands was read as one (`DASH_AS_MINUS`): say so
  // where that changed the reading. A dash the result does not use (one in a
  // label, `Pos <dash>1: ...`) gives the same coordinates when left alone.
  const dash = [...trimmed.matchAll(DASH_AS_MINUS)][0]?.[2];
  if (dash === undefined || !('coordinates' in result)) return result;
  const dashLeftAlone = parseGpsText(canonicalizeGpsSymbols(trimmed, false), depthReference, input);
  if (
    'coordinates' in dashLeftAlone &&
    dashLeftAlone.coordinates.latitude === result.coordinates.latitude &&
    dashLeftAlone.coordinates.longitude === result.coordinates.longitude
  ) {
    return result;
  }
  const dashNote = i18n.t('validation.gps.dashReadAsMinus', { dash });
  return { ...result, note: result.note === undefined ? dashNote : `${dashNote} ${result.note}` };
}

/** `parseGps` for input that is not a link, with look-alike marks mapped to ASCII. */
function parseGpsText(text: string, depthReference: number, input: string): GpsParseResult | GpsParseError {
  const format = detectFormat(text);

  // Grid references and codes have their own parsers.
  switch (format) {
    case 'UTM':
      return parseUTM(text, depthReference);
    case 'MGRS':
      return parseMGRS(text);
    case 'PlusCodes':
      return parsePlusCodes(text, depthReference);
    case 'What3words':
      return parseWhat3words(text);
  }

  // Any way of writing a degree pair, read strictly (whole input).
  const degrees = parseDegreePair(text, depthReference, input);
  if (degrees) return degrees;

  // Not shaped like a bare pair (e.g. "Entrance: 20°30'15"N 87°18'27"W"): the
  // per-format parsers, which find the coordinates inside the text.
  switch (format) {
    case 'DD':
      return parseDD(text, depthReference);
    case 'DMS':
      return parseDMS(text, depthReference);
    case 'DDM':
      return parseDDM(text, depthReference);
  }

  // Try all parsers in order (DD first as most common)
  const parsers: Array<[GpsFormat, (s: string) => GpsParseResult | GpsParseError]> = [
    ['DD', (s) => parseDD(s, depthReference)],
    ['DMS', (s) => parseDMS(s, depthReference)],
    ['DDM', (s) => parseDDM(s, depthReference)],
    ['UTM', (s) => parseUTM(s, depthReference)],
    ['MGRS', parseMGRS],
    ['PlusCodes', (s) => parsePlusCodes(s, depthReference)],
    ['What3words', parseWhat3words],
  ];

  for (const [, parser] of parsers) {
    const result = parser(text);
    if ('coordinates' in result) {
      return result;
    }
  }

  return {
    message: i18n.t('validation.gps.couldNotParse'),
    original: input,
  };
}

/**
 * Whether to show the "latitude and longitude may be swapped" hint for what the
 * user typed. Only an implicit order (two bare numbers, signed degrees) can be
 * swapped: hemisphere letters, a grid reference, a Plus Code or a map link
 * already say which value is the latitude, so a far-north UTM point (Svalbard)
 * or "78.9 N, 15.5 E" does not trip it. For input that failed to parse, the
 * raw-pair check (`looksTransposedRawDD`) applies as before.
 */
export function gpsSwapHint(input: string, result: GpsParseResult | GpsParseError): boolean {
  if ('coordinates' in result) {
    return (
      result.orderExplicit !== true &&
      looksTransposed(result.coordinates.latitude, result.coordinates.longitude)
    );
  }
  return looksTransposedRawDD(input);
}

/**
 * Format GPS coordinates to a specific format.
 * Supports DD, DMS, and DDM output formats.
 *
 * For unsupported formats (UTM, MGRS, PlusCodes, What3words), falls back
 * to DD format silently. This is intentional to avoid breaking UI code
 * that may pass any format. For strict format handling, check the format
 * before calling this function.
 */
export function formatGpsCoordinates(
  coords: GpsCoordinates,
  format: GpsFormat = 'DD',
  precision: number = 6
): string {
  const { latitude, longitude } = coords;

  switch (format) {
    case 'DD':
      return `${formatFixed(latitude, undefined, precision)}, ${formatFixed(longitude, undefined, precision)}`;

    case 'DMS': {
      const latDir = latitude >= 0 ? 'N' : 'S';
      const lonDir = longitude >= 0 ? 'E' : 'W';
      const absLat = Math.abs(latitude);
      const absLon = Math.abs(longitude);
      const latDeg = Math.floor(absLat);
      const latMin = Math.floor((absLat - latDeg) * 60);
      const latSec = ((absLat - latDeg) * 60 - latMin) * 60;
      const lonDeg = Math.floor(absLon);
      const lonMin = Math.floor((absLon - lonDeg) * 60);
      const lonSec = ((absLon - lonDeg) * 60 - lonMin) * 60;
      return `${latDeg}°${latMin}'${formatFixed(latSec, undefined, 2)}"${latDir} ${lonDeg}°${lonMin}'${formatFixed(lonSec, undefined, 2)}"${lonDir}`;
    }

    case 'DDM': {
      const latDir = latitude >= 0 ? 'N' : 'S';
      const lonDir = longitude >= 0 ? 'E' : 'W';
      const absLat = Math.abs(latitude);
      const absLon = Math.abs(longitude);
      const latDeg = Math.floor(absLat);
      const latMin = (absLat - latDeg) * 60;
      const lonDeg = Math.floor(absLon);
      const lonMin = (absLon - lonDeg) * 60;
      return `${latDeg}°${formatFixed(latMin, undefined, 4)}'${latDir} ${lonDeg}°${formatFixed(lonMin, undefined, 4)}'${lonDir}`;
    }

    case 'UTM':
    case 'MGRS':
    case 'PlusCodes':
    case 'What3words':
      // These formats require complex encoding - fall back to DD with warning
      // Throwing would break UI, so return DD with a note
      return `${formatFixed(latitude, undefined, precision)}, ${formatFixed(longitude, undefined, precision)}`;

    default:
      return `${formatFixed(latitude, undefined, precision)}, ${formatFixed(longitude, undefined, precision)}`;
  }
}
