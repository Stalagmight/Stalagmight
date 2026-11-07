/**
 * Optional integration with a PRIVATE corpus of real survey files.
 *
 * Generated files reach the parsers' edges; real files reach the shapes nobody
 * would think to generate - a `.mak` whose sibling `.dat` files are named in a
 * way one importer tolerates, a 57 MB `Data.xml`, a decade of drift in what a
 * device actually writes. A contributor who HAS such files can point the fuzzer
 * at them; everyone else gets an empty inventory and a run that is exactly as
 * valid, just narrower.
 *
 * ## Nothing here is ever published
 *
 * The repo forbids committing survey fixtures outright (CLAUDE.md, "Never commit
 * or extract fixtures"), and the reason is that a filename alone can identify a
 * cave, a landowner or an unpublished survey project. So:
 *
 *   - the corpus root is read from the environment, at CALL time, with NO
 *     default path - nothing in the tracked tree names a location;
 *   - a discovered file is identified downstream by `corpusFileId`, a 5-hex-char
 *     fold of its RELATIVE path, and that id is what a failure payload, a metric
 *     key, a log line and every rendered trace line carry. The real name never
 *     leaves this module - `corpusEntryFor` takes the id back the other way, so
 *     a replay never needs it either;
 *   - the app is only ever pointed at a COPY, so an import that migrates or
 *     rewrites a file cannot touch the original.
 *
 * ## Two identities, and why the short one is not enough on its own
 *
 * `corpusFileId` is 20 bits. That is ample for TELLING TWO FILES APART IN ONE
 * RUN's output, which is what it was built for, and far too little to RESOLVE
 * one: discovery permits 20000 entries, and by the birthday bound a corpus of a
 * few thousand files is more likely than not to hold a colliding pair. A replay
 * that looked an id up and took the first match would then materialize somebody
 * else's file and go on to open it under the recorded file's name, which is a
 * silently wrong replay - the one outcome worse than a skipped one.
 *
 * So an entry carries BOTH:
 *
 *   - `id`   - the 5-hex crc20. The ONLY identifier that appears in anything
 *              publishable: log lines, metric keys, failure payloads, issue
 *              text, `describeSteps` output. This is the privacy rule and it
 *              does not move.
 *   - `pathHash` - 16 hex characters of SHA-256 over the same relative path.
 *              A LOOKUP key, never rendered. A trace step may carry it (see
 *              `fuzzTrace.ts`'s `corpusFile` step) so a replay resolves the file
 *              it recorded rather than one that folds to the same 20 bits.
 *
 * Discovery also COUNTS crc20 collisions and reports the colliding ids (never
 * the paths), so a run whose metric keys are ambiguous says so out loud, and a
 * bare-id lookup that matches more than one entry resolves to NOTHING rather
 * than to a guess.
 *
 * ## Read at call time, never frozen
 *
 * `STALAGMIGHT_FUZZ_CORPUS_DIR` is read inside every function, never captured in
 * a module-level const. Same rule as `streamV2()` in
 * `scenarios/undoRedoFuzzActions.ts`: a replay restores the recording's
 * environment from inside its own `main()`, which runs long after this module
 * was imported, and a frozen const would silently replay against the wrong
 * corpus (or against none).
 */
import * as crypto from 'crypto';
import * as fs from 'fs';
import * as path from 'path';

import { shortId } from './crc32';

/** The env knob. Unset means "no corpus", which is a supported configuration. */
export const CORPUS_DIR_ENV = 'STALAGMIGHT_FUZZ_CORPUS_DIR';

/** Extensions the app has an importer for, mapped to the format name. */
const FORMAT_BY_EXTENSION: Readonly<Record<string, CorpusFormat>> = {
  '.dmp': 'dmp',
  '.tml': 'tml',
  '.tmlu': 'tmlu',
  '.dat': 'dat',
  '.mak': 'mak',
  '.srv': 'srv',
  '.wpj': 'wpj',
  '.gpx': 'gpx',
};

export type CorpusFormat = 'dmp' | 'tml' | 'tmlu' | 'dat' | 'mak' | 'srv' | 'wpj' | 'gpx';

export interface CorpusEntry {
  /** POSIX-style path relative to the corpus root. Never logged, never recorded; see `id`. */
  relPath: string;
  format: CorpusFormat;
  /** The publishable stand-in for `relPath`. Short, and therefore collidable. */
  id: string;
  /** The RESOLUTION key: 16 hex of SHA-256 over `relPath`. Never rendered. */
  pathHash: string;
  /** Size in bytes, so a caller can skip what it does not want to open. */
  bytes: number;
}

