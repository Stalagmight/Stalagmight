import { getCurrentWindow } from '@tauri-apps/api/window';

let warned = false;

export async function applyWindowTitle(title: string): Promise<void> {
  try {
    await getCurrentWindow().setTitle(title);
  } catch (e) {
    if (!warned) {
      warned = true;
      console.warn('Native window title unavailable, using document.title:', e);
    }
    document.title = title;
  }
}

export function resetWindowTitleWarningForTest(): void {
  warned = false;
}
