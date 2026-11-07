/**
 * The trace steps the import-dialog fuzzer records, as pure functions of a move.
 *
 * Split out of the scenario's closure for two reasons. The step a move records
 * when it returns and the step a STALL leaves behind (the move that never
 * returned, #576 HFI-9) have to be the same step, so they come from one place.
 * And the file-opening steps are where a corpus file's real name used to leak
 * into every corpus trace (#576 HFI-13); with the construction here, a test can
 * build exactly what a run writes and look for the name in it.
 */
import * as path from 'path';

import { corpusEntryRel, isCorpusEntryRel, type TraceStep } from '../lib/fuzzTrace';
import { describeSource, type FileSource, type FuzzMove } from './importDialogGenerators';
import { shareableTestId } from '../../../scripts/lib/shareable-selector.mjs';

/**
 * The step that PUT a move's input file on disk, recorded before the step that
 * opens it. A generated file travels as its spec; a corpus file as its id and
 * resolution hash and the directory it is copied into - never its name. A
 * tracked fixture or a replayed file needs no step.
 */
export function sourceTraceStep(source: FileSource): TraceStep | null {
  if (source.origin === 'gen') {
    return { kind: 'genFile', spec: source.spec, rel: source.rel, label: describeSource(source) };
  }
  if (source.origin === 'corpus') {
    return {
      kind: 'corpusFile',
      corpusId: source.id,
      // The resolution key. `corpusId` is 20 bits and labels the file in
      // everything a human reads; this is what a replay looks it up by, so a
      // corpus holding a second file that folds to the same id cannot be
      // materialized in its place.
      corpusHash: source.hash,
      // The DIRECTORY the copy lands in; the `route`/`fileDialog` step that
      // follows names the entry inside it as `corpusEntryRel(dirRel)`.
      rel: source.dirRel,
      label: describeSource(source),
    };
  }
  return null;
}

/**
 * The step a move records once it has been performed, or null for a move whose
 * step cannot be known before it runs (a RELATIVE map click is recorded at the
 * absolute point it resolved to).
 *
 * A file move records `source.rel`, which for a corpus source is the entry
 * token and never the copy's basename - see `fuzzTrace.CORPUS_ENTRY_SEGMENT`.
 * An `input` move records the value as DRAWN, before the shell substitutes a
 * real path for the projects-directory placeholder.
 */
export function moveTraceStep(move: FuzzMove): TraceStep | null {
  switch (move.kind) {
    case 'ui':
      return move.double === true
        ? { kind: 'ui', testid: move.testid, double: true, label: move.label }
        : { kind: 'ui', testid: move.testid, label: move.label };
    case 'input':
      return { kind: 'input', testid: move.testid, value: move.value, mode: move.mode, label: move.label };
    case 'openFile':
      return move.via === 'route'
        ? { kind: 'route', rel: move.source.rel, label: move.label }
        : { kind: 'fileDialog', trigger: move.trigger, dialog: 'open', rel: move.source.rel, label: move.label };
    case 'keyFile':
      return { kind: 'fileDialog', trigger: `key:${move.chord}`, dialog: 'open', rel: move.source.rel, label: move.label };
    case 'cancelDialog':
      return { kind: 'fileDialog', trigger: move.trigger, dialog: 'open', rel: null, label: move.label };
    case 'closeProject':
      return { kind: 'mutate', action: 'closeProject', args: [], label: move.label };
    case 'mapClick':
      return move.relative === true ? null : { kind: 'mapClick', lngLat: move.lngLat, label: move.label };
    case 'undo':
      return { kind: 'undo' };
    case 'redo':
      return { kind: 'redo' };
    case 'save':
      return { kind: 'save' };
    default:
      return null;
  }
}

/**
 * Where a REPLAY's corpus entry tokens point.
 *
 * A `corpusFile` step materializes a copy and binds its directory's token to the
 * entry file the copy produced; the `route` / `fileDialog` step after it carries
 * only the token and is resolved here. A token nobody bound - ddmin cut the
 * `corpusFile` step away, or this machine's corpus does not have the entry -
 * resolves to null, which the shell skips and counts like any missing file.
 */
export class CorpusEntryPaths {
  private readonly bound = new Map<string, string>();

  /** Bind the entry copied into `dirRel` to the absolute path of the copy. */
  bind(dirRel: string, absolutePath: string): void {
    this.bound.set(corpusEntryRel(dirRel), absolutePath);
  }

  /**
   * The absolute path a token stands for, null for an unbound token, and
   * undefined when `rel` is not a token at all (an ordinary session path).
   */
  resolve(rel: string): string | null | undefined {
    if (!isCorpusEntryRel(rel)) return undefined;
    return this.bound.get(rel.split('\\').join('/')) ?? null;
  }
}

