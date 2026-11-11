/**
 * Unit test - #270: a transport failure must never surface a bare OS errno.
 *
 * The reported defect: with the SpeleoDB host blackholed at the routing level the
 * user was shown "Network error talking to SpeleoDB: Invalid argument". That is a
 * raw EINVAL - it says nothing about what failed, offers nothing to do, and does
 * not even hint that the server was unreachable. The humanized line must instead
 * name the configured endpoint (self-hosted users need to know WHICH server) and
 * say what to check, while the raw cause stays on the DTO for the log.
 *
 * The transport case is recognised STRUCTURALLY, from the backend's typed
 * `transport` discriminant - never by matching English substrings of the Rust
 * message (which is locale-dependent: it comes from the platform's strerror).
 *
 * `.tsx` so this runs in the jsdom tier - the i18n singleton it uses is
 * browser-detector backed.
 */
import { describe, it, expect, beforeEach } from 'vitest';

import { humanizeSpeleoError, unpushedLocalEdits } from './speleoError';
import { useSpeleoStore } from '../store/speleoStore';
import type { SpeleoErrorDto } from '../types/ipc.generated';

const ENDPOINT = 'https://cave.example.internal:8443/';

/** The DTO shape `speleodb_*` commands reject with. */
const dto = (over: Partial<SpeleoErrorDto> = {}): SpeleoErrorDto => ({
  message: 'Network error talking to SpeleoDB: Invalid argument',
  action: 'Retry',
  status: null,
  transport: 'Unknown',
  unpushed_local: null,
  newer_schema: null,
  ...over,
});

beforeEach(() => {
  useSpeleoStore.setState({
    session: {
      loggedIn: true,
      userEmail: 'me@example.com',
      endpoint: ENDPOINT,
      },
  });
});

describe('humanizeSpeleoError - a download a newer Stalagmight saved (#649)', () => {
  it('says it in the open dialog wording, with the file and both versions, unprefixed', () => {
    const line = humanizeSpeleoError(
      dto({
        message: 'backend text',
        action: 'Fatal',
        transport: null,
        newer_schema: { file: 'project.stalx', found: 19, supported: 18 },
      }),
      'Unable to open the project'
    );

    expect(line).toBe(
      '"project.stalx" was saved by a newer version of Stalagmight (file format 19; this version reads up to 18). Update Stalagmight to open it.'
    );
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
    expect(refused).toMatch(/could not connect/i);
    expect(timeout).toMatch(/did not respond in time/i);
    expect(dns).toMatch(/could not find/i);
    expect(tls).toMatch(/secure connection/i);
    // The catch-all (EINVAL and friends) stays generic but actionable.
    expect(unknown).toMatch(/check your network connection/i);
    // Four of the five say something the others do not.
    expect(new Set([refused, timeout, dns, tls, unknown]).size).toBe(5);
  });

  it('names the server the call was made to when the caller knows it', () => {
    // Settings' Connect tries the address on screen, not the session's.
    const line = humanizeSpeleoError(dto({ transport: 'Refused' }), 'Could not connect to SpeleoDB', {
      endpoint: 'https://unsaved.example/',
    });
    expect(line).toContain('https://unsaved.example/');
    expect(line).not.toContain(ENDPOINT);
    // A blank one falls back to the session's.
    expect(humanizeSpeleoError(dto({ transport: 'Refused' }), undefined, { endpoint: '  ' })).toContain(ENDPOINT);
  });

  it('falls back to a neutral phrase when no session endpoint is known', () => {
    useSpeleoStore.setState({ session: null });
    const line = humanizeSpeleoError(dto({ transport: 'Refused' }));
    expect(line).toContain('the configured server');
    expect(line).not.toContain('undefined');
  });

  it('leaves non-transport errors framed by their operation prefix', () => {
    // `transport` is null for every non-transport error, which is what makes it a
    // sound discriminant: a retryable 5xx is `Retry` too, but carries a status.
    const line = humanizeSpeleoError(
      dto({ message: 'Bad login.', action: 'Fatal', status: 400, transport: null }),
      'Could not connect to SpeleoDB',
    );
    expect(line).toBe('Could not connect to SpeleoDB: Bad login');
  });

  it('does not treat a retryable 5xx as a transport failure', () => {
    const line = humanizeSpeleoError(
      dto({ message: 'Server error', action: 'Retry', status: 503, transport: null }),
      'Could not push changes',
    );
    expect(line).toBe('Could not push changes: Server error');
  });
});

describe('humanizeSpeleoError - an endpoint the policy refused', () => {
  const refused = (endpoint_refusal: 'PlainHttp' | 'InvalidAddress') => dto({
    message: 'HTTPS is required. HTTP is allowed only on localhost, 127.0.0.1 or 192.168.x.x. Rejected endpoint: http://192.0.2.1/',
    action: 'Fatal', transport: null, endpoint_refusal,
  });

  it('names the automatic HTTP exceptions for a plain http:// server, never the backend text', () => {
    const line = humanizeSpeleoError(refused('PlainHttp'), 'Could not connect to SpeleoDB', { endpoint: 'http://192.0.2.1/' });
    expect(line).toContain('http://192.0.2.1/');
    // The one description of the exception set (`speleodb.httpExceptions`), nested in.
    expect(line).toContain('allowed only for localhost, 127.0.0.1 and 192.168.x.x addresses');
    expect(line).not.toContain('$t(');
    expect(line).toContain('https://');
    expect(line).not.toContain('Rejected endpoint');
    expect(line).not.toContain('Could not connect to SpeleoDB:');
  });

  it('asks for a full address when the address is unusable', () => {
    const line = humanizeSpeleoError(refused('InvalidAddress'), undefined, { endpoint: 'speleodb.lan' });
    expect(line).toMatch(/^speleodb\.lan is not a server address/);
    expect(line).not.toContain('Allow insecure HTTP');
  });

  it('ignores a refusal field that is not one it knows', () => {
    const line = humanizeSpeleoError({ ...refused('PlainHttp'), endpoint_refusal: 'Other' }, 'Prefix');
    expect(line).toBe('Prefix: HTTPS is required. HTTP is allowed only on localhost, 127.0.0.1 or 192.168.x.x. Rejected endpoint: http://192.0.2.1/');
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
      message: 'local edits never pushed',
      action: 'UnpushedLocalEdits',
      status: null,
      transport: null,
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
      unpushedLocalEdits({ message: 'boom', action: 'Fatal', status: 500 }),
    ).toBeNull();
    expect(unpushedLocalEdits(new Error('plain'))).toBeNull();
    expect(unpushedLocalEdits('a string rejection')).toBeNull();
  });

  it('tolerates a missing last_edit_at (the prompt has its own unknown wording)', () => {
    const payload = { project_id: 'p1', local_path: '/cache/p1.stalx', last_edit_at: null };
    expect(unpushedLocalEdits(refusal(payload))).toEqual(payload);
  });
});
