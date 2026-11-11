import { useEffect, useRef, useState, type CSSProperties, type ReactNode, type RefObject } from 'react';
import { createPortal } from 'react-dom';
import { useTranslation } from 'react-i18next';
import { IconButton } from '../../common/Button';
import { SettingsIcon } from '../../common/icons';
import { useAnchoredFixedMenu } from '../../../hooks/useAnchoredFixedMenu';
import { useDismissableLayer } from '../../../hooks/useDismissableLayer';

export interface LegendCogPopoverProps {
  /** Test-id stem: `<id>-legend-settings-button`, `color-scale-popover-<id>`. */
  id: string;
  /** The popover's heading and accessible name. */
  title: string;
  width: number;
  /** Runs whenever the popover closes (outside click, Escape, the cog), to end
   *  any preview it started. */
  onClose: () => void;
  /** The popover body, rendered only while open. `popRef` is the popover
   *  element (for menus that must portal inside it). */
  children: (popRef: RefObject<HTMLDivElement | null>) => ReactNode;
}

/**
 * The small cog beside a map legend and the popover it opens. The popover
 * renders into document.body (the legend sits in a z-index:1 box the side
 * panels and data table would paint over) and closes on an outside click or
 * Escape.
 */
export function LegendCogPopover({ id, title, width, onClose, children }: LegendCogPopoverProps) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const wrapRef = useRef<HTMLDivElement>(null);
  const cogRef = useRef<HTMLDivElement>(null);
  const popRef = useRef<HTMLDivElement>(null);
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;
  const openRef = useRef(false);
  openRef.current = open;

  // The placement hook reads the toolbar / status-bar heights as CSS variables
  // inherited from the app root, which a body-level element no longer
  // inherits, so carry them over.
  const [chromeVars, setChromeVars] = useState<CSSProperties>({});
  const openPopover = () => {
    const el = cogRef.current;
    if (el) {
      const cs = getComputedStyle(el);
      setChromeVars({
        '--toolbar-h': cs.getPropertyValue('--toolbar-h'),
        '--statusbar-h': cs.getPropertyValue('--statusbar-h'),
      } as CSSProperties);
    }
    setOpen(true);
  };

  const close = () => {
    setOpen(false);
    onCloseRef.current();
  };

  const style = useAnchoredFixedMenu(open, cogRef, popRef, { align: 'center', gap: 6, width });
  useDismissableLayer({ open, onDismiss: close, contains: [wrapRef, popRef], survivesUnrelatedScroll: true });

  // Escape closes the popover (an inner field or the palette list claims it
  // first by stopping it). The global Escape handler stands down while any
  // [data-dropdown-open] is present, so it does not also clear the selection.
  useEffect(() => {
    if (!open) return undefined;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape' || e.defaultPrevented) return;
      e.preventDefault();
      close();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
    // close only touches local state and the latest onClose.
  }, [open]);

  // Closed by unmounting (the legend went away): still end any preview.
  useEffect(
    () => () => {
      if (openRef.current) onCloseRef.current();
    },
    [],
  );

  return (
    <div
      ref={wrapRef}
      onMouseDown={(e) => e.stopPropagation()}
      onPointerDown={(e) => e.stopPropagation()}
      onWheel={(e) => e.stopPropagation()}
    >
      <div ref={cogRef}>
        <IconButton
          icon={<SettingsIcon size={14} />}
          size="sm"
          variant="ghost"
          active={open}
          tooltip={t('map.colorScale.settingsButton')}
          aria-label={t('map.colorScale.settingsButton')}
          aria-expanded={open}
          onClick={() => (open ? close() : openPopover())}
          className="bg-[var(--cc-text-halo-alt)]"
          data-testid={`${id}-legend-settings-button`}
        />
      </div>
      {open && createPortal(
        <div
          ref={popRef}
          role="dialog"
          aria-label={title}
          className="z-50 flex flex-col gap-2 rounded border border-border bg-panel p-2 shadow-lg text-text-primary"
          style={{ ...chromeVars, ...style }}
          data-dropdown-open
          onMouseDown={(e) => e.stopPropagation()}
          onPointerDown={(e) => e.stopPropagation()}
          onWheel={(e) => e.stopPropagation()}
          data-testid={`color-scale-popover-${id}`}
        >
          <span className="text-xs font-medium">{title}</span>
          {children(popRef)}
        </div>,
        document.body,
      )}
    </div>
  );
}
