import type { OpenDialogOptions, SaveDialogOptions } from '@tauri-apps/plugin-dialog';

export type TestFileDialogAnswer = string | string[] | null;
export interface TestFileDialogQueueEntry {
  id: string;
  value: TestFileDialogAnswer;
  triggerTestId: string;
  armed: boolean;
}

interface FileDialogTestEnv {
  DEV?: boolean;
  MODE?: string;
  VITE_E2E?: string;
}

export function testFileDialogHooksEnabled(env: FileDialogTestEnv = import.meta.env): boolean {
  return env.DEV === true || env.MODE === 'test' || env.VITE_E2E === '1';
}

export function takeTestFileDialogAnswer(kind: 'open' | 'save', triggerTestId?: string): {
  queued: boolean;
  value: TestFileDialogAnswer | undefined;
} {
  if (!testFileDialogHooksEnabled() || typeof window === 'undefined') {
    return { queued: false, value: undefined };
  }
  const hooks = window.__E2E_TEST__;
  const queue = hooks?.fileDialogAnswers?.[kind];
  if (!hooks || !queue) return { queued: false, value: undefined };
  const entry = queue?.[0];
  if (!entry?.armed) return { queued: false, value: undefined };
  if (triggerTestId && entry.triggerTestId !== triggerTestId) {
    throw new Error(`file dialog answer for ${entry.triggerTestId} cannot be consumed by ${triggerTestId}`);
  }
  if (hooks.activeFileDialogTriggerTestId !== entry.triggerTestId) {
    return { queued: false, value: undefined };
  }
  queue.shift();
  delete hooks.activeFileDialogTriggerTestId;
  hooks.fileDialogConsumptions ??= {};
  hooks.fileDialogConsumptions[entry.id] = kind;
  return { queued: true, value: entry.value };
}

export const PROJECT_EXTENSION = 'stalx';
export const DMP_EXTENSION = 'dmp';

export function ensureExtension(path: string, extension: string): string {
  const normalizedExt = extension.startsWith('.') ? extension.slice(1) : extension;
  const lowerPath = path.toLowerCase();
  const lowerExt = normalizedExt.toLowerCase();

  if (lowerPath.endsWith(`.${lowerExt}`)) {
    return path;
  }

  const lastSeparator = Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\'));
  const lastDot = path.lastIndexOf('.');
  if (lastDot > lastSeparator) {
    return `${path.slice(0, lastDot + 1)}${normalizedExt}`;
  }

  return `${path}.${normalizedExt}`;
}

export function projectSaveFilters() {
  return [{ name: 'Stalagmight Project', extensions: [PROJECT_EXTENSION] }];
}

export const TML_EXTENSIONS = ['tml', 'tmlu'];
export const GPX_EXTENSION = 'gpx';
export const COMPASS_EXTENSIONS = ['mak', 'dat'];
export const WALLS_EXTENSIONS = ['wpj', 'srv'];

/**
 * Single source of truth for the importable survey formats, grouped by the
 * open/import flow each routes to (see `routeOpenedFile`). Both the file-dialog
 * filter helpers below and the router's `isImportable` guard derive from this,
 * so the recognized extension set can't drift between the picker and the
 * dispatch. Extensions are lowercase and WITHOUT a leading dot (matching the
 * Tauri dialog filter shape); the router lowercases the path and adds the dot.
 *
 * `.stalx` (native project) is intentionally NOT here - opening a project is not
 * an import. It lives in `PROJECT_EXTENSION`.
 */
export const IMPORT_EXTENSIONS = {
  gpx: [GPX_EXTENSION],
  tml: TML_EXTENSIONS,
  compass: COMPASS_EXTENSIONS,
  walls: WALLS_EXTENSIONS,
  dmp: [DMP_EXTENSION],
} as const;

/**
 * Flat list of every importable extension (no leading dot), derived from
 * `IMPORT_EXTENSIONS`. Order matches the legacy hardcoded set
 * (gpx, tml, tmlu, mak, dat, dmp) so the router guard is unchanged.
 */
