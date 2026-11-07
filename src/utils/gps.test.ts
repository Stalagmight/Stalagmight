/**
 * Unit tests for GPS parsing utilities.
 * Tests all 7 formats: DD, DMS, DDM, UTM, MGRS, Plus Codes, What3words.
 */

import { describe, it, expect } from 'vitest';
import {
  parseDD,
  parseDMS,
  parseDDM,
  parseUTM,
  parseMGRS,
  parsePlusCodes,
  parseWhat3words,
  parseUrl,
  parseGps,
  detectFormat,
  validateCoordinates,
  formatGpsCoordinates,
  looksTransposed,
  looksTransposedRawDD,
  gpsSwapHint,
  type GpsParseResult,
  type GpsParseError,
} from './gps';

// Helper to check if result is successful
function isSuccess(result: GpsParseResult | GpsParseError): result is GpsParseResult {
  return 'coordinates' in result;
}

// Helper to check coordinates are close (for floating point comparison)
function expectCoordsClose(
  result: GpsParseResult | GpsParseError,
  expectedLat: number,
  expectedLon: number,
  _tolerance = 0.0001
) {
  expect(isSuccess(result)).toBe(true);
  if (isSuccess(result)) {
    expect(result.coordinates.latitude).toBeCloseTo(expectedLat, 4);
    expect(result.coordinates.longitude).toBeCloseTo(expectedLon, 4);
  }
}

describe('validateCoordinates', () => {
  it('accepts valid coordinates', () => {
    const result = validateCoordinates(48.8584, 2.2945);
    expect(result.valid).toBe(true);
    expect(result.errors).toHaveLength(0);
  });

  it('rejects latitude out of range', () => {
    const result = validateCoordinates(91, 0);
    expect(result.valid).toBe(false);
    expect(result.errors[0]).toContain('Latitude');
  });

  it('rejects longitude out of range', () => {
    const result = validateCoordinates(0, 181);
    expect(result.valid).toBe(false);
    expect(result.errors[0]).toContain('Longitude');
  });

  it('rejects NaN values', () => {
    const result = validateCoordinates(NaN, 0);
    expect(result.valid).toBe(false);
  });
});

describe('parseDD', () => {
  it('parses comma-separated coordinates', () => {
    const result = parseDD('48.8584, 2.2945');
    expectCoordsClose(result, 48.8584, 2.2945);
    if (isSuccess(result)) {
      expect(result.format).toBe('DD');
    }
  });

  it('parses space-separated coordinates', () => {
    const result = parseDD('48.8584 2.2945');
    expectCoordsClose(result, 48.8584, 2.2945);
  });

  it('parses negative coordinates (southern/western)', () => {
    const result = parseDD('-33.8688, 151.2093');
    expectCoordsClose(result, -33.8688, 151.2093);
  });

  it('parses both negative coordinates', () => {
    const result = parseDD('-33.8688, -70.6693');
    expectCoordsClose(result, -33.8688, -70.6693);
  });

  it('rejects invalid format', () => {
    const result = parseDD('invalid');
    expect(isSuccess(result)).toBe(false);
    if (!isSuccess(result)) {
      expect(result.format).toBe('DD');
    }
  });

  it('rejects out-of-range latitude', () => {
    const result = parseDD('91.0, 2.0');
    expect(isSuccess(result)).toBe(false);
  });
});

describe('parseDMS', () => {
  it('parses standard DMS format with symbols', () => {
    // 48°51'30"N 2°17'40"E = 48.8583, 2.2944 approximately
    const result = parseDMS('48°51\'30"N 2°17\'40"E');
    expectCoordsClose(result, 48.8583, 2.2944);
    if (isSuccess(result)) {
      expect(result.format).toBe('DMS');
    }
  });

  it('parses DMS with space separators', () => {
    const result = parseDMS('48 51 30 N 2 17 40 E');
    expectCoordsClose(result, 48.8583, 2.2944);
  });

  it('parses southern/western coordinates', () => {
    const result = parseDMS('33°51\'54"S 151°12\'33"E');
    expectCoordsClose(result, -33.865, 151.2092);
  });

  it('handles decimal seconds', () => {
    const result = parseDMS('48°51\'30.5"N 2°17\'40.5"E');
    expect(isSuccess(result)).toBe(true);
  });

  it('rejects invalid minutes (>= 60)', () => {
    const result = parseDMS('48°60\'30"N 2°17\'40"E');
    expect(isSuccess(result)).toBe(false);
  });

  it('rejects invalid seconds (>= 60)', () => {
    const result = parseDMS('48°51\'60"N 2°17\'40"E');
    expect(isSuccess(result)).toBe(false);
  });

  it('parses signed DMS with negative latitude', () => {
    // -33°51'54" 151°12'33" (Sydney) - negative lat, positive lon
    const result = parseDMS('-33°51\'54" 151°12\'33"');
    expectCoordsClose(result, -33.865, 151.2092);
    if (isSuccess(result)) {
      expect(result.format).toBe('DMS');
    }
  });

  it('parses signed DMS with negative longitude', () => {
    // 40°42'46" -74°0'22" (New York) - positive lat, negative lon
    const result = parseDMS('40°42\'46" -74°0\'22"');
    expectCoordsClose(result, 40.7128, -74.0061);
    if (isSuccess(result)) {
      expect(result.format).toBe('DMS');
    }
  });

  it('parses signed DMS with both negative', () => {
    // -33°26'16" -70°39'1" (Santiago, Chile)
    const result = parseDMS('-33°26\'16" -70°39\'1"');
    expectCoordsClose(result, -33.4378, -70.6503);
    if (isSuccess(result)) {
      expect(result.format).toBe('DMS');
    }
  });

  it('rejects ambiguous DMS without direction or sign', () => {
    // Both positive without direction letters = ambiguous
    const result = parseDMS('33°51\'54" 151°12\'33"');
    expect(isSuccess(result)).toBe(false);
  });

  // A `-0` degree token is signed: parseInt('-0') is -0, which fails every
  // `< 0` test, so the sign has to come from the text (#576 D39e).
  it('keeps the sign of a -0 latitude degree when the longitude is also negative', () => {
    const result = parseGps('-0°12\'30" -10°30\'00"');
    expectCoordsClose(result, -0.208333, -10.5);
    if (isSuccess(result)) {
      expect(result.format).toBe('DMS');
      expect(result.coordinates.latitude).toBeLessThan(0);
    }
  });

  it('accepts a -0 latitude degree as the signed component', () => {
    const result = parseGps('-0°30\'00" 11°00\'00"');
    expectCoordsClose(result, -0.5, 11.0);
    if (isSuccess(result)) {
      expect(result.format).toBe('DMS');
    }
  });

  it('accepts a -0 longitude degree as the signed component', () => {
    const result = parseGps('10°00\'00" -0°30\'00"');
    expectCoordsClose(result, 10.0, -0.5);
  });

  it('still rejects an unsigned 0 degree without direction letters', () => {
    const result = parseGps('0°30\'00" 11°00\'00"');
    expect(isSuccess(result)).toBe(false);
  });

  it('applies a -0 degree sign alongside direction letters like any other negative degree', () => {
    // Existing grammar: a leading '-' forces S/W even next to N/E.
    expectCoordsClose(parseDMS('-1°30\'00"N 11°00\'00"E'), -1.5, 11.0);
    expectCoordsClose(parseDMS('-0°30\'00"N 11°00\'00"E'), -0.5, 11.0);
    expectCoordsClose(parseDMS('-0 30 00 N 11 00 00 E'), -0.5, 11.0);
  });
});

describe('parseDDM', () => {
  it('parses standard DDM format', () => {
    // 48°51.5'N 2°17.67'E
    const result = parseDDM("48°51.5'N 2°17.67'E");
    expectCoordsClose(result, 48.8583, 2.2945);
    if (isSuccess(result)) {
      expect(result.format).toBe('DDM');
    }
  });

  it('requires direction letters for disambiguation', () => {
    // DDM now requires direction letters to avoid ambiguity with DMS
    const result = parseDDM("48°51.5' 2°17.67'");
    expect(isSuccess(result)).toBe(false);
  });

  it('parses southern/western coordinates', () => {
    const result = parseDDM("33°51.9'S 151°12.55'E");
    expectCoordsClose(result, -33.865, 151.2092);
  });

  it('rejects invalid minutes (>= 60)', () => {
    const result = parseDDM("48°60.5'N 2°17.67'E");
    expect(isSuccess(result)).toBe(false);
  });
});