export interface CorpusInventory {
  /**
   * Are there entries to draw from? The question every CALLER asks.
   *
   * Not the same question as `configured`, and conflating the two is what hid
   * the bound this module counts: a corpus whose every file sits below
   * `MAX_WALK_DEPTH` discovers zero entries, so `available` is false and the
   * run reported "corpus: none (unset or empty)" - the one message that is
   * wrong in exactly the case `tooDeep` exists to announce. A reader told the
   * knob was unset never looks at the depth bound.
   */
  available: boolean;
  /**
   * Did the knob name a directory this process could walk?
   *
   * True the moment discovery got somewhere to look, whatever it found there.
   * `configured && !available` is the interesting state - it is "your corpus is
   * set and yielded nothing", and it is the one that has to print WHY.
   */
  configured: boolean;
  /** Absolute corpus root, or null when the knob is unset or unusable. */
  root: string | null;
  entries: CorpusEntry[];
  /** `entries` grouped by format, in discovery order. */
  byFormat: Record<CorpusFormat, CorpusEntry[]>;
  /**
   * Directories the walk could not read, plus whatever it dropped on reaching
   * `MAX_WALK_ENTRIES`. Depth is NOT counted here - see `tooDeep`.
   */
  skipped: number;
  /**
   * Directories not descended into because they sat deeper than
   * `MAX_WALK_DEPTH` levels below the corpus root.
   *
   * DIRECTORIES, not the files inside them: counting those would mean walking
   * them, which is the work the bound exists to refuse. So this is a count of
   * doors not opened, and any non-zero value means the inventory is a lower
   * bound on what the corpus holds.
   *
   * It exists because the alternative to a number is an ABSENCE, and an absence
   * is unreadable: at a walk depth of 8 this integration reported 352 files with
   * total confidence, and nothing in that line distinguished "this is all there
   * is" from "this is all I looked at" - 44 more sat at levels 9 to 11, among
   * them a Walls project whose `.PATH` directives name a dozen surveys. A run
   * that sees less of the corpus than it could is a run whose coverage claim is
   * wrong by an unknown amount. Printed by `describeCorpus` whenever it is
   * non-zero, so the next corpus that outgrows the bound says so on the first
   * line of output rather than in a file count nobody can check.
   */
  tooDeep: number;
  /**
   * Every `id` that more than one entry folds to, sorted.
   *
   * IDS ONLY - listing the paths would be the leak this whole module exists to
   * prevent, and the ids are what a reader sees in the metric keys anyway. An
   * empty array is the normal case; a non-empty one is printed by
   * `describeCorpus` on every run that has it.
   */
  idCollisions: string[];
}

/**
 * Walk bounds. A corpus is somebody's data directory, not a curated tree: it can
 * contain a node_modules, a photo archive or a mounted share, and discovery runs
 * at the start of every fuzz session. So the walk is bounded in two independent
 * ways - how deep it goes, and how much it looks at in total - and it does not
 * follow symlinks, which is also what makes it terminate without a visited set.
 *
 * `MAX_WALK_DEPTH` IS DELIBERATELY THE SAME NUMBER AS `MAX_MATERIALIZED_DEPTH`,
 * and the two are pinned equal by a test. Discovery and materialization are the
 * two halves of one operation - see a file, then copy its tree - so a walk
 * shallower than the copy hides files the copy would have taken happily, and a
 * walk deeper than the copy draws entries every materialization then refuses.
 * Neither half is the interesting one on its own; the pair is.
 *
 * It was 8, and the shallow half is exactly what that cost: measured on the
 * corpus this integration was written for, 8 levels found 352 importable files
 * and 16 finds 396 - the 44 in between including three more Walls project files,
 * one of them 12 `.PATH` directives over a 14-file tree. Nothing reported the
 * gap, because a file that is never discovered leaves no trace anywhere, which
 * is why the depth bound now COUNTS what it refuses (`CorpusInventory.tooDeep`)
 * instead of folding it into the general skip counter. 16 leaves that corpus
 * with five levels of headroom and the counter to say when that stops being
 * true; the entry cap, not the depth, is what bounds the walk's total work.
 *
 * The two depths are measured from different origins and that asymmetry is the
 * safe one: this walk counts levels below the CORPUS ROOT, while `measureTree`
 * counts them below the entry's OWN directory, so every file this walk can see
 * is at depth 0 of its own materialization and can never be refused for being
 * the deep thing itself. (A companion further below it still can be, which is
 * what the `depth` skip reason reports.)
 */
export const MAX_WALK_DEPTH = 16;
const MAX_WALK_ENTRIES = 20000;

/** The corpus root as configured RIGHT NOW, or null. */
export function corpusDir(): string | null {
  const raw = process.env[CORPUS_DIR_ENV];
  if (raw === undefined || raw.trim() === '') return null;
  return path.resolve(raw.trim());
}

/** Is there a usable corpus configured? Never throws - absence is not an error. */
export function corpusAvailable(): boolean {
  const root = corpusDir();
  if (root === null) return false;
  try {
    return fs.statSync(root).isDirectory();
  } catch {
    return false;
  }
}

/**
 * The publishable identity of a corpus file: CRC-32 of its relative path, folded
 * to the low 20 bits and rendered as 5 hex characters.
 *
 * This exists because the repo's private-fixture rules make the real name
 * unpublishable - a survey filename can name a cave system, a landowner or an
 * unreleased project, and a failure payload, a metric key and a shrunk trace are
 * all things that get attached to an issue.
 *
 * The fold itself lives in `crc32.ts` as `shortId`, because `fuzzTrace.ts` needs
 * the same one: it folds a corpus reference at record time and prints the result
 * for a `corpusFile` step. A trace line naming a different id from the metric
 * key beside it would be worse than having no id at all.
 */
export function corpusFileId(relPath: string): string {
  return shortId(relPath);
}

/**
 * The RESOLUTION identity of a corpus file: 16 hex characters of SHA-256 over
 * its relative path.
 *
 * 64 bits, so a 20000-entry corpus collides with probability around 1e-11 -
 * which is the property `corpusFileId` cannot have and a replay needs. It is
 * one-way in the same sense the crc is (and rather more so), but that is not
 * why it is allowed anywhere: it is allowed because it is never RENDERED. It
 * lives in the `corpusFile` trace step as a lookup key and nowhere else, while
 * every line a human or an issue ever sees still carries `id`.
 *
 * SHA-256 rather than the full CRC-32 the id is folded from: 32 bits is a ~5%
 * collision chance over the same 20000 entries, which is not a resolution key
 * either.
 */
export function corpusPathHash(relPath: string): string {
  return crypto.createHash('sha256').update(relPath, 'utf8').digest('hex').slice(0, 16);
}

function emptyByFormat(): Record<CorpusFormat, CorpusEntry[]> {
  return { dmp: [], tml: [], tmlu: [], dat: [], mak: [], srv: [], wpj: [], gpx: [] };
}

