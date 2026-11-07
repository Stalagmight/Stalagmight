/**
 * The DMP wizard's section rows as they stand NOW (split out of
 * DmpNewImportDialog with #576 D17).
 *
 * `editState` is what React last committed. A row mutator that runs twice in
 * one event (a gesture that changes rows and then re-anchors the segment pick)
 * must build on the first change's result, not on the array on screen, which
 * React has not re-rendered yet. `stageShots(rows)` holds such a result until
 * the next commit; `currentShots()` returns it, or the committed rows.
 *
 * `editStateRef` is the committed edit state for actions that must not read a
 * captured copy (see refuseIncompleteShots in the dialog).
 */
import { useCallback, useLayoutEffect, useRef } from 'react';
import { useLatest } from '../../../hooks/useLatest';
import type { EditableShot } from '../../common/ShotTableEditor';
import type { SectionEditState } from '../../../utils/batchImportHelpers';

export function useLiveShots(editState: SectionEditState | null) {
  const editStateRef = useLatest(editState);
  const stagedRef = useRef<EditableShot[] | null>(null);
  // Dropped on every commit, by which point `editStateRef` holds the staged
  // rows: this layout effect is declared after useLatest's, so it runs after
  // it. (It used to be cleared in the render body, which a render React
  // discards would also have done.)
  useLayoutEffect(() => {
    stagedRef.current = null;
  });
  const currentShots = useCallback(
    (): EditableShot[] => stagedRef.current ?? editStateRef.current?.shots ?? [],
    [editStateRef]
  );
  const stageShots = useCallback((rows: EditableShot[]) => {
    stagedRef.current = rows;
  }, []);
  return { editStateRef, currentShots, stageShots };
}
