import { useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { speleoReadOnlyReason, speleoReadOnlyReasonKey } from '../../utils/speleoReadOnlyReason';
import { SpeleoWorkspace } from './SpeleoWorkspace';
import { SpeleoNetworkBusyIndicator } from './SpeleoNetworkBusyIndicator';
import { SectionsTabIcon, SpeleoTabIcon, useSpeleoTabState } from './workspaceTabIcons';
import type { ReactNode } from 'react';

export function LeftWorkspace({ sections }: { sections: ReactNode }) {
  const { t } = useTranslation();
  const tab = useUIStore((s) => s.leftWorkspace);
  const mode = useUIStore((s) => s.appMode);
  const refs = useRef<Array<HTMLButtonElement | null>>([]);
  const tabs = mode === 'survey' ? ['sections', 'speleodb'] as const : ['speleodb'] as const;
  const choose = useUIStore((s) => s.selectLeftWorkspace);
  const speleoState = useSpeleoTabState();
  const link = useSpeleoStore((s) => s.link);
  const session = useSpeleoStore((s) => s.session);
  // The tab's own words for its badge, as its accessible description (and tooltip).
  // Read-only says why in the same words as the map's pill and the workspace header:
  // signed out, view-only access, or just not editing right now.
  const readOnlyReason = speleoState === 'read-only' ? speleoReadOnlyReason(link, session) : null;
  const speleoNote = speleoState === 'unuploaded' ? t('speleodb.workspaceTab.unuploaded')
    : speleoState !== 'read-only' ? null
    : readOnlyReason ? t(speleoReadOnlyReasonKey(readOnlyReason)) : t('speleodb.workspaceTab.readOnly');
  return (
    <div className="h-full min-h-0 flex flex-col">
      <div role="tablist" aria-label={t('speleodb.workspaceTab.tablist')} className="flex shrink-0 border-b border-border">
        {tabs.map((id, index) => {
          const selected = tab === id;
          const warn = id === 'speleodb' && speleoState === 'unuploaded';
          const note = id === 'speleodb' ? speleoNote : null;
          // Every tab is selected the same way (the accent wash and medium weight).
          // Un-uploaded work changes only the text colour: the warning text, dimmed
          // while the tab is not selected (as idle tabs are), full strength when it
          // is. No wash or rule of its own, so it never looks selected when it is not.
          const look = selected
            ? `bg-accent-10 font-medium${warn ? ' text-warning-text' : ''}`
            : warn ? 'text-warning-text-dim' : 'text-text-secondary';
          return <button key={id} ref={(el) => { refs.current[index] = el; }}
            id={`workspace-tab-${id}`} role="tab" type="button" aria-selected={selected}
            aria-controls={`workspace-panel-${id}`} tabIndex={selected ? 0 : -1}
            aria-describedby={note ? 'workspace-tab-speleodb-note' : undefined}
            title={note ?? undefined}
            data-testid={`workspace-tab-${id}`} data-state={id === 'speleodb' ? speleoState : undefined}
            onClick={() => choose(id)}
            onKeyDown={(event) => {
              const next = event.key === 'ArrowRight' ? (index + 1) % tabs.length : event.key === 'ArrowLeft' ? (index + tabs.length - 1) % tabs.length : event.key === 'Home' ? 0 : event.key === 'End' ? tabs.length - 1 : null;
              if (next === null) return;
              event.preventDefault(); const nextTab = tabs[next]; if (nextTab) choose(nextTab); refs.current[next]?.focus();
            }}
            className={`flex-1 min-w-0 flex items-center justify-center gap-1.5 px-3 py-2 text-sm whitespace-nowrap focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent ${look}`}>
            {id === 'speleodb'
              ? <SpeleoNetworkBusyIndicator><SpeleoTabIcon state={speleoState} /></SpeleoNetworkBusyIndicator>
              : <SectionsTabIcon />}
            <span>{t(id === 'speleodb' ? 'speleodb.workspaceTab.speleodb' : 'speleodb.workspaceTab.sections')}</span>
          </button>;
        })}
      </div>
      {speleoNote && <span id="workspace-tab-speleodb-note" className="sr-only" data-testid="workspace-tab-speleodb-note">{speleoNote}</span>}
      <div role="tabpanel" id="workspace-panel-sections" aria-labelledby="workspace-tab-sections" hidden={mode !== 'survey' || tab !== 'sections'} className="flex-1 min-h-0">{sections}</div>
      <div role="tabpanel" id="workspace-panel-speleodb" aria-labelledby="workspace-tab-speleodb" hidden={tab !== 'speleodb'} className="flex-1 min-h-0"><SpeleoWorkspace /></div>
    </div>
  );
}