/**
 * Every importable file under the corpus root.
 *
 * Returns an empty inventory when nothing is configured, when the path does not
 * exist, and when it cannot be read: a corpus is optional, so its absence must
 * never be the reason a fuzz run fails.
 */
export function discoverCorpus(): CorpusInventory {
  const root = corpusDir();
  const empty: CorpusInventory = {
    available: false,
    configured: false,
    root: null,
    entries: [],
    byFormat: emptyByFormat(),
    skipped: 0,
    tooDeep: 0,
    idCollisions: [],
  };
  if (root === null || !corpusAvailable()) return empty;

  const entries: CorpusEntry[] = [];
  const byFormat = emptyByFormat();
  let skipped = 0;
  let tooDeep = 0;
  let visited = 0;

  const walk = (dir: string, depth: number): void => {
    // Two bounds, counted apart. Depth is the one a growing corpus hits by
    // simply being organised one folder deeper, and the one whose fix is a
    // number in this file - so it gets its own counter and its own line of
    // output rather than being one more anonymous skip.
    if (depth > MAX_WALK_DEPTH) {
      tooDeep++;
      return;
    }
    if (visited >= MAX_WALK_ENTRIES) {
      skipped++;
      return;
    }
    let listing: fs.Dirent[];
    try {
      listing = fs.readdirSync(dir, { withFileTypes: true });
    } catch {
      skipped++;
      return;
    }
    // Sorted, so the inventory (and therefore every `ctx.pick` over it) is the
    // same on every filesystem rather than depending on readdir order.
    for (const item of [...listing].sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0))) {
      if (visited >= MAX_WALK_ENTRIES) return;
      visited++;
      const absolute = path.join(dir, item.name);
      // Symlinks are not followed: a corpus is somebody's data directory and a
      // link out of it would put the walk somewhere they did not point us.
      if (item.isSymbolicLink()) continue;
      if (item.isDirectory()) {
        walk(absolute, depth + 1);
        continue;
      }
      if (!item.isFile()) continue;
      const format = FORMAT_BY_EXTENSION[path.extname(item.name).toLowerCase()];
      if (format === undefined) continue;
      let bytes = 0;
      try {
        bytes = fs.statSync(absolute).size;
      } catch {
        skipped++;
        continue;
      }
      const relPath = path.relative(root, absolute).split(path.sep).join('/');
      const entry: CorpusEntry = {
        relPath,
        format,
        id: corpusFileId(relPath),
        pathHash: corpusPathHash(relPath),
        bytes,
      };
      entries.push(entry);
      byFormat[format].push(entry);
    }
  };

  walk(root, 0);

  // Collisions are DETECTED, not assumed away. The old comment on `shortId`
  // said they were "possible and harmless"; they are harmless for telling two
  // files apart in one run's output and not harmless for resolving one, which
  // is what a replay does with the id in a trace.
  const byId = new Map<string, number>();
  for (const entry of entries) byId.set(entry.id, (byId.get(entry.id) ?? 0) + 1);
  const idCollisions = [...byId.entries()]
    .filter(([, count]) => count > 1)
    .map(([id]) => id)
    .sort();

  return {
    available: entries.length > 0,
    configured: true,
    root,
    entries,
    byFormat,
    skipped,
    tooDeep,
    idCollisions,
  };
}

/** What a lookup did. `ambiguous` is a crc20 collision and is never a guess. */
export type CorpusLookupOutcome = 'found' | 'missing' | 'ambiguous';

export interface CorpusLookup {
  outcome: CorpusLookupOutcome;
  entry: CorpusEntry | null;
  /** How many entries matched. 0 for a miss, >1 for an ambiguous id. */
  matches: number;
}

const HASH_RE = /^[0-9a-f]{16}$/;
const ID_RE = /^[0-9a-f]{5}$/;

/**
 * Resolve one entry from a relative path, a `pathHash`, or a bare `id`.
 *
 * Three key spaces, one function, because the callers hold different halves. A
 * LIVE run drew the entry from the inventory and holds its path. A REPLAY holds
 * only what a trace is allowed to carry: since schema v4 that is the `pathHash`
 * plus the publishable `id`, and it resolves against whatever corpus the
 * REPLAYER configured. The spaces cannot be confused - a hash is 16 bare hex
 * characters, an id is 5, and every relative path this module yields ends in one
 * of the importable extensions above, so none can be read as either.
 *
 * THE `ambiguous` OUTCOME IS THE POINT. A bare id is 20 bits, and a corpus of a
 * few thousand files probably holds a colliding pair; `find`-ing the first match
 * would materialize the wrong private file and then open it under the recorded
 * file's name. So a bare id that matches more than one entry resolves to
 * NOTHING, and the caller counts it - a narrower replay, never a wrong one.
 * A `pathHash` does not have that problem and is what a v4 trace prefers.
 *
 * A miss is not an error either. A trace recorded against somebody else's
 * corpus - or on a machine that has one at all - is replayed by skipping those
 * moves and counting them.
 */
export function corpusLookup(reference: string): CorpusLookup {
  const entries = discoverCorpus().entries;
  const key: (entry: CorpusEntry) => string = HASH_RE.test(reference)
    ? (entry) => entry.pathHash
    : ID_RE.test(reference)
      ? (entry) => entry.id
      : (entry) => entry.relPath;
  const matches = entries.filter((entry) => key(entry) === reference);
  if (matches.length === 1) return { outcome: 'found', entry: matches[0]!, matches: 1 };
  if (matches.length === 0) return { outcome: 'missing', entry: null, matches: 0 };
  return { outcome: 'ambiguous', entry: null, matches: matches.length };
}

