/**
 * [#576 FZ-FZ-INT-R2] A load whose file a save replaced while the load read it is
 * refused by the backend with FILE_REPLACED_DURING_LOAD, before anything was
 * committed (`core/state/project_load.rs`, the stamp check under `command_lock`).
 * The same load again reads the file now at the path, so `loadProject` repeats it
 * once, silently: no toast, no error UI, no telemetry failure. A second
 * FILE_REPLACED_DURING_LOAD is handled like any other load failure.
 */
import { errorCode } from '../utils/commandError';

/**
 * Run `load`; when it is refused with FILE_REPLACED_DURING_LOAD and `retry()` says
 * so (true: still wanted - not superseded by a newer switch), run it once more.
 * `retry` runs just before the second attempt, so it can reset what the first one
 * left (the progress handshake).
 */
export async function loadRetryingReplacedFile<T>(load: () => Promise<T>, retry: () => boolean): Promise<T> {
  try {
    return await load();
  } catch (e) {
    if (errorCode(e) !== 'FILE_REPLACED_DURING_LOAD' || !retry()) throw e;
    return load();
  }
}
