/**
 * Anonymous telemetry - batches event counts in memory and periodically
 * flushes them to the Rust backend via IPC.
 *
 * Self-gating: if the `is_telemetry_available` command doesn't exist
 * (telemetry feature not compiled), all calls are no-ops.
 */

import { invoke } from '@tauri-apps/api/core';
import type { IpcCommandName } from './ipc';
import { DISPLAY_DEFAULTS } from '../store/displaySelectors';

/** How often (ms) to flush batched counts to Rust. */
const FLUSH_INTERVAL_MS = 60_000;

/** How often (ms) to flush dwell time to Rust so the backend's 10-min dwell-only flush works. */
const DWELL_FLUSH_INTERVAL_MS = 60_000;

/** Active-dwell window (#31): active dwell only accrues while the user interacted
 *  within this span - it pauses when the app sits idle, unlike plain dwell. */
const ACTIVE_DWELL_WINDOW_MS = 15_000;

/** Throttle for the global interaction listener (≪ the 15s window, so it keeps the
 *  active window fresh without per-mousemove work). */
const INTERACTION_THROTTLE_MS = 1_000;

/** User-input events that count as "interaction" for active dwell. */
const INTERACTION_EVENTS = [
  'pointerdown', 'pointermove', 'keydown', 'wheel', 'touchstart', 'scroll',
] as const;
const INTERACTION_LISTENER_OPTS = { passive: true, capture: true } as const;

/** In-memory event counters. */
const counts: Record<string, number> = {};
/** Counts whose `track_events` call failed, held for the next flush under the
 *  project sequence they were recorded in (#576 TEL-2): restoring them into
 *  `counts` re-sent them under whatever sequence was current by then. */
const failedBySeq = new Map<number, Record<string, number>>();
let flushTimer: ReturnType<typeof setTimeout> | null = null;
let dwellFlushInterval: ReturnType<typeof setInterval> | null = null;
let available: boolean | null = null;

/** Source hint for the next action - consumed by dispatchers. */
let _source = 'unknown';
let _sourceTimer: ReturnType<typeof setTimeout> | null = null;

/**
 * Pending source for a project open/create/import flow. Unlike `_source`, this
 * does NOT auto-reset after 50ms - it bridges the file-picker / dialog gap
 * between the originating click and the terminal IPC. Set at the entry point,
 * peeked at each terminal command, cleared when the flow completes
 * (setView('single')) or is cancelled.
 */
let _pendingProjectSource: SourceName | null = null;

/** Dialog completion flag - set before closeDialog() in success paths. */
let _dialogCompleted: string | null = null;

/** When true, trackAction/trackCommand calls are suppressed (for programmatic UI changes). */
let _silent = false;

/** When true, persistence side effects are suppressed (for programmatic UI changes
 *  that must not write to the user's config - e.g. onboarding's transient panel toggles).
 *  Separate from `_silent` so a caller can skip telemetry WITHOUT also dropping a persist. */
let _persistSuppressed = false;

/** Whether telemetry collection is enabled. Starts false; set by initTelemetry / setTelemetryEnabled. */
let _enabled = false;

/** Auto-incrementing project sequence (0 = global, 1 = first project opened, etc.). */
let _projectSeq = 0;

/** Current workflow state for dwell time tracking. */
let _dwellState: { key: string; since: number } | null = null;

/** Active-dwell state (#31), mirroring `_dwellState`. `accruedMs` holds active
 *  milliseconds not yet emitted as whole seconds; `lastTick` is the cursor up to
 *  which active time has been counted. */
let _activeDwell: { key: string; accruedMs: number; lastTick: number } | null = null;
/** Timestamp (ms) of the last user interaction; 0 = none yet (idle). */
let _lastInteraction = 0;
/** Last time the throttled interaction mark ran. */
let _lastInteractionMark = 0;
let _interactionAttached = false;

/** Last undoable command - for regret detection. */
let _lastUndoable: { command: string; time: number } | null = null;
const REGRET_WINDOW_MS = 15_000;

/** Canonical navigation action names - parsed by registry exhaustiveness tests. */
export type NavigationAction =
  | 'center-on-shot'
  | 'center-on-station'
  | 'fit-to-bounds'
  | 'pan'
  | 'pan-to-shot'
  | 'zoom'
  | 'zoom-in'
  | 'zoom-out'
  | 'zoom-to-section';

