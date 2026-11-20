// Compare two characterisation dumps (stream K of
// docs/plans/2026-11-20_station-graph-model.md).
//
// The dumps come from the `#[ignore]`d `characterization_dump` test in
// src-tauri/crates/core/src/core/state/characterization_dump_tests.rs:
//
//   STALAGMIGHT_CHAR_OUT=<dir> cargo test -p stalagmight-core --lib \
//       characterization_dump -- --ignored --nocapture
//
// Run it on the build before the model switch and on the build after, then:
//
//   node scripts/dev/compare-characterization.mjs <old-dir> <new-dir> [options]
//
// Options:
//   --scenario <name>   only this scenario (repeatable)
//   --max <n>           changes printed per file (default 8; 0 = all)
//   --tol <x>           numbers within x of each other are equal (default 0;
//                       the dumper already rounds derived floats to 1e-9)
//   --summary           one line per file, no change list
//
// Files are matched per scenario by name with the step number stripped
// (`07-merge-south-onto-hub-stations.json` -> `merge-south-onto-hub-stations.json`),
// so a step added or removed on one side does not shift every later file.
// JSON files are compared structurally: object keys by name; arrays of
// stations, closures or other records by their identity (`station_ref`,
// `closure_id`, `id`) when every element has one, else by position. Text
// exports (.csv, .tmlu, .tml.xml, .svg) are compared line by line.
//
// Exit code: 0 when the dumps are identical, 1 when anything differs, 2 on a
// usage error.

import * as fs from 'node:fs';
import * as path from 'node:path';

function usage(message) {
  if (message) console.error(message);
  console.error(
    'usage: node scripts/dev/compare-characterization.mjs <old-dir> <new-dir> ' +
      '[--scenario <name>]... [--max <n>] [--tol <x>] [--summary]',
  );
  process.exit(2);
}

function parseArgs(argv) {
  const options = { dirs: [], scenarios: [], max: 8, tol: 0, summary: false };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === '--scenario') options.scenarios.push(argv[++i] ?? usage('--scenario needs a name'));
    else if (arg === '--max') options.max = Number(argv[++i] ?? usage('--max needs a number'));
    else if (arg === '--tol') options.tol = Number(argv[++i] ?? usage('--tol needs a number'));
    else if (arg === '--summary') options.summary = true;
    else if (arg === '--help' || arg === '-h') usage();
    else if (arg.startsWith('--')) usage(`unknown option ${arg}`);
    else options.dirs.push(arg);
  }
  if (options.dirs.length !== 2) usage('give exactly two dump directories');
  if (!Number.isFinite(options.max) || options.max < 0) usage('--max must be a number >= 0');
  if (!Number.isFinite(options.tol) || options.tol < 0) usage('--tol must be a number >= 0');
  for (const dir of options.dirs) {
    if (!fs.existsSync(dir) || !fs.statSync(dir).isDirectory()) usage(`not a directory: ${dir}`);
  }
  return options;
}

// --- Structural JSON diff ---------------------------------------------------

function isObject(v) {
  return v !== null && typeof v === 'object' && !Array.isArray(v);
}

/** The identity of a record in an array, or null when it has none. */
function identityOf(item) {
  if (!isObject(item)) return null;
  const ref = item.station_ref;
  if (isObject(ref) && 'line_id' in ref && 'station_index' in ref) {
    return `slot ${ref.line_id}:${ref.station_index}`;
  }
  if (typeof item.closure_id === 'number') return `closure ${item.closure_id}`;
  if (typeof item.id === 'number' || typeof item.id === 'string') return `id ${item.id}`;
  return null;
}

/** Keyed form of an array when every element has a unique identity. */
function keyed(items) {
  const map = new Map();
  for (const item of items) {
    const id = identityOf(item);
    if (id === null || map.has(id)) return null;
    map.set(id, item);
  }
  return map;
}

function short(value) {
  const text = JSON.stringify(value);
  if (text === undefined) return 'undefined';
  return text.length > 120 ? `${text.slice(0, 117)}...` : text;
}

function equalNumbers(a, b, tol) {
  return a === b || (tol > 0 && Math.abs(a - b) <= tol);
}

