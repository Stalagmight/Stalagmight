/**
 * Unit test - #270: a transport failure must never surface a bare OS errno.
 *
 * The reported defect: with the SpeleoDB host blackholed at the routing level the
 * user was shown "Network error talking to SpeleoDB: Invalid argument". That is a
 * raw EINVAL - it says nothing about what failed, offers nothing to do, and does
 * not even hint that the server was unreachable. The humanized line must instead
 * name the configured endpoint (self-hosted users need to know WHICH server) and
 * say what to check, while the raw cause stays on the CommandError for the log.
 *
 * The transport case is recognised STRUCTURALLY, from the backend's typed
 * `transport` payload - never by matching English substrings of the Rust
 * message (which is locale-dependent: it comes from the platform's strerror).
 *
 * `.tsx` so this runs in the jsdom tier - the i18n singleton it uses is
 * browser-detector backed.
 */
import { describe, it, expect, beforeEach } from 'vitest';

import { dumpCandidateKind, humanizeSpeleoError, unpushedLocalEdits } from './speleoError';
import { useSpeleoStore } from '../store/speleoStore';
import type { CommandError } from '../types/ipc.generated';

const ENDPOINT = 'https://cave.example.internal:8443/';

/** The CommandError `speleodb_*` commands reject with (#576 C4): a transport failure by default. */
const dto = (over: Partial<CommandError> = {}): CommandError => ({
  code: 'SPELEODB_RETRY',
  message: 'Network error talking to SpeleoDB: Invalid argument',
  transport: 'Unknown',
  ...over,
});

/** The same rejection with no transport cause: the backend omits `transport` for every other error. */
const nonTransport = (over: Partial<CommandError>): CommandError => {
  const { transport: _transport, ...rest } = dto(over);
  return rest;
};

beforeEach(() => {
  useSpeleoStore.setState({
    session: {
      loggedIn: true,
      userEmail: 'me@example.com',
      endpoint: ENDPOINT,
      keychainBacked: true,
      storage: 'keychain',
      keychain: 'available',
    },
  });
});

describe('humanizeSpeleoError - transport failures (#270)', () => {
  it('never surfaces the raw errno, and names the server that could not be reached', () => {
    const line = humanizeSpeleoError(dto(), 'Could not finish editing');

    expect(line).not.toContain('Invalid argument');
    expect(line).not.toContain('Network error talking to SpeleoDB');
    expect(line).toContain(ENDPOINT);
    expect(line).toMatch(/could not reach/i);
    // Self-framed: the operation prefix is NOT glued on top of it.
    expect(line).not.toContain('Could not finish editing');
  });

  it('keeps the genuinely useful causes distinguishable', () => {
    const refused = humanizeSpeleoError(dto({ transport: 'Refused' }));
    const timeout = humanizeSpeleoError(dto({ transport: 'Timeout' }));
    const dns = humanizeSpeleoError(dto({ transport: 'Dns' }));
    const tls = humanizeSpeleoError(dto({ transport: 'Tls' }));
    const unknown = humanizeSpeleoError(dto({ transport: 'Unknown' }));

    for (const line of [refused, timeout, dns, tls, unknown]) {
      expect(line).toContain(ENDPOINT);
    }
    expect(refused).toMatch(/refused/i);
    expect(timeout).toMatch(/did not respond in time/i);
    expect(dns).toMatch(/could not find/i);
    expect(tls).toMatch(/secure connection/i);
    // The catch-all (EINVAL and friends) stays generic but actionable.
    expect(unknown).toMatch(/check your network connection/i);
    // Four of the five say something the others do not.
    expect(new Set([refused, timeout, dns, tls, unknown]).size).toBe(5);
  });

  it('falls back to a neutral phrase when no session endpoint is known', () => {
    useSpeleoStore.setState({ session: null });
    const line = humanizeSpeleoError(dto({ transport: 'Refused' }));
    expect(line).toContain('the configured server');
    expect(line).not.toContain('undefined');
  });

  it('leaves non-transport errors framed by their operation prefix', () => {
    // `transport` is absent on every non-transport error, which is what makes it a
    // sound discriminant: a retryable 5xx is `SPELEODB_RETRY` too, but carries a status.
    const line = humanizeSpeleoError(
      nonTransport({ message: 'Bad login.', code: 'SPELEODB_FATAL', status: 400 }),
      'Login to SpeleoDB failed',
    );
    expect(line).toBe('Login to SpeleoDB failed: Bad login');
  });

  it('does not treat a retryable 5xx as a transport failure', () => {
    const line = humanizeSpeleoError(
      nonTransport({ message: 'Server error', code: 'SPELEODB_RETRY', status: 503 }),
      'Could not push changes',
    );
    expect(line).toBe('Could not push changes: Server error');
  });
});