/** Canonical panel names - parsed by registry exhaustiveness tests. */
export type PanelName =
  | 'data-table'
  | 'detail'
  | 'sections';

/** Canonical selection types - parsed by registry exhaustiveness tests. */
export type SelectionType =
  | 'section'
  | 'shot'
  | 'station';

/** Canonical context-menu action names - parsed by registry exhaustiveness tests. */
export type ContextMenuAction =
  | 'remove-closure'
  | 'set-shot-kind'
  | 'cycle-shot-kind'
  | 'unmerge';

/** Canonical source names - parsed by registry exhaustiveness tests. */
export type SourceName =
  | 'context-menu'
  | 'data-table'
  | 'dblclick'
  | 'detail-panel'
  | 'dialog'
  | 'display-menu'
  | 'drag-drop'
  | 'empty-state'
  | 'hamburger'
  | 'keyboard'
  | 'map'
  | 'mouse'
  | 'os-file-association'
  | 'recent-list'
  | 'recent-map'
  | 'sections-panel'
  | 'toolbar'
  | 'touch';

/** Commands that push to the undo stack. */
const UNDOABLE_COMMANDS: ReadonlySet<string> = new Set([
  'create_section', 'update_section', 'delete_section', 'reverse_section', 'move_section', 'move_segment',
  'create_shot', 'update_shot', 'delete_shot', 'set_shot_kind', 'split_shot_at_distance',
  'merge_stations', 'unmerge_stations', 'unmerge_station_pair',
  'set_zero_datum', 'remove_zero_datum', 'set_station_comment', 'move_comment', 'move_station',
  'create_loop_closure', 'remove_loop_closure',
  'create_section_and_merge', 'delete_shot_and_split', 'delete_last_shot_and_section',
  'delete_shot_range_split', 'delete_shot_range_shift',
  'import_sections', 'execute_tml_import', 'execute_compass_import', 'execute_walls_import',
  'execute_gpx_import', 'execute_batch',
] satisfies IpcCommandName[]);

/**
 * Check if the telemetry feature is compiled into the Rust backend.
 * Cached after first call.
 */
async function checkAvailable(): Promise<boolean> {
  if (available !== null) return available;
  try {
    available = await invoke<boolean>('is_telemetry_available');
  } catch {
    available = false;
  }
  return available;
}

/** Flush batched counts to the Rust backend. */
async function flushToRust(): Promise<void> {
  // Cancel the pending flush timeout, not just forget it: when this flush comes from
  // advanceProjectSeq or the dwell interval, a timeout left running would fire at its
  // stale deadline and flush later events early - and orphan the next one in turn.
  if (flushTimer !== null) {
    clearTimeout(flushTimer);
    flushTimer = null;
  }

  // Every batch is taken, and tagged with its sequence, before the first await:
  // the current counts belong to `_projectSeq` as it is NOW (advanceProjectSeq
  // bumps it right after this call), and the held batches to the sequences
  // they failed under.
  const batches = new Map(failedBySeq);
  failedBySeq.clear();
  if (Object.keys(counts).length > 0) {
    // Clear immediately so new events during flush aren't lost
    const current = { ...counts };
    for (const key of Object.keys(counts)) {
      delete counts[key];
    }
    batches.set(_projectSeq, mergeCounts(batches.get(_projectSeq), current));
  }

  for (const [projectSeq, events] of batches) {
    try {
      await invoke('track_events', { events, projectSeq });
    } catch {
      // Hold them for the next flush, still under their own sequence.
      failedBySeq.set(projectSeq, mergeCounts(failedBySeq.get(projectSeq), events));
    }
  }
}

/** `into` plus `from`, key by key (a fresh object when `into` is absent). */
function mergeCounts(
  into: Record<string, number> | undefined,
  from: Record<string, number>,
): Record<string, number> {
  const merged = into ?? {};
  for (const [key, value] of Object.entries(from)) {
    merged[key] = (merged[key] || 0) + value;
  }
  return merged;
}

/** Set source for the next action. Auto-resets after 50ms to prevent bleed. */
export function setSource(source: SourceName): void {
  _source = source;
  if (_sourceTimer) clearTimeout(_sourceTimer);
  _sourceTimer = setTimeout(() => { _source = 'unknown'; }, 50);
}

