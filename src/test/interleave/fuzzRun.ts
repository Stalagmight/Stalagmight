/**
 * [#576 FZ-INT] One run of the frontend interleaving fuzzer: a seeded list of
 * actions - user operations on the REAL stores, and schedule steps that decide
 * which pending IPC call the reference backend applies and answers next - played
 * against fresh store modules, then drained to quiescence and checked.
 *
 * The action list is the whole input: a schedule step carries a number in [0,1)
 * that picks among the calls pending at that moment, so a list replays exactly,
 * and stays meaningful when the shrinker removes actions from it.
 *
 * Invariants (ids used in reports): see `INVARIANTS` below.
 */
import { vi } from 'vitest';

import { BackendModel, type Storage } from './backendModel';
import { IpcScheduler } from './ipcScheduler';

export const INVARIANTS = {
  'identity': 'the store never shows one project generation\'s data under another\'s identity (checked after every step)',
  'display-target': 'a display write lands only in a file (or cache) of the project it was made in',
  'edit-target': 'an edit is applied only to the project it was issued in (a reload of the same file counts as it)',
  'display-lost': 'the last display write made in a project still open at the end is what that project stores',
  'display-shown': 'after quiescence the display settings shown equal what the open project stores',
  'edit-lost': 'an edit issued in a project still open at the end was not refused or dropped',
  'dirty-flag': 'after quiescence a clean (isDirty=false) project has no unsaved change in the backend',
  'state': 'after quiescence the store shows exactly the backend\'s open project (generation, file, sections, elevation, undo/redo, warnings)',
  'config-lost': 'after quiescence each config field holds the last value the user set (absent faults on its writer)',
  'config-shown': 'after quiescence the store\'s config equals the backend\'s for the fuzzed fields',
  'liveness': 'every operation settles once no call is pending (no queue stalls after a rejected or crashed call)',
} as const;

export type InvariantId = keyof typeof INVARIANTS;

export interface Violation {
  inv: InvariantId;
  msg: string;
  step: number;
}

export type UserOp =
  | { k: 'edit'; n: number }
  | { k: 'elev'; n: number }
  | { k: 'undo' }
  | { k: 'redo' }
  | { k: 'save' }
  | { k: 'autosave' }
  | { k: 'saveAs'; n: number }
  | { k: 'load'; path: string }
  | { k: 'close' }
  | { k: 'create'; n: number; deferred: boolean }
  | { k: 'importNew'; n: number }
  | { k: 'refresh' }
  | { k: 'grid' }
  | { k: 'geom' }
  | { k: 'toggleGrid' }
  | { k: 'panel'; v: number }
  | { k: 'patch'; field: 'font_size' | 'conservative_tank_gas'; v: number }
  | { k: 'gas'; v: number }
  | { k: 'gasFlush' }
  | { k: 'tick' }
  | { k: 'reloadConfig' };

export type Action = { t: 'op'; op: UserOp } | { t: 'run' | 'exec' | 'fault'; r: number };

export interface RunResult {
  violations: Violation[];
  trace: string[];
  unmodelled: string[];
  steps: number;
  opsStarted: number;
  opsSkipped: number;
}