/**
 * [#270] `unpushedLocalEdits` gates the refused-open prompt. That prompt names the
 * project and when the local copy was last edited, and offers an IRREVERSIBLE
 * "discard my changes" button - so a payload it cannot describe must fall back to the
 * generic error path rather than raise a prompt with blanks in it. These pin the
 * malformed-payload guards, which the reader would otherwise be free to "simplify".
 */
describe('unpushedLocalEdits payload guards (#270)', () => {
  const refusal = (unpushed_local: unknown) =>
    ({
      code: 'SPELEODB_UNPUSHED_LOCAL_EDITS',
      message: 'local edits never pushed',
      unpushed_local,
    }) as unknown;

  it('accepts a well-formed refusal', () => {
    const payload = {
      project_id: 'p1',
      local_path: '/cache/p1.stalx',
      last_edit_at: '2026-07-26T09:00:00Z',
    };
    expect(unpushedLocalEdits(refusal(payload))).toEqual(payload);
  });

  it('rejects a null payload', () => {
    expect(unpushedLocalEdits(refusal(null))).toBeNull();
  });

  it('rejects a non-object payload', () => {
    expect(unpushedLocalEdits(refusal('p1'))).toBeNull();
  });

  it('rejects a non-string project_id', () => {
    expect(
      unpushedLocalEdits(refusal({ project_id: 42, local_path: '/cache/p1.stalx' })),
    ).toBeNull();
  });

  it('rejects an EMPTY local_path - the prompt cannot open a file it cannot name', () => {
    expect(unpushedLocalEdits(refusal({ project_id: 'p1', local_path: '' }))).toBeNull();
  });

  it('rejects a missing local_path', () => {
    expect(unpushedLocalEdits(refusal({ project_id: 'p1' }))).toBeNull();
  });

  it('rejects an error that is not this refusal at all', () => {
    expect(
      unpushedLocalEdits({ code: 'SPELEODB_FATAL', message: 'boom', status: 500 }),
    ).toBeNull();
    expect(unpushedLocalEdits(new Error('plain'))).toBeNull();
    expect(unpushedLocalEdits('a string rejection')).toBeNull();
  });

  it('tolerates a missing last_edit_at (the prompt has its own unknown wording)', () => {
    const payload = { project_id: 'p1', local_path: '/cache/p1.stalx', last_edit_at: null };
    expect(unpushedLocalEdits(refusal(payload))).toEqual(payload);
  });
});

/**
 * [#258] The picker's verb comes from the CommandError, not from the candidates' file
 * extensions. Extracting a `.stalx` and adopting an `ariane.tml` are different
 * operations, so reading this wrong shows the wrong dialog - and hard-coding it
 * left the Adopt copy with no production caller at all.
 */
describe('dumpCandidateKind (#258)', () => {
  const ambiguous = (kind: unknown) =>
    ({
      ...nonTransport({
        code: 'SPELEODB_AMBIGUOUS',
        dump_candidates: [{ entry_path: 'a', size_bytes: 1, modified: null }],
      }),
      dump_candidate_kind: kind,
    }) as unknown;

  it('reads the kind the classifier decided', () => {
    expect(dumpCandidateKind(ambiguous('ariane'))).toBe('ariane');
    expect(dumpCandidateKind(ambiguous('stalx'))).toBe('stalx');
  });

  it('falls back to stalx when no kind is carried', () => {
    // An older backend, or any error that is not an ambiguity at all. `stalx` is
    // both the commoner ambiguity and the flow that works without adoption, so it
    // is the safe default - never a silent switch INTO the adopt verb.
    expect(dumpCandidateKind(ambiguous(null))).toBe('stalx');
    expect(dumpCandidateKind(ambiguous(undefined))).toBe('stalx');
    expect(dumpCandidateKind(new Error('plain'))).toBe('stalx');
    expect(dumpCandidateKind('a string rejection')).toBe('stalx');
  });

  it('never infers the kind from the candidates themselves', () => {
    // A set of `.tml` paths with no kind stays `stalx`: the extension is not the
    // discriminant, precisely so the two copies of that rule cannot disagree.
    const tmlish = nonTransport({
      code: 'SPELEODB_AMBIGUOUS',
      dump_candidates: [
        { entry_path: 'one/ariane.tml', size_bytes: 1, modified: null },
      ],
    });
    expect(dumpCandidateKind(tmlish)).toBe('stalx');
  });
});