export const IMPORTABLE_EXTENSIONS: readonly string[] = [
  ...IMPORT_EXTENSIONS.gpx,
  ...IMPORT_EXTENSIONS.tml,
  ...IMPORT_EXTENSIONS.compass,
  ...IMPORT_EXTENSIONS.walls,
  ...IMPORT_EXTENSIONS.dmp,
];

/**
 * `IMPORTABLE_EXTENSIONS` plus `PROJECT_EXTENSION`, for surfaces (drag-drop)
 * that accept both an import and a native project open and let `routeOpenedFile`
 * decide which. Extensions have no leading dot, matching `IMPORTABLE_EXTENSIONS`.
 */
export const DROPPABLE_EXTENSIONS: readonly string[] = [...IMPORTABLE_EXTENSIONS, PROJECT_EXTENSION];

/**
 * Whether `path` ends in one of `extensions` (no leading dot), matched
 * case-insensitively. Defaults to `DROPPABLE_EXTENSIONS`.
 */
export function isDroppableFilePath(path: string, extensions: readonly string[] = DROPPABLE_EXTENSIONS): boolean {
  const lowerPath = path.toLowerCase();
  return extensions.some((ext) => lowerPath.endsWith(`.${ext}`));
}

/**
 * Filters for the unified "Open / Import" picker: every file the app can open
 * (native `.stalx` projects) or import (DMP / Ariane / Compass / Walls / GPX).
 * The first entry is the combined "everything" filter so a single selection
 * accepts any supported file; the rest narrow to one format each.
 *
 * This is the filter set of every door that can OPEN something: the hamburger's
 * Open / Import row in both views, Ctrl+O, the EmptyState card, and Ctrl+I with
 * no project open. The in-project Import doors (toolbar button, hamburger
 * Import row, Ctrl+I) use `inProjectImportFilters` instead (#508) - see there
 * for why. Every one of them routes the choice through `routeOpenedFile`, so the
 * filters decide only what the picker shows, never how a file is handled.
 *
 * A `.stalx` picked through an Open door is a plain project open, guarded by
 * the router's SpeleoDB hand-back and unsaved-changes prompts (#270). Through
 * an in-project Import door it instead asks "Open as project" or "Add to
 * <current project>" (#674); that choice is the router's, not the picker's.
 */
export function openOrImportFilters() {
  return [
    {
      // Combined entry: native project + everything importable. Derived from
      // IMPORTABLE_EXTENSIONS so it tracks the router/dispatch automatically.
      name: 'Stalagmight & Survey Files',
      extensions: [PROJECT_EXTENSION, ...IMPORTABLE_EXTENSIONS],
    },
    { name: 'Stalagmight Project', extensions: [PROJECT_EXTENSION] },
    { name: 'MNemo DMP', extensions: [DMP_EXTENSION] },
    { name: 'Ariane Survey', extensions: TML_EXTENSIONS },
    { name: 'Compass Survey', extensions: COMPASS_EXTENSIONS },
    { name: 'Walls Survey', extensions: WALLS_EXTENSIONS },
    { name: 'GPX Files', extensions: [GPX_EXTENSION] },
    { name: 'All Files', extensions: ['*'] },
  ];
}

/**
 * Filters for the IN-PROJECT "Import" picker (#508): `.dmp` only.
 *
 * With a project open, "import" means one thing - add MNemo data to THIS
 * project. Every other importable format is new-project-only: picking one only
 * ever leads to the "close this project and import as a new one?" prompt, which
 * is the Open row's job, not Import's. So the in-project Import doors (hamburger
 * row, toolbar button, Ctrl+I) narrow to the one format that is genuinely
 * additive, and Open keeps `openOrImportFilters` for everything else.
 *
 * This is a picker-side affordance only: `routeOpenedFile` keeps its full
 * extension dispatch, so drag-drop and the OS file-launch channel are unchanged
 * (and a `.dmp` reaching the router with no project open still seeds a new one).
 * No 'All Files' entry: nothing else is additive, so anything picked through it
 * could only end at the router's unsupported-file dialog. (It would not reach
 * the DMP parser - the router stopped assuming DMP for unknown extensions in
 * #407, `routeOpenedFile`'s final `else`.)
 */
