import { describe, it, expect } from 'vitest';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

/**
 * Mock contract tripwire (issue #275).
 *
 * `e2e-playwright/mocks/tauri-ipc.ts` hand-simulates the Rust IPC backend for
 * every Playwright spec. Unknown commands fall through to
 * `default: console.warn(...); return null`, so a new `#[tauri::command]`
 * added to the registry without a matching mock case makes any spec that
 * exercises it fail silently (returns null instead of real data) rather than
 * with a clear error. This test parses both sides of the contract and fails
 * loudly on drift in either direction. It bit PR #59's
 * `split_shot_at_distance`; see the class documented in CLAUDE.md.
 */

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const REPO_ROOT = path.resolve(__dirname, '../../../');
const LIB_RS_PATH = path.join(REPO_ROOT, 'src-tauri/src/lib.rs');
const MOCK_TS_PATH = path.join(REPO_ROOT, 'e2e-playwright/mocks/tauri-ipc.ts');

const libRsSource = fs.readFileSync(LIB_RS_PATH, 'utf8');
const mockTsSource = fs.readFileSync(MOCK_TS_PATH, 'utf8');

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

/**
 * Parses the `tauri::generate_handler![...]` command list out of
 * `src-tauri/src/lib.rs`. Every entry there is fully qualified
 * (`commands::<module>::<fn>`), so this extracts the last path segment as
 * the wire command name Tauri dispatches on.
 */