/** Push one `{ path, kind, old, new }` per difference between `a` and `b`. */
function diffJson(a, b, at, tol, out) {
  if (typeof a === 'number' && typeof b === 'number') {
    if (!equalNumbers(a, b, tol)) out.push({ path: at, kind: 'changed', old: a, new: b });
    return;
  }
  if (Array.isArray(a) && Array.isArray(b)) {
    const ka = keyed(a);
    const kb = keyed(b);
    if (ka && kb && (a.length > 0 || b.length > 0)) {
      for (const [id, item] of ka) {
        if (kb.has(id)) diffJson(item, kb.get(id), `${at}[${id}]`, tol, out);
        else out.push({ path: `${at}[${id}]`, kind: 'removed', old: item });
      }
      for (const [id, item] of kb) {
        if (!ka.has(id)) out.push({ path: `${at}[${id}]`, kind: 'added', new: item });
      }
      return;
    }
    const n = Math.min(a.length, b.length);
    for (let i = 0; i < n; i++) diffJson(a[i], b[i], `${at}[${i}]`, tol, out);
    for (let i = n; i < a.length; i++) out.push({ path: `${at}[${i}]`, kind: 'removed', old: a[i] });
    for (let i = n; i < b.length; i++) out.push({ path: `${at}[${i}]`, kind: 'added', new: b[i] });
    return;
  }
  if (isObject(a) && isObject(b)) {
    const keys = new Set([...Object.keys(a), ...Object.keys(b)]);
    for (const key of [...keys].sort()) {
      const sub = at ? `${at}.${key}` : key;
      if (!(key in b)) out.push({ path: sub, kind: 'removed', old: a[key] });
      else if (!(key in a)) out.push({ path: sub, kind: 'added', new: b[key] });
      else diffJson(a[key], b[key], sub, tol, out);
    }
    return;
  }
  if (JSON.stringify(a) !== JSON.stringify(b)) out.push({ path: at, kind: 'changed', old: a, new: b });
}

function describeJsonChange(change) {
  const where = change.path || '(root)';
  if (change.kind === 'added') return `+ ${where}: ${short(change.new)}`;
  if (change.kind === 'removed') return `- ${where}: ${short(change.old)}`;
  return `~ ${where}: ${short(change.old)} -> ${short(change.new)}`;
}

// --- Line diff for text exports ---------------------------------------------

/** Longest common subsequence table, for files small enough to afford it. */
function lineDiff(a, b) {
  const n = a.length;
  const m = b.length;
  if (n * m > 25_000_000) {
    // Too big for the table: compare by position.
    const out = [];
    for (let i = 0; i < Math.max(n, m); i++) {
      if (a[i] !== b[i]) {
        if (i < n) out.push({ kind: 'removed', line: i + 1, text: a[i] });
        if (i < m) out.push({ kind: 'added', line: i + 1, text: b[i] });
      }
    }
    return out;
  }
  const width = m + 1;
  const table = new Uint32Array((n + 1) * width);
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      table[i * width + j] =
        a[i] === b[j]
          ? table[(i + 1) * width + j + 1] + 1
          : Math.max(table[(i + 1) * width + j], table[i * width + j + 1]);
    }
  }
  const out = [];
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (a[i] === b[j]) {
      i++;
      j++;
    } else if (table[(i + 1) * width + j] >= table[i * width + j + 1]) {
      out.push({ kind: 'removed', line: i + 1, text: a[i] });
      i++;
    } else {
      out.push({ kind: 'added', line: j + 1, text: b[j] });
      j++;
    }
  }
  for (; i < n; i++) out.push({ kind: 'removed', line: i + 1, text: a[i] });
  for (; j < m; j++) out.push({ kind: 'added', line: j + 1, text: b[j] });
  return out;
}

function describeLineChange(change) {
  const sign = change.kind === 'added' ? '+' : '-';
  const text = change.text.length > 140 ? `${change.text.slice(0, 137)}...` : change.text;
  return `${sign} line ${change.line}: ${text}`;
}

// --- Files --------------------------------------------------------------------

function listFiles(dir) {
  return fs
    .readdirSync(dir, { withFileTypes: true })
    .filter((e) => e.isFile())
    .map((e) => e.name)
    .sort();
}

function listScenarios(dir) {
  return fs
    .readdirSync(dir, { withFileTypes: true })
    .filter((e) => e.isDirectory())
    .map((e) => e.name)
    .sort();
}

/** Map step-number-free keys to file names (`#2` for a repeated key). */
function stepKeys(files) {
  const map = new Map();
  const seen = new Map();
  for (const file of files) {
    const base = file.replace(/^\d+-/, '');
    const n = (seen.get(base) ?? 0) + 1;
    seen.set(base, n);
    map.set(n === 1 ? base : `${base}#${n}`, file);
  }
  return map;
}

function compareFile(oldPath, newPath, options) {
  const a = fs.readFileSync(oldPath, 'utf8');
  const b = fs.readFileSync(newPath, 'utf8');
  if (a === b) return { changes: [], lines: [] };
  if (oldPath.endsWith('.json')) {
    let ja;
    let jb;
    try {
      ja = JSON.parse(a);
      jb = JSON.parse(b);
    } catch (e) {
      return { changes: [], lines: lineDiff(a.split('\n'), b.split('\n')), note: `not JSON (${e.message})` };
    }
    const changes = [];
    diffJson(ja, jb, '', options.tol, changes);
    return { changes, lines: [] };
  }
  return { changes: [], lines: lineDiff(a.split('\n'), b.split('\n')) };
}

