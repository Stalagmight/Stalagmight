import { useScreenOccupant } from '../../hooks/useScreenOccupant';
import { useExitConfirm, type ExitConfirmRequest } from '../../utils/exitConfirm';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';

/** The in-app replacement for the native exit-path `confirm` (see `utils/exitConfirm`). */
export default function ExitConfirmDialog() {
  const request = useExitConfirm((s) => s.request);
  if (!request) return null;
  return <ExitConfirm key={request.id} request={request} />;
}

function ExitConfirm({ request }: { request: ExitConfirmRequest }) {
  useScreenOccupant(true);
  const save = request.saveLabel;
  return (
    <DialogWrapper title={request.title} onClose={() => request.answer('cancel')} size="sm" data-testid="exit-confirm-dialog">
      <div className="space-y-4">
        <p className="text-sm text-text-secondary" data-testid="exit-confirm-message">{request.message}</p>
        <div className="flex flex-wrap justify-end gap-2 pt-2 border-t border-border">
          <Button onClick={() => request.answer('cancel')} {...(save ? {} : { 'data-primary-action': '' })} data-testid="exit-confirm-cancel">
            {request.cancelLabel}
          </Button>
          <Button variant="danger" onClick={() => request.answer('confirm')} data-testid="exit-confirm-ok">
            {request.confirmLabel}
          </Button>
          {save && (
            <Button variant="primary" onClick={() => request.answer('save')} data-primary-action="" data-testid="exit-confirm-save">
              {save}
            </Button>
          )}
        </div>
      </div>
    </DialogWrapper>
  );
}