/** Read and reset the current source. */
export function consumeSource(): string {
  const s = _source;
  _source = 'unknown';
  if (_sourceTimer) { clearTimeout(_sourceTimer); _sourceTimer = null; }
  return s;
}

/**
 * Set the entry-point source for a project open/create/import flow. Survives
 * the file-picker / dialog async gap until the terminal command applies it.
 * Call synchronously at the originating click.
 */
export function setProjectActionSource(source: SourceName): void {
  _pendingProjectSource = source;
}

/** Read the pending project-action source without clearing it (multi-command flows). */
export function peekProjectActionSource(): SourceName | undefined {
  return _pendingProjectSource ?? undefined;
}

/** Clear the pending project-action source (flow complete or cancelled). */
export function clearProjectActionSource(): void {
  _pendingProjectSource = null;
}

/** Run a callback with tracking suppressed (for programmatic UI changes like onboarding). */
export function withTrackingSuppressed(fn: () => void): void {
  _silent = true;
  try { fn(); } finally { _silent = false; }
}

/**
 * Run a callback with persistence suppressed. Mirrors `withTrackingSuppressed`
 * but for the separate persistence-suppression concern: a side effect that
 * writes to the user's config (e.g. `savePanelLayout`) should early-return while
 * inside this callback, even though telemetry may still be wanted. Onboarding's
 * transient panel toggles want BOTH suppressed, so they nest the two wrappers.
 */
export function withPersistSuppressed(fn: () => void): void {
  _persistSuppressed = true;
  try { fn(); } finally { _persistSuppressed = false; }
}

/**
 * Whether persistence is currently suppressed (inside a `withPersistSuppressed`
 * callback). The synchronous prefix of any side effect started inside the
 * callback observes it - e.g. `savePanelLayout` skips the config write so a
 * temporary onboarding-driven layout isn't persisted.
 */
export function isPersistSuppressed(): boolean {
  return _persistSuppressed;
}

/** Increment project sequence and force-flush pending events from previous project. */
export async function advanceProjectSeq(): Promise<void> {
  flushDwell();
  // Bump before awaiting: flushToRust has already taken its snapshot and read the
  // old sequence by its first await, and anything tracked during the round trip -
  // even when the dwell interval flushes it - belongs to the new project.
  const flushed = flushToRust();
  _projectSeq++;
  await flushed;
}

/**
 * Switch the dwell-time state. Flushes accumulated seconds on the previous
 * state and starts timing the new one. Passing the same key is a no-op.
 */
export function setDwellState(key: string): void {
  if (!_enabled) return;
  if (_dwellState && _dwellState.key === key) return;
  const now = Date.now();
  if (_dwellState) {
    const secs = Math.round((now - _dwellState.since) / 1000);
    if (secs > 0) {
      counts[`dwell.${_dwellState.key}`] = (counts[`dwell.${_dwellState.key}`] || 0) + secs;
    }
  }
  _dwellState = { key, since: now };
  // Active-dwell mirror (#31): settle the old state's active time, start the new one.
  accrueActiveDwell(now);
  emitActiveDwell();
  _activeDwell = { key, accruedMs: 0, lastTick: now };
}

/** Credit active milliseconds for the current state up to `now`, capped at the end
 *  of the active window (last interaction + 15s). A no-op while idle (#31). */
function accrueActiveDwell(now: number): void {
  if (!_activeDwell) return;
  const end = Math.min(now, _lastInteraction + ACTIVE_DWELL_WINDOW_MS);
  if (end > _activeDwell.lastTick) {
    _activeDwell.accruedMs += end - _activeDwell.lastTick;
    _activeDwell.lastTick = end;
  }
}

/** Emit accrued active-dwell as whole seconds, carrying the sub-second remainder. */
function emitActiveDwell(): void {
  if (!_activeDwell) return;
  const secs = Math.floor(_activeDwell.accruedMs / 1000);
  if (secs > 0) {
    const key = `active-dwell.${_activeDwell.key}`;
    counts[key] = (counts[key] || 0) + secs;
    _activeDwell.accruedMs -= secs * 1000;
  }
}

/** Register a user interaction (#31): credit active time up to now, open a fresh 15s
 *  window, and skip the idle gap that preceded this interaction so it isn't counted. */
