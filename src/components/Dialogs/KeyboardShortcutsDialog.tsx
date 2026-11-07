import { useState, useMemo, useCallback } from 'react';
import { useTranslation } from 'react-i18next';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import { defaultConfig } from '../../types/config';
import type { ShortcutsConfig } from '../../types/config';
import {
  formatShortcut,
  formatActionName,
  isMac,
} from '../../utils/shortcuts';
import {
  SHORTCUT_CATEGORIES,
  RESERVED_DISPLAY_ENTRIES,
} from '../../utils/shortcutCategories';
import { DialogWrapper } from './DialogWrapper';
import { Button } from '../common/Button';
import { setSource, trackDialogComplete } from '../../utils/telemetry';
import { useShallow } from 'zustand/react/shallow';

export default function KeyboardShortcutsDialog() {
  const { t } = useTranslation();
  const { closeDialog, openDialog } = useUIStore(
    useShallow((s) => ({ closeDialog: s.closeDialog, openDialog: s.openDialog })),
  );
  const config = useProjectStore((s) => s.config);
  const shortcuts = config?.shortcuts ?? defaultConfig.shortcuts;

  const [filter, setFilter] = useState('');
  const mac = isMac();

  const filteredCategories = useMemo(() => {
    const search = filter.trim().toLowerCase();
    if (!search) return SHORTCUT_CATEGORIES;

    return SHORTCUT_CATEGORIES.map((cat) => ({
      ...cat,
      actions: cat.actions.filter((action) => {
        const name = formatActionName(action).toLowerCase();
        if (name.includes(search)) return true;
        const bindings = shortcuts[action];
        if (Array.isArray(bindings)) {
          for (const b of bindings) {
            if (formatShortcut(b, mac).toLowerCase().includes(search)) return true;
            if (b.toLowerCase().includes(search)) return true;
          }
        }
        return false;
      }),
    })).filter((cat) => cat.actions.length > 0);
  }, [filter, shortcuts, mac]);

  const filteredReserved = useMemo(() => {
    const search = filter.trim().toLowerCase();
    if (!search) return RESERVED_DISPLAY_ENTRIES;

    return RESERVED_DISPLAY_ENTRIES.filter((entry) => {
      const desc = t(entry.descriptionKey).toLowerCase();
      if (desc.includes(search)) return true;
      return entry.shortcuts.some(
        (s) =>
          formatShortcut(s, mac).toLowerCase().includes(search) ||
          s.toLowerCase().includes(search),
      );
    });
  }, [filter, mac, t]);

  const hasResults = filteredCategories.length > 0 || filteredReserved.length > 0;

  const handleClose = useCallback(() => {
    trackDialogComplete('keyboard-shortcuts');
    closeDialog();
  }, [closeDialog]);

  const handleCustomize = useCallback(() => {
    trackDialogComplete('keyboard-shortcuts');
    closeDialog();
    setSource('dialog');
    setTimeout(() => openDialog('settings', { initialCategory: 'shortcuts' }), 0);
  }, [closeDialog, openDialog]);

  return (
    <DialogWrapper
      title={t('keyboardShortcuts.title')}
      onClose={handleClose}
      size="5xl"
      overflow="hidden"
      bodyClassName="flex-1 flex flex-col overflow-hidden"
      data-testid="keyboard-shortcuts-dialog"
    >
      <div className="flex flex-col flex-1 min-h-0 gap-3 p-4">
        {/* Filter input */}
        <div className="relative shrink-0">
          <input
            type="text"
            data-autofocus
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            placeholder={t('keyboardShortcuts.search.placeholder')}
            aria-label={t('keyboardShortcuts.search.aria')}
            className="w-full px-3 py-2 pl-9 bg-background border border-border rounded text-sm"
            data-testid="keyboard-shortcuts-search"
          />
          <svg
            className="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-text-secondary"
            fill="none"
            stroke="currentColor"
            viewBox="0 0 24 24"
          >
            <path
              strokeLinecap="round"
              strokeLinejoin="round"
              strokeWidth={2}
              d="M21 21l-6-6m2-5a7 7 0 11-14 0 7 7 0 0114 0z"
            />
          </svg>
        </div>

        {/* Scrollable grid */}
        <div className="flex-1 overflow-auto min-h-0">
          {!hasResults ? (
            <p className="text-text-secondary text-sm text-center py-8">
              {t('keyboardShortcuts.noResults')}
            </p>
          ) : (
            <div className="h-[70vh] max-h-full">
              <div className="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-4">
                {filteredCategories.map((cat) => (
                  <CategoryCard
                    key={cat.id}
                    labelKey={cat.labelKey}
                    actions={cat.actions}
                    shortcuts={shortcuts}
                    mac={mac}
                  />
                ))}
              </div>

              {/* Reserved section - full width */}
              {filteredReserved.length > 0 && (
                <div className="mt-4">
                  <h3 className="text-sm font-medium text-text-secondary mb-2">
                    {t('keyboardShortcuts.reserved.title')}
                  </h3>
                  <div className="bg-background-50 rounded-lg p-3 grid grid-cols-1 md:grid-cols-2 gap-2 text-sm">
                    {filteredReserved.map(({ shortcuts: keys, descriptionKey }) => (
                      <div key={keys.join('+')} className="flex items-center gap-2">
                        <span className="inline-flex items-center gap-1 shrink-0">
                          {keys.map((s, i) => (
                            <span key={s} className="inline-flex items-center gap-1">
                              {i > 0 && <span className="text-text-secondary text-xs">/</span>}
                              <kbd className="inline-flex items-center justify-center px-1.5 py-0.5 text-xs font-medium bg-panel border border-border rounded min-w-[1.5rem] text-text-secondary">
                                {formatShortcut(s, mac)}
                              </kbd>
                            </span>
                          ))}
                        </span>
                        <span className="text-text-secondary text-xs">{t(descriptionKey)}</span>
                      </div>
                    ))}
                  </div>
                </div>
              )}
            </div>
          )}
        </div>

        {/* Footer */}
        <div className="shrink-0 flex justify-end border-t border-border pt-3">
          <Button variant="secondary" onClick={handleCustomize} data-testid="keyboard-shortcuts-customize">
            {t('keyboardShortcuts.customize')}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}

function CategoryCard({
  labelKey,
  actions,
  shortcuts,
  mac,
}: {
  labelKey: string;
  actions: (keyof ShortcutsConfig)[];
  shortcuts: ShortcutsConfig;
  mac: boolean;
}) {
  const { t } = useTranslation();

  return (
    <div className="bg-panel-20 rounded-lg p-3">
      <h3 className="text-sm font-medium mb-2">{t(labelKey)}</h3>
      <div className="space-y-1">
        {actions.map((action) => {
          const bindings = shortcuts[action] || [];
          return (
            <div key={action} className="flex items-center justify-between gap-2 py-0.5">
              <span className="text-sm text-text-primary truncate">
                {formatActionName(action)}
              </span>
              <span className="inline-flex items-center gap-1 shrink-0">
                {bindings.length > 0 ? (
                  bindings.map((b, i) => (
                    <span key={`${b}-${i}`} className="inline-flex items-center gap-1">
                      {i > 0 && <span className="text-text-secondary text-xs">/</span>}
                      <kbd className="inline-flex items-center justify-center px-1.5 py-0.5 text-xs font-medium bg-panel border border-border rounded min-w-[1.5rem] text-text-secondary whitespace-nowrap">
                        {formatShortcut(b, mac)}
                      </kbd>
                    </span>
                  ))
                ) : (
                  <span className="text-text-secondary text-xs italic">-</span>
                )}
              </span>
            </div>
          );
        })}
      </div>
    </div>
  );
}
