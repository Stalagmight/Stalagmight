import { describe, it, expect } from 'vitest';
import { cleanEndpointInput, endpointsMatch } from '../speleoEndpoint';

// The forms `auth.rs::normalize_endpoint` collapses into one server identity.
// The UI has to collapse the same ones, or it tells a user their file belongs
// to a different server than the one they are demonstrably signed in to.
describe('endpointsMatch', () => {
  it('collapses a trailing slash, host case and a default port', () => {
    const forms = [
      'https://sdb.example.com/',
      'https://sdb.example.com',
      'https://sdb.example.com:443',
      'https://SDB.Example.com/',
      '  https://sdb.example.com/  ',
    ];
    for (const form of forms) {
      expect(endpointsMatch('https://sdb.example.com/', form)).toBe(true);
    }
  });

  it('keeps a non-default port distinct', () => {
    expect(endpointsMatch('https://sdb.example.com', 'https://sdb.example.com:8443')).toBe(false);
  });

  it('separates different hosts and schemes', () => {
    expect(endpointsMatch('https://sdb.example.com', 'https://other.example.com')).toBe(false);
    expect(endpointsMatch('https://sdb.example.com', 'http://sdb.example.com')).toBe(false);
  });

  it('falls back to a lightweight comparison for anything URL cannot parse', () => {
    expect(endpointsMatch('sdb.example.com/', 'SDB.example.com')).toBe(true);
    expect(endpointsMatch('sdb.example.com', 'other.example.com')).toBe(false);
  });

  it('never matches a missing endpoint', () => {
    expect(endpointsMatch(null, null)).toBe(false);
    expect(endpointsMatch(undefined, 'https://sdb.example.com')).toBe(false);
    expect(endpointsMatch('', '')).toBe(false);
  });
});

// #675: what copy-paste puts around an address. Mirrors the Rust
// `clean_strips_only_what_the_user_cannot_see_at_the_ends` cases.
describe('cleanEndpointInput', () => {
  it('strips whitespace and invisible characters at the ends only', () => {
    const cases: Array<[string, string]> = [
      ['https://www.speleodb.org', 'https://www.speleodb.org'],
      ['  https://www.speleodb.org \r\n', 'https://www.speleodb.org'],
      ['\u00A0https://www.speleodb.org\u3000', 'https://www.speleodb.org'],
      ['\u200B\uFEFFhttps://www.speleodb.org\u2060', 'https://www.speleodb.org'],
      ['\u200Ehttps://www.speleodb.org\u202C\u00AD', 'https://www.speleodb.org'],
      ['\u0085https://www.speleodb.org\u0085', 'https://www.speleodb.org'],
      ['https://www.speleo db.org', 'https://www.speleo db.org'],
      ['https://www.speleo\u200Bdb.org', 'https://www.speleo\u200Bdb.org'],
      [' \t\n\u200B\uFEFF ', ''],
    ];
    for (const [input, expected] of cases) {
      expect(cleanEndpointInput(input), JSON.stringify(input)).toBe(expected);
    }
  });

  it('stays linear on a long interior run (a hostile .stalx endpoint)', () => {
    const hostile = 'https://a' + ' '.repeat(200_000) + 'b';
    const started = performance.now();
    expect(cleanEndpointInput(hostile)).toBe(hostile);
    expect(endpointsMatch(hostile, 'https://a')).toBe(false);
    expect(performance.now() - started).toBeLessThan(500);
  });

  it('treats a pasted spelling as the same server', () => {
    expect(endpointsMatch('https://sdb.example.com', '\u200Bhttps://sdb.example.com/\uFEFF')).toBe(true);
  });
});
