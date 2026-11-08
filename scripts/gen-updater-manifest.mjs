// Updater manifest generator (issue #579).
//
// Builds one Tauri v2 updater manifest (a static JSON, not signed itself -
// each platform entry carries its own minisign signature) from a directory of
// release artifacts. Two tracks exist, `latest-stable.json` and
// `latest-weekly.json`; this script emits ONE manifest per invocation, so the
// release workflow picks the file name via `--out` and uploads it to the
// rolling `updater` GitHub release afterward (see docs/releasing.md).
//
// Usage:
//   node scripts/gen-updater-manifest.mjs \
//     --version 0.5.1 --tag v0.5.1 --dir release-artifacts \
//     --notes-file notes.md --out latest-stable.json [--include-darwin true]
//
// `--version` is what the updater compares against the running app's own
// version - it is the manifest version, not necessarily the tag. A weekly
// build's version carries a `-weekly.YYYYMMDD` suffix on the NEXT unconsumed
// base (the base no stable release has shipped yet), never on the last
// stable's own base - a `-weekly.` suffix is a semver PRE-release, so
// `0.5.1-weekly.20260817` sorts BELOW plain `0.5.1`, not above it. Baking it
// onto the next base keeps every weekly version strictly above the last
// stable and strictly below the next one, which is the actual invariant the
// updater's "only ever offer something newer" comparison depends on.
//
// `--tag` names the GitHub release the artifacts were uploaded to; asset URLs
// are built from it as
// `https://github.com/Stalagmight/Stalagmight/releases/download/<tag>/<file>`.
//
// Each platform's update artifact and its detached `.sig` file (the `.sig`
// file's content becomes the manifest's `signature` field, verbatim) must
// both be present in `--dir`:
//   - linux-x86_64:   *.AppImage        + *.AppImage.sig
//   - windows-x86_64: *-setup.exe       + *-setup.exe.sig  (NOT the MSI)
//   - darwin-aarch64: *.app.tar.gz      + *.app.tar.gz.sig (only when
//                                                            --include-darwin true)
//
// Deterministic key order, so a manifest diff is reviewable: version, notes,
// pub_date, platforms (in the fixed order above). `pub_date` is always "now",
// UTC, RFC 3339 - it stamps when the manifest was generated, not when the
// artifacts were built.

import * as fs from 'fs';
import * as path from 'path';
import { fileURLToPath } from 'url';

export const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

// Single-repo tool by design, not a config gap: this script only ever runs
// from this repository's own CI (the release job and the repoint dispatch
// workflow, both guarded separately - the latter on `github.repository`), so
// a hardcoded base is simpler than a flag nothing would ever set differently.
const GITHUB_RELEASE_BASE = 'https://github.com/Stalagmight/Stalagmight/releases/download';

// X.Y.Z, optionally followed by a dot/alnum suffix (`-rc1`, `-weekly.20260817`).
const VERSION_RE = /^\d+\.\d+\.\d+(-[0-9A-Za-z.]+)?$/;

// vX.Y.Z, optionally suffixed - the same shape release-tag.sh accepts.
// Flows unvalidated into a URL path segment otherwise, and this script is
// also exposed as a bare pnpm command that a caller could invoke by hand.
const TAG_RE = /^v\d+\.\d+\.\d+(-[0-9A-Za-z.]+)?$/;