/**
 * `corpusLookup` reduced to "the entry, or nothing".
 *
 * Kept for the callers that treat every non-resolution the same way. Anything
 * that wants to tell an ambiguous id apart from an absent one - which is worth
 * a distinct metric, because the two say different things about the corpus -
 * should call `corpusLookup` directly.
 */
export function corpusEntryFor(reference: string): CorpusEntry | null {
  return corpusLookup(reference).entry;
}

/**
 * Bounds on a single materialization.
 *
 * The unit is a DIRECTORY TREE, not a file (see below), so the bounds are on the
 * tree: one folder holding a decade of dive logs is not something to copy once
 * per fuzz iteration. Measured against the real corpus this was built for, the
 * largest Walls project tree is 275 files / 28 MB / 9 levels deep, so all three
 * bounds clear it with room and none of them is tuned to it.
 *
 * Exceeding any bound copies NOTHING and says which bound it was. A partial copy
 * would be the worst outcome available: the import would fail on a missing
 * companion and the run would record that as the app's answer to a real project.
 *
 * `MAX_MATERIALIZED_DEPTH` and `MAX_WALK_DEPTH` are the same number on purpose,
 * and a test pins them equal; the reasoning lives on `MAX_WALK_DEPTH`, because
 * the failure the pairing prevents is a discovery-side one.
 */
export const MAX_MATERIALIZED_BYTES = 64 * 1024 * 1024;
export const MAX_MATERIALIZED_FILES = 4000;
export const MAX_MATERIALIZED_DEPTH = 16;

/** Which bound stopped a materialization, or what went wrong instead. */
export type MaterializeSkip = 'bytes' | 'files' | 'depth' | 'io';

export interface MaterializedCorpusEntry {
  /** Path of the entry file inside `destDir`. Hand THIS to the app. Empty on a skip. */
  path: string;
  /** Files copied alongside it, at any depth. */
  copied: number;
  bytes: number;
  /** True when a bound was exceeded or a copy failed, and NOTHING was copied. */
  skipped: boolean;
  /** Why. Null when `skipped` is false. */
  reason: MaterializeSkip | null;
  /**
   * Every distinct path SEGMENT the copy created under `destDir` - file
   * basenames and directory names alike, at any depth.
   *
   * The caller needs it for redaction: `importDialogFuzz.ts` rewrites each of
   * these out of a failure payload, and a nested companion's directory name can
   * identify a cave system exactly as well as a file's can. Returning the list
   * is what keeps that caller from having to re-walk the tree it just wrote.
   */
  names: string[];
}

interface TreeMeasurement {
  /** POSIX-relative paths under the source directory, sorted. */
  files: { rel: string; size: number }[];
  bytes: number;
  skip: MaterializeSkip | null;
}

/**
 * Measure the tree before copying any of it, so a bound is a decision rather
 * than a place the copy stopped.
 *
 * Symlinks are skipped at every level, files and directories alike, for the same
 * reason the discovery walk does not follow them: a link can point outside the
 * directory the caller meant to copy. That is also what makes the recursion
 * terminate without a visited set.
 */
function measureTree(sourceDir: string): TreeMeasurement {
  const files: { rel: string; size: number }[] = [];
  let bytes = 0;
  let skip: MaterializeSkip | null = null;

  const walk = (dir: string, prefix: string, depth: number): void => {
    if (skip !== null) return;
    if (depth > MAX_MATERIALIZED_DEPTH) {
      skip = 'depth';
      return;
    }
    let listing: fs.Dirent[];
    try {
      listing = fs.readdirSync(dir, { withFileTypes: true });
    } catch {
      skip = 'io';
      return;
    }
    for (const item of [...listing].sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0))) {
      if (skip !== null) return;
      if (item.isSymbolicLink()) continue;
      const absolute = path.join(dir, item.name);
      const rel = prefix === '' ? item.name : `${prefix}/${item.name}`;
      if (item.isDirectory()) {
        walk(absolute, rel, depth + 1);
        continue;
      }
      if (!item.isFile()) continue;
      let size = 0;
      try {
        size = fs.statSync(absolute).size;
      } catch {
        skip = 'io';
        return;
      }
      files.push({ rel, size });
      bytes += size;
      if (files.length > MAX_MATERIALIZED_FILES) skip = 'files';
      else if (bytes > MAX_MATERIALIZED_BYTES) skip = 'bytes';
    }
  };

  walk(sourceDir, '', 0);
  return { files, bytes, skip };
}

/**
 * Copy an entry's CONTAINING DIRECTORY - the whole tree below it, structure
 * preserved - into `destDir`, and return the copied entry's path.
 *
 * The directory rather than the file, because two of these formats are not one
 * file: a Compass `.mak` names `.dat` survey files by relative path, and a Walls
 * `.wpj` names a tree of `.srv`. Copying the `.mak` alone produces an import
 * failure that says nothing about the app - it says the harness took half a
 * project.
 *
 * AND THE WHOLE TREE, not just the immediate children, because both of those
 * relative paths can descend. Walls `.PATH` directives are exactly that: a
 * project file that names a subfolder and then lists the surveys inside it.
 * Measured on the corpus this integration was written for, the two `.wpj`
 * projects that use `.PATH` name 127 and 12 of them, ALL of them below the
 * project's own directory, and an immediate-children copy took 1 file of 275 and
 * 1 of 14. So those two corpus cases were not exercising the Walls importer at
 * all - they were exercising its missing-file handling, and reporting it as
 * coverage of a real project.
 *
 * The copy is what the app is pointed at, always. An import can migrate a file
 * in place, and a corpus is unversioned data somebody would not get back.
 *
 * EVERY file is copied, not just the importable extensions. That costs a little
 * I/O on a directory holding unrelated files, and it buys correctness on the
 * formats that reference companions this module has no catalog for - a Compass
 * project can name a `.clp` closure file, and a whitelist would silently drop it
 * and turn a real parse into a fake one. The bounds are what keep the cost
 * bounded instead.
 *
 * ALL OR NOTHING. A tree over any bound copies nothing and reports which bound;
 * a copy that fails part-way is UNDONE and reported as `io`. A half-copied
 * project is indistinguishable from a corrupt one from inside the app, and the
 * fuzzer would file the difference against the importer.
 */