function markInteraction(): void {
  if (!_enabled) return;
  const now = Date.now();
  accrueActiveDwell(now);
  _lastInteraction = now;
  if (_activeDwell) _activeDwell.lastTick = now;
}

/** Throttled window listener - keeps the active window fresh without per-event work. */
function onInteraction(): void {
  if (!_enabled) return;
  const now = Date.now();
  if (now - _lastInteractionMark < INTERACTION_THROTTLE_MS) return;
  _lastInteractionMark = now;
  markInteraction();
}

function attachInteractionListeners(): void {
  if (_interactionAttached || typeof window === 'undefined') return;
  _interactionAttached = true;
  for (const ev of INTERACTION_EVENTS) {
    window.addEventListener(ev, onInteraction, INTERACTION_LISTENER_OPTS);
  }
}

function detachInteractionListeners(): void {
  if (!_interactionAttached || typeof window === 'undefined') return;
  _interactionAttached = false;
  for (const ev of INTERACTION_EVENTS) {
    window.removeEventListener(ev, onInteraction, INTERACTION_LISTENER_OPTS);
  }
}

/** Flush the current dwell state's accumulated time (e.g., before app close). */
function flushDwell(): void {
  if (!_dwellState) return;
  const now = Date.now();
  const secs = Math.round((now - _dwellState.since) / 1000);
  if (secs > 0) {
    counts[`dwell.${_dwellState.key}`] = (counts[`dwell.${_dwellState.key}`] || 0) + secs;
  }
  _dwellState = { ..._dwellState, since: now };
  // Active-dwell (#31): settle accrued active time without resetting the state.
  accrueActiveDwell(now);
  emitActiveDwell();
}

/** Track a user action with source. */
export function trackAction(action: string, source?: string): void {
  if (!_enabled || _silent) return;
  const src = source ?? consumeSource();
  const key = `action.${action}:${src}`;
  counts[key] = (counts[key] || 0) + 1;
  scheduleFlush();
}

/** Track a navigation action. */
export function trackNav(nav: NavigationAction, source?: SourceName): void {
  trackAction(`nav.${nav}`, source);
}

/** Track a panel open/close toggle. */
export function trackPanelToggle(panel: PanelName, opening: boolean): void {
  trackAction(opening ? `panel-open.${panel}` : `panel-close.${panel}`);
}

/** Track a panel reveal-mode change (pin = stays open; hover = transient). */
export function trackPanelMode(panel: PanelName, mode: 'pin' | 'hover'): void {
  trackAction(mode === 'pin' ? `panel-pin.${panel}` : `panel-unpin.${panel}`);
}

/** Track a selection event. */
export function trackSelect(type: SelectionType): void {
  trackAction(`select.${type}`);
}

/** Track an add-to-selection event. */
export function trackSelectAdd(type: SelectionType): void {
  trackAction(`select-add.${type}`);
}

/** Track a context-menu action. */
export function trackContextMenuAction(action: ContextMenuAction, source?: SourceName): void {
  trackAction(action, source);
}

/**
 * Track an IPC command invocation.
 * Called from `invokeIpc` for every IPC call.
 * Includes source hint (keyboard, toolbar, etc.) when available.
 */
export function trackCommand(command: string, sourceOverride?: string): void {
  if (!_enabled) return;
  const src = sourceOverride ?? consumeSource();
  const key = src && src !== 'unknown' ? `command.${command}:${src}` : `command.${command}`;
  counts[key] = (counts[key] || 0) + 1;

  if (command === 'undo' && _lastUndoable && (Date.now() - _lastUndoable.time) < REGRET_WINDOW_MS) {
    const key = `undo-after.${_lastUndoable.command}`;
    counts[key] = (counts[key] || 0) + 1;
    _lastUndoable = null;
  } else if (UNDOABLE_COMMANDS.has(command)) {
    _lastUndoable = { command, time: Date.now() };
  }

  scheduleFlush();
}

/** Mark dialog as completed. Call before closeDialog() in success paths. */
export function trackDialogComplete(dialogName: string): void {
  _dialogCompleted = dialogName;
}