/** mulberry32: small, fast, good enough to spread choices. */
export function rng(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const PATHS = ['/caves/CaveA.stalx', '/caves/CaveA copy.stalx', '/caves/CaveB.stalx'];

/** The action list a seed stands for. */
export function generate(seed: number, length?: number): Action[] {
  const next = rng(seed);
  const n = length ?? 40 + Math.floor(next() * 50);
  const faults = next() < 0.5;
  // Three decimals, exactly what `describeAction` prints: a printed trace replays as generated.
  const choice = () => Math.floor(next() * 1000) / 1000;
  const actions: Action[] = [];
  const weighted: Array<[number, () => Action]> = [
    [34, () => ({ t: 'run', r: choice() })],
    [8, () => ({ t: 'exec', r: choice() })],
    [faults ? 3 : 0, () => ({ t: 'fault', r: choice() })],
    [7, () => ({ t: 'op', op: { k: 'edit', n: actions.length } })],
    [3, () => ({ t: 'op', op: { k: 'elev', n: actions.length } })],
    [4, () => ({ t: 'op', op: { k: 'undo' } })],
    [2, () => ({ t: 'op', op: { k: 'redo' } })],
    [3, () => ({ t: 'op', op: { k: 'save' } })],
    [3, () => ({ t: 'op', op: { k: 'autosave' } })],
    [2, () => ({ t: 'op', op: { k: 'saveAs', n: actions.length } })],
    [5, () => ({ t: 'op', op: { k: 'load', path: PATHS[Math.floor(next() * PATHS.length)]! } })],
    [2, () => ({ t: 'op', op: { k: 'close' } })],
    [2, () => ({ t: 'op', op: { k: 'create', n: actions.length, deferred: next() < 0.5 } })],
    [2, () => ({ t: 'op', op: { k: 'importNew', n: actions.length } })],
    [2, () => ({ t: 'op', op: { k: 'refresh' } })],
    [5, () => ({ t: 'op', op: { k: 'grid' } })],
    [2, () => ({ t: 'op', op: { k: 'geom' } })],
    [3, () => ({ t: 'op', op: { k: 'toggleGrid' } })],
    [2, () => ({ t: 'op', op: { k: 'panel', v: 200 + Math.floor(next() * 200) } })],
    [2, () => ({ t: 'op', op: { k: 'patch', field: next() < 0.5 ? 'font_size' : 'conservative_tank_gas', v: Math.floor(next() * 1000) } })],
    [2, () => ({ t: 'op', op: { k: 'gas', v: 5 + Math.floor(next() * 20) } })],
    [1, () => ({ t: 'op', op: { k: 'gasFlush' } })],
    [2, () => ({ t: 'op', op: { k: 'tick' } })],
    [1, () => ({ t: 'op', op: { k: 'reloadConfig' } })],
  ];
  const total = weighted.reduce((s, [w]) => s + w, 0);
  // The session starts with CaveA open, as most sessions do.
  actions.push({ t: 'op', op: { k: 'load', path: PATHS[0]! } });
  while (actions.length < n) {
    let roll = next() * total;
    for (const [w, make] of weighted) {
      roll -= w;
      if (roll < 0) {
        actions.push(make());
        break;
      }
    }
  }
  return actions;
}

export function describeAction(a: Action): string {
  if (a.t !== 'op') return `${a.t}(${a.r.toFixed(3)})`;
  const { k, ...rest } = a.op;
  const params = Object.values(rest);
  return params.length ? `${k}(${params.map((p) => JSON.stringify(p)).join(', ')})` : k;
}

/** The inverse of {@link describeAction} over a whole list: how a pinned regression trace is written. */
export function parseActions(text: string): Action[] {
  const actions: Action[] = [];
  for (const token of text.match(/[a-zA-Z]+(\([^)]*\))?/g) ?? []) {
    const [, k = '', inner] = /^([a-zA-Z]+)(?:\((.*)\))?$/.exec(token) ?? [];
    const params = inner ? (JSON.parse(`[${inner}]`) as unknown[]) : [];
    if (k === 'run' || k === 'exec' || k === 'fault') {
      actions.push({ t: k, r: Number(params[0]) });
      continue;
    }
    const names: Record<string, string[]> = {
      edit: ['n'], elev: ['n'], saveAs: ['n'], importNew: ['n'], load: ['path'], create: ['n', 'deferred'], panel: ['v'],
      patch: ['field', 'v'], gas: ['v'],
    };
    const op: Record<string, unknown> = { k };
    (names[k] ?? []).forEach((name, i) => {
      op[name] = params[i];
    });
    actions.push({ t: 'op', op: op as UserOp });
  }
  return actions;
}