export function materializeCorpusEntry(
  entry: CorpusEntry,
  destDir: string,
): MaterializedCorpusEntry {
  const root = corpusDir();
  if (root === null) throw new Error(`${CORPUS_DIR_ENV} is not set; nothing to materialize`);

  const sourceDir = path.dirname(path.join(root, entry.relPath));
  try {
    if (!fs.statSync(sourceDir).isDirectory()) {
      throw new Error('not a directory');
    }
  } catch (err) {
    // The ERRNO CODE, never the message. Node builds an fs error message out of
    // the full path it tried - `ENOENT: no such file or directory, stat
    // '/home/.../<a real cave name>/...'` - and this throw is not caught by
    // `sourceOnDisk`, so it escapes into the scenario's failure payload BEFORE
    // any redaction for this entry has been registered, and from there into the
    // printed detail and the campaign report. The id is the publishable name for
    // exactly this reason; the code says what went wrong without saying where.
    const code = (err as NodeJS.ErrnoException).code;
    throw new Error(`corpus entry ${entry.id} is not readable (${code ?? 'io'})`);
  }

  const measured = measureTree(sourceDir);
  if (measured.skip !== null) {
    return { path: '', copied: 0, bytes: measured.bytes, skipped: true, reason: measured.skip, names: [] };
  }

  // Only the entries THIS call creates are removable on an abort, so a caller
  // that pointed two materializations at one directory cannot lose the first.
  const createdTop: string[] = [];
  const names = new Set<string>();
  const existedBefore = fs.existsSync(destDir);
  fs.mkdirSync(destDir, { recursive: true });

  let copied = 0;
  let bytes = 0;
  try {
    for (const file of measured.files) {
      const segments = file.rel.split('/');
      for (const segment of segments) names.add(segment);
      if (segments.length > 1) {
        const top = segments[0]!;
        if (!createdTop.includes(top) && !fs.existsSync(path.join(destDir, top))) createdTop.push(top);
        fs.mkdirSync(path.dirname(path.join(destDir, ...segments)), { recursive: true });
      } else if (!createdTop.includes(file.rel)) {
        createdTop.push(file.rel);
      }
      fs.copyFileSync(path.join(sourceDir, ...segments), path.join(destDir, ...segments));
      copied++;
      bytes += file.size;
    }
  } catch {
    if (existedBefore) {
      for (const top of createdTop) fs.rmSync(path.join(destDir, top), { recursive: true, force: true });
    } else {
      fs.rmSync(destDir, { recursive: true, force: true });
    }
    return { path: '', copied: 0, bytes: 0, skipped: true, reason: 'io', names: [] };
  }

  return {
    path: path.join(destDir, path.basename(entry.relPath)),
    copied,
    bytes,
    skipped: false,
    reason: null,
    names: [...names].sort(),
  };
}

/**
 * One line a run can print without naming anything private.
 *
 * A crc20 collision is called out here rather than buried in a metric, because
 * it changes how the run's own output reads: two files share a metric key and a
 * log tag, and a trace recorded now can only be replayed through the `pathHash`.
 * The colliding IDS are named and the paths are not, which is the same rule as
 * everywhere else in this module.
 *
 * A depth refusal is called out for the same kind of reason and a stronger one:
 * it makes the file count on this very line a LOWER BOUND, so a reader who is
 * not told cannot interpret the number they were given.
 */
export function describeCorpus(inventory: CorpusInventory): string {
  // Gated on `configured`, NOT on `available`. A corpus that was walked and
  // yielded nothing still has to report why it yielded nothing, and the depth
  // note below is the whole reason `tooDeep` is counted separately: the case it
  // was built for - every importable file one level too deep - is precisely the
  // case that produces zero entries, so an `available` gate here made the
  // warning unreachable exactly when it was needed.
  if (!inventory.configured) return `corpus: none (${CORPUS_DIR_ENV} unset, missing or unreadable)`;
  const counts = (Object.keys(inventory.byFormat) as CorpusFormat[])
    .filter((format) => inventory.byFormat[format].length > 0)
    .map((format) => `${format}=${inventory.byFormat[format].length}`)
    .join(' ');
  const formats = counts === '' ? 'no importable format' : counts;
  const notes: string[] = [];
  if (inventory.skipped > 0) notes.push(`${inventory.skipped} dir(s) skipped`);
  if (inventory.tooDeep > 0) {
    notes.push(
      `${inventory.tooDeep} dir(s) deeper than ${MAX_WALK_DEPTH} levels NOT searched - ` +
        'this count is a lower bound',
    );
  }
  return (
    `corpus: ${inventory.entries.length} file(s) [${formats}]` +
    (notes.length > 0 ? ` (${notes.join('; ')})` : '') +
    (inventory.idCollisions.length > 0
      ? ` WARNING: ${inventory.idCollisions.length} crc20 id(s) name more than one file ` +
        `[${inventory.idCollisions.join(',')}]; those ids are ambiguous in metric keys and ` +
        `resolve to nothing on replay - a trace's pathHash is what resolves them`
      : '')
  );
}

