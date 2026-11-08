// Updater pubkey/endpoint sanity gate (issue #579).
//
// `src-tauri/tauri.conf.json` commits the `plugins.updater` block (the
// minisign public key and the stable endpoint, see docs/releasing.md). The
// block's `pubkey` must be non-placeholder, valid base64 of a minisign key,
// and every `endpoints` entry must be an https `github.com` URL, so a typo or
// a placeholder pubkey goes red here rather than surfacing later as a
// client-side verification failure nobody can reproduce from the repo alone.
// A config without the block passes: the app then starts with the updater
// off (`updater_configured` in src-tauri/src/commands/app_update.rs).
//
// Usage:  node scripts/check-updater-pubkey.mjs   (or: pnpm lint:updater-pubkey)

import * as fs from 'fs';
import * as path from 'path';
import { fileURLToPath } from 'url';

export const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

const CONF = 'src-tauri/tauri.conf.json';

function isValidBase64(s) {
  if (typeof s !== 'string' || s.length === 0) return false;
  if (!/^[A-Za-z0-9+/]+={0,2}$/.test(s)) return false;
  if (s.length % 4 !== 0) return false;
  return Buffer.from(s, 'base64').length > 0;
}

// A Tauri updater pubkey is the base64 of a minisign .pub file: an
// "untrusted comment:" line followed by a base64 key line whose decoded
// form is 42 bytes - a 2-byte "Ed" algorithm tag, an 8-byte key id, and
// the 32-byte Ed25519 public key. Base64 decodability alone would accept
// any blob (e.g. "AAAAAAAA") that no .sig file could ever verify against,
// and this key ships baked into released clients - so enforce the real
// shape, not just the encoding.
export function isMinisignPubkey(s) {
  if (!isValidBase64(s)) return false;
  const text = Buffer.from(s, 'base64').toString('utf8');
  const lines = text.split(/\r?\n/).filter((l) => l.trim().length > 0);
  if (lines.length !== 2) return false;
  if (!lines[0].startsWith('untrusted comment:')) return false;
  const keyLine = lines[1].trim();
  if (!/^[A-Za-z0-9+/]+={0,2}$/.test(keyLine)) return false;
  const key = Buffer.from(keyLine, 'base64');
  return key.length === 42 && key.subarray(0, 2).toString('latin1') === 'Ed';
}

// Returns a list of problem strings; empty means either the updater plugin
// is not configured yet, or its config is sane.
export function checkUpdaterConfig(root) {
  const confPath = path.join(root, CONF);
  let conf;
  try {
    conf = JSON.parse(fs.readFileSync(confPath, 'utf8'));
  } catch (e) {
    return [`${CONF} is missing or not valid JSON: ${e.message}`];
  }

  const updater = conf.plugins?.updater;
  if (!updater) return [];

  const problems = [];
  if (!isMinisignPubkey(updater.pubkey)) {
    problems.push(
      `${CONF}: plugins.updater.pubkey is not a valid minisign public key ` +
        `(expected the base64 of a .pub file: an "untrusted comment:" line plus a 42-byte Ed25519 key)`
    );
  }

  const endpoints = Array.isArray(updater.endpoints) ? updater.endpoints : [];
  if (endpoints.length === 0) {
    problems.push(`${CONF}: plugins.updater.endpoints is empty`);
  }
  for (const ep of endpoints) {
    let url;
    try {
      url = new URL(ep);
    } catch {
      problems.push(`${CONF}: endpoint "${ep}" is not a valid URL`);
      continue;
    }
    if (url.protocol !== 'https:') {
      problems.push(`${CONF}: endpoint "${ep}" is not https`);
    }
    if (url.hostname !== 'github.com') {
      problems.push(`${CONF}: endpoint "${ep}" is not a github.com URL`);
    }
    // The release pipeline publishes manifests to exactly these two asset
    // paths on the rolling `updater` release (see docs/releasing.md); an
    // endpoint pointing anywhere else passes DNS/TLS checks yet updates
    // nobody, so pin the full path here.
    if (url.protocol === 'https:' && url.hostname === 'github.com') {
      const expected = /^\/Stalagmight\/Stalagmight\/releases\/download\/updater\/latest-(stable|weekly)\.json$/;
      if (!expected.test(url.pathname)) {
        problems.push(
          `${CONF}: endpoint "${ep}" does not point at the published manifest path ` +
            `(/Stalagmight/Stalagmight/releases/download/updater/latest-<stable|weekly>.json)`
        );
      }
    }
  }
  return problems;
}

function main() {
  const problems = checkUpdaterConfig(REPO_ROOT);
  if (problems.length === 0) {
    console.log('✓ updater pubkey/endpoint config: none configured yet, or configured and sane');
    return;
  }
  for (const p of problems) console.error(`✗ ${p}`);
  process.exit(1);
}

const isMain = process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (isMain) main();
