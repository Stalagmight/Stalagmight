/**
 * An oracle finding as a SHAREABLE artifact carries it - `findings/<n>/finding.json`
 * (e2e/agent/lib/oracles.ts) and `report.md` (scripts/generate-repro-report.mjs):
 * codes, counts and code locations, never the app's own text (#576 W21-NF2).
 *
 * A finding's message and detail quote the app: a log line, a panic message, the
 * error boundary's fallback text, `check_project_integrity` messages, the names
 * of leftover temp files (a project's own file name), a probe's error. Those name
 * sections, stations and projects of the loaded data. The full finding stays in
 * the session's `actions.jsonl` (the `finding` event), which docs/agent-harness.md
 * lists as not for sharing.
 *
 * Fail-closed: a message is kept only for a kind whose message the oracle words
 * from fixed text and numbers; any other kind - a new one included - says what
 * was withheld and how long it was. Projecting a projection changes nothing, so
 * the report can run it over a finding.json that already went through it.
 */
import * as os from 'os';

/** Identifier characters only, as `oracles.findingsSummary` keys them. */
const label = (text) => String(text).replace(/[^\w:.-]/g, '?').slice(0, 120);

/** Kinds whose message the oracle writes from fixed words and numbers alone. */
const HARNESS_WORDED = new Set([
  'error-boundary',
  'ring-overflow',
  'integrity-save-blocked',
  'integrity-messages',
  'memory-growth',
  'unresponsive',
]);
const LOG_KINDS = new Set(['log-warn', 'log-error']);
/** `oracles.ts` LEFTOVER_TEMP_SUFFIXES: a leftover's code-defined part. */
const TEMP_SUFFIXES = ['.stalx.tmp', '.download.tmp', '.vacuum.tmp'];
/** A source location as the panic hook records it: `<file>:<line>[:<col>]`. */
const CODE_LOCATION = /^[\w./\\$~+:-]{1,400}:\d+(?::\d+)?$/;
/** A thread name as Rust's runtime and tokio give them (`main`, `tokio-runtime-worker`). */
const THREAD_NAME = /^[\w.:<>-]{1,80}$/;
/** The panic hook's marker file name. */
const MARKER_MESSAGE = /^backend panic marker: crash-[\w-]{1,80}\.json$/;

const isObject = (value) => value !== null && typeof value === 'object' && !Array.isArray(value);
const count = (value, fallback) => (typeof value === 'number' && Number.isFinite(value) ? value : fallback);
const home = os.homedir();
const foldHome = (text) => (home && text.startsWith(home) ? `$HOME${text.slice(home.length)}` : text);

function shareableMessage(kind, finding) {
  const message = typeof finding.message === 'string' ? finding.message : '';
  if (HARNESS_WORDED.has(kind)) return message;
  if (kind === 'crash-marker' && MARKER_MESSAGE.test(message)) return message;
  if (kind === 'leftover-temp') {
    const leftovers = isObject(finding.detail) ? finding.detail.leftovers : undefined;
    const n = Array.isArray(leftovers) ? leftovers.length : count(leftovers, null);
    return `${n ?? 'some'} leftover write-staging temp file(s) beside the project (names withheld)`;
  }
  if (LOG_KINDS.has(kind)) return `line withheld: it can name survey data; see the session's actions.jsonl`;
  return `message withheld: it can quote the app's text; see the session's actions.jsonl`;
}

function shareableDetail(kind, detail) {
  if (detail === undefined) return undefined;
  if (kind === 'crash-marker' && isObject(detail)) {
    const out = {};
    if (typeof detail.location === 'string') {
      const location = foldHome(detail.location);
      out.location = CODE_LOCATION.test(location) ? location : '(withheld)';
    }
    if (typeof detail.thread === 'string' && THREAD_NAME.test(detail.thread)) out.thread = detail.thread;
    if (typeof detail.message === 'string' || 'messageChars' in detail) {
      out.messageChars = typeof detail.message === 'string' ? detail.message.length : count(detail.messageChars, 0);
    }
    if (typeof detail.file === 'string') out.file = label(detail.file.split(/[\\/]/).pop() ?? '');
    return out;
  }
  if (kind === 'error-boundary') {
    return { textChars: typeof detail === 'string' ? detail.length : count(detail?.textChars, 0) };
  }
  if (kind === 'integrity-save-blocked' || kind === 'integrity-messages') {
    return { messages: Array.isArray(detail) ? detail.length : count(detail?.messages, 0) };
  }
  if (kind === 'leftover-temp' && isObject(detail)) {
    const names = Array.isArray(detail.leftovers) ? detail.leftovers : null;
    const suffixes = {};
    for (const suffix of TEMP_SUFFIXES) {
      const n = names ? names.filter((name) => String(name).endsWith(suffix)).length : count(detail.suffixes?.[suffix], 0);
      if (n > 0) suffixes[suffix] = n;
    }
    return { leftovers: names ? names.length : count(detail.leftovers, 0), suffixes };
  }
  if (kind === 'memory-growth' && isObject(detail)) {
    const out = {};
    for (const key of ['baselineMb', 'currentMb', 'backendMb']) {
      if (typeof detail[key] === 'number') out[key] = detail[key];
    }
    if (Array.isArray(detail.webkit)) {
      out.webkit = detail.webkit.map((p) => ({ role: label(p?.role ?? '?'), mb: count(p?.mb, 0) }));
    }
    return out;
  }
  if (isObject(detail) && Object.keys(detail).length === 1 && typeof detail.withheldChars === 'number') return detail;
  return { withheldChars: JSON.stringify(detail)?.length ?? 0 };
}

/**
 * `finding` ({severity, kind, code?, message, detail?}) as a shareable artifact
 * carries it. `message` stays where the oracle worded it; `detail` keeps code
 * locations (a panic's `file:line`), counts and harness-defined labels.
 */
export function shareableFinding(finding) {
  const f = isObject(finding) ? finding : {};
  const kind = label(f.kind ?? '?');
  const out = { severity: label(f.severity ?? '?'), kind };
  if (typeof f.code === 'string') out.code = label(f.code);
  out.message = shareableMessage(kind, f);
  const detail = shareableDetail(kind, f.detail);
  if (detail !== undefined) out.detail = detail;
  return out;
}
