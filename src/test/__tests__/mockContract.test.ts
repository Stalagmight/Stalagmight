import { describe, it, expect } from 'vitest';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { IPC_COMMAND_NAMES } from '../../types/ipc/IpcCommands';
import type { IpcCommandName } from '../../utils/ipc';
import { INERT_COMMANDS } from '../../../e2e-playwright/mocks/ipc/inert';

/**
 * Mock contract tripwire (issue #275).
 *
 * `e2e-playwright/mocks/tauri-ipc.ts` hand-simulates the Rust IPC backend for
 * every Playwright spec. Unknown commands fall through to
 * `default: console.warn(...); return null`, so a new `#[tauri::command]`
 * added to the registry without a matching mock case makes any spec that
 * exercises it fail silently (returns null instead of real data) rather than
 * with a clear error. This test compares both sides of the contract and fails
 * loudly on drift in either direction. It bit PR #59's
 * `split_shot_at_distance`; see the class documented in CLAUDE.md.
 *
 * The registered side is the generated command manifest (#576 C3:
 * `IPC_COMMAND_NAMES`, read out of `generate_handler!` by `pnpm gen:ipc-types`
 * and gated fresh by CI), not a re-parse of lib.rs. The mocked side is still the
 * case labels; what each case RETURNS is checked by `typecheck:playwright`
 * through `MockReply` (e2e-playwright/mocks/ipc/types.ts).
 */

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const REPO_ROOT = path.resolve(__dirname, '../../../');
const MOCK_TS_PATH = path.join(REPO_ROOT, 'e2e-playwright/mocks/tauri-ipc.ts');
// The command families split out of the mock (#576 E8): each module exports one
// `MockCommandFamily` factory with its own `switch (cmd)`.
const MOCK_FAMILY_DIR = path.join(REPO_ROOT, 'e2e-playwright/mocks/ipc');

const mockTsSource = fs.readFileSync(MOCK_TS_PATH, 'utf8');
const mockFamilySources = fs
  .readdirSync(MOCK_FAMILY_DIR)
  .filter((name) => name.endsWith('.ts'))
  .sort()
  .map((name) => ({ name, source: fs.readFileSync(path.join(MOCK_FAMILY_DIR, name), 'utf8') }))
  .filter(({ source }) => /export const \w+: MockCommandFamily\b/.test(source));

/**
 * Scans `source` starting at `openIndex` (which must point at an opening
 * bracket character) and returns the index of its matching closing bracket,
 * skipping over `//` and `/* *‍/` comments and `'`, `"`, `` ` `` string /
 * template literal contents so brackets inside comments or strings don't
 * confuse the depth count. Works for both Rust and TypeScript source.
 */
function findMatchingBracket(source: string, openIndex: number, openChar: string, closeChar: string): number {
  let depth = 0;
  let i = openIndex;
  const n = source.length;
  while (i < n) {
    const ch = source[i];
    if (ch === '/' && source[i + 1] === '/') {
      const nl = source.indexOf('\n', i);
      i = nl === -1 ? n : nl + 1;
      continue;
    }
    if (ch === '/' && source[i + 1] === '*') {
      const end = source.indexOf('*/', i + 2);
      i = end === -1 ? n : end + 2;
      continue;
    }
    if (ch === "'" || ch === '"' || ch === '`') {
      const quote = ch;
      i += 1;
      while (i < n) {
        if (source[i] === '\\') {
          i += 2;
          continue;
        }
        if (source[i] === quote) {
          i += 1;
          break;
        }
        i += 1;
      }
      continue;
    }
    if (ch === openChar) {
      depth += 1;
    } else if (ch === closeChar) {
      depth -= 1;
      if (depth === 0) return i;
    }
    i += 1;
  }
  throw new Error(
    `findMatchingBracket: no matching '${closeChar}' found for '${openChar}' opened at index ${openIndex}`
  );
}

/**
 * Returns a copy of `source` with `//` and `/* *‍/` comment BODIES blanked to
 * spaces (string and template-literal contents are preserved verbatim, and a
 * `//` inside a string is not treated as a comment). Length and indices are
 * unchanged. Without this pass, a comment mentioning a
 * `commands::mod::fn` path or a `case 'name':` label would inject a phantom
 * entry into the extracted command sets.
 */
