/**
 * [#576 FZ-INT] The interleaving fuzzer's reference backend: a small in-memory model
 * of the commands the stores send, applied in the order the scheduler executes
 * them, with the real backend's guards mirrored (file:line in each arm).
 *
 * It is the oracle's source of truth, so it also records what each write was made
 * for and where it landed (`landed`), and keeps the history of every project
 * "generation" (one per slot commit: a load, a create or a close) so an oracle can
 * tell which project the store is showing. A generation's marker rides in the
 * project's `location` field, which no store code reads or writes.
 *
 * Everything is synthetic.
 */
import { projectStateEnvelope } from '../projectStateEnvelope';
import { defaultConfig } from '../../types/ipc/defaultConfig';
import type { Config } from '../../types/ipc/Config';

/** A command error as the IPC rejects with it (`CommandError`). */
export interface ModelError {
  code: string;
  message: string;
}

export type Outcome = { ok: true; value: unknown } | { ok: false; error: unknown };

interface SectionRec {
  id: number;
  name: string;
}

interface Content {
  sections: SectionRec[];
  surfaceElevation: number | null;
}

/** A stored display value and the write that put it there (null: none). */
export interface StoredDisplay {
  value: string;
  writeId: number | null;
}

interface FileRec {
  createdAt: string;
  name: string;
  content: Content;
  display: Map<string, StoredDisplay>;
}

export interface Generation {
  id: number;
  /** null: the close that left no project open. */
  createdAt: string | null;
  name: string | null;
  /** Every file this session's project has been at (a Save As adds one). */
  paths: Array<string | null>;
}

interface OpenProject {
  gen: number;
  createdAt: string;
  name: string;
  filePath: string | null;
  /** No database yet: a deferred create, until its first save. */
  deferred: boolean;
  content: Content;
  nextSectionId: number;
  undo: Content[];
  redo: Content[];
  /** The display settings of a project with no database (`display_settings_cache`). */
  cache: Map<string, StoredDisplay>;
  /** `opened_from` (display_settings.rs note_open_project_moved). */
  openedFrom: Array<string | null>;
  /** What `get_project_warnings` answers: a loaded project carries one naming its generation. */
  warnings: Array<Record<string, unknown>>;
}

/** Where a display write was stored: a file, or a generation's in-memory cache. */
export type Storage = { file: string } | { cacheOf: number };

export interface Landing {
  writeId: number;
  key: string;
  value: string;
  storage: Storage;
  /** The generation open when it was applied. */
  gen: number;
}

export interface EditLanding {
  marker: string;
  gen: number;
}

const DISPLAY_DEFAULTS: Record<string, string> = {
  showGrid: 'false',
  showStationLabels: 'false',
  gridSpacing: '10',
  geometryMode: 'measured',
};

/** The commands `execute` checks `expectedProject` on (save_project + the modelled mutations). */
const CHECKS_PROJECT = new Set([
  'save_project', 'create_section', 'set_surface_elevation', 'import_sections', 'undo', 'redo',
]);

const err = (code: string, message: string): Outcome => ({ ok: false, error: { code, message } satisfies ModelError });
const ok = (value: unknown = null): Outcome => ({ ok: true, value });

function cloneContent(c: Content): Content {
  return { sections: c.sections.map((s) => ({ ...s })), surfaceElevation: c.surfaceElevation };
}

function sameContent(a: Content, b: Content): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

/** The full display-settings answer of a load: stored values over the defaults (loading.rs `from_stored`). */
function displayAnswer(stored: Map<string, StoredDisplay>): Record<string, unknown> {
  const get = (key: string) => stored.get(key)?.value ?? DISPLAY_DEFAULTS[key];
  return {
    showStations: true, showShots: true, showSectionNames: true, showStationLabels: get('showStationLabels') === 'true',
    showShotData: false, showReferenceShots: true, showJumps: true, showZeroDatum: true,
    showGrid: get('showGrid') === 'true', showSatellite: true, showDepthGradient: false,
    showStationComments: false, showShotComments: false,
    gridSpacing: Number(get('gridSpacing')), geometryMode: get('geometryMode'),
  };
}