// Tauri's signer writes the .sig as ONE base64 line wrapping a whole minisign
// signature file, and that base64 line is exactly what the manifest's
// `signature` field must carry (the updater decodes it, then parses
// minisign). So a valid .sig is a single base64 line that decodes to an
// "untrusted comment:" header followed by a base64 body line. Checking only
// "non-empty" let a truncated or corrupted .sig through, which ships a
// manifest that fails closed on every client that tries to verify it. The
// check once expected the decoded (plain-text) form instead, which no real
// Tauri .sig has, and so refused the v0.5.0 release's genuine signatures.
//
// The checks below mirror what the client does with the field
// (tauri-plugin-updater's strict STANDARD base64 decode, then
// minisign-verify's `Signature::decode`): canonical base64 at both levels, a
// 74-byte signature line (2-byte algorithm, 8-byte key id, 64-byte
// signature), a "trusted comment: " line, and a 64-byte global signature.
const MINISIGN_COMMENT_RE = /^untrusted comment:/;
const TRUSTED_COMMENT_PREFIX = 'trusted comment: ';
const BASE64_LINE_RE = /^[A-Za-z0-9+/]+=*$/;
const SIGNATURE_LINE_BYTES = 74;
const GLOBAL_SIGNATURE_BYTES = 64;
const SIGNATURE_ALGORITHMS = new Set(['Ed', 'ED']);

// Decodes `s` only if it is canonical base64 (what a strict decoder accepts):
// Node's decoder silently tolerates bad padding and truncation, so round-trip
// it and refuse anything that does not come back byte-for-byte.
function strictBase64(s) {
  if (!BASE64_LINE_RE.test(s)) return null;
  const bytes = Buffer.from(s, 'base64');
  return bytes.toString('base64') === s ? bytes : null;
}

// Order here is the order platforms appear in the emitted manifest.
const PLATFORMS = [
  { key: 'linux-x86_64', label: 'Linux', artifactRe: /\.AppImage$/, required: true },
  { key: 'windows-x86_64', label: 'Windows', artifactRe: /-setup\.exe$/, required: true },
  { key: 'darwin-aarch64', label: 'macOS', artifactRe: /\.app\.tar\.gz$/, required: false },
];

export function parseArgs(argv) {
  const getFlag = (name) => {
    const i = argv.indexOf(name);
    return i >= 0 ? argv[i + 1] : undefined;
  };
  const version = getFlag('--version');
  const tag = getFlag('--tag');
  const dir = getFlag('--dir');
  const notes = getFlag('--notes');
  const notesFile = getFlag('--notes-file');
  const out = getFlag('--out');
  const includeDarwinRaw = getFlag('--include-darwin') ?? 'false';

  const missing = [];
  if (!version) missing.push('--version');
  if (!tag) missing.push('--tag');
  if (!dir) missing.push('--dir');
  if (!out) missing.push('--out');
  if (missing.length > 0) {
    throw new Error(`missing required flag(s): ${missing.join(', ')}`);
  }
  if (notes !== undefined && notesFile !== undefined) {
    throw new Error('--notes and --notes-file are mutually exclusive');
  }
  if (includeDarwinRaw !== 'true' && includeDarwinRaw !== 'false') {
    throw new Error(`--include-darwin must be "true" or "false", got "${includeDarwinRaw}"`);
  }
  return { version, tag, dir, notes, notesFile, out, includeDarwin: includeDarwinRaw === 'true' };
}

function findOne(dir, re, label) {
  const entries = fs.readdirSync(dir).filter((f) => re.test(f));
  if (entries.length > 1) {
    throw new Error(`multiple ${label} update artifacts matched in ${dir}: ${entries.join(', ')}`);
  }
  return entries[0];
}