describe('parseUTM', () => {
  it('parses standard UTM format', () => {
    // 31U 448251 5411932 is near Paris
    const result = parseUTM('31U 448251 5411932');
    expect(isSuccess(result)).toBe(true);
    if (isSuccess(result)) {
      expect(result.format).toBe('UTM');
      // Approximate check - UTM conversion has some tolerance
      expect(result.coordinates.latitude).toBeGreaterThan(48);
      expect(result.coordinates.latitude).toBeLessThan(50);
    }
  });

  it('parses UTM with comma separators', () => {
    const result = parseUTM('31U, 448251, 5411932');
    expect(isSuccess(result)).toBe(true);
  });

  it('rejects invalid zone number', () => {
    const result = parseUTM('61U 448251 5411932');
    expect(isSuccess(result)).toBe(false);
  });

  it('rejects invalid band letter', () => {
    const result = parseUTM('31I 448251 5411932');
    expect(isSuccess(result)).toBe(false);
  });
});

describe('parseMGRS', () => {
  // Note: parseMGRS validates format only - full MGRS conversion requires a dedicated library.
  // These tests validate format recognition and validation logic.

  it('validates valid MGRS format', () => {
    // 31UDQ4825111932 is near Paris
    // Format validation should succeed but not return coordinates (requires library)
    const result = parseMGRS('31UDQ4825111932');
    expect(isSuccess(result)).toBe(false);
    if (!isSuccess(result)) {
      expect(result.format).toBe('MGRS');
      expect(result.message).toContain('library');
    }
  });

  it('validates different precision levels', () => {
    // Test 1km precision (4 digits) vs 1m precision (10 digits)
    const result1km = parseMGRS('31UDQ4811');
    const result1m = parseMGRS('31UDQ4825111932');
    // Both should be recognized as valid MGRS format
    expect(isSuccess(result1km)).toBe(false);
    expect(isSuccess(result1m)).toBe(false);
    if (!isSuccess(result1km) && !isSuccess(result1m)) {
      expect(result1km.format).toBe('MGRS');
      expect(result1m.format).toBe('MGRS');
      // Both should indicate library required, not format error
      expect(result1km.message).toContain('library');
      expect(result1m.message).toContain('library');
    }
  });

  it('rejects invalid zone number', () => {
    const result = parseMGRS('61ABC1234567890');
    expect(isSuccess(result)).toBe(false);
    if (!isSuccess(result)) {
      expect(result.message).toContain('zone');
    }
  });

  it('rejects invalid band letter', () => {
    const result = parseMGRS('31IAB1234512345');
    expect(isSuccess(result)).toBe(false);
    if (!isSuccess(result)) {
      expect(result.message).toContain('band');
    }
  });

  it('rejects odd-length coordinates', () => {
    const result = parseMGRS('31UDQ48251');
    expect(isSuccess(result)).toBe(false);
    if (!isSuccess(result)) {
      expect(result.message).toContain('even');
    }
  });

  it('rejects invalid format', () => {
    const result = parseMGRS('not-mgrs');
    expect(isSuccess(result)).toBe(false);
    if (!isSuccess(result)) {
      expect(result.format).toBe('MGRS');
    }
  });
});

describe('parsePlusCodes', () => {
  it('parses valid Plus Code with exact coordinates', () => {
    // 8FW4V75V+8Q is near Paris - known coordinates
    // Reference: https://plus.codes/8FW4V75V+8Q = 48.858350, 2.294495
    const result = parsePlusCodes('8FW4V75V+8Q');
    expect(isSuccess(result)).toBe(true);
    if (isSuccess(result)) {
      expect(result.format).toBe('PlusCodes');
      // Check both lat and lon with reasonable tolerance
      expect(result.coordinates.latitude).toBeCloseTo(48.8584, 3);
      expect(result.coordinates.longitude).toBeCloseTo(2.2945, 3);
    }
  });

  it('parses longer Plus Code (refinement >10 chars)', () => {
    // 8FW4V75V+8QX is a refinement of the above code (11 chars)
    // The refinement grid is 4x5, so +8QX refines +8Q further
    const result = parsePlusCodes('8FW4V75V+8QX');
    expect(isSuccess(result)).toBe(true);
    if (isSuccess(result)) {
      expect(result.format).toBe('PlusCodes');
      // Should be near same location but more precise
      expect(result.coordinates.latitude).toBeCloseTo(48.8584, 3);
      expect(result.coordinates.longitude).toBeCloseTo(2.2945, 3);
    }
  });

  it('parses lowercase Plus Code', () => {
    const result = parsePlusCodes('8fw4v75v+8q');
    expect(isSuccess(result)).toBe(true);
  });

  it('rejects invalid Plus Code format', () => {
    const result = parsePlusCodes('invalid');
    expect(isSuccess(result)).toBe(false);
    if (!isSuccess(result)) {
      expect(result.format).toBe('PlusCodes');
    }
  });

  it('rejects Plus Code with invalid characters', () => {
    const result = parsePlusCodes('8FW4V75V+01');
    expect(isSuccess(result)).toBe(false);
  });
});

describe('parseWhat3words', () => {
  it('validates What3words format', () => {
    const result = parseWhat3words('filled.count.soap');
    // Should fail with message about API requirement
    expect(isSuccess(result)).toBe(false);
    if (!isSuccess(result)) {
      expect(result.format).toBe('What3words');
      expect(result.message).toContain('API');
    }
  });

  it('validates What3words with slashes', () => {
    const result = parseWhat3words('///filled.count.soap');
    expect(isSuccess(result)).toBe(false);
    if (!isSuccess(result)) {
      expect(result.format).toBe('What3words');
    }
  });

  it('rejects invalid What3words format', () => {
    const result = parseWhat3words('invalid');
    expect(isSuccess(result)).toBe(false);
    if (!isSuccess(result)) {
      expect(result.message).toContain('Invalid');
    }
  });
});

describe('parseUrl', () => {
  it('extracts from Google Maps @ URL', () => {
    const result = parseUrl('https://www.google.com/maps/@48.8584,2.2945,15z');
    expectCoordsClose(result, 48.8584, 2.2945);
  });

  it('extracts from Google Maps place URL with text name (falls back to @)', () => {
    const result = parseUrl('https://www.google.com/maps/place/Eiffel+Tower/@48.8584,2.2945,15z');
    expectCoordsClose(result, 48.8584, 2.2945);
  });

  it('extracts DMS from Google Maps place name (short URL)', () => {
    // 20°12'02.0"N 87°29'59.8"W
    const result = parseUrl(
      'https://www.google.com/maps/place/20%C2%B012\'02.0%22N+87%C2%B029\'59.8%22W/'
    );
    expectCoordsClose(result, 20.200556, -87.499944);
  });

  it('extracts DMS from Google Maps place name over viewport @', () => {
    // The @ coordinates (20.200562,-87.5005877) are the viewport center, NOT the pin.
    // The place name DMS (20°12'02.0"N 87°29'59.8"W) is the correct location.
    const result = parseUrl(
      'https://www.google.com/maps/place/20%C2%B012\'02.0%22N+87%C2%B029\'59.8%22W/@20.200562,-87.5005877,500m/data=!3m2!1e3!4b1!4m4!3m3!8m2!3d20.200562!4d-87.499944?entry=ttu'
    );
    expectCoordsClose(result, 20.200556, -87.499944);
  });

  it('extracts !3d/!4d data params for place URLs without coordinate names', () => {
    const result = parseUrl(
      'https://www.google.com/maps/place/Some+Cave/@20.2,-87.5,500m/data=!3m2!1e3!4b1!4m4!3m3!8m2!3d20.200556!4d-87.499944'
    );
    expectCoordsClose(result, 20.200556, -87.499944);
  });

  it('extracts from Google Maps query URL', () => {
    const result = parseUrl('https://maps.google.com/?q=48.8584,2.2945');
    expectCoordsClose(result, 48.8584, 2.2945);
  });

  it('extracts from OpenStreetMap URL', () => {
    const result = parseUrl('https://www.openstreetmap.org/#map=15/48.8584/2.2945');
    expectCoordsClose(result, 48.8584, 2.2945);
  });

  it('extracts from Bing Maps URL', () => {
    const result = parseUrl('https://www.bing.com/maps?cp=48.8584~2.2945');
    expectCoordsClose(result, 48.8584, 2.2945);
  });

  it('rejects non-URL input', () => {
    const result = parseUrl('not a url');
    expect(isSuccess(result)).toBe(false);
  });
});

