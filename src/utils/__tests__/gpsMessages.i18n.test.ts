/**
 * [#576 NF-W6-3] Every parser message in src/utils/gps.ts comes from en.json
 * (validation.gps.*), like the notes and messages that already did. The GPS
 * fields of EditStationDialog / ZeroDatumConfigDialog show these messages.
 *
 * Per bad input: (1) the message is the exact English gps.ts produced before the
 * move (snapshotted below, so no wording drifted and the input still lands on
 * the same branch); (2) the key exists and the message is what i18n renders for
 * it; (3) with the key's value swapped for a marker the parser returns the
 * marker, so a message re-hard-coded in gps.ts fails even though its English
 * would still look right.
 *
 * Real i18n (not a key-echo mock).
 */
import { afterEach, describe, expect, it } from 'vitest';

import i18n from '../../i18n';
import {
  parseDD,
  parseDDM,
  parseDMS,
  parseGps,
  parseMGRS,
  parsePlusCodes,
  parseUrl,
  parseUTM,
  parseWhat3words,
  validateCoordinates,
  type GpsParseError,
  type GpsParseResult,
} from '../gps';

const message = (result: GpsParseResult | GpsParseError): string =>
  'message' in result ? result.message : `(parsed: ${JSON.stringify(result.coordinates)})`;

const K = (name: string) => `validation.gps.${name}`;

/** [what, the parser call, key, params, the English gps.ts returned before the move]. */
const CASES: Array<[string, () => string, string, Record<string, unknown>, string]> = [
  ['DD format', () => message(parseDD('abc')), K('ddFormat'), {}, 'Invalid DD format. Expected: lat, lon (e.g., 48.8584, 2.2945)'],
  ['latitude range', () => message(parseDD('95, 10')), K('latitudeRange'), { min: -90, max: 90 }, 'Latitude must be between -90 and 90'],
  ['longitude range', () => message(parseDD('10, 200')), K('longitudeRange'), { min: -180, max: 180 }, 'Longitude must be between -180 and 180'],
  ['latitude not a number', () => validateCoordinates(NaN, 0).errors.join('; '), K('latitudeInvalid'), {}, 'Latitude must be a valid number'],
  ['longitude not a number', () => validateCoordinates(0, Infinity).errors.join('; '), K('longitudeInvalid'), {}, 'Longitude must be a valid number'],
  ['DMS format', () => message(parseDMS('abc')), K('dmsFormat'), {}, "Invalid DMS format. Expected: deg°min'sec\"N deg°min'sec\"E or deg min sec N deg min sec E (direction letters or signed degrees required)"],
  ['DMS minutes range', () => message(parseDMS(`Entrance 48°61'30"N 2°17'40"E`)), K('dmsMinutesRange'), {}, 'Minutes must be between 0 and 59'],
  ['DMS seconds range', () => message(parseDMS(`Entrance 48°51'61"N 2°17'40"E`)), K('secondsRange'), {}, 'Seconds must be between 0 and 59.99'],
  ['DDM format', () => message(parseDDM('abc')), K('ddmFormat'), {}, "Invalid DDM format. Expected: deg°min.dec'N deg°min.dec'E (e.g., 48°51.5'N 2°17.67'E)"],
  ['DDM minutes range', () => message(parseDDM(`Entrance 48°61.5'N 2°17.67'E`)), K('ddmMinutesRange'), {}, 'Minutes must be between 0 and 59.99'],
  ['degree pair, DMS minutes range', () => message(parseGps(`48°61'30"N 2°17'40"E`)), K('dmsMinutesRange'), {}, 'Minutes must be between 0 and 59'],
  ['degree pair, DDM minutes range', () => message(parseGps(`48°61.5'N 2°17.67'E`)), K('ddmMinutesRange'), {}, 'Minutes must be between 0 and 59.99'],
  ['degree pair, seconds range', () => message(parseGps(`48°51'61"N 2°17'40"E`)), K('secondsRange'), {}, 'Seconds must be between 0 and 59.99'],
  ['UTM format', () => message(parseUTM('abc')), K('utmFormat'), {}, 'Invalid UTM format. Expected: zone band easting northing (e.g., 31U 448251 5411932 or 31 U 448251 5411932)'],
  ['UTM zone range', () => message(parseUTM('61U 448251 5411932')), K('utmZoneRange'), {}, 'UTM zone must be between 1 and 60'],
  ['UTM band letter', () => message(parseUTM('31I 448251 5411932')), K('utmBandLetter'), {}, 'Invalid UTM band letter (must be C-X, excluding I and O)'],
  ['UTM easting range', () => message(parseUTM('31U 50000 5411932')), K('utmEastingRange'), {}, 'UTM easting typically between 100000 and 900000'],
  ['UTM northing range', () => message(parseUTM('31U 448251 10000001')), K('utmNorthingRange'), {}, 'UTM northing must be between 0 and 10000000'],
  ['MGRS format', () => message(parseMGRS('abc')), K('mgrsFormat'), {}, 'Invalid MGRS format. Expected: zoneNumberBandLetter 100kmSquare coordinates (e.g., 31UDQ4825111932)'],
  ['MGRS zone range', () => message(parseMGRS('61UDQ4825111932')), K('mgrsZoneRange'), {}, 'MGRS zone must be between 1 and 60'],
  ['MGRS band letter', () => message(parseMGRS('31IDQ4825111932')), K('mgrsBandLetter'), {}, 'Invalid MGRS band letter (must be C-X, excluding I and O)'],
  ['MGRS digits', () => message(parseMGRS('31UDQ482511193')), K('mgrsDigits'), {}, 'MGRS coordinates must have even length between 2 and 10 digits'],
  ['MGRS needs a library', () => message(parseMGRS('31UDQ4825111932')), K('mgrsNeedsLibrary'), {}, 'MGRS format recognized but conversion requires a dedicated library. Please use DD, DMS, DDM, UTM, or Plus Codes for offline coordinate conversion.'],
  ['Plus Code format', () => message(parsePlusCodes('abc')), K('plusCodeFormat'), {}, 'Invalid Plus Code format. Expected: 8 characters + "+" + 2+ characters (e.g., 8FW4V75V+8Q)'],
  ['What3words format', () => message(parseWhat3words('a.b')), K('what3wordsFormat'), {}, 'Invalid What3words format. Expected: word.word.word (e.g., filled.count.soap)'],
  ['What3words needs the API', () => message(parseWhat3words('filled.count.soap')), K('what3wordsNeedsApi'), {}, 'What3words requires API access for conversion. Please use DD, DMS, DDM, UTM, MGRS, or Plus Codes for offline use.'],
  ['not a map link', () => message(parseUrl('abc')), K('notMapUrl'), {}, 'Input does not appear to be a map URL'],
  ['map link without coordinates', () => message(parseUrl('https://example.com/maps/somewhere')), K('mapUrlNoCoordinates'), {}, 'Could not extract coordinates from URL. Supported: Google Maps (place name, @lat,lon, or ?q=lat,lon), OpenStreetMap (#map=zoom/lat/lon), Bing Maps (cp=lat~lon)'],
  ['empty input', () => message(parseGps('   ')), K('emptyInput'), {}, 'Empty input'],
  ['no format fits', () => message(parseGps('hello world')), K('couldNotParse'), {}, `Could not parse GPS coordinates. Supported formats: DD (48.8584, 2.2945), DMS (48°51'30"N 2°17'40"E), DDM (48°51.5'N 2°17.67'E), UTM (31U 448251 5411932), MGRS (31UDQ4825111932), Plus Codes (8FW4V75V+8Q), URL (Google/OSM/Bing Maps links), What3words (requires API)`],
];