function readSig(dir, artifactFile, label) {
  const sigFile = `${artifactFile}.sig`;
  const sigPath = path.join(dir, sigFile);
  if (!fs.existsSync(sigPath)) {
    throw new Error(`missing ${label} update signature: ${sigFile} (expected alongside ${artifactFile} in ${dir})`);
  }
  const sig = fs.readFileSync(sigPath, 'utf8').trim();
  if (sig.length === 0) {
    throw new Error(`${label} update signature is empty: ${sigPath}`);
  }
  const outer = strictBase64(sig);
  if (outer === null) {
    throw new Error(
      `${label} update signature is not a single base64 line (Tauri's .sig format): ${sigPath}`,
    );
  }
  const notMinisign = (why) =>
    new Error(`${label} update signature does not decode to a minisign signature (${why}): ${sigPath}`);
  const lines = outer.toString('utf8').split('\n');
  if (!MINISIGN_COMMENT_RE.test(lines[0] ?? '')) {
    throw notMinisign('missing "untrusted comment:" header');
  }
  const signature = strictBase64((lines[1] ?? '').trim());
  if (signature === null) {
    throw notMinisign('second line is not base64');
  }
  if (
    signature.length !== SIGNATURE_LINE_BYTES ||
    !SIGNATURE_ALGORITHMS.has(signature.subarray(0, 2).toString('latin1'))
  ) {
    throw notMinisign(`second line is not a ${SIGNATURE_LINE_BYTES}-byte Ed25519 signature`);
  }
  if (!(lines[2] ?? '').startsWith(TRUSTED_COMMENT_PREFIX)) {
    throw notMinisign('missing "trusted comment:" line');
  }
  const global = strictBase64((lines[3] ?? '').trim());
  if (global === null || global.length !== GLOBAL_SIGNATURE_BYTES) {
    throw notMinisign(`fourth line is not a ${GLOBAL_SIGNATURE_BYTES}-byte global signature`);
  }
  return sig;
}

// Pure: builds the manifest object from already-resolved inputs. Takes real
// filesystem reads on `dir` (artifact discovery + sig content), but nothing
// else - no writes, no network. `notes` is the already-resolved string (the
// CLI layer resolves --notes vs --notes-file before calling this).
export function buildManifest({ version, tag, dir, notes, includeDarwin }) {
  if (!VERSION_RE.test(version)) {
    throw new Error(`--version "${version}" is not a valid version (expected X.Y.Z or X.Y.Z-suffix)`);
  }
  if (!TAG_RE.test(tag)) {
    throw new Error(`--tag "${tag}" is not a valid release tag (expected vX.Y.Z or vX.Y.Z-suffix)`);
  }

  const platforms = {};
  for (const plat of PLATFORMS) {
    if (plat.key === 'darwin-aarch64' && !includeDarwin) continue;
    const artifact = findOne(dir, plat.artifactRe, plat.label);
    if (!artifact) {
      throw new Error(`no ${plat.label} update artifact found in ${dir} (expected to match ${plat.artifactRe})`);
    }
    const signature = readSig(dir, artifact, plat.label);
    // encodeURIComponent leaves '.', '-', '_' and alphanumerics untouched, so
    // the underscore-separated bundler names round-trip unchanged; it only
    // bites on the characters that would actually break a URL.
    const url = `${GITHUB_RELEASE_BASE}/${tag}/${encodeURIComponent(artifact)}`;
    platforms[plat.key] = { url, signature };
  }

  return {
    version,
    notes: notes ?? '',
    pub_date: new Date().toISOString(),
    platforms,
  };
}

function resolveNotes(opts) {
  if (opts.notesFile === undefined) return opts.notes;
  try {
    return fs.readFileSync(opts.notesFile, 'utf8');
  } catch (e) {
    throw new Error(`could not read --notes-file "${opts.notesFile}": ${e.message}`);
  }
}

function main() {
  const argv = process.argv.slice(2);
  let opts;
  try {
    opts = parseArgs(argv);
  } catch (e) {
    console.error(`Error: ${e.message}`);
    console.error(
      'Usage: node scripts/gen-updater-manifest.mjs --version <semver> --tag <tag> --dir <dir> ' +
        '(--notes <string>|--notes-file <path>) --out <path> [--include-darwin <true|false>]',
    );
    process.exit(1);
  }

  let manifest;
  try {
    const notes = resolveNotes(opts);
    manifest = buildManifest({ ...opts, notes });
  } catch (e) {
    console.error(`Error: ${e.message}`);
    process.exit(1);
  }

  fs.writeFileSync(opts.out, `${JSON.stringify(manifest, null, 2)}\n`);
  console.log(
    `wrote ${opts.out}: version ${manifest.version}, ${Object.keys(manifest.platforms).length} platform(s)`,
  );
}

const isMain = process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (isMain) main();