/**
 * What printed output must never contain once an entry has been copied, and
 * what to say instead - ONE list per PROCESS.
 *
 * The copy keeps its real names on disk (see `materializeCorpusEntry`), and the
 * app echoes them back: an import that fails quotes the path it was handed, and
 * that text reaches a scenario's failure message, the error log the oracle sweep
 * reports, and whatever a runner prints. The scenario that materializes the copy
 * is the only code that knows the names, and the runners that print are not in
 * its closure - so the list lives here, beside the rule, and every printer runs
 * its text through `redactCorpusText` (#576 HFI-13). Empty, and therefore a
 * no-op, in any process that never copied a corpus entry.
 */
const REDACTED_DIRS: Redaction[] = [];
const REDACTED_NAMES: Redaction[] = [];
/**
 * A path segment this short identifies nothing, and replacing it everywhere
 * would garble every line it occurs in (a Walls `.PATH` folder named `1`, a
 * Compass folder `A2`). Counted in characters (code points of the composed
 * form), so one letter macOS writes decomposed is one.
 */
export const MIN_REDACTED_NAME = 3;
/**
 * The minimum for a name with a character outside ASCII (#576 RV19b-P4). The
 * garbling reason is ASCII's: the harness writes ASCII, so a name in another
 * script occurs only where the app's text quotes it - and two CJK characters
 * are a whole cave name.
 */
const MIN_REDACTED_NON_ASCII_NAME = 2;

/** Is `name` long enough to fold (`MIN_REDACTED_NAME`, `MIN_REDACTED_NON_ASCII_NAME`)? */
function foldsName(name: string): boolean {
  const chars = Array.from(name.normalize('NFC')).length;
  return chars >= MIN_REDACTED_NAME || (chars >= MIN_REDACTED_NON_ASCII_NAME && /[^\x00-\x7f]/.test(name));
}

/** One registered name (or copy directory), and what it folds to. */
interface Redaction {
  /** The name as it is on disk, or one of its Unicode normal forms; the list is ordered by its length. */
  readonly text: string;
  readonly say: string;
  /**
   * A run of the name that every form keeps verbatim (letters, digits and
   * `-._~`, which no escape and no URL encoder touches), checked with
   * `includes` before the pattern runs; empty when the name has none.
   */
  readonly anchor: string;
  readonly pattern: RegExp;
}

/** One or more backslashes: an escape written once, or escaped again (JSON of a `{:?}` form, and so on). */
const BACKSLASHES = '\\\\+';
/** A byte `{:?}` writes as `\xHH`, after its backslashes. */
const BYTE = 'x[0-9A-Fa-f]{2}';
/**
 * Where a character sits in the name, which decides how its escapes take the
 * backslashes in front of them (#576 RV18-W23-1). Taken as `BACKSLASHES`
 * everywhere, a run of N backslashes was tried from each of its N backslashes
 * and read to its end each time, and two runs side by side (a backslash of the
 * name before an escaped character) split it every way there is: 40k
 * backslashes took seconds, a 1 MB line did not finish.
 * - `first`: an escape starts where a run of backslashes (or of `\xHH` bytes)
 *   starts, never inside one. It folds the same text - a match from inside a
 *   run also matches from the run's start, the extra backslashes taken in -
 *   and each run is read once.
 * - `afterBackslash`: the name's backslash before it (`BACKSLASHES`) has taken
 *   the run, so an escape takes exactly one more backslash and a quote none.
 *   `\\+\\u` and `\\+"` match what `\\+\\+u` and `\\+\\*"` did, one way only.
 * - `inside`: after any other character, as before - the match comes to a run
 *   from that character, so at the run's start.
 */
type CharAt = 'first' | 'afterBackslash' | 'inside';
/** Escapes that name a control character, Rust's (`\0`) and JSON's (`\b`, `\f`) together. */
const NAMED_ESCAPES: Readonly<Record<string, string>> = { '\n': 'n', '\r': 'r', '\t': 't', '\0': '0', '\b': 'b', '\f': 'f' };

/** `value` in hex, at least `width` digits, each letter matching either case. */
function hexPattern(value: number, width: number): string {
  return value
    .toString(16)
    .padStart(width, '0')
    .replace(/[a-f]/g, (digit) => `[${digit}${digit.toUpperCase()}]`);
}

/** What a URL encoder leaves as it is (RFC 3986 unreserved); every other character it may write `%HH`. */
const UNRESERVED = /^[A-Za-z0-9._~-]$/;

/** `c` percent-encoded: its UTF-8 bytes, each `%HH` in either case. */
function percentPattern(c: string): string {
  return Array.from(Buffer.from(c, 'utf8'), (byte) => `%${hexPattern(byte, 2)}`).join('');
}

/** The named references an HTML writer uses for markup characters. */
const HTML_NAMED: Readonly<Record<string, string>> = { '&': 'amp', '<': 'lt', '>': 'gt', '"': 'quot', "'": 'apos' };

/**
 * `c` as an HTML writer may write it (#576 RV19b-P5): a numeric character
 * reference, decimal or hex in either case with any leading zeros, and for a
 * markup character its named one (`&amp;`).
 */
function htmlPattern(c: string): string {
  const code = c.codePointAt(0) ?? 0;
  const named = HTML_NAMED[c];
  return [`&#0*${code};`, `&#[xX]0*${hexPattern(code, 1)};`, ...(named === undefined ? [] : [`&${named};`])].join('|');
}

