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

import { foldLocalPaths } from './lib/fold-local-paths.mjs';
import { shareableFinding } from './lib/shareable-finding.mjs';
import { shareableSelector, shareableTestId } from './lib/shareable-selector.mjs';

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
  // Folded: it is the path the caller gave, which can name the user's own
  // directories (#576 W51, W58).
  process.stderr.write(`no actions.jsonl in ${foldLocalPaths(sessionDir)}\n`);
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

/**
 * A value this report reads back from actions.jsonl or a finding.json, printed
 * only in the form the harness writes it (#576 W49-NF1). The tool's schema or
 * the recorder gives it a number, an integer, a word of a fixed set or a name,
 * but a forged or hand-edited copy can put any string there - a newline and a
 * `## heading` start a markdown line of their own, as a map_feature_click id
 * did (#576 W47-NF2) - or a name of the user's data. Any other value is given
 * as its shape (see argShape). A real session's values print as they always did.
 */
const id = (v) => (Number.isInteger(v) ? String(v) : argShape(v));
const num = (v) => (typeof v === 'number' && Number.isFinite(v) ? String(v) : argShape(v));
const word = (words, v) => (words.has(v) ? v : argShape(v));
/**
 * A name the harness writes: a tool, an IPC command, a store or its action, a
 * scenario, a session id, a screenshot label (`os_screenshot`'s schema allows
 * `[\w.-]{1,64}`). Identifier characters only, as `logFindingCode` keeps a code.
 */
const NAME = /^[\w.:-]{1,80}$/;
const code = (v, shape = NAME) => (typeof v === 'string' && shape.test(v) ? v : argShape(v));
/**
 * An `ipc` step's command: a NAME, or a Tauri plugin command
 * `plugin:<plugin>|<command>` whose command is a NAME (#576 W58). The `|` prints
 * escaped (`plugin:dialog\|open`), as the report printed it before W49-NF1, so
 * it cannot split a markdown table cell. Anything else - a space, a newline, a
 * backtick, a second `|` - only a forged log holds, and is its shape.
 */
const PLUGIN_COMMAND = /^(plugin:[\w-]{1,80})\|([\w.:-]{1,80})$/;
const ipcCommand = (v) => {
  const plugin = typeof v === 'string' ? PLUGIN_COMMAND.exec(v) : null;
  return plugin ? `${plugin[1]}\\|${plugin[2]}` : code(v);
};
/** `git rev-parse --short HEAD`, or `unknown` (e2e/agent/lib/build.ts `gitHeadFor`). */
const GIT_HEAD = /^(?:[0-9a-f]{4,64}|unknown)$/;
/** An X display (`:89`, `:0.0`) or macOS's `aqua`; a host-prefixed DISPLAY is the user's env. */
const DISPLAY = /^(?::\d{1,5}(?:\.\d{1,3})?|aqua)?$/;
const VERSION = /^(?:\d+\.\d+\.\d+[\w.+-]*|\?)$/;
const BUTTONS = new Set(['left', 'right', 'middle']);
const DIALOG_KINDS = new Set(['open', 'save']);

/**
 * A `press_keys` step's keys (#576 W39-NF2). `browser.keys` presses a WebDriver
 * key NAME as that key and types any other string grapheme by grapheme, so a
 * run of single characters - or one longer string - spells text, a station or
 * project name, as surely as `type_text` does. A key name is a code and stays;
 * a WebDriver key code point (U+E000..U+E05D) stays as its code point; every
 * run of other keys is only counted, in characters, as `type_text`'s text is.
 * The names are `UNICODE_CHARACTERS` of @wdio/utils, less `" "`, which types a
 * space between words.
 */
const WEBDRIVER_KEY_NAMES = new Set([
  'NULL', 'Unidentified', 'Cancel', 'Help', 'Backspace', 'Back space', 'Tab', 'Clear', 'Return', 'Enter',
  'Shift', 'Control', 'Control Left', 'Control Right', 'Alt', 'Pause', 'Escape', 'Space',
  'PageUp', 'Pageup', 'Page_Up', 'PageDown', 'Pagedown', 'Page_Down', 'End', 'Home',
  'ArrowLeft', 'Left arrow', 'Arrow_Left', 'ArrowUp', 'Up arrow', 'Arrow_Up',
  'ArrowRight', 'Right arrow', 'Arrow_Right', 'ArrowDown', 'Down arrow', 'Arrow_Down',
  'Insert', 'Delete', 'Semicolon', 'Equals',
  'Numpad 0', 'Numpad 1', 'Numpad 2', 'Numpad 3', 'Numpad 4', 'Numpad 5', 'Numpad 6', 'Numpad 7', 'Numpad 8', 'Numpad 9',
  'Multiply', 'Add', 'Separator', 'Subtract', 'Decimal', 'Divide',
  'F1', 'F2', 'F3', 'F4', 'F5', 'F6', 'F7', 'F8', 'F9', 'F10', 'F11', 'F12',
  'Command', 'Meta', 'ZenkakuHankaku', 'Zenkaku_Hankaku',
]);
const WEBDRIVER_KEY_CODE = /^[\uE000-\uE05D]$/;
const describeKeys = (keys) => {
  const parts = [];
  let typed = 0;
  const flush = () => {
    if (typed > 0) parts.push(`${typed} character${typed === 1 ? '' : 's'}`);
    typed = 0;
  };
  for (const key of Array.isArray(keys) ? keys : keys == null ? [] : [keys]) {
    if (typeof key === 'string' && WEBDRIVER_KEY_NAMES.has(key)) {
      flush();
      parts.push(JSON.stringify(key));
    } else if (typeof key === 'string' && WEBDRIVER_KEY_CODE.test(key)) {
      flush();
      parts.push(`U+${key.charCodeAt(0).toString(16).toUpperCase()}`);
    } else {
      typed += String(key).length;
    }
  }
  flush();
  return `[${parts.join(', ')}]`;
};