describe('detectFormat', () => {
  it('detects DD format', () => {
    expect(detectFormat('48.8584, 2.2945')).toBe('DD');
  });

  it('detects DMS format', () => {
    expect(detectFormat('48°51\'30"N 2°17\'40"E')).toBe('DMS');
  });

  it('detects DDM format', () => {
    expect(detectFormat("48°51.5'N 2°17.67'E")).toBe('DDM');
  });

  it('detects UTM format', () => {
    expect(detectFormat('31U 448251 5411932')).toBe('UTM');
  });

  it('detects MGRS format', () => {
    expect(detectFormat('31UDQ4825111932')).toBe('MGRS');
  });

  it('detects Plus Codes format', () => {
    expect(detectFormat('8FW4V75V+8Q')).toBe('PlusCodes');
  });

  it('detects What3words format', () => {
    expect(detectFormat('filled.count.soap')).toBe('What3words');
    expect(detectFormat('///filled.count.soap')).toBe('What3words');
  });

  it('returns undefined for URLs', () => {
    expect(detectFormat('https://maps.google.com')).toBeUndefined();
  });
});

describe('parseGps', () => {
  it('auto-detects and parses DD', () => {
    const result = parseGps('48.8584, 2.2945');
    expectCoordsClose(result, 48.8584, 2.2945);
  });

  it('auto-detects and parses DMS', () => {
    const result = parseGps('48°51\'30"N 2°17\'40"E');
    expect(isSuccess(result)).toBe(true);
  });

  it('auto-detects and parses URLs', () => {
    const result = parseGps('https://www.google.com/maps/@48.8584,2.2945,15z');
    expectCoordsClose(result, 48.8584, 2.2945);
  });

  it('returns error for empty input', () => {
    const result = parseGps('');
    expect(isSuccess(result)).toBe(false);
    if (!isSuccess(result)) {
      expect(result.message).toContain('Empty');
    }
  });

  it('returns error for unparseable input', () => {
    const result = parseGps('completely invalid garbage');
    expect(isSuccess(result)).toBe(false);
  });

  it('parses DD with extra internal whitespace and newlines', () => {
    const result = parseGps('  48.8584  ,\n   2.2945  ');
    expectCoordsClose(result, 48.8584, 2.2945);
  });

  it('parses Plus Codes with inserted whitespace', () => {
    const result = parseGps(' 8FW4V75V + 8Q ');
    expect(isSuccess(result)).toBe(true);
    if (isSuccess(result)) {
      expect(result.format).toBe('PlusCodes');
    }
  });
});

describe('formatGpsCoordinates', () => {
  const coords = { latitude: 48.8584, longitude: 2.2945, depth_reference: 0 };

  it('formats as DD', () => {
    const result = formatGpsCoordinates(coords, 'DD', 4);
    expect(result).toBe('48.8584, 2.2945');
  });

  it('formats as DMS', () => {
    const result = formatGpsCoordinates(coords, 'DMS');
    expect(result).toContain('°');
    expect(result).toContain('N');
    expect(result).toContain('E');
  });

  it('formats as DDM', () => {
    const result = formatGpsCoordinates(coords, 'DDM');
    expect(result).toContain('°');
    expect(result).toContain('N');
    expect(result).toContain('E');
  });

  it('handles negative coordinates in DMS', () => {
    const southWest = { latitude: -33.8688, longitude: -151.2093, depth_reference: 0 };
    const result = formatGpsCoordinates(southWest, 'DMS');
    expect(result).toContain('S');
    expect(result).toContain('W');
  });

  it('falls back to DD for unsupported formats', () => {
    // UTM/MGRS/PlusCodes formatting not implemented, falls back to DD
    const result = formatGpsCoordinates(coords, 'UTM');
    expect(result).toContain('48.8584');
  });
});

describe('looksTransposed', () => {
  // Injection check: a stub that always returns `false` would still pass a
  // suite that only asserts "does not fire on plausible input" - every test
  // below pairs a firing case with a structurally close non-firing one, so a
  // constant-false (or constant-true) implementation fails at least one.

  it('fires when the given latitude is extreme and the longitude reads as a plausible latitude', () => {
    // True fix would be lat 12, lon 77 (Bengaluru); entered swapped.
    expect(looksTransposed(77, 12)).toBe(true);
  });

  it('fires in the other direction (negative extreme latitude)', () => {
    // True fix would be lat -9, lon -80; entered swapped.
    expect(looksTransposed(-80, -9)).toBe(true);
  });

  it('does not fire on a plausible, non-extreme pair', () => {
    // Paris: neither value is past the threshold.
    expect(looksTransposed(48.8584, 2.2945)).toBe(false);
  });

  // Regression: a 60-degree threshold false-positived on real diving/caving
  // latitudes. Iceland, Norway and Sweden all sit past 60 N.
  it('does not fire on real high-latitude dive/cave regions (Iceland)', () => {
    expect(looksTransposed(64.25, -21.12)).toBe(false);
  });

  it('does not fire on real high-latitude dive/cave regions (Norway)', () => {
    expect(looksTransposed(66, 14)).toBe(false);
  });

  it('does not fire on real high-latitude dive/cave regions (Sweden)', () => {
    expect(looksTransposed(63, 15)).toBe(false);
  });

  it('does not fire when both values are genuinely extreme (real polar survey)', () => {
    expect(looksTransposed(78, -80)).toBe(false);
  });

  it('does not fire when the latitude is extreme but swapping would not help', () => {
    // Swapped longitude (77) is itself past the threshold - not a plausible latitude either.
    expect(looksTransposed(85, 77)).toBe(false);
  });

  // NaN never satisfies a numeric comparison in either direction, so this
  // holds regardless of the explicit `Number.isFinite` guard in the
  // implementation - it documents the behavior rather than pinning the guard
  // itself (see the guard's own comment for why nothing CAN make it
  // load-bearing here).
  it('does not fire on a non-finite latitude', () => {
    expect(looksTransposed(NaN, 12)).toBe(false);
  });
});

describe('looksTransposedRawDD', () => {
  // Regression: `parseDD("120, 45")` rejects 120 as an impossible latitude
  // before any `GpsCoordinates` exist, so the coords-based `looksTransposed`
  // never runs and the swap hint silently never showed up next to the range
  // error. This is the raw-input path that fills that gap.

  it('fires on a swapped pair where the first field is an impossible latitude', () => {
    // 120 cannot be a latitude at all; swapped (45, 120) is a valid DD pair.
    expect(looksTransposedRawDD('120, 45')).toBe(true);
  });

  it('fires on the same shape space-separated', () => {
    expect(looksTransposedRawDD('120 45')).toBe(true);
  });

  it('fires just past the boundary too (95, 45) - a swap genuinely fixes it', () => {
    // Deliberately not carved out: 95 is exactly as impossible a latitude as
    // 120 is, and swapping produces a DD pair `parseDD` would accept either
    // way - there is no principled reason to withhold the hint here.
    expect(looksTransposedRawDD('95, 45')).toBe(true);
  });

  it('does not fire when the first field is already a valid latitude', () => {
    expect(looksTransposedRawDD('48.8584, 2.2945')).toBe(false);
  });

  it('does not fire when swapping would not fix it either (both fields impossible)', () => {
    // 200 is not a valid latitude NOR a valid longitude, so swapping leaves a
    // pair `parseDD` would still reject - nothing for the hint to promise.
    expect(looksTransposedRawDD('200, 45')).toBe(false);
  });

  it('does not fire on input that is not bare DD shape', () => {
    // DMS-with-symbols input fails the same `parseDD` regex this mirrors, so
    // it must not be misread as a DD pair either.
    expect(looksTransposedRawDD('48°51\'30"N 2°17\'40"E')).toBe(false);
  });

  it('does not fire on empty input', () => {
    expect(looksTransposedRawDD('')).toBe(false);
  });
});

