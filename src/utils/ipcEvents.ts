import { listen, type Event, type UnlistenFn } from '@tauri-apps/api/event';
import type { IpcEvents } from '../types/ipc/IpcEvents';

export { IPC_EVENTS } from '../types/ipc/IpcEvents';

/** Every event the backend emits to the webview (generated from `core::events`, #576 C13). */
export type IpcEventName = keyof IpcEvents;

/**
 * `listen()` for a backend event, typed by the generated event manifest: the name
 * must be one the backend declares, and the handler gets that event's payload
 * type - pass `IPC_EVENTS.<NAME>` rather than spelling the name.
 */
export function listenIpc<E extends IpcEventName>(
  event: E,
  handler: (event: Event<IpcEvents[E]>) => void
): Promise<UnlistenFn> {
  return listen<IpcEvents[E]>(event, handler);
}