/**
 * What one character of a name can look like in text the app wrote and the
 * harness read back (#576 RV17-1). Printable ASCII is itself - `"` and `\` with
 * any number of backslashes in front, one level of escaping per layer (Rust's
 * `{:?}`, then JSON around it). Every other character - a control, and ANY
 * non-ASCII one - is itself or one of its escapes, at any depth: Rust's
 * `\u{hex}` (`{:?}` writes it for a combining mark, a no-break or zero-width
 * space, a C1 control, ... - the set follows Unicode, so it is not listed),
 * JSON's `\uXXXX` (an ASCII-only encoder writes one for every non-ASCII
 * character, past U+FFFF a surrogate pair), and a control's named escape.
 * U+FFFD also stands for bytes that are not UTF-8: Node reads such a file name
 * with U+FFFD in their place, and Rust's `{:?}` of the path writes them `\xHH`.
 * Any character but a letter, a digit and `-._~` is also its percent-encoding,
 * as a `file://` URL writes it (`Caf%C3%A9%20Nord.srv`, #576 RV18-W23-5), and
 * its HTML character reference (`htmlPattern`); a space is also the `+` a form
 * encoder writes (#576 RV19b-P5).
 * `at` is where `c` sits in the name (`CharAt`).
 */
function charPattern(c: string, at: CharAt): string {
  const lead = at === 'afterBackslash' ? '\\\\' : at === 'first' ? `(?<!\\\\)${BACKSLASHES}` : BACKSLASHES;
  // Its percent-encoding and its HTML references.
  const encoded = `${percentPattern(c)}|${htmlPattern(c)}`;
  if (c === '"') {
    const quote = at === 'afterBackslash' ? '"' : at === 'first' ? '(?<!\\\\)\\\\*"' : '\\\\*"';
    return `(?:${quote}|${encoded})`;
  }
  if (c === '\\') return `(?:${lead}|${encoded})`;
  const code = c.codePointAt(0) ?? 0;
  if (code >= 0x20 && code < 0x7f) {
    const literal = c.replace(/[\\^$.*+?()[\]{}|/]/g, '\\$&');
    if (UNRESERVED.test(c)) return literal;
    return `(?:${literal}|${encoded}${c === ' ' ? '|\\+' : ''})`;
  }
  let json = '';
  for (let i = 0; i < c.length; i++) json += `${i === 0 ? lead : BACKSLASHES}u${hexPattern(c.charCodeAt(i), 4)}`;
  const forms = [c, `${lead}u\\{0*${hexPattern(code, 1)}\\}`, json, encoded];
  const named = NAMED_ESCAPES[c];
  if (named !== undefined) forms.push(`${lead}${named}`);
  if (code === 0xfffd) {
    const notAfterByte = at === 'first' ? `(?<!\\\\${BYTE})` : '';
    forms.push(`${notAfterByte}${lead}${BYTE}(?:${BACKSLASHES}${BYTE})*`);
  }
  return `(?:${forms.join('|')})`;
}

function redactionFor(text: string, say: string): Redaction {
  const runs = text.match(/[A-Za-z0-9._~-]+/g) ?? [];
  const anchor = runs.reduce((longest, run) => (run.length > longest.length ? run : longest), '');
  const chars = Array.from(text);
  const at = (i: number): CharAt => (i === 0 ? 'first' : chars[i - 1] === '\\' ? 'afterBackslash' : 'inside');
  return { text, say, anchor, pattern: new RegExp(chars.map((c, i) => charPattern(c, at(i))).join('')) };
}

/**
 * Every tag the fold writes (`Redaction.say`), and the longest one's length.
 * The fold leaves each of these as it finds it (#576 W33-NF4): a line is folded
 * more than once on its way out (a tool result folded, cut, and folded again
 * into the action log), and a corpus name that occurs inside a tag - `corpus`,
 * `file`, the id - turned `<corpus #id>/file` into `<<corpus #id>/file #id>/...`
 * on the second fold, and on the first when an earlier name's tag held it.
 */
const FOLD_TAGS = new Set<string>();
let longestFoldTag = 0;
const TAG_OPEN = '<corpus #';
const TAG_FILE = '/file';

/**
 * Register one materialized entry: its copy directory, and every file and
 * directory name the copy created at any depth (`MaterializedCorpusEntry.names`),
 * each matched in every escaped form it can take (`charPattern`), folded to the
 * same tag.
 */
export function registerCorpusRedactions(id: string, destination: string, names: readonly string[]): void {
  const tag = `${TAG_OPEN}${id}>`;
  // Each in both Unicode normal forms too (#576 RV18-W23-5): a macOS file name
  // comes decomposed (`e` then U+0301), and a layer that normalizes it on the
  // way writes it composed (U+00E9) - or the other way round.
  const add = (list: Redaction[], text: string, say: string): void => {
    for (const form of new Set([text, text.normalize('NFC'), text.normalize('NFD')])) {
      if (!list.some((entry) => entry.text === form)) list.push(redactionFor(form, say));
    }
    FOLD_TAGS.add(say);
    longestFoldTag = Math.max(longestFoldTag, say.length);
  };
  if (destination !== '') add(REDACTED_DIRS, destination, tag);
  for (const name of names) {
    if (!foldsName(name)) continue;
    add(REDACTED_NAMES, name, `${tag}${TAG_FILE}`);
  }
  // Longest first, so a name that is a prefix of another cannot leave the
  // other's tail behind (`cave` before `cave-north.srv` would print `-north.srv`,
  // and `corpus-3` before `corpus-30` would tag the wrong entry).
  REDACTED_DIRS.sort((a, b) => b.text.length - a.text.length);
  REDACTED_NAMES.sort((a, b) => b.text.length - a.text.length);
}

/**
 * `text` cut at every tag the fold writes (`FOLD_TAGS`), as `[raw, tag, raw,
 * ..., raw]` - raw text at every even index. A tag reads from `<corpus #` to the
 * first `>`, and on through `/file` when that is a tag too; any other text that
 * starts `<corpus #` is raw, so a name in it still folds. Linear in `text`: no
 * stretch is searched for `>` twice, and a candidate longer than every tag is
 * not looked up.
 */
