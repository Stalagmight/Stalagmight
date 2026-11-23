import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { isEditingBlockedNow } from '../../hooks/useEditingBlocked';
import { trackDialogComplete } from '../../utils/telemetry';
import type { StationRef } from '../../types/project';
import { applyStationGridEdits, type StationGridEdit } from '../SingleSystemView/stationGridEdits';
import { DialogWrapper } from './DialogWrapper';
import { ShownIdentityBatchPrompt, ShownIdentityPrompt } from './ShownIdentityPrompt';

const DIALOG_ID = 'shown-identity-edit';

/**
 * The "whose station is this?" prompt as a dialog of its own (RS 5.4). Three
 * callers, by `dialogData`:
 *
 * - A data-table Name or Comment cell on a foreign slot or a loop end
 *   (`stationRef`, `kind`, `value`): going on sends the same
 *   `set_station_name` / `set_station_comment` the cell would have (they
 *   change the shown station on every line); Cancel drops the edit, so the
 *   cell shows what it showed before. The map's Comment tool commits the
 *   same way (ui-3).
 * - A paste, fill or range clear over such cells (`edits`, review finding
 *   ui-2): one prompt lists every station it changes on other lines; going
 *   on sends every edit of the batch, the native ones included, Cancel
 *   drops them all.
 * - A map action through such a slot (`stationRef`, `kind`, `onProceed`):
 *   Move Comment's pick-up or drop; going on closes the prompt and runs it.
 */
export default function ShownIdentityEditDialog() {
  const { t } = useTranslation();
  const { closeDialog, dialogData } = useUIStore();
  const { setStationName, updateStationComment } = useProjectStore();
  const stationRef = dialogData.stationRef as StationRef | undefined;
  const kind = dialogData.kind === 'comment' ? 'comment' : 'name';
  const value = dialogData.value as string | null | undefined;
  const edits = Array.isArray(dialogData.edits) ? (dialogData.edits as StationGridEdit[]) : null;
  const onProceed = typeof dialogData.onProceed === 'function'
    ? (dialogData.onProceed as () => Promise<void> | void)
    : null;
  const [busy, setBusy] = useState(false);

  if (!stationRef && !edits) return null;

  const proceed = async () => {
    if (isEditingBlockedNow()) {
      closeDialog();
      return;
    }
    if (onProceed) {
      // The action may open a dialog of its own (Move Comment's swap), so
      // this one goes first.
      trackDialogComplete(DIALOG_ID);
      closeDialog();
      await onProceed();
      return;
    }
    setBusy(true);
    try {
      if (edits) {
        await applyStationGridEdits(edits, { setStationName, updateStationComment });
      } else if (stationRef && kind === 'name') {
        if (typeof value === 'string' && value !== '') await setStationName(stationRef, value);
      } else if (stationRef) {
        await updateStationComment(stationRef, value ?? null);
      }
    } finally {
      setBusy(false);
    }
    if (useUIStore.getState().activeDialog === DIALOG_ID) {
      trackDialogComplete(DIALOG_ID);
      closeDialog();
    }
  };

  const title = edits
    ? 'dialogs.shownIdentity.titleBatch'
    : kind === 'name'
      ? 'dialogs.shownIdentity.titleName'
      : 'dialogs.shownIdentity.titleComment';

  return (
    <DialogWrapper title={t(title)} onClose={closeDialog} size="md" data-testid="shown-identity-edit-dialog">
      {edits ? (
        <ShownIdentityBatchPrompt edits={edits} onProceed={() => void proceed()} onCancel={closeDialog} busy={busy} />
      ) : (
        stationRef && (
          <ShownIdentityPrompt
            stationRef={stationRef}
            edit={kind}
            newName={kind === 'name' && typeof value === 'string' ? value : undefined}
            onProceed={() => void proceed()}
            onCancel={closeDialog}
            busy={busy}
          />
        )
      )}
    </DialogWrapper>
  );
}
