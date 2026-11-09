import { describe, it, expect } from 'vitest';
import { endpointsMatch } from '../speleoEndpoint';

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