/** A load's cancellation token (`AppState::start_new_load`). */
interface LoadToken {
  cancelled: boolean;
}

/** The project a save or an edit was made for (`expectedProject`, expected_project.rs `ProjectIdentity`). */
interface MadeFor {
  createdAt: string;
  filePath: string | null;
}

export class BackendModel {
  readonly files = new Map<string, FileRec>();
  readonly gens: Generation[] = [{ id: 0, createdAt: null, name: null, paths: [] }];
  open: OpenProject | null = null;
  config: Config = structuredClone(defaultConfig);
  readonly landed: Landing[] = [];
  readonly refused: Array<{ writeId: number; key: string; value: string }> = [];
  readonly editsApplied: EditLanding[] = [];
  /** Which display write a (key, value) belongs to: the harness registers each write it makes. */
  writeIdOf: (key: string, value: string) => number | null = () => null;
  private activeLoad: LoadToken | null = null;
  private loadingId = 0;
  private createdCounter = 0;

  constructor() {
    const file = (createdAt: string, name: string): FileRec => ({
      createdAt,
      name,
      content: { sections: [{ id: 1, name: `${name}-s1` }], surfaceElevation: null },
      display: new Map(),
    });
    // Two copies of one project share created_at (#576 NF-W8FE-3).
    this.files.set('/caves/CaveA.stalx', file('2026-01-01T10:00:00Z', 'CaveA'));
    this.files.set('/caves/CaveA copy.stalx', file('2026-01-01T10:00:00Z', 'CaveA'));
    this.files.set('/caves/CaveB.stalx', file('2026-02-02T10:00:00Z', 'CaveB'));
  }

  get currentGen(): number {
    return this.open?.gen ?? this.gens[this.gens.length - 1]!.id;
  }

  genOf(id: number): Generation | undefined {
    return this.gens[id];
  }

  /** Effects a command has when it is SENT (the pool starts it at once). */
  onSend(command: string, args: Record<string, unknown> | undefined): { token?: LoadToken; early?: Outcome } {
    switch (command) {
      case 'cancel_loading':
        // app_state.rs cancel_loading: flags the active load, takes no lock.
        if (this.activeLoad) this.activeLoad.cancelled = true;
        return { early: ok(null) };
      case 'load_project_full_binary': {
        // project_load.rs:389-400: ALREADY_OPEN, then start_new_load, before the file is read.
        const path = String(args?.path);
        if (this.open && this.open.filePath === path) return { early: err('ALREADY_OPEN', 'already open') };
        return { token: this.startLoad() };
      }
      case 'create_project_full':
        // project_create.rs: a create starts a load too (its cancel token).
        return { token: this.startLoad() };
      default:
        return {};
    }
  }

  private startLoad(): LoadToken {
    if (this.activeLoad) this.activeLoad.cancelled = true;
    const token = { cancelled: false };
    this.activeLoad = token;
    return token;
  }

  private finishLoad(token: LoadToken): void {
    if (this.activeLoad === token) this.activeLoad = null;
  }

  /**
   * expected_project.rs:90-108 `names_open_project`, the rule a display-setting write
   * (display_settings.rs:86) and every save and edit are decided by: both none, or the
   * same created_at and the file the session was committed from, is at, or was moved
   * off by a Save As (`opened_from`, session_commit.rs:198, display_settings.rs:109).
   */
  private namesOpenProject(madeFor: MadeFor | null): boolean {
    const open = this.open;
    if (madeFor === null || open === null) return madeFor === null && open === null;
    return (
      madeFor.createdAt === open.createdAt &&
      (madeFor.filePath === open.filePath || open.openedFrom.includes(madeFor.filePath))
    );
  }

  /**
   * expected_project.rs:115-134 `refuse_other_project`: a save or a survey edit whose
   * `expectedProject` does not name the open project is refused with PROJECT_CHANGED
   * and nothing is written. A missing argument is `None` on the wire, "made with no
   * project open" (pool.rs:85-97 `run_mutation`, project.rs:70-76 `save_project`).
   */
  private refuseOtherProject(args: Record<string, unknown> | undefined, what: string): Outcome | null {
    const madeFor = (args?.expectedProject as MadeFor | null | undefined) ?? null;
    if (this.namesOpenProject(madeFor)) return null;
    return err('PROJECT_CHANGED', `${what} was not applied: another project is open now.`);
  }

