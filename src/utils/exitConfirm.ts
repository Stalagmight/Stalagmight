/**
 * An in-app yes/no question for the exit paths that used the native
 * `@tauri-apps/plugin-dialog` `confirm`: the window-close tier for an import or a
 * dive plan in progress, the account-change guard, the unsent SpeleoDB input
 * question (`utils/workspaceDrafts`) and the unsaved-changes question before an
 * open from SpeleoDB. No guard or SpeleoDB flow uses a native dialog.
 *
 * Same overlay shape as the SpeleoDB exit prompt (`utils/speleoExit`): its own
 * store, mounted by App above whatever dialog is open, awaited by the caller. A
 * second question while one is open answers the first with "no".
 *
 * A question about unsaved survey edits may also carry a third, non-destructive
 * answer - save them on this computer, then go on (frontend#6: edits must never
 * be only discardable). Ask it with {@link askExitChoice}.
 */
import { create } from 'zustand';

export interface ExitConfirmOptions {
  title: string;
  message: string;
  confirmLabel: string;
  cancelLabel: string;
  /** Offer "save, then go on" as the primary answer (see {@link askExitChoice}). */
  saveLabel?: string;
}

/** `confirm` = the (destructive) confirm label, `save` = save first, `cancel` = stay. */
export type ExitChoice = 'confirm' | 'save' | 'cancel';

export interface ExitConfirmRequest extends ExitConfirmOptions {
  id: number;
  answer: (choice: ExitChoice | boolean) => void;
}

export const useExitConfirm = create<{ request: ExitConfirmRequest | null }>(() => ({ request: null }));

let nextId = 0;

/** Ask, and resolve with the choice. Dismissing (Escape, X, backdrop) is `cancel`. */
export function askExitChoice(options: ExitConfirmOptions): Promise<ExitChoice> {
  useExitConfirm.getState().request?.answer('cancel');
  return new Promise<ExitChoice>((resolve) => {
    const id = ++nextId;
    let answered = false;
    const answer = (choice: ExitChoice | boolean) => {
      if (answered) return;
      answered = true;
      if (useExitConfirm.getState().request?.id === id) useExitConfirm.setState({ request: null });
      resolve(choice === true ? 'confirm' : choice === false ? 'cancel' : choice);
    };
    useExitConfirm.setState({ request: { ...options, id, answer } });
  });
}

/** Ask, and resolve with the answer. Dismissing (Escape, X, backdrop) is "no". */
export async function askExitConfirm(options: Omit<ExitConfirmOptions, 'saveLabel'>): Promise<boolean> {
  return (await askExitChoice(options)) === 'confirm';
}
