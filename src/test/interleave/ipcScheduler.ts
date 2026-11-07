/**
 * [#576 FZ-INT] The interleaving fuzzer's IPC fake: every call the stores make
 * through Tauri's `invoke` is held PENDING until the fuzzer's schedule says what
 * happens to it, so the order in which the backend applies calls and the order in
 * which their answers reach the stores are both under the seed's control.
 *
 * A call goes `sent` -> `executed` (the reference backend applied it, see
 * `backendModel.ts`) -> delivered (its promise settles). The schedule may run both
 * steps at once, execute a call and deliver it later (after other calls ran), or
 * FAULT it: reject it unapplied, as a crashed command answers (`INTERNAL_ERROR`,
 * #576 RV5-6) or a transport failure does. A few calls never wait (telemetry, the
 * SpeleoDB status refinements, which this fuzzer does not model): they answer at
 * once with a fixed value. A command neither modelled nor listed there answers
 * `undefined` at once and is recorded in `unmodelled`, so a store that starts
 * sending a new command shows up in the CI test instead of passing unseen.
 */
import type { BackendModel, Outcome } from './backendModel';

/** Answered at once, outside the schedule. */
export const AMBIENT: Record<string, () => unknown> = {
  track_events: () => null,
  flush_telemetry: () => null,
  speleodb_link_status: () => ({
    is_remote: false, edit_enabled: true, has_unpushed_edits: false, unwound: false, permission: null,
    lock_holder: null, remote_state: 'unknown', access_denied: false, server_error: null, exclude_geojson: false,
    undo_mode: 'local', force_editing: false, allow_force_edit: false, project_id: null,
  }),
  speleodb_session: () => ({
    logged_in: false, user_email: null, endpoint: null, keychain_backed: false, storage: null,
    keychain: { available: false, reason: null },
  }),
  list_recent_projects: () => [],
  take_unrecognised_shot_kinds: () => [],
};

/** Scheduled, answered by the fake itself (no backend state behind them). */
const FIXED: Record<string, () => unknown> = {
  check_project_integrity: () => ({ messages: [], save_blocked: false }),
  // commands/project.rs check_create_target: a pool command that only runs the
  // write-path check on the file a create would open (nothing created, cancelled
  // or emitted); every path this model knows is writable, so it answers Ok.
  check_create_target: () => null,
};

/** Commands whose error type is a plain string (config.rs), not a CommandError. */
const STRING_ERRORS = new Set(['patch_config', 'update_gas_config', 'update_panel_sizes', 'get_config']);

export interface PendingCall {
  seq: number;
  command: string;
  args: Record<string, unknown> | undefined;
  phase: 'sent' | 'executed';
  outcome?: Outcome;
  token?: unknown;
  resolve: (value: unknown) => void;
  reject: (error: unknown) => void;
}

export class IpcScheduler {
  readonly pending: PendingCall[] = [];
  readonly unmodelled = new Set<string>();
  readonly faulted: Array<{ command: string; args: Record<string, unknown> | undefined }> = [];
  readonly trace: string[] = [];
  sent = 0;
  /**
   * Per command: its newest-issued call, the generation open when it was issued, and
   * whether it was faulted. The stores apply a read only if no newer one of theirs has
   * landed, so an OLDER answer delivered after the newest one faulted is not a fresh read.
   */
  private readonly lastIssued = new Map<string, { seq: number; gen: number | null; faulted: boolean }>();
  /**
   * [#576 W44-NF3] Each load's random `loadToken`, as the trace shows it: numbered in the
   * order the loads were sent, so a replay of a seed prints the same trace.
   */
  private readonly loadNames = new Map<string, string>();

  constructor(private readonly backend: BackendModel) {}

  /** What the mocked `invoke` does. */
  invoke = (command: string, args?: Record<string, unknown>): Promise<unknown> => {
    const ambient = AMBIENT[command];
    if (ambient) return Promise.resolve(ambient());
    if (!FIXED[command] && !this.isModelled(command)) {
      this.unmodelled.add(command);
      return Promise.resolve(undefined);
    }
    this.sent += 1;
    const loadToken = args?.loadToken;
    if (typeof loadToken === 'string' && !this.loadNames.has(loadToken)) {
      this.loadNames.set(loadToken, `load-${this.loadNames.size + 1}`);
    }
    return new Promise((resolve, reject) => {
      const call: PendingCall = { seq: this.sent, command, args, phase: 'sent', resolve, reject };
      this.lastIssued.set(command, { seq: call.seq, gen: this.backend.open?.gen ?? null, faulted: false });
      const { token, early } = FIXED[command] ? {} : this.backend.onSend(command, args);
      call.token = token;
      if (early) {
        call.phase = 'executed';
        call.outcome = early;
      }
      this.pending.push(call);
    });
  };

