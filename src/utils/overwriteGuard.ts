/**
 * The overwrite check for a save path the app REWROTE after the OS save dialog
 * returned it (#576 D35).
 *
 * The native save dialog asks "replace?" for the name the user typed. The app
 * then fixes the extension (`ensureExtension`), so `map.svg` typed for a PNG
 * export is written as `map.png` - a file no prompt was ever shown for, which
 * the export writers replace without a word. `resolveSavePath` closes that hole
 * for every caller that writes to a dialog answer: when the final name differs
 * from the answer and that file exists, it asks first.
 *
 * The question is raised through a tiny store rendered by
 * `RewrittenPathOverwritePrompt` (mounted once in App) as a layered alert, NOT
 * through the single `activeDialog` slot: the callers include the Export dialog
 * and the SpeleoDB prompts, and opening a dialog there would evict the very
 * dialog that is waiting for the answer.
 */
import { create } from 'zustand';
import { errorCode } from './commandError';
import { ensureExtension } from './fileDialogs';
import { invokeIpc } from './ipc';
import { extractFilename } from './path';

export interface OverwritePromptRequest {
  /** The path that will be written. */
  path: string;
  fileName: string;
  /** The extension the format requires, with its dot (e.g. `.png`). */
  extension: string;
  /** The existence probe failed, so the file MAY exist (fail closed, as E10-D1). */
  existenceUnknown: boolean;
  resolve: (replace: boolean) => void;
}

interface OverwritePromptState {
  request: OverwritePromptRequest | null;
}

export const useOverwritePromptStore = create<OverwritePromptState>(() => ({ request: null }));

/**
 * Ask whether to replace `path`. Resolves true to replace, false to cancel. A
 * second request while one is open cancels the first, so no caller is left
 * waiting on a question that is no longer on screen.
 */
export function confirmOverwrite(
  path: string,
  extension: string,
  existenceUnknown: boolean,
): Promise<boolean> {
  return new Promise<boolean>((resolve) => {
    useOverwritePromptStore.getState().request?.resolve(false);
    const request: OverwritePromptRequest = {
      path,
      fileName: extractFilename(path),
      extension: extension.startsWith('.') ? extension : `.${extension}`,
      existenceUnknown,
      resolve: (replace) => {
        if (useOverwritePromptStore.getState().request === request) {
          useOverwritePromptStore.setState({ request: null });
        }
        resolve(replace);
      },
    };
    useOverwritePromptStore.setState({ request });
  });
}

/**
 * Whether a file is already at `path`, which a save or export is about to write
 * (#576 HFI-2). The backend answers (`save_target_exists`), under the write-path
 * policy of the write that follows: the webview's fs plugin has no scope for
 * user paths, so its `exists()` threw for every path in the real app and every
 * prompt said "could not check". Rejects when the check failed or the path is
 * refused; the callers then ask as if the file were there (E10-D1).
 */
export function saveTargetExists(path: string): Promise<boolean> {
  return invokeIpc('save_target_exists', { path }, { purpose: 'check whether a save would replace a file' });
}

/**
 * What the overwrite prompts need to know about `path` before a save or export
 * writes it: `present` asks first, `existenceUnknown` words the question "could
 * not check". A failed check asks (fail closed, #576 E10-D1). A path the
 * write-path policy refuses (`WRITE_PATH_REFUSED`: an app or system folder) is
 * refused by the write too, with the reason, so it goes straight there - one
 * dialog, the refusal, not a "could not check" prompt in front of it
 * (#576 HFI-2-D2).
 */
export async function saveTargetPresence(path: string): Promise<{ present: boolean; existenceUnknown: boolean }> {
  try {
    return { present: await saveTargetExists(path), existenceUnknown: false };
  } catch (e) {
    if (errorCode(e) === 'WRITE_PATH_REFUSED') return { present: false, existenceUnknown: false };
    return { present: true, existenceUnknown: true };
  }
}

/**
 * The path a save-dialog answer is actually written to, or `null` when the user
 * declined to replace an existing file there.
 *
 * Only a REWRITTEN name is checked: when `ensureExtension` leaves the answer
 * as-is, the OS dialog already asked about exactly that file. A probe that
 * fails asks too (fail closed), like Save As (#576 E10-D1); a refused path goes
 * to the write, which refuses it (#576 HFI-2-D2).
 */
export async function resolveSavePath(selected: string, extension: string): Promise<string | null> {
  const finalPath = ensureExtension(selected, extension);
  if (finalPath === selected) return finalPath;

  const { present, existenceUnknown } = await saveTargetPresence(finalPath);
  if (!present) return finalPath;
  return (await confirmOverwrite(finalPath, extension, existenceUnknown)) ? finalPath : null;
}