/** The store modules a run drives: imported fresh for every run (`vi.resetModules`). */
async function loadModules() {
  const [project, ui, gas, session] = await Promise.all([
    import('../../store/projectStore'),
    import('../../store/uiStore'),
    import('../../utils/gasConfigDisk'),
    import('../../store/projectSession'),
    // The stores import these lazily; loaded here so their `await import` resolves
    // within a tick or two, inside the settle window below.
    import('../../utils/memoryProbe'),
    import('../../utils/loadProjectBinary'),
  ]);
  return { project: project.useProjectStore, ui: ui.useUIStore, gas, session };
}

type Modules = Awaited<ReturnType<typeof loadModules>>;

const immediate = (globalThis as unknown as { setImmediate: (cb: () => void) => void }).setImmediate;

/** Let every continuation the last step released run, until no new call appears. */
async function settle(sched: IpcScheduler): Promise<void> {
  let last = sched.sent;
  let stable = 0;
  for (let i = 0; i < 400 && stable < 4; i += 1) {
    await new Promise<void>((resolve) => immediate(resolve));
    if (sched.sent === last) stable += 1;
    else {
      stable = 0;
      last = sched.sent;
    }
  }
}

interface DisplayWrite {
  id: number;
  key: string;
  value: string;
  madeGen: number | null;
  faulted: boolean;
}

interface ConfigIntent {
  value: unknown;
  writer: 'patch' | 'gas' | 'panel';
}

/** Where the test file's `@tauri-apps/api/core` mock forwards `invoke`: a global, because
 *  `vi.resetModules` gives every run its own instance of this module. */
const bridge = globalThis as unknown as { __fzIntInvoke?: IpcScheduler['invoke'] };

/**
 * Switches for the shapes of the OPEN findings (report.json), so a run can leave a
 * known bug out and keep looking for new ones. All true = the full search space.
 */
export interface RunOptions {
  /** FZ-FZ-INT-1: start a project switch (load, create, close) while another is in flight. */
  overlapSwitches: boolean;
  /** FZ-FZ-INT-4: start a switch while a save is in flight, or a save while a switch is. */
  saveAcrossSwitch: boolean;
  /** FZ-FZ-INT-3 (fixed): patch a [gas] field from Settings while the Plan-mode gas writer writes another. */
  gasFieldPatches: boolean;
  /** FZ-FZ-INT-6: count a display write made in one session that lands after a reload of its file. */
  crossSessionDisplay: boolean;
  /** FZ-FZ-INT-7: issue an edit (or undo/redo) while a project switch is in flight. */
  editAcrossSwitch: boolean;
}

export const ALL_SHAPES: RunOptions = {
  overlapSwitches: true,
  saveAcrossSwitch: true,
  gasFieldPatches: true,
  crossSessionDisplay: true,
  editAcrossSwitch: true,
};

/**
 * What CI runs: every shape except those of the open findings. FZ-FZ-INT-1, -4, -6
 * and -7 are fixed (their shapes are on); FZ-FZ-INT-3 is still open.
 */
export const KNOWN_OPEN_EXCLUDED: RunOptions = {
  overlapSwitches: true,
  saveAcrossSwitch: true,
  gasFieldPatches: true,
  crossSessionDisplay: true,
  editAcrossSwitch: true,
};

export const DEFAULT_OPTIONS = ALL_SHAPES;