describe('depthReference threading (#524)', () => {
  it('defaults to 0 ("not recorded") when omitted', () => {
    const result = parseDD('48.8584, 2.2945');
    expect(isSuccess(result)).toBe(true);
    if (isSuccess(result)) expect(result.coordinates.depth_reference).toBe(0);
  });

  it('parseDD carries a positive depthReference straight through, unflipped', () => {
    // Sign convention pin: `GpsCoordinates.depth_reference` is +down (meters
    // below the surface, docs/inclination.md), so an entered positive depth
    // must land as the SAME positive number - not negated.
    const result = parseDD('48.8584, 2.2945', 12.5);
    expect(isSuccess(result)).toBe(true);
    if (isSuccess(result)) expect(result.coordinates.depth_reference).toBe(12.5);
  });

  it('parseDMS threads depthReference', () => {
    const result = parseDMS('48°51\'30"N 2°17\'40"E', 7);
    expect(isSuccess(result)).toBe(true);
    if (isSuccess(result)) expect(result.coordinates.depth_reference).toBe(7);
  });

  it('parseDDM threads depthReference', () => {
    const result = parseDDM("48°51.5'N 2°17.67'E", -3);
    expect(isSuccess(result)).toBe(true);
    if (isSuccess(result)) expect(result.coordinates.depth_reference).toBe(-3);
  });

  it('parseUTM threads depthReference', () => {
    const result = parseUTM('31U 448251 5411932', 42);
    expect(isSuccess(result)).toBe(true);
    if (isSuccess(result)) expect(result.coordinates.depth_reference).toBe(42);
  });

  it('parsePlusCodes threads depthReference', () => {
    const result = parsePlusCodes('8FW4V75V+8Q', 9);
    expect(isSuccess(result)).toBe(true);
    if (isSuccess(result)) expect(result.coordinates.depth_reference).toBe(9);
  });

  it('parseUrl threads depthReference through its DD-pattern branches', () => {
    const result = parseUrl('https://www.google.com/maps/@48.8584,2.2945,15z', 5);
    expect(isSuccess(result)).toBe(true);
    if (isSuccess(result)) expect(result.coordinates.depth_reference).toBe(5);
  });

  it('parseGps threads depthReference through format auto-detection', () => {
    const result = parseGps('48.8584, 2.2945', 15.75);
    expect(isSuccess(result)).toBe(true);
    if (isSuccess(result)) expect(result.coordinates.depth_reference).toBe(15.75);
  });
});

// ---------------------------------------------------------------------------
// Coordinate entry review (#576 D39e, D37). Expected UTM values come from PROJ
// (`cs2cs +proj=utm +zone=Z [+south] +datum=WGS84 +to +proj=longlat`), not from
// this module; degree values are plain deg + min/60 + sec/3600 arithmetic.
// ---------------------------------------------------------------------------

const DDM_LON = 87 + 18.5 / 60; // 87°18.5'

describe('parseDDM keeps the sign of a -0 degree (D39e)', () => {
  it('a -0 latitude degree stays negative, like any other minus degree', () => {
    expectCoordsClose(parseDDM("-0°30.5'N 87°18.5'W"), -30.5 / 60, -DDM_LON);
  });

  it('a -0 longitude degree stays negative', () => {
    expectCoordsClose(parseDDM("10°00.000'N -0°30.000'E"), 10, -0.5);
  });
});

/** parseGps that must succeed; returns the result for further checks. */
function parsedOk(input: string): GpsParseResult {
  const result = parseGps(input);
  if (!isSuccess(result)) {
    throw new Error(`expected ${JSON.stringify(input)} to parse, got: ${result.message}`);
  }
  return result;
}

function expectRejected(input: string, message?: RegExp) {
  const result = parseGps(input);
  expect(isSuccess(result), `${JSON.stringify(input)} should be rejected`).toBe(false);
  if (!isSuccess(result) && message) expect(result.message).toMatch(message);
}

describe('UTM band letter is checked against the northing (D37)', () => {
  it('"33S" with a southern-hemisphere northing is read as the southern hemisphere, with a note', () => {
    for (const input of ['33S 512345 8765432', '33 S 512345 8765432', '33s 512345 8765432']) {
      const result = parsedOk(input);
      expect(result.coordinates.latitude).toBeCloseTo(-11.1680928, 5);
      expect(result.coordinates.longitude).toBeCloseTo(15.1130686, 5);
      expect(result.note).toMatch(/southern hemisphere/);
    }
  });

  it('"33S" with a northing that fits band S keeps the band reading and names the southern alternative', () => {
    const result = parsedOk('33S 512345 4000000');
    expect(result.coordinates.latitude).toBeCloseTo(36.1446395, 5);
    expect(result.coordinates.longitude).toBeCloseTo(15.1372234, 5);
    // The southern reading (-54.15) is band F, so that is the unambiguous input to suggest.
    expect(result.note).toContain('33F 512345 4000000');
  });

  it('band S cave regions (Kentucky, Spain) keep their northern value', () => {
    expectCoordsClose(parsedOk('16S 612345 4123456'), 37.2508825, -85.7332107);
    expectCoordsClose(parsedOk('30S 300000 4200000'), 37.9255817, -5.2755282);
  });

  it('"33N" north of band N is read as the northern hemisphere, with a note', () => {
    const result = parsedOk('33N 512345 8765432');
    expect(result.coordinates.latitude).toBeCloseTo(78.9587568, 5);
    expect(result.note).toMatch(/northern hemisphere/);
  });

  it('"33N" inside band N needs no note (both readings are the same point)', () => {
    expect(parsedOk('33N 512345 500000').note).toBeUndefined();
  });

  it('rejects a band letter the northing cannot have', () => {
    expectRejected('33U 512345 8765432', /band U/);
    expectRejected('16R 451234 2266789', /band R/);
    expectRejected('33K 512345 8765432', /band K/);
  });

  it('rejects "S" when neither band S nor the southern hemisphere fits', () => {
    expectRejected('33S 512345 100000', /neither/);
  });

  it('rejects northings beyond the UTM limits (84N, 80S)', () => {
    expectRejected('33X 512345 9400000', /band X/);
    expectRejected('33C 512345 1000000', /band C/);
  });

  // Northing of each band's middle latitude at zone 33's central meridian (cs2cs).
  const BAND_MID_NORTHING: Array<[string, number, number]> = [
    ['C', 1564815, -76], ['D', 2457136, -68], ['E', 3348589, -60], ['F', 4238962, -52],
    ['G', 5128127, -44], ['H', 6016052, -36], ['J', 6902798, -28], ['K', 7788519, -20],
    ['L', 8673446, -12], ['M', 9557873, -4], ['N', 442127, 4], ['P', 1326554, 12],
    ['Q', 2211481, 20], ['R', 3097202, 28], ['S', 3983948, 36], ['T', 4871873, 44],
    ['U', 5761038, 52], ['V', 6651411, 60], ['W', 7542864, 68], ['X', 8658370, 78],
  ];

  it.each(BAND_MID_NORTHING)('band %s accepts a northing inside the band', (band, northing, lat) => {
    const result = parsedOk(`33${band} 500000 ${northing}`);
    expect(result.coordinates.latitude).toBeCloseTo(lat, 4);
    expect(result.coordinates.longitude).toBeCloseTo(15, 6);
  });

  it.each(BAND_MID_NORTHING.filter(([band]) => band !== 'N' && band !== 'S'))(
    'band %s rejects a northing four bands away',
    (band) => {
      const index = BAND_MID_NORTHING.findIndex(([b]) => b === band);
      const south = index < 10;
      const other = south ? (index + 4 <= 9 ? index + 4 : index - 4) : index + 4 <= 19 ? index + 4 : index - 4;
      expectRejected(`33${band} 500000 ${BAND_MID_NORTHING[other]![1]}`, new RegExp(`band ${band}`));
    }
  );
});

const DMS_LAT = 20 + 30 / 60 + 15 / 3600; // 20°30'15"
const DMS_LON = 87 + 18 / 60 + 27 / 3600; // 87°18'27"
const DDM_LAT = 20 + 30.5 / 60; // 20°30.5'