  private isModelled(command: string): boolean {
    return MODELLED.has(command);
  }

  private execute(call: PendingCall): void {
    const fixed = FIXED[call.command];
    call.outcome = fixed
      ? { ok: true, value: fixed() }
      : this.backend.execute(call.command, call.args, call.token as never);
    call.phase = 'executed';
  }

  /** Whether `command`'s newest-issued call, issued in the project open now, was faulted. */
  staleAfterFault(command: string): boolean {
    const last = this.lastIssued.get(command);
    return last !== undefined && last.faulted && last.gen === (this.backend.open?.gen ?? null);
  }

  private deliver(call: PendingCall): void {
    this.pending.splice(this.pending.indexOf(call), 1);
    const outcome = call.outcome!;
    if (outcome.ok) call.resolve(outcome.value);
    else call.reject(outcome.error);
  }

  private describe(call: PendingCall): string {
    const loadToken = call.args?.loadToken;
    const shown = typeof loadToken === 'string' ? { ...call.args, loadToken: this.loadNames.get(loadToken) ?? 'load-?' } : call.args;
    const args = shown === undefined ? '' : JSON.stringify(shown);
    return `#${call.seq} ${call.command}${args.length > 120 ? `${args.slice(0, 117)}...` : args}`;
  }

  /** Execute and deliver the call `r` picks (r in [0,1)); false when nothing is pending. */
  run(r: number): boolean {
    const call = pick(this.pending, r);
    if (!call) return false;
    if (call.phase === 'sent') this.execute(call);
    this.trace.push(`run ${this.describe(call)} -> ${summary(call.outcome!)}`);
    this.deliver(call);
    return true;
  }

  /** Execute (apply) a not-yet-executed call without delivering its answer. */
  exec(r: number): boolean {
    const call = pick(this.pending.filter((c) => c.phase === 'sent'), r);
    if (!call) return false;
    this.execute(call);
    this.trace.push(`exec ${this.describe(call)} -> ${summary(call.outcome!)} (answer held)`);
    return true;
  }

  /** Reject a not-yet-executed call unapplied: a crash (INTERNAL_ERROR) or a transport failure. */
  fault(r: number): boolean {
    const call = pick(this.pending.filter((c) => c.phase === 'sent' && FAULTABLE.has(c.command)), r);
    if (!call) return false;
    this.pending.splice(this.pending.indexOf(call), 1);
    this.faulted.push({ command: call.command, args: call.args });
    const last = this.lastIssued.get(call.command);
    if (last?.seq === call.seq) last.faulted = true;
    this.trace.push(`FAULT ${this.describe(call)}`);
    call.reject(
      STRING_ERRORS.has(call.command)
        ? 'fault injected by the interleaving fuzzer'
        : { code: 'INTERNAL_ERROR', message: 'The operation failed unexpectedly (fuzzer fault)' },
    );
    return true;
  }

  /** The oldest pending call, executed if needed and delivered: the drain order. */
  runOldest(): boolean {
    return this.pending.length > 0 && this.run(0);
  }
}

function pick<T>(items: T[], r: number): T | undefined {
  if (items.length === 0) return undefined;
  return items[Math.min(items.length - 1, Math.floor(r * items.length))];
}

function summary(outcome: Outcome): string {
  if (outcome.ok) {
    if (outcome.value instanceof ArrayBuffer) return `ok(${outcome.value.byteLength} bytes)`;
    const text = JSON.stringify(outcome.value) ?? 'undefined';
    return `ok(${text.length > 80 ? `${text.slice(0, 77)}...` : text})`;
  }
  const e = outcome.error as { code?: string };
  return `err(${e.code ?? String(outcome.error)})`;
}

/** Applied by `BackendModel.execute` (plus `FIXED`). */
const MODELLED = new Set([
  'cancel_loading', 'load_project_full_binary', 'create_project_full', 'close_project', 'save_project',
  'get_project_state_binary', 'can_undo', 'can_redo', 'create_section', 'set_surface_elevation', 'undo', 'redo',
  'import_sections', 'get_project_warnings',
  'save_display_setting', 'get_config', 'patch_config', 'update_gas_config', 'update_panel_sizes',
]);

/**
 * The calls a fault may hit: the post-edit reads that sit inside the edit queue
 * (RV5-6), the refresh, the setting writes and the saves. Loads, creates and
 * closes are not faulted: their failure paths open dialogs this tier does not model.
 */
const FAULTABLE = new Set([
  'can_undo', 'can_redo', 'get_project_warnings', 'check_project_integrity', 'get_project_state_binary',
  'save_display_setting', 'save_project', 'get_config', 'patch_config', 'update_gas_config', 'update_panel_sizes',
]);