export async function runActions(actions: Action[], options: RunOptions = DEFAULT_OPTIONS): Promise<RunResult> {
  vi.resetModules();
  const backend = new BackendModel();
  const sched = new IpcScheduler(backend);
  bridge.__fzIntInvoke = sched.invoke;
  // Silenced before the stores load: some bind `console` methods when they evaluate.
  const consoleSpies = (['error', 'warn', 'info', 'debug', 'log'] as const).map((level) =>
    vi.spyOn(console, level).mockImplementation(() => {}),
  );
  const m: Modules = await loadModules().catch((e: unknown) => {
    for (const spy of consoleSpies) spy.mockRestore();
    throw e;
  });
  const violations: Violation[] = [];
  const writes: DisplayWrite[] = [];
  const edits = new Map<
    string,
    { madeGen: number | null; duringSwitch?: boolean; outcome?: { success: boolean; code?: string | null } }
  >();
  const configIntent = new Map<string, ConfigIntent>();
  /** `sent`: the scheduler's call count when the op started (its own calls come after). */
  const ops: Array<{ label: string; settled: boolean; sent: number }> = [];
  const SWITCHES = new Set(['load', 'create', 'close', 'importNew']);
  const SAVES = new Set(['save', 'autosave', 'saveAs']);
  const EDITS = new Set(['edit', 'elev', 'undo', 'redo']);
  const inFlight = (labels: Set<string>) => ops.some((o) => !o.settled && labels.has(o.label));
  /** The IPC whose answer decides each switch. */
  const SWITCH_IPC: Record<string, string> = {
    load: 'load_project_full_binary', create: 'create_project_full', importNew: 'create_project_full',
    close: 'close_project',
  };
  /** The scheduler's call count when the last switch op settled. */
  let switchSettledSent = 0;
  /**
   * [#576 FZ-FZ-INT-7] Whether a switch requested so far is still undecided: more than
   * one in flight, or the one in flight not answered yet. An edit requested then is
   * refused by design (store/switchGate.ts). Once the only switch in flight has been
   * answered, the store shows what it left open, and an edit made there is not.
   * Switches run one after another (utils/projectTransition.ts), so a switch's own IPC
   * is sent after it started AND after the switch before it settled.
   */
  const switchUndecided = (): boolean => {
    const running = ops.filter((o) => !o.settled && SWITCHES.has(o.label));
    if (running.length !== 1) return running.length > 1;
    const { label, sent } = running[0]!;
    const after = Math.max(sent, switchSettledSent);
    const answered = new RegExp(`^run #(\\d+) ${SWITCH_IPC[label]!}\\b`);
    return !sched.trace.some((line) => Number(answered.exec(line)?.[1] ?? 0) > after);
  };
  let step = 0;
  let opsStarted = 0;
  let opsSkipped = 0;
  const violate = (inv: InvariantId, msg: string) => {
    if (!violations.some((v) => v.inv === inv && v.msg === msg)) violations.push({ inv, msg, step });
  };
  backend.writeIdOf = (key, value) => {
    for (let i = writes.length - 1; i >= 0; i -= 1) {
      const w = writes[i]!;
      if (w.key === key && w.value === value) return w.id;
    }
    return null;
  };
  const genShown = (): number | null => {
    const loc = m.project.getState().project?.location;
    return typeof loc === 'string' && loc.startsWith('gen-') ? Number(loc.slice(4)) : null;
  };
  const track = (label: string, p: Promise<unknown>, after?: (value: unknown) => void) => {
    const entry = { label, settled: false, sent: sched.sent };
    ops.push(entry);
    opsStarted += 1;
    const settled = () => {
      entry.settled = true;
      if (SWITCHES.has(label)) switchSettledSent = sched.sent;
    };
    p.then(
      (value) => {
        settled();
        after?.(value);
      },
      settled,
    );
  };
  const checkIdentity = () => {
    const project = m.project.getState().project;
    if (!project) return;
    const g = genShown();
    const gen = g === null ? undefined : backend.genOf(g);
    if (!gen) {
      violate('identity', `store shows a project with no generation marker (${String(project.location)})`);
      return;
    }
    if (project.created_at !== gen.createdAt || !gen.paths.includes(project.file_path ?? null)) {
      violate(
        'identity',
        `store shows gen-${g} data (created_at ${gen.createdAt}, files ${JSON.stringify(gen.paths)}) as created_at ` +
          `${project.created_at} at ${String(project.file_path)}`,
      );
    }
  };

  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'setInterval', 'clearInterval'] });
  try {
    // App start: the config is loaded before any project opens.
    const configLoaded = m.project.getState().loadConfig();
    await settle(sched);
    while (sched.runOldest()) await settle(sched);
    await configLoaded;

    for (const action of actions) {
      step += 1;
      if (action.t === 'run') sched.run(action.r);
      else if (action.t === 'exec') sched.exec(action.r);
      else if (action.t === 'fault') {
        const before = sched.faulted.length;
        sched.fault(action.r);
        const hit = sched.faulted[before];
        if (hit?.command === 'save_display_setting') {
          const id = backend.writeIdOf(String(hit.args?.key), String(hit.args?.value));
          if (id !== null) writes[id]!.faulted = true;
        }
      } else if (action.t === 'op') {
        const started = startOp(action.op);
        if (started) sched.trace.push(`op ${describeAction(action)}`);
        else {
          opsSkipped += 1;
          sched.trace.push(`op ${describeAction(action)} (skipped: precondition)`);
        }
      }
      await settle(sched);
      checkIdentity();
    }

    // Drain: answer everything in send order, let timers run, until nothing moves.
    for (let round = 0; round < 2000; round += 1) {
      await settle(sched);
      checkIdentity();
      if (sched.runOldest()) continue;
      if (vi.getTimerCount() > 0) {
        vi.advanceTimersByTime(1000);
        continue;
      }
      break;
    }
    await settle(sched);
    checkQuiescent();
  } finally {
    vi.useRealTimers();
    for (const spy of consoleSpies) spy.mockRestore();
  }
  return {
    violations,
    trace: sched.trace,
    unmodelled: [...sched.unmodelled],
    steps: step,
    opsStarted,
    opsSkipped,
  };

  /** Start a user operation; false when its UI precondition does not hold (skipped). */
  function startOp(op: UserOp): boolean {
    const P = m.project.getState();
    const U = m.ui.getState();
    const open = P.project !== null && !P.loading.isLoading;
    const gen = genShown();
    if (SWITCHES.has(op.k) && !options.overlapSwitches && inFlight(SWITCHES)) return false;
    if (SWITCHES.has(op.k) && !options.saveAcrossSwitch && inFlight(SAVES)) return false;
    if (SAVES.has(op.k) && !options.saveAcrossSwitch && inFlight(SWITCHES)) return false;
    if (EDITS.has(op.k) && !options.editAcrossSwitch && inFlight(SWITCHES)) return false;
    const display = (key: string, value: string, apply: () => void): boolean => {
      if (!open) return false;
      // Only a Survey-mode change persists (uiStore setGridSpacing / toggleDisplay).
      if (key !== 'geometryMode' && U.appMode !== 'survey') return false;
      writes.push({ id: writes.length, key, value, madeGen: gen, faulted: false });
      apply();
      return true;
    };
    switch (op.k) {
      case 'edit': {
        if (!open) return false;
        const marker = `e${op.n}`;
        const entry: { madeGen: number | null; duringSwitch: boolean; outcome?: { success: boolean; code?: string | null } } =
          { madeGen: gen, duringSwitch: switchUndecided() };
        edits.set(marker, entry);
        track(op.k, P.createSection({
            name: marker, surveyor: null, date: null, color: '#123456', direction: null, quality: 1, shots: [],
          }), (o) => {
          entry.outcome = o as { success: boolean; code?: string | null };
        });
        return true;
      }
      case 'elev': {
        if (!open) return false;
        const value = 100 + op.n;
        const entry: { madeGen: number | null; duringSwitch: boolean; outcome?: { success: boolean; code?: string | null } } =
          { madeGen: gen, duringSwitch: switchUndecided() };
        edits.set(`elev:${value}`, entry);
        track(op.k, P.setSurfaceElevation(value), (o) => {
          entry.outcome = o as { success: boolean; code?: string | null };
        });
        return true;
      }
      case 'undo':
      case 'redo':
        if (!open) return false;
        track(op.k, op.k === 'undo' ? P.undo() : P.redo());
        return true;
      case 'save':
        if (!open) return false;
        track(op.k, P.saveProject());
        return true;
      case 'autosave':
        if (!open || !P.isDirty) return false;
        track(op.k, P.saveProject(undefined, true));
        return true;
      case 'saveAs':
        if (!open) return false;
        track(op.k, P.saveProject(`/caves/saved/SaveAs${op.n}.stalx`));
        return true;
      case 'load':
        track(op.k, P.loadProject(op.path));
        return true;
      case 'close':
        if (P.project === null) return false;
        track(op.k, P.closeProject());
        return true;
      case 'create':
        track(
          op.k,
          P.createProject(
            `New${op.n}`,
            undefined,
            op.deferred ? { deferInitialSave: true } : { savePath: `/caves/new/New${op.n}.stalx` },
          ),
        );
        return true;
      case 'importNew':
        // The DMP wizard's "import as a new project": a deferred create, the import
        // into it (whose command makes the first save), then saveAfterImport.
        track(
          op.k,
          (async () => {
            const created = await P.createProject(`Imp${op.n}`, undefined, { deferInitialSave: true });
            if (!created.success) return;
            m.ui.getState().setImportContext('new-project');
            const marker = `i${op.n}`;
            edits.set(marker, { madeGen: genShown() });
            const imported = await m.project.getState().importSections([
              {
                section_index: 0, name: marker, surveyor: null, date: null, color: '#654321', direction: null,
                quality: 1, shots: [], placement: { type: 'origin' }, station_comments: [], reverse: false,
              },
            ]);
            if (imported.success) await m.project.getState().saveAfterImport();
            m.ui.getState().setImportContext(null);
          })(),
        );
        return true;
      case 'refresh':
        if (P.project === null) return false;
        track(op.k, P.refreshProjectState());
        return true;
      case 'grid': {
        const value = 11 + writes.length;
        return display('gridSpacing', String(value), () => U.setGridSpacing(value));
      }
      case 'geom': {
        const next = U.geometryMode === 'measured' ? 'closureAdjusted' : 'measured';
        return display('geometryMode', next, () => U.setGeometryMode(next));
      }
      case 'toggleGrid':
        return display('showGrid', String(!U.showGrid), () => U.toggleDisplay('showGrid'));
      case 'panel':
        configIntent.set('panels.sections_panel_width', { value: op.v, writer: 'panel' });
        track(op.k, U.savePanelSizes('sections', op.v));
        return true;
      case 'patch': {
        if (op.field === 'conservative_tank_gas' && !options.gasFieldPatches) return false;
        const path = op.field === 'font_size' ? ['display', 'font_size'] : ['gas', 'conservative_tank_gas'];
        const value = op.field === 'font_size' ? 8 + (op.v % 12) : op.v % 2 === 0;
        configIntent.set(path.join('.'), { value, writer: 'patch' });
        track(op.k, P.patchConfig([{ path, value }]));
        return true;
      }
      case 'gas':
        configIntent.set('gas.swim_speed_m_per_min', { value: op.v, writer: 'gas' });
        m.gas.queueGasConfigWrite({ swim_speed_m_per_min: op.v });
        return true;
      case 'gasFlush':
        track(op.k, m.gas.flushGasConfigWrite());
        return true;
      case 'tick':
        vi.advanceTimersByTime(500);
        return true;
      case 'reloadConfig':
        track(op.k, P.reloadConfigInOrder());
        return true;
    }
  }

  function checkQuiescent(): void {
    const P = m.project.getState();
    const U = m.ui.getState();
    for (const op of ops) if (!op.settled) violate('liveness', `operation '${op.label}' never settled`);
    if (sched.pending.length > 0) violate('liveness', `${sched.pending.length} call(s) still pending`);

    // Where each display write landed.
    for (const landing of backend.landed) {
      const w = writes[landing.writeId];
      if (!w || w.key !== 'gridSpacing' || w.madeGen === null) continue; // unique values only
      const made = backend.genOf(w.madeGen)!;
      if (!storageOf(made, landing.storage, w.madeGen)) {
        violate(
          'display-target',
          `gridSpacing=${w.value} made in gen-${w.madeGen} (${JSON.stringify(made.paths)}) landed in ` +
            `${JSON.stringify(landing.storage)} while gen-${landing.gen} was open`,
        );
      }
    }
    // Which project each edit reached.
    for (const applied of backend.editsApplied) {
      const e = edits.get(applied.marker);
      if (!e || e.madeGen === null) continue;
      const made = backend.genOf(e.madeGen)!;
      const at = backend.genOf(applied.gen)!;
      const same =
        applied.gen === e.madeGen || (at.createdAt === made.createdAt && made.paths.includes(at.paths[0] ?? null));
      if (!same) {
        violate(
          'edit-target',
          `edit ${applied.marker} issued in gen-${e.madeGen} (${JSON.stringify(made.paths)}) was applied to ` +
            `gen-${applied.gen} (${JSON.stringify(at.paths)})`,
        );
      }
    }

    const open = backend.openSummary();
    const shown = P.project;
    if (open === null) {
      if (shown !== null) violate('state', `backend has no project open; store shows gen-${String(genShown())}`);
    } else if (shown === null) {
      violate('state', `backend has gen-${open.gen} (${String(open.filePath)}) open; store shows none`);
    } else {
      const sections = (shown.sections ?? []).map((s) => s.name ?? '');
      const mismatch: string[] = [];
      if (genShown() !== open.gen) mismatch.push(`gen ${String(genShown())} vs ${open.gen}`);
      if ((shown.file_path ?? null) !== open.filePath) mismatch.push(`file ${String(shown.file_path)} vs ${String(open.filePath)}`);
      // A crashed read leaves what it would have refreshed stale until the next one
      // answers (the store logs it; the internal-error banner is the backend's).
      const stale = (command: string) => sched.staleAfterFault(command);
      // can_undo and can_redo are one refresh (`refreshUndoState`, a Promise.all): either crashing keeps both.
      const undoStale = stale('can_undo') || stale('can_redo');
      if (!stale('get_project_state_binary')) {
        if (JSON.stringify(sections) !== JSON.stringify(open.sections))
          mismatch.push(`sections ${JSON.stringify(sections)} vs ${JSON.stringify(open.sections)}`);
        if ((shown.surface_elevation ?? null) !== open.elevation)
          mismatch.push(`elevation ${String(shown.surface_elevation)} vs ${String(open.elevation)}`);
      }
      if (!undoStale && P.canUndo !== open.canUndo) mismatch.push(`canUndo ${P.canUndo} vs ${open.canUndo}`);
      if (!undoStale && P.canRedo !== open.canRedo) mismatch.push(`canRedo ${P.canRedo} vs ${open.canRedo}`);
      const warnings = P.warnings.map((w) => w.message);
      if (!stale('get_project_warnings') && JSON.stringify(warnings) !== JSON.stringify(open.warnings))
        mismatch.push(`warnings ${JSON.stringify(warnings)} vs ${JSON.stringify(open.warnings)}`);
      if (mismatch.length) violate('state', `store vs backend: ${mismatch.join('; ')}`);

      if (genShown() === open.gen) {
        const ui: Record<string, string> = {
          gridSpacing: String(U.gridSpacing),
          geometryMode: U.geometryMode,
          showGrid: String(U.showGrid),
        };
        for (const [key, value] of Object.entries(ui)) {
          const stored = backend.storedDisplay(key);
          // A failed write leaves the value shown for the session (uiStore persistDisplaySetting logs it).
          if (writes.some((w) => w.key === key && w.faulted && w.madeGen === open.gen)) continue;
          const crossSession = backend.landed.some(
            (l) => l.key === key && l.gen === open.gen && writes[l.writeId]?.madeGen !== open.gen,
          );
          if (crossSession && !options.crossSessionDisplay) continue;
          if (stored !== value) violate('display-shown', `${key}: shown ${value}, stored ${String(stored)} (gen-${open.gen})`);
        }
        if (!P.isDirty && backend.openHasUnsavedChanges()) {
          violate('dirty-flag', `gen-${open.gen} shows clean but its unsaved sections ${JSON.stringify(open.sections)} differ from its file`);
        }
      }
      // The last display write made in the open project, per key, is what it stores.
      const lastByKey = new Map<string, DisplayWrite>();
      for (const w of writes) if (w.madeGen === open.gen) lastByKey.set(w.key, w);
      for (const [key, w] of lastByKey) {
        if (w.faulted) continue;
        const stored = backend.storedDisplay(key);
        if (stored !== w.value) {
          violate('display-lost', `${key}: last write made in gen-${open.gen} was ${w.value}, it stores ${String(stored)}`);
        }
      }
      for (const [marker, e] of edits) {
        if (e.madeGen !== open.gen || !e.outcome || e.outcome.success) continue;
        // Refused here (no code) while a switch was undecided: the switch gate's design.
        if (e.duringSwitch && !e.outcome.code) continue;
        const faulted = sched.faulted.some((f) => f.command === 'create_section' || f.command === 'set_surface_elevation');
        if (!faulted) violate('edit-lost', `edit ${marker} issued in open gen-${open.gen} was refused (${String(e.outcome.code)})`);
      }
    }

    // Config: the last value set per field, on disk and in the store.
    // The Plan-mode gas writer sends `patch_config` too (FZ-INT-3-D1): a faulted patch
    // excuses the writer whose field it carried.
    const gasWriterPatch = (args: Record<string, unknown> | undefined) =>
      ((args?.patches as Array<{ path: string[] }> | undefined) ?? []).some(
        (p) => p.path.join('.') === 'gas.swim_speed_m_per_min',
      );
    const faultedWriters = new Set(
      sched.faulted.map((f) =>
        f.command === 'patch_config' && gasWriterPatch(f.args)
          ? 'gas'
          : ({ patch_config: 'patch', update_gas_config: 'gas', update_panel_sizes: 'panel' })[f.command as 'patch_config'],
      ),
    );
    const read = (root: unknown, path: string) =>
      path.split('.').reduce<unknown>((at, part) => (at as Record<string, unknown> | undefined)?.[part], root);
    for (const [path, intent] of configIntent) {
      const onDisk = read(backend.config, path);
      if (!faultedWriters.has(intent.writer) && onDisk !== intent.value) {
        violate('config-lost', `${path}: last set ${JSON.stringify(intent.value)} (${intent.writer}), backend holds ${JSON.stringify(onDisk)}`);
      }
      const inStore = read(P.config, path);
      // The panel sizes live in uiStore; projectStore's config copy catches up on the next re-read.
      if (intent.writer === 'panel') continue;
      if (!sched.faulted.some((f) => f.command === 'get_config') && inStore !== onDisk) {
        violate('config-shown', `${path}: store ${JSON.stringify(inStore)}, backend ${JSON.stringify(onDisk)}`);
      }
    }
  }

  function storageOf(made: { paths: Array<string | null> }, storage: Storage, madeGen: number): boolean {
    return 'cacheOf' in storage ? storage.cacheOf === madeGen : made.paths.includes(storage.file);
  }
}