function main() {
  const options = parseArgs(process.argv.slice(2));
  const [oldDir, newDir] = options.dirs;
  const report = [];
  let differing = 0;
  let identical = 0;

  // Top-level files (manifest, mismatches).
  const top = [...new Set([...listFiles(oldDir), ...listFiles(newDir)])].sort();
  const topLines = [];
  for (const file of top) {
    const inOld = fs.existsSync(path.join(oldDir, file));
    const inNew = fs.existsSync(path.join(newDir, file));
    if (!inOld || !inNew) {
      topLines.push(`  ${file}: only in ${inOld ? 'old' : 'new'}`);
      differing++;
      continue;
    }
    const result = compareFile(path.join(oldDir, file), path.join(newDir, file), options);
    const count = result.changes.length + result.lines.length;
    if (count === 0) {
      identical++;
      continue;
    }
    differing++;
    topLines.push(`  ${file}: ${count} change(s)`);
    if (!options.summary) {
      const list = [...result.changes.map(describeJsonChange), ...result.lines.map(describeLineChange)];
      const shown = options.max === 0 ? list : list.slice(0, options.max);
      for (const line of shown) topLines.push(`      ${line}`);
      if (shown.length < list.length) topLines.push(`      ... ${list.length - shown.length} more`);
    }
  }
  if (topLines.length > 0) report.push('(top level)', ...topLines);

  let scenarios = [...new Set([...listScenarios(oldDir), ...listScenarios(newDir)])].sort();
  if (options.scenarios.length > 0) scenarios = scenarios.filter((s) => options.scenarios.includes(s));
  const perScenario = [];

  for (const scenario of scenarios) {
    const oldScenario = path.join(oldDir, scenario);
    const newScenario = path.join(newDir, scenario);
    const hasOld = fs.existsSync(oldScenario);
    const hasNew = fs.existsSync(newScenario);
    if (!hasOld || !hasNew) {
      report.push(`${scenario}: only in ${hasOld ? 'old' : 'new'}`);
      perScenario.push({ scenario, differing: 1, identical: 0 });
      differing++;
      continue;
    }
    const oldKeys = stepKeys(listFiles(oldScenario));
    const newKeys = stepKeys(listFiles(newScenario));
    const keys = [...new Set([...oldKeys.keys(), ...newKeys.keys()])];
    // Keep the old dump's step order; files only the new one has go last.
    const order = (k) => oldKeys.get(k) ?? `~${newKeys.get(k)}`;
    keys.sort((x, y) => order(x).localeCompare(order(y)));
    const lines = [];
    let scenarioDiffering = 0;
    let scenarioIdentical = 0;
    for (const key of keys) {
      const oldFile = oldKeys.get(key);
      const newFile = newKeys.get(key);
      if (!oldFile || !newFile) {
        lines.push(`  ${oldFile ?? newFile}: only in ${oldFile ? 'old' : 'new'}`);
        scenarioDiffering++;
        continue;
      }
      const result = compareFile(path.join(oldScenario, oldFile), path.join(newScenario, newFile), options);
      const count = result.changes.length + result.lines.length;
      const label = oldFile === newFile ? oldFile : `${oldFile} -> ${newFile}`;
      if (count === 0) {
        scenarioIdentical++;
        continue;
      }
      scenarioDiffering++;
      const kind = result.lines.length > 0 ? 'line' : 'value';
      lines.push(`  ${label}: ${count} ${kind} change(s)${result.note ? ` (${result.note})` : ''}`);
      if (!options.summary) {
        const list = [...result.changes.map(describeJsonChange), ...result.lines.map(describeLineChange)];
        const shown = options.max === 0 ? list : list.slice(0, options.max);
        for (const line of shown) lines.push(`      ${line}`);
        if (shown.length < list.length) lines.push(`      ... ${list.length - shown.length} more`);
      }
    }
    perScenario.push({ scenario, differing: scenarioDiffering, identical: scenarioIdentical });
    differing += scenarioDiffering;
    identical += scenarioIdentical;
    if (scenarioDiffering > 0) {
      report.push(`${scenario}: ${scenarioDiffering} file(s) differ, ${scenarioIdentical} identical`, ...lines);
    }
  }

  for (const line of report) console.log(line);
  console.log('');
  console.log('summary:');
  for (const s of perScenario) {
    console.log(`  ${s.scenario.padEnd(28)} ${s.differing === 0 ? 'identical' : `${s.differing} differ`} (${s.identical} identical)`);
  }
  console.log(`  total: ${differing} differing, ${identical} identical`);
  process.exit(differing === 0 ? 0 : 1);
}

main();