export function dmpImportFilters() {
  return [{ name: 'MNemo DMP', extensions: [DMP_EXTENSION] }];
}

/**
 * Filters for the IN-PROJECT Import doors (#674): `.dmp` (added to the open
 * project) and `.stalx` (the router asks whether to open it as a project or add
 * it to the open one). Everything else is new-project-only and stays on Open.
 */
export function inProjectImportFilters() {
  return [
    { name: 'MNemo DMP & Stalagmight Project', extensions: [DMP_EXTENSION, PROJECT_EXTENSION] },
    ...dmpImportFilters(),
    { name: 'Stalagmight Project', extensions: [PROJECT_EXTENSION] },
  ];
}

export function exportSaveFilters(label: string, extension: string) {
  const normalizedExt = extension.startsWith('.') ? extension.slice(1) : extension;
  return [{ name: label, extensions: [normalizedExt] }];
}

/** Open a native picker, or consume the next answer queued by the E2E harness. */
export async function openDialog(options?: OpenDialogOptions, triggerTestId?: string): Promise<string | string[] | null> {
  const answer = takeTestFileDialogAnswer('open', triggerTestId);
  if (answer.queued) return answer.value ?? null;
  const { open } = await import('@tauri-apps/plugin-dialog');
  return open(options);
}

/**
 * Wrapper around Tauri's `save()` dialog that fixes Android compatibility.
 *
 * On Android, the native SAF save dialog (ACTION_CREATE_DOCUMENT) returns
 * content:// URIs that the Rust backend cannot use as filesystem paths, and
 * the activity result callback is unreliable on some devices (e.g. Boox).
 * Instead, we show an in-app directory picker + filename input. This requires
 * MANAGE_EXTERNAL_STORAGE permission.
 *
 * On desktop, delegates to the native save dialog.
 */
export async function saveDialog(options?: SaveDialogOptions, triggerTestId?: string): Promise<string | null> {
  const answer = takeTestFileDialogAnswer('save', triggerTestId);
  if (answer.queued) {
    if (Array.isArray(answer.value)) throw new Error('save dialog test answer must be a single path');
    return answer.value ?? null;
  }

  if (navigator.userAgent.includes('Android')) {
    // Try the native SAF dialog first. If the result is a content:// URI,
    // extract the filesystem path. Fall back to the in-app picker if the
    // native dialog hangs or fails.
    const result = await Promise.race([
      nativeSaveDialog(options),
      // If the native dialog doesn't resolve within 2 minutes, it's hung
      new Promise<'timeout'>((r) => setTimeout(() => r('timeout'), 120_000)),
    ]);

    if (result === 'timeout' || result === null) {
      // Native dialog hung or was cancelled - fall back to in-app picker
      return result === 'timeout' ? androidSaveDialog(options) : null;
    }

    // Convert content:// URI to filesystem path
    return resolveAndroidPath(result);
  }

  const { save } = await import('@tauri-apps/plugin-dialog');

  if (options?.defaultPath && !isAbsolutePath(options.defaultPath)) {
    try {
      const { documentDir, sep } = await import('@tauri-apps/api/path');
      const dir = await documentDir();
      // Only prepend a real absolute documents dir; otherwise let the native
      // dialog pick its own default rather than producing a relative path.
      if (dir && isAbsolutePath(dir)) {
        options = { ...options, defaultPath: `${dir}${sep()}${options.defaultPath}` };
      }
    } catch {
      // Documents dir unavailable - fall back to the native dialog default.
    }
  }

  return save(options);
}

async function nativeSaveDialog(options?: SaveDialogOptions): Promise<string | null> {
  const { save } = await import('@tauri-apps/plugin-dialog');
  return save(options);
}