  private commitGen(open: OpenProject | null): void {
    const id = this.gens.length;
    this.gens.push({
      id,
      createdAt: open?.createdAt ?? null,
      name: open?.name ?? null,
      paths: open ? [open.filePath] : [],
    });
    if (open) open.gen = id;
    this.open = open;
  }

  /** The command's effect and answer, at the moment the scheduler runs it. */
  execute(command: string, args: Record<string, unknown> | undefined, token?: LoadToken): Outcome {
    // Every save (commands.rs:604-610, in `execute` under command_lock) and every
    // survey mutation (app_state.rs:672-676 `guard_metadata_mutation`, the first gate
    // of `guard_mutation`, e.g. undo.rs:69 / :621, ahead of NOTHING_TO_UNDO) first
    // checks the project it was made for.
    if (CHECKS_PROJECT.has(command)) {
      const refused = this.refuseOtherProject(args, command);
      if (refused) return refused;
    }
    switch (command) {
      case 'load_project_full_binary':
        return this.load(String(args?.path), token!);
      case 'create_project_full':
        return this.create(args ?? {}, token!);
      case 'close_project':
        this.commitGen(null);
        return ok({ warnings: [] });
      case 'save_project':
        return this.save(args ?? {});
      case 'get_project_warnings':
        return ok(this.open?.warnings ?? []);
      case 'get_project_state_binary':
        return ok(this.open ? this.envelope(this.open) : new ArrayBuffer(0));
      case 'can_undo':
        return ok((this.open?.undo.length ?? 0) > 0);
      case 'can_redo':
        return ok((this.open?.redo.length ?? 0) > 0);
      case 'create_section':
      case 'set_surface_elevation':
        return this.edit(command, args ?? {});
      case 'import_sections': {
        // commands/import.rs: the import, then finalize_deferred_import - a deferred
        // project's first save, in the same command (and the same `expectedProject`
        // scope, import.rs:266, so that save passes the check the import passed).
        const first = (args?.sections as Array<{ name: string | null }> | undefined)?.[0];
        const imported = this.edit('create_section', { name: first?.name ?? 'imported' });
        if (!imported.ok) return imported;
        if (this.open?.deferred) this.save({});
        return ok({ warnings: [], remote_link: null, import_receipt: null });
      }
      case 'undo':
      case 'redo':
        return this.history(command);
      case 'save_display_setting':
        return this.saveDisplaySetting(args ?? {});
      case 'get_config':
        return ok(structuredClone(this.config));
      case 'patch_config': {
        for (const patch of (args?.patches as Array<{ path: string[]; value: unknown }>) ?? []) {
          let at: Record<string, unknown> = this.config as unknown as Record<string, unknown>;
          for (const part of patch.path.slice(0, -1)) at = at[part] as Record<string, unknown>;
          at[patch.path[patch.path.length - 1]!] = patch.value;
        }
        return ok({ keychain_residue: null, persistent_residue: null });
      }
      case 'update_gas_config':
        // config.rs update_gas_config: the whole [gas] block.
        this.config.gas = structuredClone(args?.gas as Config['gas']);
        return ok(null);
      case 'update_panel_sizes': {
        const p = this.config.panels as unknown as Record<string, unknown>;
        if (args?.sectionsPanelWidth != null) p.sections_panel_width = args.sectionsPanelWidth;
        if (args?.detailPanelWidth != null) p.detail_panel_width = args.detailPanelWidth;
        return ok(null);
      }
      default:
        throw new Error(`backend model: ${command} is not modelled`);
    }
  }

