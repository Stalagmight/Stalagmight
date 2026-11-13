/**
 * Import from MNemo over USB (#725): the IPC calls and the words the dialog
 * shows for each way a pull can fail. The backend lives in
 * `src-tauri/src/commands/mnemo/`.
 */

import { invokeIpc, getErrorMessage } from './ipc';
import type { MnemoError } from '../types/ipc/MnemoError';
import type { MnemoPort } from '../types/ipc/MnemoPort';
import type { MnemoPullResult } from '../types/ipc/MnemoPullResult';

/** Event carrying `MnemoPullProgress` while the survey data streams in. */
export const MNEMO_PULL_PROGRESS_EVENT = 'mnemo-pull-progress';

export function listMnemoPorts(): Promise<MnemoPort[]> {
  return invokeIpc<MnemoPort[]>('list_mnemo_ports', undefined, {
    purpose: 'list USB serial ports for Import from MNemo',
  });
}

export function pullMnemo(port: string): Promise<MnemoPullResult> {
  return invokeIpc<MnemoPullResult>('pull_mnemo', { port }, {
    purpose: 'read an MNemo and save its memory as a .dmp',
    level: 'info',
  });
}

export function cancelMnemoPull(): Promise<void> {
  return invokeIpc<void>('cancel_mnemo_pull', undefined, { purpose: 'cancel Import from MNemo' });
}

function isMnemoError(e: unknown): e is MnemoError {
  return typeof e === 'object' && e !== null && typeof (e as { kind?: unknown }).kind === 'string';
}

/** True when the rejection is the user's own cancel, which is not an error to show. */
export function isMnemoCancelled(e: unknown): boolean {
  return isMnemoError(e) && e.kind === 'cancelled';
}

/** What to tell the user when a list or pull fails, including what to do about it. */
export function mnemoErrorMessage(e: unknown): string {
  if (!isMnemoError(e)) return getErrorMessage(e, 'Could not read the MNemo.');
  switch (e.kind) {
    case 'unsupported':
      return 'This version of Stalagmight cannot reach USB devices.';
    case 'port_not_found':
      return 'That device is no longer connected. Plug it in and press Refresh.';
    case 'port_busy':
      return 'Another program is using the MNemo. Close it, for example the MNemo desktop app, and try again.';
    case 'permission_denied':
      return (
        'Your user account may not open this device. On Linux, add yourself to the dialout group ' +
        '(uucp on Arch), then log out and back in.'
      );
    case 'no_answer':
      return 'The MNemo did not answer. Check it is switched on and showing its main menu, then try again.';
    case 'not_mnemo':
      return 'The device answered, but not as an MNemo does, so nothing was read from it.';
    case 'incomplete':
      return e.received > 0
        ? `The transfer stopped after ${formatBytes(e.received)}, before the end of the data. ` +
            'Check the cable and try again.'
        : 'The MNemo stopped answering before it sent its data. Try again.';
    case 'too_large':
      return 'The device sent more data than an MNemo holds, so the transfer was stopped.';
    case 'cancelled':
      return 'Cancelled.';
    case 'already_running':
      return 'A transfer from the MNemo is already running.';
    case 'failed':
      return e.message;
  }
}

/** `512 bytes`, `12.3 kB`, `1.4 MB`. */
export function formatBytes(n: number): string {
  if (n < 1000) return `${n} bytes`;
  if (n < 1_000_000) return `${(n / 1000).toFixed(1)} kB`;
  return `${(n / 1_000_000).toFixed(1)} MB`;
}

/** How a port is named in the picker. */
export function mnemoPortLabel(port: MnemoPort): string {
  const what = port.product ?? port.manufacturer ?? 'USB serial device';
  return port.likely_mnemo ? `${what} (${port.name})` : `${what} (${port.name}), not recognised as an MNemo`;
}

/** The port to preselect: the single MNemo-looking one, else none. */
export function preselectMnemoPort(ports: MnemoPort[]): string | null {
  const likely = ports.filter((p) => p.likely_mnemo);
  return likely.length === 1 ? (likely[0]?.name ?? null) : null;
}
