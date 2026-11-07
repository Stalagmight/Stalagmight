/**
 * #576 NF-W12H-1: a REPLAY numbers the files it opens, and the projects it
 * creates, the way its recording did.
 *
 * The import-dialog fuzzer names every project it creates
 * `import-<seed>-<n>.stalx`, where `n` counts the file-opening moves so far.
 * The live walk advanced that count as it drew a move; a replay never did, so
 * every project a replay created took number 0, the second create landed on
 * the first one's file, and F1's `projectSwapped` skip - which compares the
 * project FILE before and after a flow - could not fire. A replay of a green
 * recording failed F1 at `project.created_at`.
 *
 * The live walk is modelled here by what its draw does with each drawn move
 * (`FileSeq.noteMove`); the replay side is the scenario's own step-to-move
 * conversion, `moveFromTraceStep`.
 */
import { describe, expect, it } from 'vitest';

import type { TraceStep } from './fuzzTrace';
import { PROJECT_DIR_PLACEHOLDER, type FuzzMove } from '../scenarios/importDialogGenerators';
import { FileSeq, moveFromTraceStep, moveTraceStep } from '../scenarios/importDialogTrace';

const file = (n: number) => ({ origin: 'disk' as const, rel: `fuzz-files/fuzz-${n}.dmp` });
const savePath: FuzzMove = {
  kind: 'input',
  testid: 'project-save-path-input',
  value: PROJECT_DIR_PLACEHOLDER,
  mode: 'type',
  label: 'redirect the save path into the session directory',
};
const isSavePath = (move: FuzzMove | null): boolean => move?.kind === 'input' && move.value === PROJECT_DIR_PLACEHOLDER;

/**
 * A recorded walk that creates three projects: one after a picker open, one
 * after a routed file, a cancelled picker and an armed-keyboard open, and one
 * after another picker open.
 */
const WALK: FuzzMove[] = [
  { kind: 'openFile', source: file(0), via: 'dialog', trigger: 'toolbar-import-button', label: 'import via toolbar-import-button' },
  savePath,
  { kind: 'ui', testid: 'create-and-continue-button', label: 'create the project' },
  { kind: 'openFile', source: file(1), via: 'route', trigger: 'routeOpenedFile', label: 'import via router' },
  { kind: 'cancelDialog', trigger: 'toolbar-import-button', label: 'cancel the picker' },
  { kind: 'keyFile', chord: 'Ctrl+O', source: file(2), label: 'INTERJECT: Ctrl+O with an armed picker' },
  savePath,
  { kind: 'openFile', source: file(3), via: 'dialog', trigger: 'global-open-import-button', label: 'import via global-open-import-button' },
  { kind: 'undo', label: 'INTERJECT: undo mid-flow' },
  savePath,
];

/** The live walk: each drawn move is counted, then recorded once performed. */
function record(walk: FuzzMove[]): { steps: TraceStep[]; projectNumbers: number[] } {
  const live = new FileSeq();
  const steps: TraceStep[] = [];
  const projectNumbers: number[] = [];
  for (const move of walk) {
    live.noteMove(move);
    if (isSavePath(move)) projectNumbers.push(live.value);
    const step = moveTraceStep(move);
    if (step !== null) steps.push(step);
  }
  return { steps, projectNumbers };
}

/** The replay: each recorded step turned back into the move it dispatches. */
function replay(steps: TraceStep[]): number[] {
  const files = new FileSeq();
  const projectNumbers: number[] = [];
  for (const step of steps) {
    const move = moveFromTraceStep(step, files, 'import-dialog fuzz');
    if (isSavePath(move)) projectNumbers.push(files.value);
  }
  return projectNumbers;
}

describe('the import-dialog fuzz replay numbers files like its recording (#576 NF-W12H-1)', () => {
  it('gives each project it creates the number the recording gave it', () => {
    const recorded = record(WALK);
    expect(recorded.projectNumbers).toEqual([1, 3, 4]);
    expect(replay(recorded.steps)).toEqual(recorded.projectNumbers);
  });

  it('never gives two projects created after different file opens the same file', () => {
    const numbers = replay(record(WALK).steps);
    expect(new Set(numbers).size).toBe(numbers.length);
  });

  it('counts exactly the moves that open a file', () => {
    const files = new FileSeq();
    const counted = WALK.map((move) => {
      const before = files.value;
      files.noteMove(move);
      return `${move.kind}:${files.value - before}`;
    });
    expect(counted).toEqual([
      'openFile:1',
      'input:0',
      'ui:0',
      'openFile:1',
      'cancelDialog:0',
      'keyFile:1',
      'input:0',
      'openFile:1',
      'undo:0',
      'input:0',
    ]);
  });

  it('turns each recorded step back into the move that recorded it', () => {
    const { steps } = record(WALK);
    const files = new FileSeq();
    const replayed = steps.map((step) => moveFromTraceStep(step, files, 'import-dialog fuzz'));
    // `undo` replays under its own label; every other move comes back whole.
    expect(replayed).toEqual(WALK.map((move) => (move.kind === 'undo' ? { kind: 'undo', label: 'replay undo' } : move)));
  });

  it('still refuses a store action it does not replay, naming the scenario', () => {
    const step: TraceStep = { kind: 'mutate', action: 'loadProject', args: [], label: 'load' };
    expect(() => moveFromTraceStep(step, new FileSeq(), 'import-dialog fuzz')).toThrow(
      'refusing to replay mutate "loadProject": import-dialog fuzz replays closeProject',
    );
  });
});
