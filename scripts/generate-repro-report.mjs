// Turn an agent session into a paste-ready repro report (report.md in the
// session dir). Aggregates the action log, oracle findings, and evidence paths
// into a bug-report skeleton. Absolute $HOME-bearing paths are rewritten to
// artifact-relative / $HOME placeholders because reports get pasted into public
// GitHub issues.
//
// Usage:
//   node scripts/generate-repro-report.mjs <sessionDir> [--title "..."] [--summary "..."]
//
// Then (after human review) file it:
//   gh issue create --title "..." --body-file <sessionDir>/report.md

import * as fs from 'fs';
import * as path from 'path';

import { shareableFinding } from './lib/shareable-finding.mjs';

const args = process.argv.slice(2);
if (args.length === 0 || args[0].startsWith('--')) {
  process.stderr.write('usage: node scripts/generate-repro-report.mjs <sessionDir> [--title ...] [--summary ...]\n');
  process.exit(2);
}
const sessionDir = args[0];
const getFlag = (name) => {
  const i = args.indexOf(name);
  return i >= 0 ? args[i + 1] : undefined;
};
const jsonlPath = path.join(sessionDir, 'actions.jsonl');
if (!fs.existsSync(jsonlPath)) {
  process.stderr.write(`no actions.jsonl at ${jsonlPath}\n`);
  process.exit(1);
}