function parseRegisteredRustCommands(source: string): string[] {
  const macroMatch = /tauri::generate_handler!\s*\[/.exec(source);
  if (!macroMatch) {
    throw new Error(
      'parseRegisteredRustCommands: could not find `tauri::generate_handler![` in src-tauri/src/lib.rs. ' +
        'Has the invoke_handler registration moved or been renamed?'
    );
  }
  const openIndex = source.indexOf('[', macroMatch.index);
  const closeIndex = findMatchingBracket(source, openIndex, '[', ']');
  const block = stripComments(source.slice(openIndex + 1, closeIndex));

  const commandRegex = /\bcommands::[A-Za-z_][A-Za-z0-9_]*::([A-Za-z_][A-Za-z0-9_]*)\b/g;
  const names = new Set<string>();
  let match: RegExpExecArray | null;
  while ((match = commandRegex.exec(block)) !== null) {
    names.add(match[1]!);
  }

  if (names.size === 0) {
    throw new Error(
      'parseRegisteredRustCommands: found the generate_handler! block but extracted zero command names. ' +
        'The `commands::<module>::<fn>` pattern this test looks for may no longer match.'
    );
  }
  return [...names].sort();
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
 * Deliberately scopes to only the FIRST `switch (cmd)` block in the file.
 * The file also defines a second, unused legacy helper
 * (`setupTauriMocksWithProject`) with its own tiny `switch (cmd)` that no
 * spec calls into; it is not "the mock" this tripwire is about.
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

// ----------------------------------------------------------------------------
// Allowlist: registered Rust commands with no mock case in the shared switch,
// each with a one-line reason it's deliberately unmocked today. If a command
// listed here gains real e2e coverage, add a mock case and remove the entry
// (the "allowlist entries stay accurate" test below catches a stale removal
// the other way: a command that no longer needs the exemption).
// ----------------------------------------------------------------------------
const UNMOCKED_COMMAND_REASONS: Record<string, string> = {
  // Dev/debug-only, not reachable via any UI flow a spec drives.
  debug_trigger_panic: 'dev-only crash-testing command, not reachable via any UI flow',
  get_memory_stats: 'dev/debug memory probe, not reachable via any UI flow',

  // Telemetry pipeline: fire-and-forget background reporting, not asserted
  // on by mocked e2e specs.
  flush_telemetry: 'background telemetry flush, not exercised by mocked e2e specs',
  is_telemetry_available: 'background telemetry probe, not exercised by mocked e2e specs',
  set_telemetry_enabled: 'background telemetry toggle, not exercised by mocked e2e specs',
  track_events: 'background telemetry event batch, not exercised by mocked e2e specs',

  // Recovery/corruption-repair workflow: covered via the file's own
  // `__MOCK_IPC_OVERRIDES__` escape hatch, set per-spec, not the shared switch.
  attempt_repair: 'covered via __MOCK_IPC_OVERRIDES__ in corruption-recovery.spec.ts',
  export_salvageable_data: 'covered via __MOCK_IPC_OVERRIDES__ in corruption-recovery.spec.ts',
  list_backups: 'covered via __MOCK_IPC_OVERRIDES__ in corruption-recovery.spec.ts / recovery-integrity.spec.ts',
  restore_from_backup: 'covered via __MOCK_IPC_OVERRIDES__ in corruption-recovery.spec.ts / recovery-integrity.spec.ts',

  // Undo/redo history dropdown: covered via a spec-local invoke override
  // layered on top of the shared mock, not the shared switch itself.
  get_undo_stack: 'covered via a spec-local invoke override in undo-redo-dropdown.spec.ts',
  get_redo_stack: 'covered via a spec-local invoke override in undo-redo-dropdown.spec.ts',
  undo_to: 'covered via a spec-local invoke override in undo-redo-dropdown.spec.ts',
  redo_to: 'covered via a spec-local invoke override in undo-redo-dropdown.spec.ts',

  // Legacy/alternate import-format two-step parse+execute flows: no mocked
  // spec drives the parse step end to end (only the "execute" commands for
  // the formats that ARE covered, e.g. execute_walls_import, are mocked).
  parse_compass: 'no mocked spec drives the compass parse+execute two-step import flow',
  execute_compass_import: 'no mocked spec drives the compass parse+execute two-step import flow',
  parse_walls: 'walls-import-dialog.spec.ts uses execute_walls_import directly; parse_walls is not called',
  speleodb_done: 'SpeleoDB write-workflow, env-gated live tests only, not mocked e2e',
  speleodb_get_revisions: 'SpeleoDB write-workflow, env-gated live tests only, not mocked e2e',
  speleodb_open_project: 'SpeleoDB write-workflow, env-gated live tests only, not mocked e2e',
  speleodb_open_project_discarding_local:
    'SpeleoDB write-workflow, env-gated live tests only, not mocked e2e',
  speleodb_push: 'SpeleoDB write-workflow, env-gated live tests only, not mocked e2e',
  speleodb_restore_revision: 'SpeleoDB write-workflow, env-gated live tests only, not mocked e2e',
  speleodb_start_editing: 'SpeleoDB write-workflow, env-gated live tests only, not mocked e2e',
  speleodb_update: 'SpeleoDB write-workflow, env-gated live tests only, not mocked e2e',

  // Dock/panel layout persistence: only unit-tested (uiStore.savePanelSizes.test.ts).
  // The one e2e spec that resizes a panel (panel-interactions.spec.ts) only
  // asserts the client-side width, never the backend persist call.
  update_panel_layout: 'persistence round-trip is unit-tested (uiStore), not asserted by any e2e spec',
  update_panel_sizes: 'persistence round-trip is unit-tested (uiStore), not asserted by any e2e spec',

  // Tile-cache detail commands: no e2e spec drives a real tile fetch (map
  // tiles come from mocked layers); get_cache_stats/clear_tile_cache/
  // get_network_status/get_tile_providers ARE mocked and cover the tile
  // cache UI paths specs actually exercise.
  fetch_tile: 'no e2e spec drives a real tile fetch (map tiles are mocked at the layer level)',
  get_tile_fetch_stats: 'no e2e spec drives a real tile fetch (map tiles are mocked at the layer level)',

  // Section-graph / relationship queries and warning-resolution: no
  // frontend caller reaches these from any mocked e2e flow today.
  get_rigid_connected_section_ids:
    'SectionAdjustDialog calls it (section-adjust-dialog.spec.ts) but no spec asserts on the derived connected count; a null response is swallowed by the dialog\'s own .catch()',
  export_tml:
    'export-dialog.spec.ts drives the TML export flow, but only asserts the dialog closes afterward, which happens regardless of the (missing) mock response',
  get_section_relationships:
    'ConfirmDialog calls it when deleting a section, but no seeded e2e project has merges/closures/offsets on the deleted section, so a null fallback renders identically to a real empty response',
  create_section_and_merge: 'no mocked spec drives the drag-to-merge-during-import flow',
  resolve_warning: 'no mocked spec drives the warning-resolution flow',
  unresolve_warning: 'no mocked spec drives the warning-resolution flow',
  get_recent_logs_since: 'no frontend caller (only get_recent_logs, the unbounded variant, is used)',

  // Real, wired feature with genuinely zero e2e coverage today (not a
  // silent-failure risk yet since no spec touches it, but worth a follow-up
  // issue: e2e-playwright has no spec for the comment-move tool at all).
  move_comment: 'move-comment tool + swap-comments dialog have no e2e-playwright coverage of any kind yet',
};

// ----------------------------------------------------------------------------
// Allowlist: mock `case` labels in the shared switch with no matching
// registered Rust command. Most are deliberate legacy-name fallthrough
// aliases grouped with a real command; a few are stale/dead pre-existing
// mock cases with no Rust command and no frontend caller.
// ----------------------------------------------------------------------------
const STALE_MOCK_CASE_REASONS: Record<string, string> = {
  get_project: 'fallthrough alias for get_project_state (legacy command name kept for compat)',
  get_recent_projects: 'fallthrough alias for list_recent_projects (legacy command name kept for compat)',
  get_warnings: 'fallthrough alias for get_project_warnings (legacy command name kept for compat)',
  parse_dmp: 'fallthrough alias for import_dmp (legacy command name kept for compat)',
  get_stations: 'stale pre-existing dead mock case; no matching Rust command and no frontend caller',
  get_undo_state: 'stale pre-existing dead mock case; no matching Rust command and no frontend caller',
  move_shot: 'stale pre-existing dead mock case; the move_shot tool dispatches the move_station IPC command',
};

describe('mock contract tripwire (#275)', () => {
  it('parses a non-empty registered Rust command list', () => {
    expect(parseRegisteredRustCommands(libRsSource).length).toBeGreaterThan(0);
  });

  it('parses a non-empty mocked command list', () => {
    expect(parseMockedCommands(mockTsSource).length).toBeGreaterThan(0);
  });

  it('every registered Rust command has a mock case or an allowlist reason', () => {
    const registered = parseRegisteredRustCommands(libRsSource);
    const mocked = new Set(parseMockedCommands(mockTsSource));

    const missing = registered.filter((cmd) => !mocked.has(cmd) && !(cmd in UNMOCKED_COMMAND_REASONS));

    if (missing.length > 0) {
      throw new Error(
        `${missing.length} registered #[tauri::command](s) have no mock case in e2e-playwright/mocks/tauri-ipc.ts ` +
          `and no allowlist entry: ${missing.join(', ')}. ` +
          'Add a mock case in tauri-ipc.ts or allowlist with a reason.'
      );
    }
  });

  it('every mock case corresponds to a registered command or an allowlist reason', () => {
    const registered = new Set(parseRegisteredRustCommands(libRsSource));
    const mocked = parseMockedCommands(mockTsSource);

    const orphaned = mocked.filter((cmd) => !registered.has(cmd) && !(cmd in STALE_MOCK_CASE_REASONS));

    if (orphaned.length > 0) {
      throw new Error(
        `${orphaned.length} mock case(s) in e2e-playwright/mocks/tauri-ipc.ts have no matching registered ` +
          `#[tauri::command] and no allowlist entry: ${orphaned.join(', ')}. ` +
          'Add a mock case in tauri-ipc.ts or allowlist with a reason.'
      );
    }
  });

  it('UNMOCKED_COMMAND_REASONS has no stale entries', () => {
    const registered = new Set(parseRegisteredRustCommands(libRsSource));
    const mocked = new Set(parseMockedCommands(mockTsSource));

    const stale = Object.keys(UNMOCKED_COMMAND_REASONS).filter(
      (cmd) => !registered.has(cmd) || mocked.has(cmd)
    );

    expect(stale, 'these allowlist entries no longer need an exemption; remove them').toEqual([]);
  });

  it('ignores command paths and case labels that appear only in comments', () => {
    const rustSource = [
      'fn main() { builder.invoke_handler(tauri::generate_handler![',
      '    commands::project::real_command,',
      '    // TODO: also register commands::project::phantom_from_line_comment here',
      '    /* commands::project::phantom_from_block_comment, */',
      '    commands::export::other_real_command,',
      ']); }',
    ].join('\n');
    expect(parseRegisteredRustCommands(rustSource)).toEqual(['other_real_command', 'real_command']);

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

  it('STALE_MOCK_CASE_REASONS has no stale entries', () => {
    const registered = new Set(parseRegisteredRustCommands(libRsSource));
    const mocked = new Set(parseMockedCommands(mockTsSource));

    const stale = Object.keys(STALE_MOCK_CASE_REASONS).filter(
      (cmd) => registered.has(cmd) || !mocked.has(cmd)
    );

    expect(stale, 'these allowlist entries no longer need an exemption; remove them').toEqual([]);
  });
});

it('retired SpeleoDB commands are absent from IPC and browser mocks', () => {
  for (const command of [
    'speleodb_patch_project',
    'speleodb_set_exclude_geojson',
    'speleodb_set_undo_mode',
    'speleodb_get_project_metadata',
  ]) {
    expect(libRsSource).not.toContain(`commands::speleodb::${command},`);
    expect(mockTsSource).not.toContain(`case '${command}':`);
  }
  const barrel = fs.readFileSync(path.join(REPO_ROOT, 'src/types/ipc.generated.ts'), 'utf8');
  // Also covers the retired SpeleoProjectMetadataPatch.
  expect(barrel).not.toContain('SpeleoProjectMetadata');
});
