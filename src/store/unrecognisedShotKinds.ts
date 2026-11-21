import { invokeIpc } from '../utils/ipc';
import { useUIStore } from './uiStore';
import type { UnrecognisedShotKind } from '../types/ipc/UnrecognisedShotKind';

/**
 * #27: shots whose stored `kind` this build could not read. The backend
 * already failed them closed to Reference (so nothing routes through a leg it
 * could not parse) and hands them over exactly once, but that is a
 * substitution the user never agreed to, so a load asks what they should be.
 *
 * `stillThisLoad` is the staleness guard: a user who switched projects before
 * this resolved must not get a dialog about a file that is no longer open.
 * The list has already been consumed from the backend, so losing this dialog
 * loses the prompt for the session, leaving the substitutions unreviewed.
 * Best-effort: the shots are already safely Reference either way.
 *
 * Split out of `projectStore.ts`'s `loadProject` (that file is held by the TS
 * file-size ratchet).
 */
export async function askAboutUnrecognisedShotKinds(loadingId: bigint, stillThisLoad: () => boolean): Promise<void> {
  try {
    const found = await invokeIpc<UnrecognisedShotKind[]>(
      'take_unrecognised_shot_kinds',
      // Scoped to THIS load: a cancelled load runs on to its next checkpoint,
      // so without the id an abandoned project's shot ids could be handed to
      // the one actually open.
      { loadingId },
      { purpose: 'resolve unreadable shot kinds' },
    );
    if (stillThisLoad() && found.length > 0) {
      useUIStore.getState().openDialog('unrecognised-shot-kind', { shots: found });
    }
  } catch {
    /* best-effort: the shots are already safely Reference either way */
  }
}