/**
 * Convert Android content:// URIs or raw paths to filesystem paths.
 * Common patterns from SAF:
 *   content://com.android.providers.downloads.documents/document/raw:/storage/emulated/0/Download/file.stalx
 *   content://com.android.externalstorage.documents/document/primary:Download/file.stalx
 */
function resolveAndroidPath(uri: string): string {
  // Already a filesystem path
  if (uri.startsWith('/')) return uri;

  // Try to extract path from raw:/ pattern (Downloads provider)
  const rawMatch = uri.match(/raw[:%]3A(.+)/i);
  if (rawMatch) {
    return decodeURIComponent(rawMatch[1]!);
  }

  // Try to extract from primary: pattern (external storage provider)
  const primaryMatch = uri.match(/primary[:%]3A(.+)/i);
  if (primaryMatch) {
    return `/storage/emulated/0/${decodeURIComponent(primaryMatch[1]!)}`;
  }

  // Can't parse - return as-is and hope for the best
  return uri;
}

function isAbsolutePath(p: string): boolean {
  return p.startsWith('/') || /^[a-zA-Z]:[/\\]/.test(p);
}

interface AndroidDir { label: string; path: string }

async function getAndroidDirs(): Promise<AndroidDir[]> {
  const { downloadDir, documentDir } = await import('@tauri-apps/api/path');
  const dirs: AndroidDir[] = [];
  try { dirs.push({ label: 'Downloads', path: await downloadDir() }); } catch { /* skip */ }
  try { dirs.push({ label: 'Documents', path: await documentDir() }); } catch { /* skip */ }
  // Common shared storage paths
  const shared = '/storage/emulated/0';
  if (!dirs.some(d => d.path.includes('Download'))) {
    dirs.push({ label: 'Downloads', path: `${shared}/Download` });
  }
  if (!dirs.some(d => d.path.includes('Document'))) {
    dirs.push({ label: 'Documents', path: `${shared}/Documents` });
  }
  return dirs;
}

/**
 * Shows an in-app save dialog for Android: a directory picker dropdown + filename input.
 * Returns the full filesystem path, or null if cancelled.
 */