describe('parseGps notes a minus sign that overrides N or E (D39e)', () => {
  it('parseGps agrees, and says the minus sign overrode the letter', () => {
    const result = parsedOk("-0°30.5'N 87°18.5'W");
    expect(result.coordinates.latitude).toBeCloseTo(-30.5 / 60, 6);
    expect(result.note).toMatch(/minus sign/i);
  });

  it('a -1 degree with N reads the same way and carries the same note', () => {
    const result = parsedOk("-1°30.5'N 87°18.5'W");
    expect(result.coordinates.latitude).toBeCloseTo(-(1 + 30.5 / 60), 6);
    expect(result.note).toMatch(/minus sign/i);
  });

  it('a redundant minus with S needs no note', () => {
    const result = parsedOk("-0°30.5'S 87°18.5'W");
    expect(result.coordinates.latitude).toBeCloseTo(-30.5 / 60, 6);
    expect(result.note).toBeUndefined();
  });
});

describe('degree pairs: silent misreads fixed', () => {
  it('a leading S on the latitude is not ignored', () => {
    expectCoordsClose(parsedOk(`S 20°30'15" -87°18'27"`), -DMS_LAT, -DMS_LON);
  });

  it('rejects decimal degrees followed by minutes instead of reading from mid-number', () => {
    expectRejected(`48.5°51'30"N 2°17'40"E`, /whole number/);
    expectRejected(`20.5°30.5'N 87°18.5'W`, /whole number/);
  });
});

describe('degree pairs: common typed and pasted forms', () => {
  const cases: Array<[string, number, number]> = [
    // decimal degrees with hemisphere letters, prefix or suffix, any case
    ['20.5 N, 87.3 W', 20.5, -87.3],
    ['20.5N 87.3W', 20.5, -87.3],
    ['N 20.5, W 87.3', 20.5, -87.3],
    ['n20.5 w87.3', 20.5, -87.3],
    ['20.5°N, 87.3°W', 20.5, -87.3],
    ['N20.50833° W087.30833°', 20.50833, -87.30833],
    ['S 33.865 E 151.2099', -33.865, 151.2099],
    ['20.5N, -87.3', 20.5, -87.3],
    // signs: explicit plus, Unicode minus and en dash from copy-paste
    ['+20.5, -87.3', 20.5, -87.3],
    ['20.5, +87.3', 20.5, 87.3],
    ['\u221220.5, 87.3', -20.5, 87.3],
    ['20.5, \u201387.3', 20.5, -87.3],
    ['20.5°, -87.3°', 20.5, -87.3],
    // Garmin-style degrees and decimal minutes
    ["N20°30.500' W087°18.500'", DDM_LAT, -DDM_LON],
    ['N20 30.123 W087 18.456', 20 + 30.123 / 60, -(87 + 18.456 / 60)],
    ['20 30.123 N 87 18.456 W', 20 + 30.123 / 60, -(87 + 18.456 / 60)],
    ["-20°30.5' -87°18.5'", -DDM_LAT, -DDM_LON],
    // DMS symbol variants: masculine ordinal, ring, curly quotes, doubled apostrophes
    [`20\u00BA30'15"N 87\u00BA18'27"W`, DMS_LAT, -DMS_LON],
    [`20\u02DA30'15"N 87\u02DA18'27"W`, DMS_LAT, -DMS_LON],
    [`20°30\u201915\u201DN 87°18\u201927\u201DW`, DMS_LAT, -DMS_LON],
    [`20°30'15''N 87°18'27''W`, DMS_LAT, -DMS_LON],
    [`20\u00BA30.5'N 87\u00BA18.5'W`, DDM_LAT, -DDM_LON],
    ['N 20 30 15 W 87 18 27', DMS_LAT, -DMS_LON],
    [`N20°30'15" W87°18'27"`, DMS_LAT, -DMS_LON],
    [`\u221220°30'15" 87°18'27"`, -DMS_LAT, DMS_LON],
    // explicit letters make longitude-first input unambiguous
    ['87.3 W, 20.5 N', 20.5, -87.3],
    [`87°18'27"W 20°30'15"N`, DMS_LAT, -DMS_LON],
  ];

  it.each(cases)('%s', (input, lat, lon) => {
    expectCoordsClose(parseGps(input), lat, lon);
  });

  it('reports the format actually read', () => {
    expect(parsedOk('20.5 N, 87.3 W').format).toBe('DD');
    expect(parsedOk('N20 30.123 W087 18.456').format).toBe('DDM');
    expect(parsedOk(`20\u00BA30'15"N 87\u00BA18'27"W`).format).toBe('DMS');
  });

  it('still rejects ambiguous or contradictory pairs', () => {
    expectRejected('20.5N 87.3', /letter/);
    expectRejected('20.5 N 87.3 N', /N or S/);
    expectRejected('20.5 E 87.3 W', /N or S/);
    expectRejected(`20°30'15"N 87°18.5'W`, /different formats/);
    expectRejected(`20°30'15" 87°18'27"`);
    expectRejected("20°30.5' 87°18.5'");
    expectRejected('20,5 -87,3');
    expectRejected(`20°-30'15"N 87°18'27"W`);
  });

  it('keeps plain decimal degrees and labelled pastes working as before', () => {
    expectCoordsClose(parseGps('20.508333,-87.308333'), 20.508333, -87.308333);
    expectCoordsClose(parseGps('20 30'), 20, 30);
    expectCoordsClose(parseGps(`Entrance: 20°30'15"N 87°18'27"W`), DMS_LAT, -DMS_LON);
  });
});

describe('map links', () => {
  it('prefers the OpenStreetMap marker (mlat/mlon) over the view centre', () => {
    expectCoordsClose(
      parseGps('https://www.openstreetmap.org/?mlat=20.51&mlon=-87.31#map=15/20.5/-87.3'),
      20.51,
      -87.31
    );
  });

  it('reads Google Maps URLs API query= links and geo: URIs', () => {
    expectCoordsClose(parseGps('https://www.google.com/maps/search/?api=1&query=20.5,-87.3'), 20.5, -87.3);
    expectCoordsClose(parseGps('geo:20.5,-87.3'), 20.5, -87.3);
  });
});

describe('gpsSwapHint', () => {
  it('fires for a bare decimal pair that looks swapped', () => {
    expect(gpsSwapHint('-87.3, 20.5', parseGps('-87.3, 20.5'))).toBe(true);
  });

  it('still fires on the raw pair when the swapped input fails to parse', () => {
    expect(gpsSwapHint('120, 45', parseGps('120, 45'))).toBe(true);
  });

  it('does not fire when the input itself says which value is the latitude', () => {
    // Svalbard (78.96N) as UTM, and explicit N/E pairs: neither can be swapped.
    expect(gpsSwapHint('33X 512345 8765432', parseGps('33X 512345 8765432'))).toBe(false);
    expect(gpsSwapHint("90°0.0'N 0°0.0'E", parseGps("90°0.0'N 0°0.0'E"))).toBe(false);
    expect(gpsSwapHint('78.9 N, 15.5 E', parseGps('78.9 N, 15.5 E'))).toBe(false);
  });
});

describe('detectFormat (entry review)', () => {
  it('detects UTM with decimal easting/northing', () => {
    expect(detectFormat('16Q 451234.5 2266789.25')).toBe('UTM');
  });
});

// ---------------------------------------------------------------------------
// #576 review follow-up (R4). Expected values are plain deg + min/60 + sec/3600
// arithmetic; the band-edge northings are cs2cs FORWARD projections of the
// latitude named in each row (`+proj=utm +zone=33 [+south] +datum=WGS84`, at
// 15E, the zone's central meridian), so the expected latitude is that input.
// ---------------------------------------------------------------------------

