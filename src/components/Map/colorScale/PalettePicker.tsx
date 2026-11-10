import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from 'react';
import { useTranslation } from 'react-i18next';
import { useAnchoredFixedMenu } from '../../../hooks/useAnchoredFixedMenu';
import { useDismissableLayer } from '../../../hooks/useDismissableLayer';
import { ChevronDownIcon, PinIcon } from '../../common/icons';
import { getMapThemeColors } from '../../../utils/themeColors';
import {
  ALL_PALETTES,
  PALETTE_GROUPS,
  cssGradient,
  findPalette,
  getPalette,
  paletteMatches,
  parsePaletteQuery,
  resolveChoice,
  THEME_PALETTE_ID,
  type ColorScaleKind,
  type Palette,
  type PaletteChoice,
  type PaletteGroupId,
} from '../../../utils/palettes';

export interface PalettePickerProps {
  kind: ColorScaleKind;
  /** The persisted choice (what the trigger shows and what Escape reverts to). */
  value: PaletteChoice;
  pinned: readonly string[];
  /** Hover / arrow preview (null = back to `value`). */
  onPreview: (choice: PaletteChoice | null) => void;
  onSelect: (choice: PaletteChoice) => void;
  onTogglePin: (id: string) => void;
  testIdPrefix: string;
}

interface Row {
  group: PaletteGroupId;
  palette: Palette;
}

const PANEL_WIDTH = 300;

/**
 * Searchable, grouped palette picker. A dedicated combobox rather than the
 * shared `Select`: it needs gradient swatches in both the trigger and the rows,
 * collapsible folders, a pin toggle inside each row and a live preview on
 * hover / arrow keys, none of which `Select` offers.
 *
 * Keyboard: focus stays in the search box; Up / Down move through the visible
 * palettes (previewing each on the map), Enter picks, Escape closes and
 * reverts the preview. A trailing `_r` (matplotlib's reversed-map suffix)
 * picks the palette reversed.
 */