const home = process.env.HOME || '';
const absSession = path.resolve(sessionDir);
// A fuzz run copies each private corpus entry it draws into
// `fuzz-files/corpus-<n>/` and keeps its real names there; the app quotes that
// path when an import fails. The harness redacts what it records (#576 RV12-F1),
// but this script only sees the log and cannot know the names, so it also folds
// everything after such a directory - up to the closing quote of the quoted path
// a log line carries, else to the end of the line - to a placeholder.
const CORPUS_COPY = /(fuzz-files[\\/]corpus-\d+)[\\/][^"'`\n]*/g;
const scrub = (s) => {
  if (typeof s !== 'string') return s;
  let out = s;
  if (absSession) out = out.split(absSession).join('<artifacts>');
  if (home) out = out.split(home).join('$HOME');
  return out.replace(CORPUS_COPY, '$1/<corpus file>');
};

/**
 * Render a chosen file as its EXTENSION only. `scrub` rewrites the $HOME prefix
 * but keeps every segment after it, and in this project the basename is the
 * sensitive part: survey filenames name real caves, and the repo bans cave data
 * leaving it. The extension is what a repro actually needs - it selects the
 * importer - so it is the whole of the reproducible signal here.
 */
const redactPath = (value) => {
  // Whitelist, not escape: `path.extname` returns whatever followed the last
  // dot, so a path ending `.stalx\n## forged heading` yields that verbatim and
  // would carry markdown straight past the sanitizer. A real extension is a
  // short alphanumeric run; anything else is dropped rather than repaired.
  const ext = (p) => {
    const raw = path.extname(String(p));
    return /^\.[A-Za-z0-9]{1,12}$/.test(raw) ? raw : '';
  };
  const one = (p) => `<file${ext(p)}>`;
  if (value == null) return '<none>';
  if (Array.isArray(value)) {
    return value.length === 1 ? one(value[0]) : `<${value.length} files${ext(value[0] ?? '')}>`;
  }
  return one(value);
};

/**
 * Neutralize a caller-supplied string for a markdown list item. Tool params are
 * bare strings, and a newline or a backtick in a testid or selector escapes the
 * step and injects arbitrary markdown into a report headed for a public issue.
 * Also capped: nothing here is worth unbounded width.
 */
const mdInline = (s, max = 80) => {
  const flat = String(s).replace(/[\r\n]+/g, ' ');
  const clipped = flat.length > max ? `${flat.slice(0, max)}...` : flat;
  return clipped.replace(/[`<>|]/g, (c) => ({ '`': "'", '<': '&lt;', '>': '&gt;', '|': '\\|' })[c]);
};

/**
 * An argument's SHAPE, never its value (#576 RV18-W23-3). The arguments of an
 * `ipc` / `store_action` call are the project's own data - a station, section
 * or project name - and this report goes to public issues, which keep only
 * codes, counts and code locations (docs/agent-harness.md). A string is its
 * length. A number is its type: it spells no text, but a coordinate, depth or
 * length is survey data all the same. A boolean or null stays - one bit picked
 * by the caller cannot name anything. An object keeps the keys that read as
 * field names (a lowercase-led identifier, the camelCase / snake_case of a
 * command parameter or struct field); any other key - a map keyed by station
 * names - is only counted. An array is its length and element types. Bounded:
 * two levels of keys, at most 12 per object.
 */
const FIELD_NAME = /^[a-z][A-Za-z0-9_]{0,63}$/;
const MAX_KEYS = 12;
const kindOf = (v) => (v === null ? 'null' : Array.isArray(v) ? 'array' : typeof v);
const argShape = (value, depth = 0) => {
  const kind = kindOf(value);
  if (kind === 'null' || kind === 'boolean') return String(value);
  if (kind === 'string') return `string(${value.length})`;
  if (kind === 'array') {
    const kinds = [...new Set(value.map(kindOf))];
    return value.length ? `array(${value.length}) of ${kinds.join('|')}` : 'array(0)';
  }
  if (kind !== 'object') return kind;
  const keys = Object.keys(value);
  if (depth >= 2) return `object(${keys.length} keys)`;
  const named = keys.filter((k) => FIELD_NAME.test(k));
  const parts = named.slice(0, MAX_KEYS).map((k) => `${k}: ${argShape(value[k], depth + 1)}`);
  if (named.length > MAX_KEYS) parts.push(`+${named.length - MAX_KEYS} more`);
  if (keys.length > named.length) parts.push(`${keys.length - named.length} key(s) withheld`);
  return `{${parts.join(', ')}}`;
};

const records = fs
  .readFileSync(jsonlPath, 'utf8')
  .split('\n')
  .filter((l) => l.trim())
  .map((l) => {
    try {
      return JSON.parse(l);
    } catch {
      return null;
    }
  })
  .filter(Boolean);

const start = records.find((r) => r.type === 'session-start') ?? {};
const end = records.find((r) => r.type === 'session-end');
const actions = records.filter((r) => r.type === 'action');
const findingEvents = records.filter((r) => r.type === 'event' && r.event === 'finding');

// Findings captured to disk (finding.json under findings/*/).
const findingsDir = path.join(sessionDir, 'findings');
let capturedFindings = [];
try {
  capturedFindings = fs
    .readdirSync(findingsDir, { withFileTypes: true })
    .filter((e) => e.isDirectory())
    .map((e) => {
      const fp = path.join(findingsDir, e.name, 'finding.json');
      try {
        return { dir: e.name, ...JSON.parse(fs.readFileSync(fp, 'utf8')) };
      } catch {
        return { dir: e.name };
      }
    });
} catch {
  // No findings dir.
}

let version = '?';
try {
  version = JSON.parse(fs.readFileSync(path.join(process.cwd(), 'package.json'), 'utf8')).version;
} catch {
  // Best-effort.
}

const title = getFlag('--title') || `Agent-found issue in session ${start.sessionId ?? path.basename(sessionDir)}`;
const summary = getFlag('--summary') || '_Describe the bug in one or two sentences._';

// Compact step list: mutating actions only.
//
// The recorder stamps `mutating` on every action from the tool that made it
// (`e2e/agent/mcp/server.ts`), so this reads the fact rather than re-deriving
// it. The list below is ONLY for sessions recorded before that field existed:
// it is deliberately frozen, and adding a tool to it is always wrong - set
// `mutates` at the tool definition instead. It was a live defect for exactly
// this reason, having silently omitted drag, scroll, select_option, hover,
// file_dialog_answer and map_feature_click as they were added.
const LEGACY_MUTATING = new Set([
  'click', 'type_text', 'press_keys', 'eval', 'ipc', 'store_action', 'map_click', 'map_view',
  'seed', 'driver_script', 'run_scenario',
]);
const isLegacyMutating = (tool) =>
  LEGACY_MUTATING.has(tool) || (typeof tool === 'string' && tool.startsWith('os_'));
const isMutating = (a) =>
  typeof a.mutating === 'boolean' ? a.mutating : isLegacyMutating(a.tool);
const steps = actions
  .filter((a) => isMutating(a))
  .map((a) => {
    const detail = describeAction(a);
    const flag = a.outcome?.ok === false ? ' **(failed)**' : '';
    return `${a.seq}. \`${a.tool}\` ${detail}${flag}`;
  });

const md = [];
const P = (s = '') => md.push(s);

P(`# ${title}`);
P('');
P(summary);
P('');
P('## Environment');
P('');
P(`- Commit: \`${start.gitHead ?? '?'}\``);
P(`- App version: \`${version}\``);
P(`- Platform: \`${process.platform}\``);
P(`- Display: \`${start.display ?? '?'}\`  headed: \`${Boolean(start.headed)}\``);
P(`- Fresh profile: \`${Boolean(start.freshProfile)}\``);
P(`- Session: \`${start.sessionId ?? path.basename(sessionDir)}\`  actions: \`${end?.actions ?? actions.length}\``);
P('');
P('## Steps to reproduce');
P('');
if (steps.length) steps.forEach((s) => P(scrub(s)));
else P('_No mutating actions recorded._');
P('');
P('Automated replay (regenerate + run):');
P('');
P('```bash');
P(`node scripts/generate-replay.mjs <artifacts>`);
P('replay.spec.ts in the artifacts dir records the exact steps (runner: see e2e/agent/replays/README.md)');
P('```');
P('');
P('## Expected vs actual');
P('');
P('- Expected: _what should have happened._');
P('- Actual: _what happened (see findings below)._');
P('');
P('## Oracle findings');
P('');
// Every finding goes through the shareable projection (#576 HFI2-F2, W21-NF2):
// codes, counts and code locations. A log line, a panic message, the error
// boundary's text, integrity messages and leftover file names all quote the
// app, which names the data it is about - a section, a project, a station -
// and neither scrub above can know that. This report is pasted into public
// issues; the full finding is the `finding` event in the session's
// actions.jsonl, which is not shared. A log finding is named by its target
// (`oracles.logLineCode`), read off the line for a session older than `code`.
const LOG_FINDING_KINDS = new Set(['log-warn', 'log-error']);
const LOG_TARGET = /^(?:ERROR|WARN)\s+([A-Za-z_]\w*(?:::[A-Za-z_]\w*)*):\s/;
const logFindingCode = (f) => {
  if (typeof f.code === 'string' && /^[\w:.-]{1,120}$/.test(f.code)) return f.code;
  return LOG_TARGET.exec(String(f.message ?? ''))?.[1] ?? 'unattributed';
};
const allFindings = (capturedFindings.length ? capturedFindings : findingEvents.map((e) => e.detail)).filter(Boolean);
if (allFindings.length) {
  for (const raw of allFindings) {
    const f = shareableFinding(LOG_FINDING_KINDS.has(raw.kind) ? { ...raw, code: logFindingCode(raw) } : raw);
    P(`- **[${f.severity}] ${f.kind}**${f.code ? ` from \`${f.code}\`` : ''} ${scrub(f.message)}`);
    if (f.detail !== undefined) {
      P('  ```json');
      P('  ' + scrub(JSON.stringify(f.detail, null, 2)).split('\n').join('\n  '));
      P('  ```');
    }
    if (raw.dir) P(`  evidence: \`findings/${raw.dir}/\``);
  }
} else {
  P('_No findings recorded (add the failure detail manually)._');
}
P('');
P('## Evidence');
P('');
// The two logs hold the full text this report withholds (docs/agent-harness.md
// "Not for sharing"); the withheld findings above point at actions.jsonl, so
// they stay listed for whoever triages the run, labelled (#576 RV18-W23-7).
P('- Action log: `actions.jsonl` (local triage only, not for sharing: it holds the full text this report withholds)');
P('- Screenshots: `screenshots/`');
if (capturedFindings.length) {
  P('- Findings: `findings/` (each `finding.json` is shareable; the screenshot and crash-marker copies beside it are not)');
}
P('- Driver log: `tauri-driver.log` (local triage only, not for sharing)');
P('');
P('## Notes');
P('');
P('_Paths above are relative to the session artifact directory. Attach this report and the specific files you checked; do not attach the whole directory, which can hold corpus copies, screenshots and logs that name private files._');
P('');

const outPath = path.join(sessionDir, 'report.md');
fs.writeFileSync(outPath, md.join('\n'));
process.stdout.write(`wrote ${outPath}\n`);

function describeAction(a) {
  const p = a.params ?? {};
  switch (a.tool) {
    case 'click':
    case 'map_click':
      return p.testid ? `testid=${mdInline(p.testid)}` : p.selector ? `selector=${mdInline(p.selector)}` : p.lngLat ? `lngLat=${JSON.stringify(p.lngLat)}` : `coords=(${p.x},${p.y})`;
    case 'map_feature_click': {
      const which =
        p.featureType === 'station'
          ? `station index=${p.stationIndex}`
          : `shot id=${p.shotId}`;
      return `${which} in section ${p.sectionId}${p.button === 'right' ? ' (right)' : ''}`;
    }
    case 'type_text':
      // The typed text is data the user would enter: its length only - see argShape.
      return `${argShape(String(p.text ?? ''))}${p.testid ? ` into ${mdInline(p.testid)}` : ''}`;
    case 'hover':
      return p.testid ? `testid=${mdInline(p.testid)}` : `selector=${mdInline(p.selector)}`;
    case 'drag': {
      const end = (kindTestid, kindSelector, x, y) =>
        kindTestid
          ? `testid=${mdInline(kindTestid)}`
          : kindSelector
            ? `selector=${mdInline(kindSelector)}`
            : `(${x},${y})`;
      const from = end(p.fromTestid, p.fromSelector, p.fromX, p.fromY);
      const to = end(p.toTestid, p.toSelector, p.toX, p.toY);
      return `${from} -> ${to}${p.button && p.button !== 'left' ? ` (${p.button})` : ''}`;
    }
    case 'scroll': {
      const target = p.testid
        ? `testid=${mdInline(p.testid)}`
        : p.selector
          ? `selector=${mdInline(p.selector)}`
          : 'window';
      // Two modes on one tool: a pixel delta, or scroll-into-view.
      return p.deltaX != null || p.deltaY != null
        ? `${target} by (${p.deltaX ?? 0},${p.deltaY ?? 0})`
        : `${target} into view`;
    }
    case 'select_option':
      return `${p.testid ? `testid=${mdInline(p.testid)}` : `selector=${mdInline(p.selector)}`} = "${mdInline(p.value)}"`;
    case 'file_dialog_answer':
      // The chosen path is redacted to its extension - see redactPath.
      return p.cancel
        ? `${p.kind} dialog cancelled (trigger ${mdInline(p.triggerTestid)})`
        : `${p.kind} dialog answered ${redactPath(p.paths ?? p.path)} (trigger ${mdInline(p.triggerTestid)})`;
    case 'press_keys':
      return JSON.stringify(p.keys ?? []);
    case 'eval':
    case 'driver_script':
      return `\`${String(p.script ?? '').slice(0, 60).replace(/\n/g, ' ')}\``;
    case 'seed':
      return p.mode === 'basic'
        ? `basic project "${p.projectName ?? 'harness-project'}"`
        : `open ${p.mode === 'fractal' ? 'fractal ' : ''}fixture ${p.name ?? '(default)'}`;
    case 'run_scenario':
      return `${p.name} (seed=${p.seed ?? 1}${p.iterations != null ? `, iterations=${p.iterations}` : ''})`;
    // Argument shapes, not values - see argShape. store_action args are positional.
    case 'ipc':
      return `${p.command}(${argShape(p.args ?? {})})`;
    case 'store_action': {
      const list = p.args ?? [];
      return `${p.store}.${p.action}(${Array.isArray(list) ? list.map((v) => argShape(v)).join(', ') : argShape(list)})`;
    }
    case 'map_view':
      return p.set ? JSON.stringify(p.set) : 'read';
    case 'os_grant':
      return 'OS input granted for this session';
    case 'os_click':
      return `${p.testid ? `testid=${p.testid}` : `screen ${p.x},${p.y}`}${p.button === 'right' ? ' (right)' : ''}${p.double ? ' (double)' : ''}${p.force ? ' (forced, app was not frontmost)' : ''} [OS-level]`;
    case 'os_keys':
      return `${p.spec}${p.force ? ' (forced, app was not frontmost)' : ''} [OS-level]`;
    case 'os_screenshot':
      return `${p.name ?? 'os-shot'} [OS-level, real pixels]`;
    default:
      return '';
  }
}