describe('geo: links with a q= pin (R4 F1, F2)', () => {
  it("reads Android's geo:0,0?q=lat,lon(label) as the pin, not as 0,0", () => {
    for (const input of [
      'geo:0,0?q=20.5,-87.3(Cenote)',
      'geo:0,0?q=20.5,-87.3',
      'geo:0,0?q=20.5%2C-87.3(CaveA%20Entrance)',
      'geo:0,0?q=20.5,-87.3%28CaveA%29',
      'GEO:0,0?z=15&q=20.5,-87.3(CaveA)',
    ]) {
      const result = parsedOk(input);
      expect(result.coordinates.latitude, input).toBe(20.5);
      expect(result.coordinates.longitude, input).toBe(-87.3);
    }
  });

  it('rejects geo:0,0 with a q= that is a search, not coordinates', () => {
    expectRejected('geo:0,0?q=Cenote+Dos+Ojos', /geo:/);
    expectRejected('geo:0,0?q=1600+Main+Street', /geo:/);
    expectRejected('geo:0,0?q=', /geo:/);
  });

  it('uses the q= pin over the map centre in the path', () => {
    expectCoordsClose(parsedOk('geo:20.5,-87.3?q=20.51,-87.31'), 20.51, -87.31);
    expectCoordsClose(parsedOk('geo:20.5,-87.3?q=20.51,-87.31(Pin)'), 20.51, -87.31);
  });

  it('still reads a plain geo: URI, with or without parameters', () => {
    expectCoordsClose(parsedOk('geo:20.5,-87.3'), 20.5, -87.3);
    expectCoordsClose(parsedOk('geo:20.5,-87.3?z=15'), 20.5, -87.3);
    expectCoordsClose(parsedOk('geo:20.5,-87.3;u=35'), 20.5, -87.3);
    expectCoordsClose(parsedOk('geo:0,0'), 0, 0);
  });

  it('checks the range of the q= pin', () => {
    expectRejected('geo:0,0?q=120.5,-87.3', /Latitude/);
  });
});

describe('a sign that agrees with its hemisphere letter (R4 F3)', () => {
  // These parsed to the correct value on 9d60e8d5 and must keep it.
  it('accepts a minus that agrees with W on the lettered half, the other half unsigned', () => {
    expectCoordsClose(parsedOk(`20°30'15" -87°18'27"W`), DMS_LAT, -DMS_LON);
    expectCoordsClose(parsedOk(`20°30'15", -87°18'27" W`), DMS_LAT, -DMS_LON);
  });

  it('accepts a minus that agrees with a leading S, the other half unsigned', () => {
    expectCoordsClose(parsedOk(`S -20°30'15", 87°18'27"`), -DMS_LAT, DMS_LON);
  });

  it('reads the same shape in decimal degrees and decimal minutes', () => {
    expectCoordsClose(parsedOk('20.5, -87.3 W'), 20.5, -87.3);
    expectCoordsClose(parsedOk(`20°30.5' -87°18.5'W`), DDM_LAT, -DDM_LON);
  });

  it('still rejects one letter when neither half carries a sign', () => {
    expectRejected(`20°30'15" 87°18'27"W`, /letter/);
  });
});

describe('a plus sign that contradicts S or W (R4 F4)', () => {
  it('keeps the letter and says the plus sign was overridden', () => {
    const south = parsedOk('+21.5 S, 47.25 W');
    expect(south.coordinates.latitude).toBe(-21.5);
    expect(south.coordinates.longitude).toBe(-47.25);
    expect(south.note).toMatch(/plus sign and "S"/);

    const west = parsedOk('+20.5 N, +87.3 W');
    expect(west.coordinates.latitude).toBe(20.5);
    expect(west.coordinates.longitude).toBe(-87.3);
    expect(west.note).toMatch(/plus sign and "W"/);
  });

  it('a plus that agrees with N or E needs no note', () => {
    expect(parsedOk('+20.5 N, +87.3 E').note).toBeUndefined();
  });

  it('notes the same contradiction in a labelled paste', () => {
    const result = parsedOk(`Entrance: +21°30'0"S 47°15'0"W`);
    expect(result.coordinates.latitude).toBeCloseTo(-21.5, 9);
    expect(result.note).toMatch(/plus sign and "S"/);
  });

  it('does not read a +degree as a signed pair and drop the letter after it', () => {
    expectRejected(`Entrance: -24°56'52" +83°17'26"W`);
  });
});

describe('a hemisphere letter is never taken from inside a word (R4 F5)', () => {
  it('does not read W from "WGS84" or E from "elev"', () => {
    expectRejected(`48°51'30"N 2°17'40" WGS84`);
    expectRejected(`48°51.5'N 2°17.67' WGS84`);
    expectRejected(`20°30.5'N 87°18.5' elev 5`);
    expectRejected(`48 51 30 N 2 17 40 WGS84`);
  });

  it('still reads a letter followed by other text, and the direction words', () => {
    // East/West (and Spanish/French Este/Est) after the longitude parsed to
    // the right value before and must keep it.
    expectCoordsClose(parsedOk(`48°51'30"N 2°17'40"E WGS84`), 48 + 51 / 60 + 30 / 3600, 2 + 17 / 60 + 40 / 3600);
    expectCoordsClose(parsedOk(`48°51'30"N 2°17'40"East`), 48 + 51 / 60 + 30 / 3600, 2 + 17 / 60 + 40 / 3600);
    expectCoordsClose(parsedOk(`20°30'15"N 87°18'27" West`), DMS_LAT, -DMS_LON);
    expectCoordsClose(parsedOk(`20°30.5'N 87°18.5'West`), DDM_LAT, -DDM_LON);
    expectCoordsClose(parsedOk(`20°30'15"N 87°18'27" Este`), DMS_LAT, DMS_LON);
    expectCoordsClose(parsedOk(`20 30 15 N 87 18 27 W`), DMS_LAT, -DMS_LON);
  });

  it('still reads a longitude whose signs give its hemisphere, with a word after it', () => {
    // These took W or E from "WGS84"/"elev" before, which agreed with the signs.
    expectCoordsClose(parsedOk(`87°53'15"S, -106°46'0" WGS84`), -(87 + 53 / 60 + 15 / 3600), -(106 + 46 / 60));
    expectCoordsClose(parsedOk(`35°29' S, -50°14' elev 5`), -(35 + 29 / 60), -(50 + 14 / 60));
    expectCoordsClose(parsedOk(`-66°41.7'S , 30°21.8643 elev 5`), -(66 + 41.7 / 60), 30 + 21.8643 / 60);
    expectCoordsClose(parsedOk(`20 30 15 N -87 18 27 WGS84`), DMS_LAT, -DMS_LON);
  });

  it('rejects such a longitude when no sign says its hemisphere', () => {
    expectRejected(`23°6.4' N, 15°33.37' elev 5`);
    expectRejected(`20°30'15"N 87°18'27" WGS84`);
  });
});

describe('a dash-like character before a number reads as a minus sign, with a note (R4 F6)', () => {
  const dashes = ['\u2010', '\u2011', '\u2012', '\u2013', '\u2014', '\u2015', '\uFE63', '\uFF0D'].map(
    (dash) => [dash.charCodeAt(0).toString(16).toUpperCase(), dash] as const
  );

  it.each(dashes)('U+%s', (_codePoint, dash) => {
    const result = parsedOk(`${dash}21°30'15" -87°18'27"`);
    expect(result.coordinates.latitude).toBeCloseTo(-(21 + 30 / 60 + 15 / 3600), 9);
    expect(result.coordinates.longitude).toBeCloseTo(-DMS_LON, 9);
    expect(result.note).toBe(`Read "${dash}" as a minus sign.`);
  });

  it('also in a labelled paste and before decimal degrees', () => {
    expect(parsedOk(`Entrance: \u201420°30'15" 87°18'27"`).coordinates.latitude).toBeCloseTo(-DMS_LAT, 9);
    const dd = parsedOk('20.5, \uFF0D87.3');
    expect(dd.coordinates.longitude).toBe(-87.3);
    expect(dd.note).toMatch(/as a minus sign/);
  });

  it('the Unicode minus sign itself needs no note', () => {
    expect(parsedOk('\u221220.5, 87.3').note).toBeUndefined();
  });

  it('also right after a hemisphere letter', () => {
    const result = parsedOk('S\u20141.89, 69.2');
    expect(result.coordinates.latitude).toBe(-1.89);
    expect(result.note).toMatch(/as a minus sign/);
  });

  it('keeps the letter after a labelled pair (no hemisphere dropped)', () => {
    expectCoordsClose(parsedOk(`\u201458°38'12" 0°32'3.6"West`), -(58 + 38 / 60 + 12 / 3600), -(32 / 60 + 3.6 / 3600));
  });

  it('a dash inside a word is not a sign', () => {
    const result = parsedOk(`Entrance\u20142: 20°30'15"N 87°18'27"W`);
    expectCoordsClose(result, DMS_LAT, -DMS_LON);
    expect(result.note).toBeUndefined();
  });
});