/** Called from closeDialog(). Emits cancel event if no completion was registered. */
export function checkDialogCancel(dialogName: string): void {
  const completed = _dialogCompleted === dialogName;
  // A dialog that closes WITHOUT completing abandons whatever flow it was part
  // of, so drop any pending entry-point source - otherwise a project open/create/
  // import flow cancelled at a dialog (including a chained wizard step) would leak
  // its source into the next, unrelated project command (the source has no auto-
  // reset). Every case where the source must SURVIVE marks completion first:
  // dialog chaining calls trackDialogComplete(prev) before closeDialog (openDialog
  // in uiStore), and the unsaved-changes "Discard" marks complete then loads. So a
  // non-completed close always means "abandoned" → safe to clear. Runs even when
  // telemetry is off (it's a no-op on a null source) so the source can't linger.
  if (!completed) clearProjectActionSource();
  if (!_enabled) { _dialogCompleted = null; return; }
  if (completed) {
    _dialogCompleted = null;
    return;
  }
  _dialogCompleted = null;
  const key = `dialog-cancel.${dialogName}`;
  counts[key] = (counts[key] || 0) + 1;
  scheduleFlush();
}

/** Emit events for display toggles that differ from defaults. */
export function trackDisplayState(current: object): void {
  if (!_enabled) return;
  const state = current as Record<string, unknown>;
  for (const [key, defaultValue] of Object.entries(DISPLAY_DEFAULTS)) {
    const value = state[key];
    if (value !== undefined && value !== defaultValue) {
      const eventKey = `display-state.${key}:${String(value)}`;
      counts[eventKey] = (counts[eventKey] || 0) + 1;
    }
  }
  // geometryMode is an enum, not a boolean toggle in DISPLAY_DEFAULTS. Emit its
  // non-default state (default is 'measured'); only 'closureAdjusted' is registered.
  const geometryMode = state['geometryMode'];
  if (typeof geometryMode === 'string' && geometryMode !== 'measured') {
    const eventKey = `display-state.geometryMode:${geometryMode}`;
    counts[eventKey] = (counts[eventKey] || 0) + 1;
  }
  scheduleFlush();
}

function scheduleFlush(): void {
  if (flushTimer !== null) return;
  flushTimer = setTimeout(() => {
    void flushToRust();
  }, FLUSH_INTERVAL_MS);
}

/** Start the periodic dwell flush interval. */
function startDwellInterval(): void {
  if (dwellFlushInterval) return;
  attachInteractionListeners();
  dwellFlushInterval = setInterval(() => {
    flushDwell();
    if (Object.keys(counts).length > 0 || failedBySeq.size > 0) {
      void flushToRust();
    }
  }, DWELL_FLUSH_INTERVAL_MS);
}

/** Stop the periodic dwell flush interval. */
function stopDwellInterval(): void {
  detachInteractionListeners();
  if (dwellFlushInterval !== null) {
    clearInterval(dwellFlushInterval);
    dwellFlushInterval = null;
  }
}

/**
 * Initialize telemetry - checks feature availability and reads config.
 * Telemetry collection only starts if both compiled in and enabled in config.
 * Call once at app startup.
 */
export async function initTelemetry(enabled?: boolean): Promise<void> {
  await checkAvailable();
  if (available && enabled) {
    _enabled = true;
    startDwellInterval();
  }
}

/**
 * Enable or disable telemetry collection at runtime.
 * Called from the opt-in dialog and settings.
 */
export function setTelemetryEnabled(enabled: boolean): void {
  _enabled = enabled && available === true;
  if (_enabled) {
    startDwellInterval();
  } else {
    stopDwellInterval();
    // Clear any pending data
    for (const key of Object.keys(counts)) {
      delete counts[key];
    }
    failedBySeq.clear();
    _dwellState = null;
    _activeDwell = null;
    _lastInteraction = 0;
    if (flushTimer !== null) {
      clearTimeout(flushTimer);
      flushTimer = null;
    }
  }
}

/**
 * Whether the telemetry feature is available (compiled in).
 * Returns false until `initTelemetry()` has resolved.
 */
export function isTelemetryAvailable(): boolean {
  return available === true;
}

/**
 * Force-flush and optionally trigger a Rust-side flush (e.g., on app close).
 */
export async function flushTelemetry(): Promise<void> {
  if (flushTimer !== null) {
    clearTimeout(flushTimer);
    flushTimer = null;
  }
  stopDwellInterval();
  flushDwell();
  await flushToRust();

  if (available) {
    try {
      await invoke('flush_telemetry');
    } catch {
      // Best-effort on close
    }
  }
}