  private load(path: string, token: LoadToken): Outcome {
    // project_load.rs:1015-1021: re-checked under command_lock before the commit.
    if (token.cancelled) {
      this.finishLoad(token);
      return err('CANCELLED', 'Loading cancelled');
    }
    this.finishLoad(token);
    const file = this.files.get(path);
    if (!file) return err('NOT_FOUND', `no such file: ${path}`);
    const content = cloneContent(file.content);
    this.commitGen({
      gen: 0,
      createdAt: file.createdAt,
      name: file.name,
      filePath: path,
      deferred: false,
      content,
      nextSectionId: Math.max(0, ...content.sections.map((s) => s.id)) + 1,
      undo: [],
      redo: [],
      cache: new Map(),
      openedFrom: [],
      warnings: [],
    });
    const open = this.open!;
    open.warnings = [
      {
        sectionId: 1, shotId: 0, shotIndex: 0, code: 'FUZZ_GENERATION', message: `gen-${open.gen}`,
        severity: 'warning', isResolved: false,
      },
    ];
    this.loadingId += 1;
    return ok(
      this.envelope(open, {
        loading_id: this.loadingId,
        display_settings: displayAnswer(file.display),
        warnings: open.warnings,
      }),
    );
  }

  private create(args: Record<string, unknown>, token: LoadToken): Outcome {
    if (token.cancelled) {
      this.finishLoad(token);
      return err('CANCELLED', 'Loading cancelled');
    }
    this.finishLoad(token);
    this.createdCounter += 1;
    const name = String(args.name);
    const createdAt = `2026-03-${String(this.createdCounter).padStart(2, '0')}T10:00:00Z`;
    // A create always reserves its file (project_create.rs "reserved save path").
    const filePath = (args.savePath as string | undefined) ?? `/caves/new/${name}.stalx`;
    const deferred = args.deferInitialSave === true;
    const open: OpenProject = {
      gen: 0, createdAt, name, filePath, deferred,
      content: { sections: [], surfaceElevation: null },
      nextSectionId: 1, undo: [], redo: [], cache: new Map(), openedFrom: [], warnings: [],
    };
    if (!deferred) this.writeFile(filePath, open, new Map());
    this.commitGen(open);
    this.loadingId += 1;
    return ok({ loadingId: this.loadingId, project: this.projectJson(open), displaySettings: displayAnswer(new Map()) });
  }

  private writeFile(path: string, open: OpenProject, display: Map<string, StoredDisplay>): void {
    this.files.set(path, {
      createdAt: open.createdAt,
      name: open.name,
      content: cloneContent(open.content),
      display: new Map(display),
    });
  }

  private save(args: Record<string, unknown>): Outcome {
    const open = this.open;
    if (!open) return err('FAILED', 'No project open');
    const target = (args.path as string | null | undefined) ?? open.filePath;
    if (!target) return err('FAILED', 'No save path');
    if (open.deferred) {
      // The first save of a deferred project: the cache becomes the file's settings.
      this.writeFile(target, open, open.cache);
      open.cache = new Map();
      open.deferred = false;
    } else if (target === open.filePath) {
      this.files.get(target)!.content = cloneContent(open.content);
    } else {
      // Save As: the side tables travel with the project (save_sidecars.rs).
      this.writeFile(target, open, this.files.get(open.filePath!)?.display ?? new Map());
    }
    if (target !== open.filePath) {
      // display_settings.rs note_open_project_moved.
      if (!open.openedFrom.includes(open.filePath)) open.openedFrom.push(open.filePath);
      open.filePath = target;
      this.gens[open.gen]!.paths.push(target);
    }
    return ok({ saved_path: target, unlinked_copy_path: null, warnings: [] });
  }

  private edit(command: string, args: Record<string, unknown>): Outcome {
    const open = this.open;
    if (!open) return err('FAILED', 'No project open');
    open.undo.push(cloneContent(open.content));
    open.redo = [];
    if (command === 'create_section') {
      const name = String(args.name);
      open.content.sections.push({ id: open.nextSectionId, name });
      this.editsApplied.push({ marker: name, gen: open.gen });
      open.nextSectionId += 1;
      return ok({ section_id: open.nextSectionId - 1, warnings: [], remote_link: null });
    }
    open.content.surfaceElevation = (args.elevation as number | null | undefined) ?? null;
    this.editsApplied.push({ marker: `elev:${String(open.content.surfaceElevation)}`, gen: open.gen });
    return ok({ warnings: [], remote_link: null });
  }