describe('a minus sign that overrides N or E in a labelled paste (R4 F7)', () => {
  it('carries the same note as the strict reader', () => {
    const ddm = parsedOk(`Entrance: -0°30.5'N 87°18.5'W`);
    expect(ddm.coordinates.latitude).toBeCloseTo(-30.5 / 60, 9);
    expect(ddm.note).toMatch(/minus sign and "N"/);

    const dms = parsedOk(`Entrance: -20°30'15"N 87°18'27"W`);
    expect(dms.coordinates.latitude).toBeCloseTo(-DMS_LAT, 9);
    expect(dms.note).toMatch(/minus sign and "N"/);

    const lon = parsedOk(`Entrance: 20°30'15"N -87°18'27"E`);
    expect(lon.coordinates.longitude).toBeCloseTo(-DMS_LON, 9);
    expect(lon.note).toMatch(/minus sign and "E"/);
  });

  it('a minus that agrees with S needs no note', () => {
    expect(parsedOk(`Entrance: -20°30'15"S 87°18'27"W`).note).toBeUndefined();
  });
});

describe('a labelled signed pair uses the letters next to it (R4 F6/F7 follow-on)', () => {
  // The signed-degrees fallback used to ignore a hemisphere letter just
  // outside its match, so a dash read as a minus (F6) could still land a
  // longitude in the wrong hemisphere: these must use the letter or refuse.
  it('applies a letter on an unsigned degree', () => {
    expectCoordsClose(parsedOk(`Entrance: -20°30'15" 87°18'27"W`), -DMS_LAT, -DMS_LON);
    expectCoordsClose(parsedOk(`Entrance S 20°30'15" -87°18'27"`), -DMS_LAT, -DMS_LON);
  });

  it('notes a sign that disagrees with the letter next to it', () => {
    const minus = parsedOk(`Entrance: -20°30'15" -87°18'27"E`);
    expect(minus.coordinates.longitude).toBeCloseTo(-DMS_LON, 9);
    expect(minus.note).toMatch(/minus sign and "E"/);
    const plus = parsedOk(`Entrance: S +13°15'55" -50°52'35" W`);
    expect(plus.coordinates.latitude).toBeCloseTo(-(13 + 15 / 60 + 55 / 3600), 9);
    expect(plus.note).toMatch(/plus sign and "S"/);
  });

  it('refuses a letter on the wrong axis', () => {
    expectRejected(`East -5°29'15", 16°35'18"`);
    expectRejected(`Entrance: -20°30'15" 87°18'27" N`);
  });
});

describe('UTM band-edge slack (D37, R4 T1)', () => {
  // 0.05 degrees either side of a band. Each "inside the slack" row fails if
  // the slack is 0; each "outside" row fails if it is wider than 0.06.
  it('accepts a northing just across a band edge, within the slack', () => {
    expect(parsedOk('33T 500000 4423318').coordinates.latitude).toBeCloseTo(39.96, 4); // band T is 40N-48N
    expect(parsedOk('33X 500000 9332559').coordinates.latitude).toBeCloseTo(84.04, 4); // band X ends at 84N
    expect(parsedOk('33C 500000 1113950').coordinates.latitude).toBeCloseTo(-80.04, 4); // band C starts at 80S
  });

  it('rejects a northing just outside the slack', () => {
    expectRejected('33T 500000 4421098', /band T/); // 39.94N
    expectRejected('33X 500000 9334792', /band X/); // 84.06N
    expectRejected('33C 500000 1111717', /band C/); // 80.06S
  });
});

// ---------------------------------------------------------------------------
// #576 review follow-up (R5). Expected values are plain deg + min/60 + sec/3600
// arithmetic.
// ---------------------------------------------------------------------------

const PARIS_LAT = 48 + 51 / 60 + 30 / 3600; // 48°51'30"
const PARIS_LON = 2 + 17 / 60 + 40 / 3600; // 2°17'40"

describe('a fractional degree followed by minutes in a labelled paste (R5-6)', () => {
  it('is refused, never read from the middle of the number', () => {
    expectRejected(`Pos 48.5°51'30" -2°17'40"`, /whole number/);
    expectRejected(`Pos 48.5°51'30"N 2°17'40"E`, /whole number/);
    expectRejected(`Pos 48.5°51.5'N 2°17.5'W`, /whole number/);
  });

  it('still reads a labelled pair whose degrees are whole', () => {
    expectCoordsClose(parsedOk(`Pos 48°51'30" -2°17'40"`), PARIS_LAT, -PARIS_LON);
    expectCoordsClose(parsedOk(`No.20°30'15"N 87°18'27"W`), DMS_LAT, -DMS_LON);
  });
});

describe('label text next to a signed pair (R5-2)', () => {
  // 9d60e8d5 read these correctly: each degree carries its own minus sign, so
  // the letter or word next to it is part of the label.
  it.each([
    `Point E -20°30'15" -87°18'27"`,
    `Station W -20°30'15" -87°18'27"`,
    `-20°30'15" -87°18'27" South entrance`,
  ])('%s keeps its value and says the letter was read as text', (input) => {
    const result = parsedOk(input);
    expect(result.coordinates.latitude).toBeCloseTo(-DMS_LAT, 9);
    expect(result.coordinates.longitude).toBeCloseTo(-DMS_LON, 9);
    expect(result.note).toMatch(/part of the text/);
  });

  it('a separate letter or word against a minus sign is read as text, not as a contradiction', () => {
    for (const input of [`Station N -20°30'15" -87°18'27"`, `-20°30'15" -87°18'27" East entrance`]) {
      const result = parsedOk(input);
      expect(result.coordinates.latitude, input).toBeCloseTo(-DMS_LAT, 9);
      expect(result.coordinates.longitude, input).toBeCloseTo(-DMS_LON, 9);
      expect(result.note, input).toMatch(/part of the text/);
      expect(result.note, input).not.toMatch(/minus sign/);
    }
  });

  it('still refuses a wrong-axis letter touching a number, or one the signs do not settle', () => {
    expectRejected(`Pos E-20°30'15" -87°18'27"`);
    expectRejected(`Pos: -20°30'15" -87°18'27"N`);
    expectRejected(`Pos W -20°30'15" -87°18'27" S`); // longitude first, by its letters
  });
});

describe('a hemisphere taken from a direction word is said (R5-1)', () => {
  it('notes a word after an unsigned longitude', () => {
    const result = parsedOk(`-20°30'15" 87°18'27" West entrance`);
    expect(result.coordinates.longitude).toBeCloseTo(-DMS_LON, 9);
    expect(result.note).toMatch(/hemisphere from "West"/);
  });

  it('notes a word before an unsigned latitude', () => {
    const result = parsedOk(`Cave South 20°30'15" -87°18'27"`);
    expect(result.coordinates.latitude).toBeCloseTo(-DMS_LAT, 9);
    expect(result.note).toMatch(/hemisphere from "South"/);
  });

  it('notes a single letter read together with a label word before an unsigned latitude', () => {
    const result = parsedOk(`Cave S 20°30'15" -87°18'27"`);
    expect(result.coordinates.latitude).toBeCloseTo(-DMS_LAT, 9);
    expect(result.note).toMatch(/hemisphere from "S"/);
  });

  it('notes a word in a lettered paste too', () => {
    const dms = parsedOk(`Entrance: 20°30'15"N 87°18'27" West entrance`);
    expect(dms.coordinates.longitude).toBeCloseTo(-DMS_LON, 9);
    expect(dms.note).toMatch(/hemisphere from "West"/);
    const ddm = parsedOk(`20°30.5'N 87°18.5' West entrance`);
    expect(ddm.coordinates.longitude).toBeCloseTo(-DDM_LON, 9);
    expect(ddm.note).toMatch(/hemisphere from "West"/);
  });

  it('a letter next to the number, or a word that agrees with a minus sign, needs no note', () => {
    expect(parsedOk(`Entrance: -20°30'15" 87°18'27"W`).note).toBeUndefined();
    expect(parsedOk(`Entrance: 20°30'15"N 87°18'27"W`).note).toBeUndefined();
    expect(parsedOk(`-20°30'15" -87°18'27" West entrance`).note).toBeUndefined();
  });
});

