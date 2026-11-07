/**
 * Determinism mode, the I/O half: what a replay RECORDS. The pure half (masks,
 * canonical forms, the comparison) is `fuzzDeterminism.ts`; the pairing of two
 * replays is `fuzzDeterminismRun.ts`.
 *
 * One probe is active per replay, installed by the runner around
 * `scenario.run`. Every hook in the scenarios is `detProbe()?.x(...)`, so with
 * the mode off (no probe) a hook is a null check and nothing else: no extra IPC,
 * no file read, no draw - a seed's trajectory is untouched.
 *
 * With the mode on, a hook only READS: `get_project_state` and the two history
 * stacks (the same read-only commands the fuzzers' own observation calls every
 * step), the saved `.stalx` through a read-only SQLite connection, and the
 * exported file. It never draws from a PRNG, so both replays take the same path
 * they would have taken without it.
 */
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import { createRequire } from 'node:module';
import type { DatabaseSync as DatabaseSyncType } from 'node:sqlite';
import type { ScenarioContext } from '../scenarios/framework';
import {
  Masker,
  WALL_CLOCK_MASK,
  canonicalFile,
  decodeCell,
  makeRecord,
  plantDifference,
  sha256,
  type DetRecord,
  type DetRecordKind,
} from './fuzzDeterminism';

// Through `createRequire` for the reason `fixtures.ts` gives: vite cannot
// resolve the prefix-only `node:sqlite`, and vitest imports this module.
const nodeRequire = createRequire(import.meta.url);

/** Read every project/history value the fuzzers observe, as one JSON string. */
const STATE_SOURCE = `
  const core = window.__TAURI__ && window.__TAURI__.core;
  if (!core || typeof core.invoke !== 'function') throw new Error('window.__TAURI__.core.invoke unavailable');
  const read = async (command) => {
    try {
      return { ok: await core.invoke(command) };
    } catch (err) {
      return { err: String(err && err.message ? err.message : err) };
    }
  };
  return JSON.stringify({
    project: await read('get_project_state'),
    undo: await read('get_undo_stack'),
    redo: await read('get_redo_stack'),
  });
`;

let active: DetProbe | null = null;

/** The probe of the replay running now, or null when the mode is off. */
export function detProbe(): DetProbe | null {
  return active;
}

export function setDetProbe(probe: DetProbe | null): void {
  active = probe;
}

export class DetProbe {
  readonly records: DetRecord[] = [];
  private readonly masker = new Masker(os.tmpdir());
  /** Trace steps drawn so far: the step index a divergence reports. */
  private steps = 0;
  private fused = 0;
  private states = 0;

  /**
   * @param replay which of the two replays this is
   * @param plantAt for the self-test knob: replay B perturbs its Nth state record
   */
  constructor(
    readonly replay: 'A' | 'B',
    private readonly plantAt: number | null = null,
  ) {}

  /** A directory whose spelling differs per replay (the session dir). */
  addRoot(dir: string | null): void {
    if (dir) this.masker.addRoot(dir);
  }

  private push(kind: DetRecordKind, label: string, value: unknown, rawHash: string | null = null, masked = 0): void {
    const maskedLabel = this.masker.text(label);
    this.masker.takeCount();
    const canonical = this.masker.value(value);
    const total = masked + this.masker.takeCount();
    this.records.push(makeRecord(kind, maskedLabel, this.steps, canonical, total, rawHash));
  }

  /** One drawn trace step (called by `TraceRecorder.record`). */
  action(step: unknown): void {
    this.steps += 1;
    this.push('action', `trace step ${this.steps}`, step);
  }

  /** An IPC answer or outcome the scenario reads. */
  answer(label: string, value: unknown): void {
    this.push('answer', label, value);
  }

  /** The project state and both history stacks, as the app reports them now. */
  async state(ctx: ScenarioContext, label: string): Promise<void> {
    const parsed = JSON.parse(await ctx.eval<string>(STATE_SOURCE)) as unknown;
    this.states += 1;
    if (this.replay === 'B' && this.plantAt === this.states) {
      // The record's number, not the path planted at: a path can carry a quoted
      // key of the app's data, and this line is a progress line people share
      // (`shareableRunnerOutput.test.ts` scans it). The divergence it causes
      // names the record and its path.
      const found = plantDifference(parsed) !== null;
      ctx.log(`determinism self-test: replay B planted a difference in state record ${this.states}${found ? '' : ' (no number found)'}`);
    }
    this.push('state', label, parsed);
  }