/**
 * `mutate` actions a REPLAYED trace may drive - the DOM allowlist's twin.
 *
 * One entry: `closeProject`, which the wave-3 auxiliary interjection plays at
 * `idle`. The set REFUSES anything else loudly, which is the point - a trace
 * naming `loadProject` or `deleteProject` must not be quietly skipped inside a
 * privileged webview with the `__E2E_TEST__` hooks live.
 *
 * `ui:confirmPlacement` is deliberately still absent: that store action
 * ABANDONS a placement rather than completing one (see the note in
 * `importDialogGenerators.ts`), so it reaches a state no user can.
 */
const REPLAYABLE_UI_ACTIONS: ReadonlySet<string> = new Set(['closeProject']);

/**
 * The number a walk gives its next input file and any project it creates now
 * (`fuzz-<n>.<ext>`, `corpus-<n>/`, `import-<seed>-<n>.stalx`).
 *
 * It counts the file-OPENING moves (`openFile`, `keyFile`) the walk has made,
 * and both walks count them alike: the live walk as it draws one, a replay as it
 * reads one back - `moveFromTraceStep` counts, so a replay cannot turn a step
 * into a move without it. A replay used to count nothing, so every project it
 * created was `import-<seed>-0.stalx`: the second create landed on the first
 * one's file, the project file never changed across a flow that replaced the
 * project, F1's `projectSwapped` skip (keyed on that file) could not fire, and
 * the replay of a green recording failed F1 at `project.created_at` (#576
 * NF-W12H-1).
 *
 * What a replay cannot see: a draw the recording made but did not record (a
 * corpus entry that could not be copied, a picker trigger that was gone when it
 * was clicked) advanced the live count and left no step, so from there on a
 * replay numbers one lower than its recording. Each file it opens still gets
 * its own number.
 */
export class FileSeq {
  private count = 0;

  /** The number the next input file, and a project created now, take. */
  get value(): number {
    return this.count;
  }

  /** Count `move` if it opens a file. */
  noteMove(move: FuzzMove): void {
    if (move.kind === 'openFile' || move.kind === 'keyFile') this.count += 1;
  }
}

/** A replayed file reference: the bytes are already on disk at `rel`. */
function replaySource(rel: string): FileSource {
  return rel.startsWith('fixtures/') ? { origin: 'fixture', name: path.basename(rel), rel } : { origin: 'disk', rel };
}

/**
 * Turn a recorded step back into a move, or null when it is not one - the
 * inverse of `moveTraceStep`. A move that opens a file is counted in `files`,
 * exactly as the live walk counts one it draws (see `FileSeq`). `scenario`
 * names the scenario in the refusal of a store action it does not replay.
 */
export function moveFromTraceStep(step: TraceStep, files: FileSeq, scenario: string): FuzzMove | null {
  const move = replayedMove(step, scenario);
  if (move !== null) files.noteMove(move);
  return move;
}

function replayedMove(step: TraceStep, scenario: string): FuzzMove | null {
  switch (step.kind) {
    case 'ui':
      return step.double === true
        ? { kind: 'ui', testid: step.testid, double: true, label: step.label }
        : { kind: 'ui', testid: step.testid, label: step.label };
    case 'input':
      return { kind: 'input', testid: step.testid, value: step.value, mode: step.mode, label: step.label };
    case 'route':
      return {
        kind: 'openFile',
        source: replaySource(step.rel),
        via: 'route',
        trigger: 'routeOpenedFile',
        label: step.label,
      };
    case 'fileDialog': {
      if (step.rel === null) return { kind: 'cancelDialog', trigger: step.trigger, label: step.label };
      const rel = Array.isArray(step.rel) ? (step.rel[0] as string) : step.rel;
      // A `key:<chord>` trigger is the ARMED-KEYBOARD picker, not a click on
      // an element - it has no testid, so it cannot go through the click
      // path at all.
      if (step.trigger.startsWith('key:')) {
        return {
          kind: 'keyFile',
          chord: step.trigger.slice('key:'.length),
          source: replaySource(rel),
          label: step.label,
        };
      }
      return {
        kind: 'openFile',
        source: replaySource(rel),
        via: 'dialog',
        trigger: step.trigger,
        label: step.label,
      };
    }
    case 'mapClick':
      return { kind: 'mapClick', lngLat: step.lngLat, label: step.label };
    case 'undo':
      return { kind: 'undo', label: 'replay undo' };
    case 'redo':
      return { kind: 'redo', label: 'replay redo' };
    case 'save':
      return { kind: 'save', label: 'replay save' };
    case 'mutate':
      // The store-action twin of the DOM allowlist: a trace names an action,
      // and only the ones this scenario produces may be dispatched.
      if (step.action === 'closeProject') return { kind: 'closeProject', label: step.label };
      throw new Error(
        `refusing to replay mutate "${shareableTestId(step.action)}": ${scenario} replays ` +
          `${REPLAYABLE_UI_ACTIONS.size === 0 ? 'no store actions' : [...REPLAYABLE_UI_ACTIONS].join(', ')}`,
      );
    default:
      return null;
  }
}