export function PalettePicker({
  kind,
  value,
  pinned,
  onPreview,
  onSelect,
  onTogglePin,
  testIdPrefix,
}: PalettePickerProps) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState('');
  const [active, setActive] = useState(-1);
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set());
  const triggerRef = useRef<HTMLButtonElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const endpoints = getMapThemeColors();

  const nameOf = (p: Palette) => (p.id === THEME_PALETTE_ID ? t('map.colorScale.themePalette') : p.name);
  const groupLabel = (g: string) => t(`map.colorScale.groups.${g}`);
  const swatch = (id: string, reversed: boolean) => cssGradient(resolveChoice({ id, reversed }, kind, endpoints));

  const { text, reversed: queryReversed } = parsePaletteQuery(query);
  const reversed = queryReversed || value.reversed;
  const searching = text.length > 0;

  const groups = useMemo(() => {
    const out: Array<{ id: PaletteGroupId; rows: Row[] }> = [];
    for (const g of PALETTE_GROUPS) {
      const members =
        g === 'pinned'
          ? pinned.map((id) => findPalette(id)).filter((p): p is Palette => p !== undefined)
          : ALL_PALETTES.filter((p) => p.group === g);
      const rows = members
        .filter((p) => paletteMatches(p, text, `${groupLabel(p.group)} ${p.id === THEME_PALETTE_ID ? nameOf(p) : ''}`))
        .map((palette) => ({ group: g, palette }));
      if (rows.length > 0) out.push({ id: g, rows });
    }
    return out;
    // groupLabel / nameOf only read `t`.
  }, [pinned, text, t]);

  const isOpenGroup = (g: string) => searching || expanded.has(g);
  const visible: Row[] = groups.flatMap((g) => (isOpenGroup(g.id) ? g.rows : []));

  // Open with Pinned, Theme and the current palette's folder expanded.
  const openPanel = () => {
    setExpanded(new Set(['pinned', 'theme', getPalette(value.id).group]));
    setQuery('');
    setActive(-1);
    setOpen(true);
  };
  const close = () => {
    setOpen(false);
    onPreview(null);
  };
  const pick = (palette: Palette) => {
    setOpen(false);
    onPreview(null);
    onSelect({ id: palette.id, reversed });
  };
  const previewRow = (i: number) => {
    setActive(i);
    const row = visible[i];
    onPreview(row ? { id: row.palette.id, reversed } : null);
  };

  const menuStyle = useAnchoredFixedMenu(open, triggerRef, panelRef, {
    align: 'center',
    gap: 4,
    width: PANEL_WIDTH,
    maxHeightCap: 420,
    remeasureKey: `${visible.length}`,
  });
  useDismissableLayer({ open, onDismiss: close, contains: [panelRef, triggerRef], survivesUnrelatedScroll: true });

  // Keep the active row in view while arrowing.
  useEffect(() => {
    if (active < 0) return;
    listRef.current?.querySelector(`[data-row-index="${active}"]`)?.scrollIntoView?.({ block: 'nearest' });
  }, [active]);

  const onSearchKey = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      if (visible.length === 0) return;
      const step = e.key === 'ArrowDown' ? 1 : -1;
      previewRow(active < 0 ? (step > 0 ? 0 : visible.length - 1) : (active + step + visible.length) % visible.length);
    } else if (e.key === 'Enter') {
      e.preventDefault();
      const row = visible[active >= 0 ? active : 0];
      if (row) pick(row.palette);
    } else if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      close();
      triggerRef.current?.focus();
    }
  };

  const current = getPalette(value.id);
  let rowIndex = -1;

  return (
    <div className="min-w-0 flex-1">
      <button
        ref={triggerRef}
        type="button"
        onClick={() => (open ? close() : openPanel())}
        aria-haspopup="listbox"
        aria-expanded={open}
        className="w-full flex items-center gap-2 px-1.5 py-1 rounded border border-border bg-background text-xs text-text-primary hover:bg-accent-10 focus:outline-none focus:ring-1 focus:ring-accent"
        data-testid={`${testIdPrefix}-trigger`}
      >
        <span
          className="h-3 w-16 flex-shrink-0 rounded-sm border border-border"
          style={{ background: swatch(value.id, value.reversed) }}
          aria-hidden="true"
        />
        <span className="truncate flex-1 text-left" data-testid={`${testIdPrefix}-current`}>
          {nameOf(current)}
        </span>
        <ChevronDownIcon size={12} />
      </button>
      {open && (
        <div
          ref={panelRef}
          className="z-50 flex flex-col rounded border border-border bg-panel shadow-lg text-xs"
          style={menuStyle}
          data-dropdown-open
          onMouseDown={(e) => e.stopPropagation()}
          data-testid={`${testIdPrefix}-panel`}
        >
          <div className="p-1.5 border-b border-border">
            <input
              autoFocus
              type="text"
              value={query}
              onChange={(e) => {
                setQuery(e.target.value);
                setActive(-1);
              }}
              onKeyDown={onSearchKey}
              placeholder={t('map.colorScale.search')}
              aria-label={t('map.colorScale.search')}
              role="combobox"
              aria-expanded
              aria-controls={`${testIdPrefix}-listbox`}
              className="w-full px-1.5 py-1 bg-background border border-border rounded focus:outline-none focus:ring-1 focus:ring-accent"
              data-testid={`${testIdPrefix}-search`}
            />
          </div>
          <div
            ref={listRef}
            id={`${testIdPrefix}-listbox`}
            role="listbox"
            className="overflow-auto py-1"
            onMouseLeave={() => onPreview(null)}
          >
            {groups.length === 0 && (
              <div className="px-2 py-2 text-text-secondary" data-testid={`${testIdPrefix}-no-results`}>
                {t('map.colorScale.noResults')}
              </div>
            )}
            {groups.map((g) => (
              <div key={g.id} role="group" aria-label={groupLabel(g.id)}>
                <button
                  type="button"
                  aria-expanded={isOpenGroup(g.id)}
                  disabled={searching}
                  onClick={() =>
                    setExpanded((prev) => {
                      const next = new Set(prev);
                      if (next.has(g.id)) next.delete(g.id);
                      else next.add(g.id);
                      return next;
                    })
                  }
                  className="w-full flex items-center gap-1 px-2 py-1 text-[11px] font-medium text-text-secondary hover:text-text-primary"
                  data-testid={`${testIdPrefix}-group-${g.id}`}
                >
                  <span className={`inline-block transition-transform ${isOpenGroup(g.id) ? '' : '-rotate-90'}`}>
                    <ChevronDownIcon size={10} />
                  </span>
                  {groupLabel(g.id)}
                  <span className="ml-auto opacity-60">{g.rows.length}</span>
                </button>
                {isOpenGroup(g.id) &&
                  g.rows.map((row) => {
                    rowIndex += 1;
                    const i = rowIndex;
                    const id = row.palette.id;
                    const isPinned = pinned.includes(id);
                    const selected = id === value.id;
                    const rowTestId = row.group === 'pinned' ? `${testIdPrefix}-pinned-${id}` : `${testIdPrefix}-option-${id}`;
                    const pinTestId = row.group === 'pinned' ? `${testIdPrefix}-pinned-pin-${id}` : `${testIdPrefix}-pin-${id}`;
                    return (
                      <div
                        key={`${row.group}:${id}`}
                        role="option"
                        aria-selected={selected}
                        data-row-index={i}
                        onMouseEnter={() => previewRow(i)}
                        onClick={() => pick(row.palette)}
                        className={`group flex items-center gap-2 pl-5 pr-1.5 py-1 cursor-pointer ${
                          i === active ? 'bg-accent-20' : selected ? 'bg-accent-10' : 'hover:bg-accent-10'
                        }`}
                        data-testid={rowTestId}
                      >
                        <span
                          className="h-3 w-20 flex-shrink-0 rounded-sm border border-border"
                          style={{ background: swatch(id, reversed) }}
                          aria-hidden="true"
                        />
                        <span className="truncate flex-1 text-text-primary">{nameOf(row.palette)}</span>
                        <button
                          type="button"
                          aria-pressed={isPinned}
                          aria-label={t(isPinned ? 'map.colorScale.unpin' : 'map.colorScale.pin', { name: nameOf(row.palette) })}
                          title={t(isPinned ? 'map.colorScale.unpin' : 'map.colorScale.pin', { name: nameOf(row.palette) })}
                          onClick={(e) => {
                            e.stopPropagation();
                            onTogglePin(id);
                          }}
                          className={`p-0.5 rounded ${
                            isPinned ? 'text-accent' : 'text-text-secondary opacity-0 group-hover:opacity-100 focus:opacity-100'
                          } hover:text-text-primary`}
                          data-testid={pinTestId}
                        >
                          <PinIcon size={12} />
                        </button>
                      </div>
                    );
                  })}
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
