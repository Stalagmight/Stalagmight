import i18n from '../i18n';
import { invokeIpc } from '../utils/ipc';
import { withThemeDepthRamp } from '../utils/exportOptions';
import { TOAST_KEYS } from '../utils/toastKeys';
import { useUIStore } from './uiStore';
import type { ExportImageOptions } from '../types/commands';

/**
 * The file-export actions: CSV, JSON, Ariane TML and map image.
 *
 * Split out of `projectStore.ts` rather than rebaselined, for the reason
 * `projectConfigActions.ts` gives and #157 requires: that file is past the
 * 1500-LOC ratchet, so it may shrink but not grow, and a file at that size is split
 * at an API boundary instead of having its baseline raised. Same factory shape as
 * its siblings, spread into `create<ProjectStore>`; these four read and write no
 * store state, so the factory takes no `set`/`get`.
 */
export interface ProjectExportActions {
  exportCsv: (path: string) => Promise<void>;
  exportJson: (path: string) => Promise<void>;
  exportTml: (path: string) => Promise<void>;
  exportImage: (path: string, options: ExportImageOptions) => Promise<void>;
}

/**
 * An export note's code and the values it carries. `count` is a number, so
 * the copy can pluralise on it; every other value is shown as sent.
 */
function parseExportNote(note: string): { code: string; values: Record<string, string | number> } {
  const at = note.indexOf('?');
  if (at < 0) return { code: note, values: {} };
  const values: Record<string, string | number> = Object.fromEntries(
    new URLSearchParams(note.slice(at + 1)),
  );
  if (typeof values.count === 'string') values.count = Number(values.count);
  return { code: note.slice(0, at), values };
}

export function createProjectExportActions(): ProjectExportActions {
  return {
    exportCsv: async (path: string) => {
      await invokeIpc('export_csv', { path }, {
        purpose: 'export project as CSV',
        fields: { path },
      });
    },

    exportJson: async (path: string) => {
      await invokeIpc('export_json', { path }, {
        purpose: 'export project as JSON',
        fields: { path },
      });
    },

    exportTml: async (path: string) => {
      // The command returns NOTES (#438): what the single-flag `.tml` format
      // could not carry about this project's north references. Empty for an
      // ordinary project. Raised here rather than in `ExportControls` so every
      // route reaches the user - the modal, the Export-mode panel and this action
      // called directly.
      const notes = await invokeIpc('export_tml', { path }, {
        purpose: 'export project as Ariane TML',
        fields: { path },
      });
      for (const note of notes ?? []) {
        // `warning`, per docs/notifications.md: the export SUCCEEDED, with a
        // caveat about what the file says. The copy is i18n's, never the
        // backend's - the note is a stable code, optionally followed by its
        // values as a query (`junction-depths-averaged?count=2&largest=0.40`,
        // #576 TML-DEPTH-D2), which the copy interpolates.
        // `exists` rather than `t`'s defaultValue: a note code this build has no
        // copy for must show as the code, never as the i18n key path, which is
        // what a bare `t()` renders for a miss.
        const { code, values } = parseExportNote(note);
        const key = `dialogs.export.notes.${code}`;
        // Keyed per CODE: one export can return several notes (#378 added a
        // second possible code), and a shared key would let the second toast
        // replace the first inside the burst window.
        useUIStore.getState().showToast(
          i18n.exists(key, values) ? i18n.t(key, values) : note,
          `${TOAST_KEYS.EXPORT_NOTE}:${code}`,
          'warning',
        );
      }
    },

    exportImage: async (path: string, options: ExportImageOptions) => {
      // Stamped here, not at the callers (#360): this is the one funnel every
      // image export passes through. Unconditional - the option is inert for the
      // non-depth modes. See `utils/exportOptions.ts`.
      await invokeIpc('export_image', {
        path,
        options: withThemeDepthRamp(options),
      }, {
        purpose: 'export map image',
        fields: { path, format: options.format },
      });
    },
  };
}