async function androidSaveDialog(options?: SaveDialogOptions): Promise<string | null> {
  const dirs = await getAndroidDirs();
  const defaultFilename = options?.defaultPath
    ? options.defaultPath.split('/').pop()!.split('\\').pop()!
    : 'untitled';

  return new Promise<string | null>((resolve) => {
    // Build modal DOM
    const overlay = document.createElement('div');
    overlay.style.cssText = 'position:fixed;inset:0;background:rgba(0,0,0,0.5);z-index:10000;display:flex;align-items:center;justify-content:center';

    const dialog = document.createElement('div');
    dialog.style.cssText = 'background:var(--color-bg-primary,#1e1e2e);color:var(--color-text-primary,#cdd6f4);border-radius:12px;padding:24px;min-width:340px;max-width:90vw;font-family:inherit;box-shadow:0 8px 32px rgba(0,0,0,0.4)';

    // Built with createElement + `.style`, NOT innerHTML, for two reasons.
    //
    // 1. CSP. The production policy is `style-src 'self'` with NO
    //    'unsafe-inline' (src-tauri/tauri.conf.json). A `style="..."` attribute
    //    inside PARSED markup is governed by that directive and is refused;
    //    the `.style` property is CSSOM, which CSP does not police - which is
    //    also why the two `style.cssText` assignments above are fine. This
    //    template previously carried seven inline style attributes, so every
    //    control here would have rendered unstyled in a packaged build. Nothing
    //    catches that locally: `devCsp` keeps 'unsafe-inline', so dev,
    //    Playwright and vitest all exercise the permissive policy only.
    //
    // 2. Injection. The old template interpolated `title`, `defaultFilename`
    //    and each directory's label/path straight into markup with no escaping.
    //    Those derive from a file path, so a filename containing a quote and an
    //    angle bracket could close an attribute and inject an element into the
    //    app's own trusted document. Text nodes and `.value` cannot do that.
    const style = (el: HTMLElement, css: Record<string, string>) => {
      for (const [k, v] of Object.entries(css)) el.style.setProperty(k, v);
    };
    const FIELD: Record<string, string> = {
      width: '100%',
      padding: '8px',
      'border-radius': '6px',
      border: '1px solid var(--color-border,#45475a)',
      background: 'var(--color-bg-secondary,#313244)',
      color: 'inherit',
      'font-size': '14px',
    };
    const LABEL: Record<string, string> = {
      display: 'block',
      'font-size': '13px',
      color: 'var(--color-text-secondary,#a6adc8)',
      'margin-bottom': '4px',
    };
    const BUTTON: Record<string, string> = {
      padding: '8px 16px',
      'border-radius': '6px',
      cursor: 'pointer',
      'font-size': '14px',
    };

    const heading = document.createElement('h3');
    heading.textContent = options?.title ?? 'Save File';
    style(heading, { margin: '0 0 16px', 'font-size': '16px', 'font-weight': '600' });

    const dirLabel = document.createElement('label');
    dirLabel.textContent = 'Save to';
    style(dirLabel, LABEL);

    const dirSelectEl = document.createElement('select');
    dirSelectEl.id = 'android-save-dir';
    style(dirSelectEl, { ...FIELD, 'margin-bottom': '12px' });
    dirs.forEach((d, i) => {
      const opt = document.createElement('option');
      opt.value = String(i);
      opt.textContent = `${d.label} (${d.path})`;
      dirSelectEl.appendChild(opt);
    });

    const nameLabel = document.createElement('label');
    nameLabel.textContent = 'File name';
    style(nameLabel, LABEL);

    const nameInput = document.createElement('input');
    nameInput.id = 'android-save-filename';
    nameInput.type = 'text';
    nameInput.value = defaultFilename;
    style(nameInput, { ...FIELD, 'box-sizing': 'border-box' });

    const actions = document.createElement('div');
    style(actions, {
      display: 'flex',
      gap: '8px',
      'justify-content': 'flex-end',
      'margin-top': '20px',
    });

    const cancel = document.createElement('button');
    cancel.id = 'android-save-cancel';
    cancel.textContent = 'Cancel';
    style(cancel, {
      ...BUTTON,
      border: '1px solid var(--color-border,#45475a)',
      background: 'transparent',
      color: 'inherit',
    });

    const confirm = document.createElement('button');
    confirm.id = 'android-save-confirm';
    confirm.textContent = 'Save';
    style(confirm, {
      ...BUTTON,
      border: 'none',
      background: 'var(--color-accent,#89b4fa)',
      color: 'var(--color-bg-primary,#1e1e2e)',
      'font-weight': '600',
    });

    actions.append(cancel, confirm);
    dialog.append(heading, dirLabel, dirSelectEl, nameLabel, nameInput, actions);

    overlay.appendChild(dialog);
    document.body.appendChild(overlay);

    const filenameInput = dialog.querySelector('#android-save-filename') as HTMLInputElement;
    const dirSelect = dialog.querySelector('#android-save-dir') as HTMLSelectElement;
    const confirmBtn = dialog.querySelector('#android-save-confirm') as HTMLButtonElement;
    const cancelBtn = dialog.querySelector('#android-save-cancel') as HTMLButtonElement;

    // Select filename without extension for easy editing
    requestAnimationFrame(() => {
      filenameInput.focus();
      const dotIndex = defaultFilename.lastIndexOf('.');
      if (dotIndex > 0) {
        filenameInput.setSelectionRange(0, dotIndex);
      }
    });

    const cleanup = () => { document.body.removeChild(overlay); };

    confirmBtn.addEventListener('click', () => {
      const dir = dirs[parseInt(dirSelect.value)];
      const fname = filenameInput.value.trim();
      cleanup();
      if (!fname || !dir) { resolve(null); return; }
      resolve(`${dir.path}/${fname}`);
    });

    cancelBtn.addEventListener('click', () => {
      cleanup();
      resolve(null);
    });

    overlay.addEventListener('click', (e) => {
      if (e.target === overlay) { cleanup(); resolve(null); }
    });
  });
}