describe('the word note only where the word sits next to another word (COORD-Q1)', () => {
  // A direction word with no other word beside it cannot be part of a name, so
  // reading it as the hemisphere is no guess and needs no note. The value is the
  // same either way: only the note depends on the neighbouring word.
  it.each([
    [`20°30'15"N 87°18'27" West`, DMS_LAT, -DMS_LON],
    [`20°30.5'N 87°18.5' West`, DDM_LAT, -DDM_LON],
    [`-20°30'15" 87°18'27" West`, -DMS_LAT, -DMS_LON],
    [`-20°30'15" 87°18'27"West`, -DMS_LAT, -DMS_LON],
    [`South 20°30'15" -87°18'27"`, -DMS_LAT, -DMS_LON],
    [`Entrance: South 20°30'15" -87°18'27"`, -DMS_LAT, -DMS_LON],
  ])('%s: a word that starts or ends the text next to the number needs no note', (input, lat, lon) => {
    const result = parsedOk(input);
    expectCoordsClose(result, lat, lon);
    expect(result.note).toBeUndefined();
  });

  it.each([
    [`20°30'15"N 87°18'27" West entrance`, DMS_LAT, -DMS_LON, 'West'],
    [`20°30.5'N 87°18.5' West entrance`, DDM_LAT, -DDM_LON, 'West'],
    [`-20°30'15" 87°18'27" West entrance`, -DMS_LAT, -DMS_LON, 'West'],
    [`-20°30'15" 87°18'27"West entrance`, -DMS_LAT, -DMS_LON, 'West'],
    [`Cave South 20°30'15" -87°18'27"`, -DMS_LAT, -DMS_LON, 'South'],
    [`Cave South20°30'15" -87°18'27"`, -DMS_LAT, -DMS_LON, 'South'],
  ])('%s: a word next to another word keeps the note', (input, lat, lon, word) => {
    const result = parsedOk(input);
    expectCoordsClose(result, lat, lon);
    expect(result.note).toMatch(new RegExp(`hemisphere from "${word}"`));
  });

  // RV3-FE-11: "another word" is any word - one that starts with a letter outside
  // ASCII (Spanish and French cave names) or with a digit counts too. Only the note
  // depends on it; the values are the ones the ASCII neighbours give.
  it.each([
    [`20°30'15"N 87°18'27" West Ña`, DMS_LAT, -DMS_LON, 'West'],
    [`20°30'15"N 87°18'27" West Ébène`, DMS_LAT, -DMS_LON, 'West'],
    [`20°30.5'N 87°18.5' West Ñandú`, DDM_LAT, -DDM_LON, 'West'],
    [`-20°30'15" 87°18'27" West Ébène`, -DMS_LAT, -DMS_LON, 'West'],
    [`Ñandú South 20°30'15" -87°18'27"`, -DMS_LAT, -DMS_LON, 'South'],
    [`20°30'15"N 87°18'27" West 2nd`, DMS_LAT, -DMS_LON, 'West'],
    [`Pit 2 South 20°30'15" -87°18'27"`, -DMS_LAT, -DMS_LON, 'South'],
  ])('%s: a non-ASCII or numeric neighbour is another word too', (input, lat, lon, word) => {
    const result = parsedOk(input);
    expectCoordsClose(result, lat, lon);
    expect(result.note).toMatch(new RegExp(`hemisphere from "${word}"`));
  });
});

describe('the dash note only where the dash changed the reading (R5-3)', () => {
  it('no note for a dash that is not the sign of a coordinate', () => {
    for (const input of [`Pos \u20141: 20°30'15"N 87°18'27"W`, `20°30'15"N 87°18'27"W \u201312`]) {
      const result = parsedOk(input);
      expectCoordsClose(result, DMS_LAT, -DMS_LON);
      expect(result.note, input).toBeUndefined();
    }
  });

  it('still notes a dash that is the sign', () => {
    expect(parsedOk(`\u201421°30'15" -87°18'27"`).note).toMatch(/as a minus sign/);
    expect(parsedOk('20.5, \u201387.3').note).toMatch(/as a minus sign/);
  });
});

describe('geo: link with a map centre and a search (R5 question b)', () => {
  it('refuses a non-zero path with an address q=, like geo:0,0', () => {
    expectRejected('geo:20.5,-87.3?q=Some+Place', /search/);
    expectRejected('geo:20.5,-87.3?z=12&q=CaveA%20Entrance', /search/);
  });

  it('still reads the path with no search, and the q= pin', () => {
    expectCoordsClose(parsedOk('geo:20.5,-87.3?q='), 20.5, -87.3);
    expectCoordsClose(parsedOk('geo:20.5,-87.3?z=12'), 20.5, -87.3);
    expectCoordsClose(parsedOk('geo:20.5,-87.3?q=20.51,-87.31(Pin)'), 20.51, -87.31);
  });
});

describe('text too long to be a coordinate is refused before parsing (R5-5)', () => {
  it('refuses non-link text over 1024 characters', () => {
    expectRejected('1'.repeat(10000), /too long/);
    expectRejected(`${'1'.repeat(1100)}, 2`, /too long/);
    expectRejected(`${'x'.repeat(1010)} 20°30'15"N 87°18'27"W`, /too long/);
  });

  it('reads coordinate text up to 1024 characters', () => {
    const pair = `20°30'15"N 87°18'27"W`;
    const input = `${'x'.repeat(1024 - pair.length - 1)} ${pair}`;
    expect(input.length).toBe(1024);
    expectCoordsClose(parsedOk(input), DMS_LAT, -DMS_LON);
  });

  it('links keep their own handling', () => {
    expectCoordsClose(parsedOk(`https://www.google.com/maps/@20.5,-87.3,15z?${'a'.repeat(1100)}`), 20.5, -87.3);
  });

  it('the badge and swap-hint helpers skip such text too', () => {
    expect(detectFormat(`${'1'.repeat(1100)}, 2`)).toBeUndefined();
    expect(looksTransposedRawDD(`${'0'.repeat(1100)}120, 45`)).toBe(false);
  });
});

describe('a map link has its own, larger length cap', () => {
  it('refuses a link over 8192 characters quickly, before any pattern runs', () => {
    // A place segment of digits is the slow case: the place-name parsers search
    // it with patterns whose time grows with the square of its length.
    const link = `https://www.google.com/maps/place/${'1'.repeat(64000)}/@20.5,-87.3,15z`;
    const started = performance.now();
    const result = parseGps(link);
    const elapsedMs = performance.now() - started;
    expect(isSuccess(result)).toBe(false);
    if (!isSuccess(result)) expect(result.message).toMatch(/too long.*at most 8192/);
    expect(elapsedMs).toBeLessThan(250);
  });

  it('reads a link up to 8192 characters and refuses one character more', () => {
    const base = 'https://www.google.com/maps/@20.5,-87.3,15z?';
    const atCap = `${base}${'a'.repeat(8192 - base.length)}`;
    expect(atCap.length).toBe(8192);
    expectCoordsClose(parsedOk(atCap), 20.5, -87.3);
    expectRejected(`${atCap}a`, /too long/);
  });

  it('still reads a realistic long Google Maps place link with data parameters', () => {
    const place = "20%C2%B030'15.0%22N+87%C2%B018'27.0%22W";
    const data =
      '!3m1!1e3!4m14!1m7!3m6!1s0x8f4c3a1b2c3d4e5f:0x1a2b3c4d5e6f7a8b!2sCaveA!8m2!3d20.5041667!4d-87.3075' +
      '!16s%2Fg%2F11c0abcd12!3m5!1s0x8f4c3a1b2c3d4e5f:0x1a2b3c4d5e6f7a8b!8m2!3d20.5041667!4d-87.3075' +
      '!16s%2Fg%2F11c0abcd12'.repeat(80);
    const link =
      `https://www.google.com/maps/place/${place}/@20.5041667,-87.3075,17z/data=${data}` +
      '?entry=ttu&g_ep=EgoyMDI2MTAyOC4wIKXMDSoASAFQAw%3D%3D';
    expect(link.length).toBeGreaterThan(1500);
    expect(link.length).toBeLessThan(2500);
    expectCoordsClose(parsedOk(link), DMS_LAT, -DMS_LON);
  });

  it('a place segment too long to be coordinates falls back to the pin', () => {
    const link = `https://www.google.com/maps/place/${'1'.repeat(5000)}/@20.5,-87.3,15z/data=!3d20.51!4d-87.31`;
    expectCoordsClose(parsedOk(link), 20.51, -87.31);
  });
});