function stripComments(source: string): string {
  const out = source.split('');
  const n = source.length;
  let i = 0;
  while (i < n) {
    const ch = source[i];
    if (ch === '/' && source[i + 1] === '/') {
      const nl = source.indexOf('\n', i);
      const end = nl === -1 ? n : nl;
      for (let j = i; j < end; j += 1) out[j] = ' ';
      i = end;
      continue;
    }
    if (ch === '/' && source[i + 1] === '*') {
      const close = source.indexOf('*/', i + 2);
      const end = close === -1 ? n : close + 2;
      for (let j = i; j < end; j += 1) {
        if (out[j] !== '\n') out[j] = ' ';
      }
      i = end;
      continue;
    }
    if (ch === "'" || ch === '"' || ch === '`') {
      const quote = ch;
      i += 1;
      while (i < n) {
        if (source[i] === '\\') {
          i += 2;
          continue;
        }
        if (source[i] === quote) {
          i += 1;
          break;
        }
        i += 1;
      }
      continue;
    }
    i += 1;
  }
  return out.join('');
}

/** Every command the backend registers, from the generated manifest. */
function registeredRustCommands(): string[] {
  return [...IPC_COMMAND_NAMES].sort();
}

/**
 * Parses the primary `switch (cmd) { ... }` dispatcher inside
 * `setupTauriMocks` (the shared mock every Playwright spec is backed by) and
 * returns every quoted `case` label, minus Tauri plugin bridge commands
 * (`plugin:window|...`, `plugin:dialog|...`, etc., which aren't
 * `#[tauri::command]`s). Case labels belonging to a *nested* switch (e.g. the
 * `switch (table)` inside the `get_raw_table` case) are excluded: they are
 * masked out before the case-label scan runs.
 *
 * Scopes to the FIRST `switch (cmd)` block in the file, which is the only one
 * (pinned below: the unused second installer that had its own tiny dispatcher
 * was deleted, #576 RV5-PI-7).
 */