function splitAtTags(text: string): string[] {
  const parts: string[] = [];
  let from = 0;
  let close = -1;
  for (let at = text.indexOf(TAG_OPEN); at !== -1; ) {
    if (close < at) close = text.indexOf('>', at + TAG_OPEN.length);
    if (close === -1) break;
    let tag: string | null = null;
    if (close + 1 - at <= longestFoldTag) {
      const head = text.slice(at, close + 1);
      if (text.startsWith(TAG_FILE, close + 1) && FOLD_TAGS.has(head + TAG_FILE)) tag = head + TAG_FILE;
      else if (FOLD_TAGS.has(head)) tag = head;
    }
    if (tag === null) {
      at = text.indexOf(TAG_OPEN, at + 1);
      continue;
    }
    parts.push(text.slice(from, at), tag);
    from = at + tag.length;
    at = text.indexOf(TAG_OPEN, from);
  }
  parts.push(text.slice(from));
  return parts;
}

/**
 * `run` (raw text) with every match of `entry` replaced by its tag, pushed onto
 * `parts` as `raw, tag, raw, ..., raw`. Each search runs on what follows the
 * last match as a string of its own, so a match is an edge exactly as the tag
 * that replaces it is to the next fold (#576 W33-NF4). Searched in place, a
 * name that ends in a backslash and was written twice in a row folded once:
 * its backslash read as an escape of the second one's opening quote
 * (`(?<!\\)` in `charPattern`), so the second one printed - until a second
 * fold, which saw a tag there instead, folded it.
 *
 * The run of backslashes a match opens with - the escape of the name's first
 * character, `\"` or `\u00e9` - is left as raw text before the tag, its length
 * in `lent` at the tag's index (0 at a raw part's), for a shorter name that ends
 * in a backslash to fold with (#576 W39-NF5): names fold longest first, and in
 * `qzab\"qzlonger.dmp` the longer name took the backslash of `qzab\`, which
 * then printed. `redactCorpusText` gives the tag back what no name took.
 */
function foldRun(run: string, entry: Redaction, parts: string[], lent: number[]): void {
  let rest = run;
  for (let match = entry.pattern.exec(rest); match !== null; match = entry.pattern.exec(rest)) {
    const opening = /^\\+/.exec(match[0])?.[0].length ?? 0;
    parts.push(rest.slice(0, match.index + opening), entry.say);
    lent.push(0, opening);
    rest = rest.slice(match.index + match[0].length);
  }
  parts.push(rest);
  lent.push(0);
}

/**
 * `text` with every registered copy directory and corpus name replaced by the
 * entry's publishable tag. Directories go first, so a full path is folded whole
 * rather than leaving `<corpus #id>/<basename>` behind.
 *
 * A tag is never folded again, whether this call wrote it or `text` already
 * held it (#576 W33-NF4): each name is looked for only in the raw text between
 * tags, so folding a folded line changes nothing.
 */
export function redactCorpusText(text: string): string {
  let parts = splitAtTags(text);
  let lent = parts.map(() => 0);
  for (const list of [REDACTED_DIRS, REDACTED_NAMES]) {
    for (const entry of list) {
      if (entry.anchor !== '' && !parts.some((part, i) => i % 2 === 0 && part.includes(entry.anchor))) continue;
      const next: string[] = [];
      const nextLent: number[] = [];
      for (let i = 0; i < parts.length; i++) {
        if (i % 2 === 0) foldRun(parts[i]!, entry, next, nextLent);
        else {
          next.push(parts[i]!);
          nextLent.push(lent[i]!);
        }
      }
      parts = next;
      lent = nextLent;
    }
  }
  // What a match lent (`foldRun`) and no name ending in a backslash took is the
  // escape of the match's first character: its tag takes it back, so the line
  // reads as it did before W39-NF5.
  for (let i = 1; i < parts.length; i += 2) {
    const back = lent[i]!;
    if (back > 0 && parts[i - 1]!.endsWith('\\'.repeat(back))) parts[i - 1] = parts[i - 1]!.slice(0, -back);
  }
  return parts.join('');
}

/**
 * `text` folded, THEN cut to its first `max` characters (#576 W33-NF3). The
 * order is the point (RV18-W23-6): a cut through a corpus name leaves a stub
 * that no fold matches, and its start prints. A cut through a tag leaves only
 * a piece of the tag.
 */
export function foldThenCut(text: string, max: number): string {
  return redactCorpusText(text).slice(0, max);
}

/**
 * `value` as it may be recorded: every string in it, keys included, through
 * `redactCorpusText` (#576 RV13-4). String by string rather than over the JSON
 * text, so a name that JSON would escape (a quote, a backslash) is still found,
 * and folding cannot break the encoding. A value with `toJSON` (a Date) is
 * folded in the form JSON would write. Returns `value` itself when no corpus
 * entry was copied in this process.
 */
export function redactCorpusValue(value: unknown): unknown {
  if (REDACTED_DIRS.length === 0 && REDACTED_NAMES.length === 0) return value;
  return foldValue(value);
}

function foldValue(value: unknown): unknown {
  if (typeof value === 'string') return redactCorpusText(value);
  if (Array.isArray(value)) return value.map(foldValue);
  if (value === null || typeof value !== 'object') return value;
  const toJSON = (value as { toJSON?: unknown }).toJSON;
  if (typeof toJSON === 'function') return foldValue((toJSON as () => unknown).call(value));
  return Object.fromEntries(Object.entries(value).map(([key, item]) => [redactCorpusText(key), foldValue(item)]));
}

/**
 * An error's message as it may be recorded or returned: `err.message` (or the
 * thrown value as text) through `redactCorpusText`. An import that fails quotes
 * the corpus copy it was handed, and the MCP server records a tool's error in the
 * session's action log, which the repro report prints (#576 RV12-F1).
 */
export function publishableErrorMessage(err: unknown): string {
  return redactCorpusText(err instanceof Error ? err.message : String(err));
}