  private history(command: 'undo' | 'redo'): Outcome {
    const open = this.open;
    if (!open) return err('FAILED', 'No project open');
    const [from, to] = command === 'undo' ? [open.undo, open.redo] : [open.redo, open.undo];
    const step = from.pop();
    if (!step) return err(command === 'undo' ? 'NOTHING_TO_UNDO' : 'NOTHING_TO_REDO', `nothing to ${command}`);
    to.push(cloneContent(open.content));
    open.content = step;
    return ok({ reversed_steps: [], warnings: [], remote_link: null });
  }

  /** display_settings.rs save_display_setting_for, under command_lock. */
  private saveDisplaySetting(args: Record<string, unknown>): Outcome {
    const key = String(args.key);
    const value = String(args.value);
    const madeFor = (args.madeFor as MadeFor | undefined) ?? null;
    const open = this.open;
    const writeId = this.writeIdOf(key, value) ?? -1;
    if (!this.namesOpenProject(madeFor)) {
      this.refused.push({ writeId, key, value });
      return err('PROJECT_CHANGED', 'The display setting was not saved: another project is open now.');
    }
    if (!open) return ok(null); // no project, no project to write into: not exercised
    const storage: Storage = open.deferred ? { cacheOf: open.gen } : { file: open.filePath! };
    const target = open.deferred ? open.cache : this.files.get(open.filePath!)!.display;
    target.set(key, { value, writeId });
    this.landed.push({ writeId, key, value, storage, gen: open.gen });
    return ok(null);
  }

  /** The stored display value of `key` for the open project, defaults filled in. */
  storedDisplay(key: string): string | null {
    const open = this.open;
    if (!open) return null;
    const stored = open.deferred ? open.cache : this.files.get(open.filePath!)?.display;
    return stored?.get(key)?.value ?? DISPLAY_DEFAULTS[key] ?? null;
  }

  /** Whether the open project holds anything its file does not (an unsaved change). */
  openHasUnsavedChanges(): boolean {
    const open = this.open;
    if (!open) return false;
    const file = open.filePath ? this.files.get(open.filePath) : undefined;
    if (!file || open.deferred) return open.content.sections.length > 0 || open.content.surfaceElevation !== null;
    return !sameContent(file.content, open.content);
  }

  openSummary(): {
    gen: number; createdAt: string; filePath: string | null; sections: string[]; elevation: number | null;
    canUndo: boolean; canRedo: boolean; warnings: string[];
  } | null {
    const open = this.open;
    if (!open) return null;
    return {
      gen: open.gen,
      createdAt: open.createdAt,
      filePath: open.filePath,
      sections: open.content.sections.map((s) => s.name),
      elevation: open.content.surfaceElevation,
      canUndo: open.undo.length > 0,
      canRedo: open.redo.length > 0,
      warnings: open.warnings.map((w) => String(w.message)),
    };
  }

  private projectJson(open: OpenProject): Record<string, unknown> {
    return {
      name: open.name,
      location: `gen-${open.gen}`,
      created_at: open.createdAt,
      modified_at: open.createdAt,
      sections: [],
      station_merges: [],
      zero_datums: [],
      loop_closures: [],
      file_path: open.filePath,
      next_section_id: open.nextSectionId,
      section_offsets: {},
      imported_file_hashes: [],
      imported_section_hashes: [],
      next_closure_id: 1,
      surface_elevation: open.content.surfaceElevation,
    };
  }

  private envelope(open: OpenProject, overrides: Record<string, unknown> = {}): ArrayBuffer {
    return projectStateEnvelope(
      {
        project: {
          name: open.name,
          location: `gen-${open.gen}`,
          created_at: open.createdAt,
          modified_at: open.createdAt,
          file_path: open.filePath,
          next_section_id: open.nextSectionId,
          surface_elevation: open.content.surfaceElevation,
          sections: open.content.sections.map((s) => ({ id: s.id, name: s.name, shots: [] })),
        },
        stations: [],
        stats: {
          section_count: open.content.sections.length, shot_count: 0, total_length: 0, min_depth: 0,
          max_depth: 0, station_count: 0, loop_count: 0, penetration_distance: 0,
        },
        frozen_shots: [],
      },
      overrides,
    );
  }
}