  /** One fused undo-fuzzer step: the request, the page's answer, then the state. */
  async step(ctx: ScenarioContext, request: Record<string, unknown>, response: object): Promise<void> {
    this.fused += 1;
    // The page's own durations are wall-clock values: masked like any other.
    const answer: Record<string, unknown> = { ...response, timing: WALL_CLOCK_MASK };
    // `bytes` is the LENGTH of the page's canonical string, which carries
    // `merged_at` / `created_at` stamps whose fraction digits vary with the
    // clock; the state record below compares that content, stamps masked.
    if ('bytes' in answer) answer.bytes = WALL_CLOCK_MASK;
    this.answer(`fused step ${this.fused}: ${String(request.kind)} ${String(request.action)}`, { request, answer });
    await this.state(ctx, `after fused step ${this.fused}`);
  }

  /** Every table of a saved `.stalx`, row by row, through a read-only connection. */
  stalx(label: string, file: string): void {
    if (!fs.existsSync(file)) {
      this.push('table', `${label}: <missing>`, null);
      return;
    }
    const { DatabaseSync } = nodeRequire('node:sqlite') as { DatabaseSync: typeof DatabaseSyncType };
    const db = new DatabaseSync(file, { readOnly: true });
    try {
      const version = db.prepare('PRAGMA user_version').get() as Record<string, unknown> | undefined;
      const tables = db
        .prepare("SELECT name, sql FROM sqlite_master WHERE type = 'table' ORDER BY name")
        .all() as { name: string; sql: string | null }[];
      this.push('table', `${label}: schema`, { userVersion: version ?? null, tables });
      for (const { name } of tables) {
        const quoted = `"${name.replace(/"/g, '""')}"`;
        const columns = db.prepare(`PRAGMA table_info(${quoted})`).all() as { name: string; pk: number }[];
        const keys = columns
          .filter((column) => column.pk > 0)
          .sort((a, b) => a.pk - b.pk)
          .map((column) => `"${column.name.replace(/"/g, '""')}"`);
        const order = keys.length > 0 ? keys.join(', ') : 'rowid';
        const rows = (db.prepare(`SELECT * FROM ${quoted} ORDER BY ${order}`).all() as Record<string, unknown>[]).map(
          (row) => Object.fromEntries(Object.entries(row).map(([column, cell]) => [column, decodeCell(cell)])),
        );
        this.push('table', `${label}: table ${name}`, { rows });
      }
    } finally {
      db.close();
    }
  }

  /** A file the app wrote (an export): its canonical form and its raw hash. */
  file(label: string, file: string): void {
    let bytes: Buffer;
    try {
      bytes = fs.readFileSync(file);
    } catch (err) {
      this.push('file', label, { unreadable: (err as NodeJS.ErrnoException).code ?? 'error' });
      return;
    }
    const { body, masked } = canonicalFile(bytes, path.extname(file));
    this.push('file', label, { name: path.basename(file), body }, sha256(bytes), masked);
  }

  /**
   * One export move's judged attempt, then the file it wrote. The attempt's
   * `size` is reported as "written" only: for a `.tml` it is the size of a
   * DEFLATED zip, which follows the random uuids a load mints for shots that
   * have none (export/tml.rs `shot_export_uuid`), and the file record compares
   * the content itself.
   */
  exportMove(seq: number, attempt: { effective: string; size: number | null }): void {
    const { size, ...rest } = attempt;
    this.answer(`export move ${seq}`, { ...rest, written: size !== null });
    this.file(`export move ${seq} file`, attempt.effective);
  }

  /**
   * A store export call and its outcome (`callProjectAction`). The file at
   * `args[0]` is recorded when the call says it succeeded.
   */
  exportCall(action: string, args: readonly unknown[], outcome: { ok: boolean; err: string | null }): void {
    if (!action.startsWith('export')) return;
    this.answer(`${action} outcome`, { args, outcome });
    const target = args[0];
    if (outcome.ok && typeof target === 'string') this.file(`${action} file`, target);
  }
}
