/**
 * The settings a config save changes, as `patch_config` patches (#576 D27-32(c)).
 *
 * A save used to post the whole `Config` built from a snapshot taken earlier (the
 * Settings dialog takes one when it opens), so every value the backend recorded in
 * between - the panel layout and widths, the recent projects, the table views - was
 * silently put back by a save that never touched it. Sending only the leaves that
 * differ from the snapshot, applied by the backend onto its LIVE config, leaves
 * everything else as the backend has it.
 *
 * A leaf is a value that is not a plain object (arrays included). An object whose
 * keys were removed (a map entry deleted) is sent whole, which is how the backend
 * replaces a map; anything added - a key, a map entry, a table the snapshot lacked -
 * is sent setting by setting (the backend creates the tables on the way). Paths are
 * arrays because some config maps are keyed by names that contain dots.
 */
import type { ConfigPatch } from '../types/ipc/ConfigPatch';

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function sameValue(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  return JSON.stringify(a) === JSON.stringify(b);
}

function diff(original: unknown, current: unknown, path: string[], out: ConfigPatch[]): void {
  if (sameValue(original, current)) return;
  // A table the snapshot did not have at all (an older or partial config) and the
  // edited copy filled in: only the settings in it are news, each on its own - not
  // the table whole, whose defaults would overwrite what the backend holds.
  if (original === undefined && isPlainObject(current) && path.length > 0) {
    diff({}, current, path, out);
    return;
  }
  if (path.length > 0 && !(isPlainObject(original) && isPlainObject(current))) {
    out.push({ path, value: current === undefined ? null : current });
    return;
  }
  if (!isPlainObject(original) || !isPlainObject(current)) return;
  const removed = Object.keys(original).some((key) => !(key in current));
  if (removed && path.length > 0) {
    out.push({ path, value: current });
    return;
  }
  for (const key of Object.keys(current)) {
    diff(original[key], current[key], [...path, key], out);
  }
}

/** The patches that turn `original` into `current` - empty when nothing changed. */
export function configPatches(original: object, current: object): ConfigPatch[] {
  const out: ConfigPatch[] = [];
  diff(original, current, [], out);
  return out;
}