/** Whether `actions` still fails with `inv`. */
async function failsWith(actions: Action[], inv: InvariantId, options: RunOptions): Promise<boolean> {
  const result = await runActions(actions, options);
  return result.violations.some((v) => v.inv === inv);
}

/**
 * Shrink a failing action list to a smaller one failing with the same invariant:
 * remove chunks (halves, quarters, ... single actions) while it still fails, then
 * turn each remaining schedule choice into 0 (oldest call first) where that keeps
 * it failing. Bounded by `budget` re-runs.
 */
export async function shrink(
  actions: Action[],
  inv: InvariantId,
  options: RunOptions = DEFAULT_OPTIONS,
  budget = 400,
): Promise<Action[]> {
  let best = actions;
  let runs = 0;
  for (let chunk = Math.max(1, Math.floor(best.length / 2)); chunk >= 1; chunk = Math.floor(chunk / 2)) {
    for (let start = 1; start < best.length && runs < budget; ) {
      const candidate = [...best.slice(0, start), ...best.slice(start + chunk)];
      runs += 1;
      if (await failsWith(candidate, inv, options)) best = candidate;
      else start += chunk;
    }
    if (chunk === 1) break;
  }
  for (let i = 0; i < best.length && runs < budget; i += 1) {
    const a = best[i]!;
    if (a.t === 'op' || a.r === 0) continue;
    const candidate = [...best];
    candidate[i] = { ...a, r: 0 };
    runs += 1;
    if (await failsWith(candidate, inv, options)) best = candidate;
  }
  return best;
}