/**
 * An `os_keys` step's chord (#576 RV19b-P6), by `describeKeys`' rule. Each step
 * presses one chord, so a run of them spells text a key at a time as a run of
 * `press_keys` keys does. A modifier and a named key of the chord grammar
 * (e2e/agent/os/README.md, `helper/src/keyspec.rs`) stay, lower-cased as the
 * grammar reads them; any other key - a letter, a digit - is counted in
 * characters.
 */
const OS_KEY_NAMES = new Set([
  'cmd', 'command', 'meta', 'super', 'win', 'shift', 'alt', 'opt', 'option', 'ctrl', 'control', 'fn', 'function',
  'return', 'enter', 'escape', 'esc', 'tab', 'space', 'delete', 'backspace', 'forwarddelete',
  'up', 'down', 'left', 'right', 'home', 'end', 'pageup', 'pagedown', 'help', 'keypadenter',
  'minus', 'equal', 'comma', 'period', 'slash', 'backslash', 'semicolon', 'quote',
  'leftbracket', 'rightbracket', 'grave', 'backtick',
  'f1', 'f2', 'f3', 'f4', 'f5', 'f6', 'f7', 'f8', 'f9', 'f10', 'f11', 'f12',
]);
const describeChord = (spec) =>
  `[${String(spec ?? '')
    .split('+')
    .map((part) => {
      const key = part.trim().toLowerCase();
      if (OS_KEY_NAMES.has(key)) return JSON.stringify(key);
      const typed = part.trim().length;
      return `${typed} character${typed === 1 ? '' : 's'}`;
    })
    .join(', ')}]`;

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

const title = getFlag('--title') || `Agent-found issue in session ${code(start.sessionId ?? path.basename(sessionDir))}`;
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
    return `${id(a.seq)}. \`${code(a.tool)}\` ${detail}${flag}`;
  });

const md = [];
const P = (s = '') => md.push(s);

P(`# ${title}`);
P('');
P(summary);
P('');
P('## Environment');
P('');
P(`- Commit: \`${start.gitHead == null ? '?' : code(start.gitHead, GIT_HEAD)}\``);
P(`- App version: \`${code(version, VERSION)}\``);
P(`- Platform: \`${process.platform}\``);
P(`- Display: \`${start.display == null ? '?' : code(start.display, DISPLAY)}\`  headed: \`${Boolean(start.headed)}\``);
P(`- Fresh profile: \`${Boolean(start.freshProfile)}\``);
P(`- Session: \`${code(start.sessionId ?? path.basename(sessionDir))}\`  actions: \`${id(end?.actions ?? actions.length)}\``);
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
    // The directory's name, or a `dir` a finding.json or a forged log carries.
    if (raw.dir) P(`  evidence: \`findings/${code(raw.dir)}/\``);
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
// Images of the app: they show its text and corpus file names, and nothing can
// fold an image (#576 W33-NF5).
P('- Screenshots: `screenshots/` (local triage only, not for sharing: they show the app\'s text)');
if (capturedFindings.length) {
  P('- Findings: `findings/` (each `finding.json` is shareable; the screenshot and crash-marker copies beside it are not)');
}
P('- Driver log: `tauri-driver.log` (local triage only, not for sharing)');
P('');
P('## Notes');
P('');
P('_Paths above are relative to the session artifact directory. Attach this report and only files docs/agent-harness.md lists as shareable (a `finding.json`, a fuzz trace); not a file marked local triage only, and not the whole directory, which holds corpus copies, screenshots and logs that name private files._');
P('');

const outPath = path.join(sessionDir, 'report.md');
fs.writeFileSync(outPath, md.join('\n'));
// Folded: the session dir is the caller's path (#576 W51, W58).
process.stdout.write(`wrote ${foldLocalPaths(outPath)}\n`);

