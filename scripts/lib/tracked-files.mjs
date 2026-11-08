// Shared tracked-file listing and reading for the whole-tree text guards
// (scripts/check-private-leaks.mjs, scripts/check-no-em-dash.mjs) - #576 E16.
//
// Both guards used to carry their own copy of this walk, and the copies had
// already drifted on the one decision that matters most for a fork PR: what a
// tracked SYMLINK is. The dash guard skipped links without reading anything;
// the leak guard followed them with readFileSync, so a dangling link failed
// one gate and passed its twin, and a link pointing outside the repo had the
// outside file's content scanned by one and nothing scanned by the other.
//
// The single policy now (NF-LINK): a symlink is scanned as the thing git
// actually stores and publishes for it - its TARGET STRING (a mode-120000 blob
// is exactly that text) - and it is never followed. So `AGENTS.md -> CLAUDE.md`
// is clean, a link whose target path carries a private name or a banned dash
// fails, a dangling link is just its (clean) text, and nothing outside the
// checkout is ever read.
//
// Symlink-ness comes from the index (`git ls-files -s`, mode 120000), so the
// guards know a path is a link before deciding on extension-based exclusions:
// a link called `logo.png` is still text (its target), not an image.

import * as fs from 'fs';
import * as path from 'path';

import { execFileSyncFolded } from './fold-local-paths.mjs';

// Skip (and FAIL on) a file larger than this. Bounds scan time against an
// adversarial multi-MB single-line file on a fork PR; no tracked text file
// legitimately approaches it.
export const MAX_SCAN_BYTES = 5 * 1024 * 1024;

// Genuinely-binary or survey-DATA extensions, skipped before reading. Text
// formats that can carry a leak or a dash (`.svg`, `.snap`, `.json`) are
// deliberately NOT here; the NUL-byte `looksBinary` check and MAX_SCAN_BYTES
// still protect against a real binary or a huge file. `dmp`/`tml`/`stalx` are
// DATA: a byte inside a fixture is the input under test.
export const BINARY_EXT = new Set([
  'png', 'jpg', 'jpeg', 'gif', 'ico', 'icns', 'pdf', 'webp',
  'woff', 'woff2', 'ttf', 'otf', 'eot', 'zip', 'gz', 'tgz', 'bz2', 'xz', '7z',
  'wasm', 'dmp', 'stalx', 'tml', 'db', 'sqlite', 'sqlite3', 'bin', 'keystore', 'jks',
  'p12', 'der', 'class', 'jar', 'mp4', 'mov', 'webm',
]);

export function hasBinaryExt(rel) {
  const base = rel.slice(rel.lastIndexOf('/') + 1);
  const ext = base.includes('.') ? base.slice(base.lastIndexOf('.') + 1).toLowerCase() : '';
  return BINARY_EXT.has(ext);
}

export function looksBinary(buf) {
  const n = Math.min(buf.length, 8000);
  for (let i = 0; i < n; i++) if (buf[i] === 0) return true;
  return false;
}

// Parse `git ls-files -s -z` output into [{rel, symlink}]. Exported for the
// fixture test; each record is `<mode> <object> <stage>\t<path>`.
export function parseLsFilesStage(out) {
  const entries = [];
  for (const rec of out.split('\0')) {
    if (!rec) continue;
    const tab = rec.indexOf('\t');
    if (tab < 0) continue;
    const mode = rec.slice(0, rec.indexOf(' '));
    entries.push({ rel: rec.slice(tab + 1), symlink: mode === '120000' });
  }
  // A path in a merge conflict appears once per stage; scan it once.
  const seen = new Set();
  return entries.filter((e) => (seen.has(e.rel) ? false : (seen.add(e.rel), true)));
}

// Every tracked path with its symlink flag, sorted. git's stderr reaches ours
// folded: a failure such as `detected dubious ownership in repository at
// '<path>'` names the checkout (#576 W58). A failing git still throws
// execFileSync's error (`Command failed: git ls-files -s -z`, its status).
export function listTracked(root) {
  const out = execFileSyncFolded('git', ['ls-files', '-s', '-z'], { cwd: root, maxBuffer: 64 * 1024 * 1024 });
  return parseLsFilesStage(out.toString('utf8')).sort((a, b) => (a.rel < b.rel ? -1 : a.rel > b.rel ? 1 : 0));
}

// A reader for `scan(entries, readFile)`: a regular file's bytes, or
// `{ symlink: '<target string>' }` for a link. `lstat` first, so a link is
// never dereferenced, live or dangling. Throws for anything unreadable (the
// guards fail closed on that).
export function makeReader(root) {
  return (rel) => {
    const full = path.join(root, rel);
    if (fs.lstatSync(full).isSymbolicLink()) return { symlink: fs.readlinkSync(full) };
    return fs.readFileSync(full);
  };
}

// Normalise one scan input: a plain string is a regular tracked file.
export function asEntry(e) {
  return typeof e === 'string' ? { rel: e, symlink: false } : e;
}

// Turn a reader result into scannable text, or a fail-closed skip reason.
// Returns {text, link} or {skip}.
export function toScanText(result, maxBytes = MAX_SCAN_BYTES) {
  let buf;
  let link = false;
  if (Buffer.isBuffer(result)) {
    buf = result;
  } else if (result && typeof result.symlink === 'string') {
    buf = Buffer.from(result.symlink, 'utf8');
    link = true;
  } else {
    return { skip: 'reader returned neither file bytes nor a symlink target' };
  }
  if (buf.length > maxBytes) {
    return { skip: `${(buf.length / 1048576).toFixed(1)} MB > ${maxBytes / 1048576} MB cap` };
  }
  if (looksBinary(buf)) {
    // A NUL byte in a path whose extension is NOT a known binary type is
    // undecidable (a mislabelled binary, or text hiding a marker past a NUL).
    // Fail closed; a genuine binary type belongs in BINARY_EXT.
    return { skip: 'binary-like content (NUL byte) in a non-binary extension' };
  }
  return { text: buf.toString('utf8'), link };
}