/**
 * Messages with no input that reaches them: `decodeOpenLocationCode` only sees
 * codes `parsePlusCodes`'s pattern already limited to the Plus Code alphabet.
 * Checked at the key: it exists and renders the English the template produced.
 */
const UNREACHABLE: Array<[string, Record<string, unknown>, string]> = [
  [K('plusCodeMissingCharacter'), { position: 4 }, 'Invalid Plus Code: missing character at position 4'],
  [K('plusCodeInvalidCharacter'), { character: 'A', position: 3 }, "Invalid Plus Code character 'A' at position 3"],
];

const saved = new Map<string, unknown>();

/** The marker `mark` puts in place of `key`'s value: the key plus the value's placeholders. */
function markerOf(key: string, original: unknown): string {
  const placeholders = typeof original === 'string' ? original.match(/\{\{\w+\}\}/g) ?? [] : [];
  return `@@${key}${placeholders.map((p) => `|${p}`).join('')}@@`;
}

function mark(key: string): void {
  const original: unknown = i18n.getResource('en', 'translation', key);
  if (!saved.has(key)) saved.set(key, original);
  i18n.addResource('en', 'translation', key, markerOf(key, original));
}

afterEach(() => {
  for (const [key, value] of saved) i18n.addResource('en', 'translation', key, value as string);
  saved.clear();
});

describe('gps.ts parser messages come from en.json (#576 NF-W6-3)', () => {
  it.each(CASES)('%s: the old English, rendered from its key', (_what, run, key, params, english) => {
    expect(run()).toBe(english);
    expect(i18n.exists(key)).toBe(true);
    expect(run()).toBe(i18n.t(key, params));
  });

  it.each(CASES)('%s: a marker in en.json reaches the message', (_what, run, key, params) => {
    mark(key);
    const expected = Object.entries(params).reduce(
      (text, [name, value]) => text.replace(`{{${name}}}`, String(value)),
      markerOf(key, saved.get(key)),
    );
    expect(run()).toBe(expected);
  });

  it('two range errors still join with "; "', () => {
    expect(message(parseDD('95, 200'))).toBe('Latitude must be between -90 and 90; Longitude must be between -180 and 180');
  });

  it.each(UNREACHABLE)('%s renders the old English', (key, params, english) => {
    expect(i18n.exists(key)).toBe(true);
    expect(i18n.t(key, params)).toBe(english);
  });
});