function describeAction(a) {
  const p = a.params ?? {};
  switch (a.tool) {
    case 'click':
    case 'map_click':
      // A geographic point is where the cave is: its shape only (#576 W33-NF2).
      // Screen coordinates stay - they place a pointer, not a cave.
      return p.testid ? `testid=${shareableTestId(p.testid)}` : p.selector ? `selector=${shareableSelector(p.selector)}` : p.lngLat ? `lngLat=${argShape(p.lngLat)}` : `coords=(${num(p.x)},${num(p.y)})`;
    case 'map_feature_click': {
      // The ids are integers in the tool's schema, but this reads whatever
      // actions.jsonl holds: anything else - a forged or hand-edited log - is
      // given as its shape, so a newline cannot start a heading (#576 W47-NF2).
      const which =
        p.featureType === 'station'
          ? `station index=${id(p.stationIndex)}`
          : `shot id=${id(p.shotId)}`;
      return `${which} in section ${id(p.sectionId)}${p.button === 'right' ? ' (right)' : ''}`;
    }
    case 'type_text':
      // The typed text is data the user would enter: its length only - see argShape.
      return `${argShape(String(p.text ?? ''))}${p.testid ? ` into ${shareableTestId(p.testid)}` : ''}`;
    case 'hover':
      return p.testid ? `testid=${shareableTestId(p.testid)}` : `selector=${shareableSelector(p.selector)}`;
    case 'drag': {
      const end = (kindTestid, kindSelector, x, y) =>
        kindTestid
          ? `testid=${shareableTestId(kindTestid)}`
          : kindSelector
            ? `selector=${shareableSelector(kindSelector)}`
            : `(${num(x)},${num(y)})`;
      const from = end(p.fromTestid, p.fromSelector, p.fromX, p.fromY);
      const to = end(p.toTestid, p.toSelector, p.toX, p.toY);
      return `${from} -> ${to}${p.button && p.button !== 'left' ? ` (${word(BUTTONS, p.button)})` : ''}`;
    }
    case 'scroll': {
      const target = p.testid
        ? `testid=${shareableTestId(p.testid)}`
        : p.selector
          ? `selector=${shareableSelector(p.selector)}`
          : 'window';
      // Two modes on one tool: a pixel delta, or scroll-into-view.
      return p.deltaX != null || p.deltaY != null
        ? `${target} by (${num(p.deltaX ?? 0)},${num(p.deltaY ?? 0)})`
        : `${target} into view`;
    }
    case 'select_option':
      // The chosen option can be a section or a station: its length only (#576 W33-NF2).
      return `${p.testid ? `testid=${shareableTestId(p.testid)}` : `selector=${shareableSelector(p.selector)}`} = ${argShape(String(p.value ?? ''))}`;
    case 'file_dialog_answer':
      // The chosen path is redacted to its extension - see redactPath.
      return p.cancel
        ? `${word(DIALOG_KINDS, p.kind)} dialog cancelled (trigger ${shareableTestId(p.triggerTestid)})`
        : `${word(DIALOG_KINDS, p.kind)} dialog answered ${redactPath(p.paths ?? p.path)} (trigger ${shareableTestId(p.triggerTestid)})`;
    case 'press_keys':
      // Named keys stay, typed text is counted - see describeKeys (#576 W39-NF2).
      return describeKeys(p.keys);
    case 'eval':
    case 'driver_script':
      // A script can name what it acts on - a station, a section: its length only (#576 W33-NF2).
      return `script ${argShape(String(p.script ?? ''))}`;
    case 'seed':
      // The project's name is the user's; a fixture is a survey file, named by its
      // extension only - see redactPath (#576 W33-NF2).
      return p.mode === 'basic'
        ? `basic project ${p.projectName == null ? '(default name)' : `named ${argShape(String(p.projectName))}`}`
        : `open ${p.mode === 'fractal' ? 'fractal ' : ''}fixture ${p.name == null ? '(default)' : redactPath(p.name)}`;
    case 'run_scenario':
      return `${code(p.name)} (seed=${num(p.seed ?? 1)}${p.iterations != null ? `, iterations=${num(p.iterations)}` : ''})`;
    // Argument shapes, not values - see argShape. store_action args are positional.
    case 'ipc':
      return `${ipcCommand(p.command)}(${argShape(p.args ?? {})})`;
    case 'store_action': {
      const list = p.args ?? [];
      return `${code(p.store)}.${code(p.action)}(${Array.isArray(list) ? list.map((v) => argShape(v)).join(', ') : argShape(list)})`;
    }
    case 'map_view':
      // The camera's center is where the cave is: the shape only (#576 W33-NF2).
      return p.set ? `set ${argShape(p.set)}` : 'read';
    case 'os_grant':
      return 'OS input granted for this session';
    case 'os_click':
      return `${p.testid ? `testid=${shareableTestId(p.testid)}` : `screen ${num(p.x)},${num(p.y)}`}${p.button === 'right' ? ' (right)' : ''}${p.double ? ' (double)' : ''}${p.force ? ' (forced, app was not frontmost)' : ''} [OS-level]`;
    case 'os_keys':
      // Named keys stay, a typed key is counted - see describeChord (#576 RV19b-P6).
      return `${describeChord(p.spec)}${p.force ? ' (forced, app was not frontmost)' : ''} [OS-level]`;
    case 'os_screenshot':
      return `${code(p.name ?? 'os-shot')} [OS-level, real pixels]`;
    default:
      return '';
  }
}
