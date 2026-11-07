import { useEffect, useRef, type Dispatch, type SetStateAction } from 'react';
import { useLatest } from '../../hooks/useLatest';
import type { CommandOutcome } from '../../store/commandResult';
import type { EditOutcome } from '../../types/ipc/EditOutcome';
import type { ShotCreated } from '../../types/ipc/ShotCreated';
import type { Section, SectionId, ShotData, ShotId, ShotUpdates } from '../../types/project';
import { setSource } from '../../utils/telemetry';
import {
  type ShotDraft,
  commitAnchorArgs,
  completeDraftData,
  endDraft,
  lateDraftEdits,
  respawnsBlank,
} from './dataTableDrafts';

interface DraftAutoCommitOptions {
  drafts: ShotDraft[];
  setDrafts: Dispatch<SetStateAction<ShotDraft[]>>;
  selectedSection: Section | null | undefined;
  createShot: (
    sectionId: SectionId,
    shot: ShotData,
    afterShotId?: ShotId,
    beforeShotId?: ShotId
  ) => Promise<CommandOutcome<ShotCreated>>;
  updateShot: (sectionId: SectionId, shotId: ShotId, updates: Partial<ShotUpdates>) => Promise<CommandOutcome<EditOutcome>>;
  /** A fresh, never-reused draft key, for the blank end draft a committed one respawns as. */
  nextDraftKey: () => number;
}

/**
 * The data table's draft "new shot" rows commit themselves: once a draft's required
 * fields are present it is created as a shot via create_shot at its anchor - one at
 * a time; the refresh after create_shot re-runs the effect for the next complete one.
 *
 * A draft whose create_shot failed keeps what was typed (DRAFT-1, #576) and is not
 * re-sent as it stands: every refresh re-runs this effect, so it would be retried -
 * and its failure announced again - on each one. Editing the row makes a new draft
 * object, which is the retry. The drafts after it wait, so a retry cannot land out
 * of order behind them.
 *
 * The row stays editable while its create is in flight, and on a large survey that
 * round trip is long, so the user may well have typed on (Left, after the Depth Out
 * that completed the row). Those late values are not dropped with the draft
 * (#576 D39(f), "Forward as follow-up edit"): once the create lands, the fields that
 * changed since its payload are applied to the created shot - the one
 * `ShotCreated.shot_id` names - with one update_shot, a second undo step.
 */
export function useDraftAutoCommit({
  drafts,
  setDrafts,
  selectedSection,
  createShot,
  updateShot,
  nextDraftKey,
}: DraftAutoCommitOptions): void {
  const draftCommitting = useRef(false);
  const failedDrafts = useRef(new WeakSet<ShotDraft>());
  // Keys of drafts whose create succeeded. Their removal is an ordinary state update, and
  // the follow-up update_shot writes the store, which renders synchronously - ahead of that
  // removal. Without this, that render would find the draft still complete, now with
  // `draftCommitting` already released, and create the shot a second time. A key is
  // dropped from the set once its draft is gone, so the set stays small. Draft keys
  // are never reused, not even across a section change (DataTable), which is what
  // lets a create that resolves after the user switched section find nothing to
  // forward to: the draft it was taken from is gone, and no other one has its key.
  const committedKeys = useRef(new Set<number>());
  // The drafts as last rendered, read when a create resolves: a cell commit is a
  // discrete event React has rendered long before the create's round trip ends.
  const draftsRef = useLatest(drafts);
  const nextDraftKeyRef = useLatest(nextDraftKey);

  useEffect(() => {
    for (const key of committedKeys.current) {
      if (!drafts.some((d) => d.key === key)) committedKeys.current.delete(key);
    }
    if (!selectedSection || draftCommitting.current) return;
    for (const draft of drafts) {
      if (committedKeys.current.has(draft.key)) continue;
      if (failedDrafts.current.has(draft)) return;
      const data = completeDraftData(draft);
      if (data === null) continue;
      const { afterShotId, beforeShotId } = commitAnchorArgs(draft, selectedSection.shots);
      const sectionId = selectedSection.id;
      draftCommitting.current = true;
      setSource('data-table');
      let committed = false;
      void createShot(sectionId, data, afterShotId, beforeShotId)
        .then((result) => {
          if (!result.success) return;
          committed = true;
          committedKeys.current.add(draft.key);
          const latest = draftsRef.current.find((d) => d.key === draft.key);
          const late = latest === undefined ? {} : lateDraftEdits(data, latest);
          setDrafts((prev) =>
            prev.flatMap((d) =>
              d.key === draft.key ? (respawnsBlank(d) ? [endDraft(nextDraftKeyRef.current())] : []) : [d]
            )
          );
          if (result.shot_id != null && Object.keys(late).length > 0) {
            void updateShot(sectionId, result.shot_id, late);
          }
        })
        .finally(() => {
          if (!committed) failedDrafts.current.add(draft);
          draftCommitting.current = false;
        });
      return;
    }
  }, [drafts, selectedSection, createShot, updateShot, setDrafts, draftsRef, nextDraftKeyRef]);
}