function parseMockedCommands(source: string): string[] {
  const switchMatch = /switch\s*\(\s*cmd\s*\)\s*\{/.exec(source);
  if (!switchMatch) {
    throw new Error(
      'parseMockedCommands: could not find `switch (cmd) {` in e2e-playwright/mocks/tauri-ipc.ts. ' +
        'Has the mock dispatcher been restructured?'
    );
  }
  const openIndex = source.indexOf('{', switchMatch.index);
  const closeIndex = findMatchingBracket(source, openIndex, '{', '}');
  let block = stripComments(source.slice(openIndex + 1, closeIndex));

  // Mask out any nested `switch (...) { ... }` blocks so their case labels
  // (e.g. table names inside `get_raw_table`'s `switch (table)`) aren't
  // mistaken for mocked IPC commands.
  for (;;) {
    const nested = /switch\s*\([^()]*\)\s*\{/.exec(block);
    if (!nested) break;
    const nestedOpen = block.indexOf('{', nested.index);
    const nestedClose = findMatchingBracket(block, nestedOpen, '{', '}');
    block = block.slice(0, nested.index) + block.slice(nestedClose + 1);
  }

  const caseRegex = /case\s+(['"])((?:(?!\1).)*)\1\s*:/g;
  const names = new Set<string>();
  let match: RegExpExecArray | null;
  while ((match = caseRegex.exec(block)) !== null) {
    const name = match[2]!;
    if (!name.startsWith('plugin:')) {
      names.add(name);
    }
  }

  if (names.size === 0) {
    throw new Error(
      'parseMockedCommands: found the switch (cmd) block but extracted zero case labels. ' +
        'The case-label pattern this test looks for may no longer match.'
    );
  }
  return [...names].sort();
}

/**
 * Every command the mock handles: the core's switch (`tauri-ipc.ts`) plus the switch of
 * each command family under `e2e-playwright/mocks/ipc/` (#576 E8). A family file whose
 * factory the core does not list in `MOCK_COMMAND_FAMILIES` is never asked about any
 * command, so its cases are not mocked - that fails here rather than counting them.
 */
function parseAllMockedCommands(): string[] {
  const listMatch = /const MOCK_COMMAND_FAMILIES[^=]*=\s*\[([^\]]*)\]/.exec(mockTsSource);
  if (!listMatch) {
    throw new Error('parseAllMockedCommands: could not find `MOCK_COMMAND_FAMILIES` in e2e-playwright/mocks/tauri-ipc.ts.');
  }
  const registeredFamilies = new Set(listMatch[1]!.split(',').map((s) => s.trim()).filter(Boolean));
  const names = new Set(parseMockedCommands(mockTsSource));
  for (const { name, source } of mockFamilySources) {
    const family = /export const (\w+): MockCommandFamily\b/.exec(source)?.[1];
    if (!family || !registeredFamilies.has(family)) {
      throw new Error(
        `parseAllMockedCommands: the command family ${family ?? '?'} (e2e-playwright/mocks/ipc/${name}) ` +
          'is not listed in MOCK_COMMAND_FAMILIES in tauri-ipc.ts, so none of its cases is reachable.'
      );
    }
    for (const cmd of parseMockedCommands(source)) names.add(cmd);
  }
  return [...names].sort();
}

/**
 * Every command the frontend names in a string literal (`invokeIpc('x', ...)`, a raw
 * `invoke('x')`, a queued call, a command list), scanned over the non-test files under
 * `src/` (the generated IPC types excluded). A registered command NOT named anywhere has
 * no UI path a spec could reach it by.
 */
function frontendCommandNames(): Set<string> {
  const srcRoot = path.join(REPO_ROOT, 'src');
  const skipDirs = new Set([path.join(srcRoot, 'test'), path.join(srcRoot, 'types', 'ipc')]);
  const files: string[] = [];
  const walk = (dir: string) => {
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) {
        if (entry.name === '__tests__' || entry.name === '__mocks__' || skipDirs.has(full)) continue;
        walk(full);
      } else if (/\.(ts|tsx)$/.test(entry.name) && !/\.test\.tsx?$/.test(entry.name)) {
        files.push(full);
      }
    }
  };
  walk(srcRoot);
  const source = files.map((f) => fs.readFileSync(f, 'utf8')).join('\n');
  return new Set(IPC_COMMAND_NAMES.filter((name) => new RegExp(`['"\`]${name}['"\`]`).test(source)));
}

/** The case groups of the first `switch (cmd) { ... }` in `source`: each run of labels with its body. */
function parseCaseGroups(source: string): Array<{ labels: string[]; body: string }> {
  const switchMatch = /switch\s*\(\s*cmd\s*\)\s*\{/.exec(source);
  if (!switchMatch) throw new Error('parseCaseGroups: no `switch (cmd) {` found');
  const openIndex = source.indexOf('{', switchMatch.index);
  const closeIndex = findMatchingBracket(source, openIndex, '{', '}');
  let block = stripComments(source.slice(openIndex + 1, closeIndex));
  // Nested blocks that are not the command's own: an inner `switch` (whose labels are
  // not commands) and every function body (whose `return` answers the callback, not
  // the command), blanked to spaces so offsets stay put.
  const mask = (pattern: RegExp) => {
    for (;;) {
      pattern.lastIndex = 0;
      const found = pattern.exec(block);
      if (!found) return;
      const open = found.index + found[0].length - 1;
      const close = findMatchingBracket(block, open, '{', '}');
      block = block.slice(0, found.index) + ' '.repeat(close + 1 - found.index) + block.slice(close + 1);
    }
  };
  mask(/switch\s*\([^()]*\)\s*\{/g);
  mask(/=>\s*\{/g);
  mask(/\bfunction\b[^{;]*\{/g);

  // A label is a whole word: `zoom_default:` is a config key, not the switch's default.
  const labelRun = /(?:\s*(?<![\w$])case\s+(['"])(?:(?!\1).)*\1\s*:)+|\s*(?<![\w$])default\s*:/g;
  const groups: Array<{ labels: string[]; start: number; end: number; isDefault: boolean }> = [];
  let match: RegExpExecArray | null;
  while ((match = labelRun.exec(block)) !== null) {
    const labels = [...match[0].matchAll(/case\s+(['"])((?:(?!\1).)*)\1\s*:/g)].map((m) => m[2]!);
    if (groups.length > 0) groups[groups.length - 1]!.end = match.index;
    groups.push({ labels, start: match.index + match[0].length, end: block.length, isDefault: labels.length === 0 });
  }
  return groups
    .filter((g) => !g.isDefault)
    .map((g) => ({ labels: g.labels, body: block.slice(g.start, g.end) }));
}

/** The text of the statement that starts at `start`, up to its `;` at bracket depth 0. */
function statementAt(source: string, start: number): string {
  let depth = 0;
  for (let i = start; i < source.length; i += 1) {
    const ch = source[i];
    if (ch === "'" || ch === '"' || ch === '`') {
      const quote = ch;
      i += 1;
      while (i < source.length && source[i] !== quote) i += source[i] === '\\' ? 2 : 1;
      continue;
    }
    if (ch === '(' || ch === '[' || ch === '{') depth += 1;
    else if (ch === ')' || ch === ']' || ch === '}') depth -= 1;
    else if (ch === ';' && depth === 0) return source.slice(start, i + 1);
  }
  return source.slice(start);
}

/**
 * The answers of a case group a scan finds untyped: a `return` whose value is not
 * written `satisfies MockReply<'<one of the group's labels>'>` (or a Promise of one), a
 * rejection, or a sub-command's own reply. Tauri plugin bridge cases are not commands
 * and have no manifest entry, so they are not held to it.
 */
function untypedReturns(source: string, file: string): string[] {
  const out: string[] = [];
  for (const { labels, body } of parseCaseGroups(source)) {
    const commands = labels.filter((l) => !l.startsWith('plugin:'));
    if (commands.length === 0) continue;
    for (const ret of body.matchAll(/\breturn\b/g)) {
      const stmt = statementAt(body, ret.index!).replace(/\s+/g, ' ');
      const typed = commands.some((c) => stmt.includes(`MockReply<'${c}'>`));
      const passThrough =
        /^return Promise\.reject\(/.test(stmt) || /^return (result|ctx\.unhandled);$/.test(stmt);
      if (!typed && !passThrough) out.push(`${file} case ${commands.join('/')}: ${stmt.slice(0, 120)}`);
    }
  }
  return out;
}

// ----------------------------------------------------------------------------
// The commands the mock deliberately does not answer: only commands NO frontend code
// names, so no spec can reach them (checked below). A command that gains a UI path
// needs a mock case, or an entry in mocks/ipc/inert.ts, before its first spec runs:
// a spec that reaches an unanswered command fails at teardown (#576 E3-D2).
// ----------------------------------------------------------------------------
const UNMOCKED_COMMAND_REASONS: Partial<Record<IpcCommandName, string>> = {
  debug_trigger_panic: 'dev-only crash trigger: no frontend code calls it (the debug build reaches it from the agent harness)',
  get_recent_logs_since: 'no frontend caller (the Debug dialog reads get_recent_logs, the unbounded variant)',
};

const inertCommands = Object.keys(INERT_COMMANDS);

describe('mock contract tripwire (#275, #576 E3)', () => {
  it('reads a non-empty registered Rust command list from the manifest', () => {
    expect(registeredRustCommands().length).toBeGreaterThan(0);
  });

  it('parses a non-empty mocked command list', () => {
    expect(parseAllMockedCommands().length).toBeGreaterThan(0);
  });

  it('every registered command has a mock case, an inert entry or a no-UI-path reason', () => {
    const mocked = new Set(parseAllMockedCommands());
    const missing = registeredRustCommands().filter(
      (cmd) => !mocked.has(cmd) && !inertCommands.includes(cmd) && !(cmd in UNMOCKED_COMMAND_REASONS)
    );
    console.log(
      `[E3] registered ${registeredRustCommands().length}, mocked ${mocked.size}, inert ${inertCommands.length}, ` +
        `unmocked (no UI path) ${Object.keys(UNMOCKED_COMMAND_REASONS).length}, missing ${missing.length}`
    );
    expect(
      missing,
      'registered #[tauri::command](s) with no mock case (e2e-playwright/mocks/tauri-ipc.ts or mocks/ipc/), ' +
        'no inert entry (mocks/ipc/inert.ts) and no reason here'
    ).toEqual([]);
  });

  it('every command the frontend names is answered by the mock (a case or an inert entry)', () => {
    const mocked = new Set(parseAllMockedCommands());
    const unanswered = [...frontendCommandNames()]
      .filter((cmd) => !mocked.has(cmd) && !inertCommands.includes(cmd))
      .sort();
    expect(unanswered, 'commands a UI path reaches that no mock case or inert entry answers').toEqual([]);
  });

  it('every mock case and inert entry names a registered command', () => {
    const registered = new Set(registeredRustCommands());
    const orphaned = [...parseAllMockedCommands(), ...inertCommands].filter((cmd) => !registered.has(cmd));
    expect(orphaned, 'mock cases / inert entries for commands the backend does not register').toEqual([]);
  });

  it('the no-UI-path reasons are only for commands no frontend code names, and nothing answers', () => {
    const named = frontendCommandNames();
    const mocked = new Set(parseAllMockedCommands());
    const registered = new Set(registeredRustCommands());
    const wrong = Object.keys(UNMOCKED_COMMAND_REASONS).filter(
      (cmd) => named.has(cmd) || mocked.has(cmd) || inertCommands.includes(cmd) || !registered.has(cmd)
    );
    expect(wrong, 'these entries have a UI path, an answer, or no command: move them or remove them').toEqual([]);
  });

  it('an inert command has a reason and no mock case of its own', () => {
    const mocked = new Set(parseAllMockedCommands());
    const bad = Object.entries(INERT_COMMANDS)
      .filter(([cmd, entry]) => mocked.has(cmd) || !entry || entry.reason.trim().length < 10)
      .map(([cmd]) => cmd);
    expect(bad).toEqual([]);
  });

  it('every reply a mock case returns is typed against the generated manifest', () => {
    const untyped = [
      ...untypedReturns(mockTsSource, 'tauri-ipc.ts'),
      ...mockFamilySources.flatMap(({ name, source }) => untypedReturns(source, `ipc/${name}`)),
    ];
    expect(untyped, "write each reply `satisfies MockReply<'<command>'>` (e2e-playwright/mocks/ipc/types.ts)").toEqual([]);
  });

  it('flags an untyped reply and accepts a typed one or a rejection (scan self-test)', () => {
    const source = [
      'switch (cmd) {',
      "  case 'good': return 1 satisfies MockReply<'good'>;",
      "  case 'bad': { const xs = [1].map((x) => { return x; }); return { success: true }; }",
      "  case 'refused': return Promise.reject({ code: 'FAILED', message: 'no' });",
      "  case 'plugin:window|close': return null;",
      '  default: return null;',
      '}',
    ].join('\n');
    expect(untypedReturns(source, 'fixture')).toEqual(['fixture case bad: return { success: true };']);
  });

  it('ignores case labels that appear only in comments', () => {
    const tsSource = [
      'switch (cmd) {',
      "  case 'real_case':",
      '    return null;',
      "  // see also case 'phantom_case': in the legacy helper",
      '  default:',
      '    return null;',
      '}',
    ].join('\n');
    expect(parseMockedCommands(tsSource)).toEqual(['real_case']);
  });
});

// [#576 RV5-PI-7] The mock has ONE dispatcher. A second, uncalled installer with
// its own `switch (cmd)` answered unannotated, non-wire shapes (save_project
// without warnings; get_project / load_project, which are not commands) that no
// MockReply check covered, and would have been a silent second mock for any spec
// that reached for it.
describe('the mock has one installer (#576 RV5-PI-7)', () => {
  it('carries no second, unchecked dispatcher', () => {
    const dispatchers = (mockTsSource.match(/switch\s*\(\s*cmd\s*\)/g) ?? []).length;
    console.log(
      `[RV5-PI-7] setupTauriMocksWithProject in the mock: ${mockTsSource.includes('setupTauriMocksWithProject')}; ` +
        `switch (cmd) dispatchers in tauri-ipc.ts: ${dispatchers}`,
    );
    expect(mockTsSource).not.toContain('setupTauriMocksWithProject');
    expect(dispatchers).toBe(1);
  });
});
