import { useCallback, useEffect, useRef, useState } from 'react';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { revealItemInDir } from '@tauri-apps/plugin-opener';
import { useUIStore } from '../../store/uiStore';
import { routeOpenedFile } from '../../utils/openOrImport';
import { clearProjectActionSource, trackDialogComplete } from '../../utils/telemetry';
import {
  MNEMO_PULL_PROGRESS_EVENT,
  cancelMnemoPull,
  formatBytes,
  isMnemoCancelled,
  listMnemoPorts,
  mnemoErrorMessage,
  mnemoPortLabel,
  preselectMnemoPort,
  pullMnemo,
} from '../../utils/mnemo';
import type { MnemoPort } from '../../types/ipc/MnemoPort';
import type { MnemoPullProgress } from '../../types/ipc/MnemoPullProgress';
import type { MnemoPullResult } from '../../types/ipc/MnemoPullResult';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';

type Phase = 'listing' | 'ready' | 'pulling' | 'done';

/**
 * Import from MNemo (#725): read a USB-connected MNemo, save its memory as a
 * `.dmp`, then hand that file to the normal open/import route, so everything
 * a `.dmp` picked from disk goes through applies here too.
 */
export default function MnemoImportDialog() {
  const closeDialog = useUIStore((s) => s.closeDialog);
  const [phase, setPhase] = useState<Phase>('listing');
  const [ports, setPorts] = useState<MnemoPort[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [received, setReceived] = useState(0);
  const [result, setResult] = useState<MnemoPullResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Set in the effect, not only at init: StrictMode runs mount, cleanup, mount,
  // and a ref left false by that first cleanup would drop every reply.
  const mounted = useRef(true);
  // A pull still running when the dialog goes away (another dialog replaced
  // it) is cancelled, so it does not hold the port with no Cancel on screen.
  const pullInFlight = useRef(false);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      if (pullInFlight.current) void cancelMnemoPull().catch(() => {});
    };
  }, []);

  const refresh = useCallback(async () => {
    setPhase('listing');
    setError(null);
    try {
      const found = await listMnemoPorts();
      if (!mounted.current) return;
      setPorts(found);
      setSelected((current) =>
        current && found.some((p) => p.name === current) ? current : preselectMnemoPort(found)
      );
    } catch (e) {
      if (!mounted.current) return;
      setPorts([]);
      setSelected(null);
      setError(mnemoErrorMessage(e));
    }
    if (mounted.current) setPhase('ready');
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    let disposed = false;
    let unlisten: UnlistenFn | undefined;
    (async () => {
      try {
        const fn = await listen<MnemoPullProgress>(MNEMO_PULL_PROGRESS_EVENT, (event) => {
          setReceived(event.payload.received);
        });
        if (disposed) fn();
        else unlisten = fn;
      } catch (e) {
        // Progress is cosmetic; the pull reports its own result.
        console.warn('[MnemoImportDialog] progress listener not available:', e);
      }
    })();
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  // Read MNemo unmounts when the pull starts, which would leave focus on the
  // page body where Escape no longer reaches the dialog. Cancel takes it over.
  const cancelRef = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (phase === 'pulling') cancelRef.current?.focus();
  }, [phase]);

  const handleDownload = async () => {
    if (!selected || phase !== 'ready') return;
    setPhase('pulling');
    setReceived(0);
    setError(null);
    pullInFlight.current = true;
    try {
      const pulled = await pullMnemo(selected);
      if (!mounted.current) return;
      setResult(pulled);
      setPhase('done');
    } catch (e) {
      if (!mounted.current) return;
      if (!isMnemoCancelled(e)) setError(mnemoErrorMessage(e));
      setPhase('ready');
    } finally {
      pullInFlight.current = false;
    }
  };

  const handleCancelPull = () => {
    void cancelMnemoPull().catch((e: unknown) => setError(mnemoErrorMessage(e)));
  };

  // Closing mid-transfer cancels it first; the dialog stays until the pull returns.
  const handleClose = () => {
    if (phase === 'pulling') {
      handleCancelPull();
      return;
    }
    clearProjectActionSource();
    closeDialog();
  };

  const handleContinue = async () => {
    if (!result || result.unreadable_reason) return;
    trackDialogComplete('mnemo-import');
    closeDialog();
    await routeOpenedFile(result.path, 'Import Failed');
  };

  const handleShowFile = () => {
    if (!result) return;
    void revealItemInDir(result.path).catch((e: unknown) => setError(mnemoErrorMessage(e)));
  };

  const pulling = phase === 'pulling';
  const losses: string[] = [];
  if (result && result.skipped_sections > 0) {
    losses.push(
      result.skipped_sections === 1
        ? '1 section could not be read and will not be imported.'
        : `${result.skipped_sections} sections could not be read and will not be imported.`
    );
  }
  if (result && result.unread_trailing_bytes > 0) {
    losses.push(
      `${formatBytes(result.unread_trailing_bytes)} after the last complete section were not read. ` +
        'This is usually a survey that was not closed on the device.'
    );
  }

  return (
    <DialogWrapper
      title="Import from MNemo"
      onClose={handleClose}
      size="md"
      closeOnBackdropClick={!pulling}
      data-testid="mnemo-import-dialog"
    >
      <div className="space-y-4">
        {phase !== 'done' && (
          <>
            <p className="text-text-secondary">
              Connect the MNemo with its USB cable and switch it on. Stalagmight reads its survey memory without
              changing anything on the device, and keeps a copy of what it read.
            </p>

            <div className="flex items-end gap-2">
              <div className="flex-1">
                <label htmlFor="mnemo-port" className="block text-sm text-text-secondary mb-1">
                  Device
                </label>
                <select
                  id="mnemo-port"
                  className="w-full"
                  value={selected ?? ''}
                  onChange={(e) => setSelected(e.target.value || null)}
                  disabled={pulling || phase === 'listing' || ports.length === 0}
                  data-testid="mnemo-port-select"
                >
                  <option value="">{ports.length === 0 ? 'No USB devices found' : 'Choose a device'}</option>
                  {ports.map((p) => (
                    <option key={p.name} value={p.name}>
                      {mnemoPortLabel(p)}
                    </option>
                  ))}
                </select>
              </div>
              <Button onClick={() => void refresh()} disabled={pulling || phase === 'listing'} data-testid="mnemo-refresh">
                Refresh
              </Button>
            </div>

            {phase === 'ready' && ports.length === 0 && !error && (
              <p className="text-sm text-text-tertiary" data-testid="mnemo-no-ports">
                No MNemo found. Check the cable and that the device is on, then press Refresh.
              </p>
            )}

            {pulling && (
              <div className="space-y-2" data-testid="mnemo-progress">
                <p className="text-sm text-text-secondary">
                  Reading the MNemo... {received > 0 ? `${formatBytes(received)} received` : 'waiting for the device'}
                </p>
                <div className="h-1.5 bg-border rounded-full overflow-hidden">
                  <div
                    className="h-full bg-accent-70 animate-pulse-shimmer"
                    role="progressbar"
                    aria-label="Reading the MNemo"
                    aria-valuetext={received > 0 ? `${formatBytes(received)} received` : 'Waiting for the device'}
                  />
                </div>
              </div>
            )}
          </>
        )}

        {phase === 'done' && result && (
          <div className="space-y-2" data-testid="mnemo-result">
            <p className="text-text-secondary" data-testid="mnemo-result-summary">
              {result.unreadable_reason
                ? `Saved ${result.file_name} (${formatBytes(result.bytes)}).`
                : `Saved ${result.file_name}: ${result.sections === 1 ? '1 section' : `${result.sections} sections`}, ` +
                  `${result.shots === 1 ? '1 shot' : `${result.shots} shots`}.`}
            </p>
            {result.firmware && <p className="text-sm text-text-tertiary">Firmware: {result.firmware}</p>}
            {losses.length > 0 && (
              <div
                className="p-3 bg-warning-10 border border-warning-20 rounded text-warning text-sm space-y-1"
                data-testid="mnemo-result-losses"
              >
                {losses.map((line) => (
                  <p key={line}>{line}</p>
                ))}
              </div>
            )}
            {result.unreadable_reason && (
              <div
                className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm"
                data-testid="mnemo-result-unreadable"
              >
                The data was saved, but none of it could be read as surveys: {result.unreadable_reason}
              </div>
            )}
          </div>
        )}

        {error && (
          <div className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm" data-testid="mnemo-error">
            {error}
          </div>
        )}

        <div className="flex justify-end gap-2 pt-4 border-t border-border">
          {phase === 'done' ? (
            <>
              <Button onClick={handleShowFile} data-testid="mnemo-show-file">
                Show file
              </Button>
              <Button onClick={handleClose} data-testid="mnemo-close">
                Close
              </Button>
              <Button
                variant="primary"
                onClick={() => void handleContinue()}
                disabled={Boolean(result?.unreadable_reason)}
                data-primary-action=""
                data-testid="mnemo-continue"
              >
                Import...
              </Button>
            </>
          ) : pulling ? (
            <Button ref={cancelRef} onClick={handleCancelPull} data-testid="mnemo-cancel-pull">
              Cancel
            </Button>
          ) : (
            <>
              <Button onClick={handleClose} data-testid="mnemo-close">
                Close
              </Button>
              <Button
                variant="primary"
                onClick={() => void handleDownload()}
                disabled={!selected || phase !== 'ready'}
                data-primary-action=""
                data-testid="mnemo-download"
              >
                Read MNemo
              </Button>
            </>
          )}
        </div>
      </div>
    </DialogWrapper>
  );
}
