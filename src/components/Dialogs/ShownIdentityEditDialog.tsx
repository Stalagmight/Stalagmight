import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { isEditingBlockedNow } from '../../hooks/useEditingBlocked';
import { trackDialogComplete } from '../../utils/telemetry';
import type { StationRef } from '../../types/project';
import { DialogWrapper } from './DialogWrapper';
import { ShownIdentityPrompt } from './ShownIdentityPrompt';

const DIALOG_ID = 'shown-identity-edit';

/**
 * The data table's "whose station is this?" (RS 5.4): a Name or Comment cell
 * edited on a foreign slot or a loop end opens this before the edit is sent.
 * Going on sends the same `set_station_name` / `set_station_comment` the cell
 * would have (they change the shown station on every line); Cancel drops the
 * edit, so the cell shows what it showed before.
 */
export default function ShownIdentityEditDialog() {
  const { t } = useTranslation();
  const { closeDialog, dialogData } = useUIStore();
  const { setStationName, updateStationComment } = useProjectStore();
  const stationRef = dialogData.stationRef as StationRef | undefined;
  const kind = dialogData.kind === 'comment' ? 'comment' : 'name';
  const value = dialogData.value as string | null | undefined;
  const [busy, setBusy] = useState(false);

  if (!stationRef) return null;

  const proceed = async () => {
    if (isEditingBlockedNow()) {
      closeDialog();
      return;
    }
    setBusy(true);
    try {
      if (kind === 'name') {
        if (typeof value === 'string' && value !== '') await setStationName(stationRef, value);
      } else {
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

  return (
    <DialogWrapper
      title={t(kind === 'name' ? 'dialogs.shownIdentity.titleName' : 'dialogs.shownIdentity.titleComment')}
      onClose={closeDialog}
      size="md"
      data-testid="shown-identity-edit-dialog"
    >
      <ShownIdentityPrompt
        stationRef={stationRef}
        edit={kind}
        onProceed={() => void proceed()}
        onCancel={closeDialog}
        busy={busy}
      />
    </DialogWrapper>
  );
}
