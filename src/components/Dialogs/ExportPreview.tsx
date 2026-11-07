import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { invokeIpc } from '../../utils/ipc';
import { withThemeDepthRamp } from '../../utils/exportOptions';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import type { ExportImageOptions } from '../../types/commands';

interface ExportPreviewProps {
  /** Export options that affect preview appearance. The SVG returned by
   *  `preview_svg_export` is pixel-for-pixel what the backend will write
   *  for these options, so preview and export cannot drift. */
  options: ExportImageOptions;
  /** When true, fill the parent container instead of the fixed preview box.
   *  Used by the large WYSIWYG preview in the Export-mode map area. */
  fill?: boolean;
}

/** Debounce delay in milliseconds */
const DEBOUNCE_DELAY = 250;

/** Preview canvas dimensions (CSS pixels). */
const PREVIEW_SIZE = 300;

/**
 * Inline-SVG preview of the map export. Calls the backend's
 * `preview_svg_export` command (debounced) so the preview is the *exact*
 * markup the export will write - no separate rendering pipeline.
 */
export default function ExportPreview({ options, fill = false }: ExportPreviewProps) {
  const { t } = useTranslation();
  const project = useProjectStore((state) => state.project);
  // The depth endpoints this preview sends are read from the live document, so
  // the theme is an INPUT to the request even though it appears nowhere in
  // `options` (#360). Without it the Export-mode panel keeps showing the old
  // theme's ramp after an in-place theme switch, while the file written from
  // the same options follows the new one - the drift this was meant to close,
  // moved rather than removed.
  const resolvedTheme = useUIStore((state) => state.resolvedTheme);

  const [svg, setSvg] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [isLoading, setIsLoading] = useState(true);

  // Track in-flight requests so a stale response can't overwrite a newer one.
  const requestSeqRef = useRef(0);
  const timeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    if (!project) {
      // Project closed (or never opened). Bump the seq so any earlier
      // in-flight `preview_svg_export` call's response is treated as
      // stale and discarded - otherwise it'd repopulate the preview
      // with SVG from a project that's no longer open.
      ++requestSeqRef.current;
      if (timeoutRef.current) {
        clearTimeout(timeoutRef.current);
        timeoutRef.current = null;
      }
      setSvg(null);
      setError(null);
      setIsLoading(false);
      return;
    }

    setIsLoading(true);
    if (timeoutRef.current) {
      clearTimeout(timeoutRef.current);
    }

    const seq = ++requestSeqRef.current;
    timeoutRef.current = setTimeout(() => {
      // `preview_svg_export` always returns SVG markup regardless of `format`
      // (PNG/PDF are SVG-derived), but the format DOES drive whether colors are
      // emitted (SVG is always monochrome). Pass the real options through so the
      // preview matches what the chosen format will actually write - including
      // the live theme's depth endpoints, which the backend cannot resolve and
      // which the export path stamps on the same way (#360).
      invokeIpc('preview_svg_export', { options: withThemeDepthRamp(options) }, {
        purpose: 'render export preview',
      })
        .then((markup) => {
          if (seq !== requestSeqRef.current) return; // stale
          setSvg(markup);
          setError(null);
          setIsLoading(false);
        })
        .catch((err: unknown) => {
          if (seq !== requestSeqRef.current) return; // stale
          setSvg(null);
          setError(err instanceof Error ? err.message : String(err));
          setIsLoading(false);
        });
    }, DEBOUNCE_DELAY);

    return () => {
      if (timeoutRef.current) {
        clearTimeout(timeoutRef.current);
        timeoutRef.current = null;
      }
    };
    // `resolvedTheme` is a dependency for the depth endpoints `withThemeDepthRamp`
    // reads off the document, not for anything in `options` - see above.
  }, [project, options, resolvedTheme]);

  return (
    <div
      data-testid="export-preview"
      className={`relative border border-border rounded overflow-hidden bg-white flex items-center justify-center ${
        fill ? 'w-full h-full' : ''
      }`}
      style={fill ? undefined : { width: PREVIEW_SIZE, height: PREVIEW_SIZE }}
    >
      {/* Inline SVG - the backend output is `<?xml ...?><svg .../>`. The
          wrapper div constrains the size; the SVG carries its own viewBox
          and scales to fit. */}
      {svg && (
        <div
          className="w-full h-full [&>svg]:w-full [&>svg]:h-full"
          dangerouslySetInnerHTML={{ __html: svg }}
        />
      )}

      {/* Loading overlay */}
      {isLoading && (
        <div
          data-testid="export-preview-loading"
          className="absolute inset-0 flex items-center justify-center bg-white/80"
        >
          <div className="flex flex-col items-center gap-2">
            <div className="w-6 h-6 border-2 border-accent border-t-transparent rounded-full animate-spin" />
            <span className="text-xs text-text-secondary">{t('dialogs.export.previewRendering')}</span>
          </div>
        </div>
      )}

      {/* Empty state when no project */}
      {!project && !isLoading && (
        <div className="absolute inset-0 flex items-center justify-center text-text-secondary text-sm">
          {t('dialogs.export.previewNoProject')}
        </div>
      )}

      {/* Error state */}
      {error && !isLoading && (
        <div
          className="absolute inset-0 flex items-center justify-center p-3 text-text-secondary text-xs text-center"
          data-testid="export-preview-error"
        >
          {t('dialogs.export.previewFailed', { error })}
        </div>
      )}
    </div>
  );
}
