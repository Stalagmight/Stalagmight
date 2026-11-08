/**
 * A fixture's name as shareable output prints it (#576 W58 Q1).
 *
 * The fixtures the repo tracks directly under `e2e/fixtures/` (`git ls-files`)
 * are public, and their names print as they are. Any other fixture a run opens
 * - a `.stalx` someone dropped into `e2e/fixtures/` for a local run, or a name
 * the knob, a trace or `--fixtures` gives - is private: a survey file's name can
 * name a cave or a landowner. Shareable output prints it as a tag,
 * `<private-fixture#xxxxx>` (`crc32.shortId` of the name, as a corpus file's id
 * is), so the lines of one run still agree with each other.
 *
 * The full name stays in what is not for sharing: the session's actions.jsonl
 * (each full failure and progress line, and the progress line a scenario
 * context writes when it first opens a private fixture) and the fixture copy in
 * the session directory.
 */
import { execFileSync } from 'child_process';
import * as fs from 'fs';
import * as path from 'path';
import { fileURLToPath } from 'url';

import { shortId } from './crc32';

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..', '..');
const FIXTURE_DIR = 'e2e/fixtures';

/** Shorter names are not folded: a one- or two-letter name would fold ordinary words. */
const MIN_FOLDED_NAME = 3;

let tracked: ReadonlySet<string> | null = null;

/**
 * The names git tracks directly under e2e/fixtures/, read once. Empty when git
 * cannot say (no checkout): then every fixture is private, which is the safe
 * way round.
 */
export function trackedFixtureNames(): ReadonlySet<string> {
  if (tracked !== null) return tracked;
  let listed = '';
  try {
    // stderr discarded: git's own error can name the checkout.
    listed = execFileSync('git', ['ls-files', '-z', '--', FIXTURE_DIR], {
      cwd: REPO_ROOT,
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'ignore'],
    });
  } catch {
    listed = '';
  }
  const prefix = `${FIXTURE_DIR}/`;
  tracked = new Set(
    listed
      .split('\0')
      .filter((rel) => rel.startsWith(prefix) && !rel.slice(prefix.length).includes('/'))
      .map((rel) => rel.slice(prefix.length)),
  );
  return tracked;
}

/** Is `name` a fixture the repo tracks (a bare name directly under e2e/fixtures/)? */
export function isTrackedFixture(name: string): boolean {
  return trackedFixtureNames().has(name);
}

let trackedAll: ReadonlySet<string> | null = null;

/**
 * Is `rel` a path git tracks in the checkout (repo-relative, `/`-separated)?
 * Read once; when git cannot say, no path is tracked, the safe way round
 * (#576 W74-NF2).
 */
export function isTrackedPath(rel: string): boolean {
  if (trackedAll === null) {
    let listed = '';
    try {
      // stderr discarded: git's own error can name the checkout.
      listed = execFileSync('git', ['ls-files', '-z'], { cwd: REPO_ROOT, encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'], maxBuffer: 64 * 1024 * 1024 });
    } catch {
      listed = '';
    }
    trackedAll = new Set(listed.split('\0').filter((entry) => entry !== ''));
  }
  return trackedAll.has(rel);
}

/** The tag a private fixture prints as: no spaces, so a `KEY=<tag>` reproduce prefix keeps its shape. */
export function fixtureTag(name: string): string {
  return `<private-fixture#${shortId(name)}>`;
}

/** Names this process was given that the repo does not track, folded wherever they show up. */
const noted = new Set<string>();

/** Remember a fixture name the knob, a trace or the CLI gave, so `foldPrivateFixtures` folds it if it is private. */
export function noteFixtureName(name: string): void {
  if (name.length >= MIN_FOLDED_NAME && !isTrackedFixture(name)) noted.add(name);
}

/** A fixture name as shareable output prints it: a tracked one as itself, anything else as its tag. */
export function shareableFixtureName(name: unknown): string {
  return typeof name === 'string' && isTrackedFixture(name) ? name : fixtureTag(String(name));
}

/** Every private name to fold: the untracked files directly in e2e/fixtures/ and every name noted, longest first. */
function privateNames(): string[] {
  let present: string[] = [];
  try {
    present = fs
      .readdirSync(path.join(REPO_ROOT, FIXTURE_DIR), { withFileTypes: true })
      .filter((entry) => entry.isFile() || entry.isSymbolicLink())
      .map((entry) => entry.name);
  } catch {
    present = [];
  }
  const names = new Set([...noted, ...present.filter((name) => !isTrackedFixture(name))]);
  return [...names].filter((name) => name.length >= MIN_FOLDED_NAME).sort((a, b) => b.length - a.length);
}

/**
 * `text` with every private fixture name replaced by its tag - not inside a
 * longer name, so `a.stalx` leaves `data.stalx` alone. Read at call time, so a
 * fixture dropped in mid-run is folded too. Folding twice changes nothing.
 */
export function foldPrivateFixtures(text: string): string {
  let out = text;
  for (const name of privateNames()) {
    if (!out.includes(name)) continue;
    const literal = name.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
    out = out.replace(new RegExp(`(?<![\\w.-])${literal}(?![\\w.-])`, 'g'), fixtureTag(name));
  }
  return out;
}
