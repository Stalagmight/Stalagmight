import { useEffect, useMemo, useRef, useState } from 'react';
import type { KeyboardEvent as ReactKeyboardEvent } from 'react';
import { useVirtualizer } from '@tanstack/react-virtual';
import { useTranslation } from 'react-i18next';
import { selectDistanceOverlayActive } from '../../store/displaySelectors';
import { useUIStore, type Selection } from '../../store/uiStore';
import { useEditingBlocked } from '../../hooks/useEditingBlocked';
import { useShallow } from 'zustand/react/shallow';
import { useProjectStore } from '../../store/projectStore';
import { Panel } from '../common/Panel';
import { Button } from '../common/Button';
import { WarningResolution } from './WarningResolution';
import { ProjectStatsActions } from './ProjectStatsActions';
import { AlertCircleIcon, AlertTriangleIcon, CheckCircleIcon, ChevronDownIcon, ChevronRightIcon, LinkOffIcon } from '../common/icons';
import { DetailLink } from '../common/DetailLink';
import { EntityBlob } from '../common/EntityBlob';
import { syntacticLevelAtLeast } from '../../theme/syntacticPalette';
import type { Station, Shot, Line, Project, ProjectStats as ProjectStatsType, ClosureRef, LoopClosure } from '../../types/project';
import type { ProjectWarning } from '../../types/commands';
import { displayStationName } from '../../utils/formatters';
import { warningMessageBody, warningMessageText } from '../../utils/stationIdentity';
import { useIdentityLabels } from '../../hooks/useIdentityLabels';
import { findStation } from '../../utils/findStation';
import { getDeleteShotDialog } from '../../utils/contextMenuHelpers';
import { formatFixed, formatPercent, formatNumber, formatDate } from '../../i18n/format';
import { useDecimalPrecision } from '../../hooks/useDecimalPrecision';
import { useActiveStations } from '../../hooks/useActiveStations';
import { setSource, trackContextMenuAction } from '../../utils/telemetry';
import { SHOT_KIND_LABEL_KEY, nextShotKind, surveyedLength } from '../../utils/shotKind';
import { countUnresolvedErrors } from '../../utils/validation';
import { shotCredit } from '../../utils/lineCredit';
import { CreditDetail, MultiShotDetail, lineCredits, useSelectedShots } from './CreditDetail';
import { CenterLocateIcon, DetailRow, StatCard } from './DetailPanelParts';
import { LineSectionDetailRow } from './LineSectionDetailRow';
import { StationDatumDetail } from './StationDatumDetail';
import { warningKey, warningMessageTestId, warningOpenShotTestId, warningResolveTestId, warningUndoResolveTestId } from '../../utils/testId';

export default function DetailPanel() {
  const { t } = useTranslation();
  const { selection, detailPanelTab, setDetailPanelTab, setSelection } = useUIStore(
    useShallow((s) => ({ selection: s.selection, detailPanelTab: s.detailPanelTab, setDetailPanelTab: s.setDetailPanelTab, setSelection: s.setSelection }))
  );
  const { project, stations, warnings } = useProjectStore();

  const unresolvedWarnings = warnings.filter((w) => !w.isResolved);
  const errorCount = countUnresolvedErrors(warnings);
  const unresolvedCount = unresolvedWarnings.length;

  return (
    <div className="h-full bg-panel flex flex-col" data-testid="detail-panel">
      {/* Tab bar */}
      <div className="flex border-b border-border shrink-0">
        <TabButton
          active={detailPanelTab === 'details'}
          onClick={() => setDetailPanelTab('details')}
          testId="detail-tab-details"
        >
          {t('detailPanel.tabs.details')}
        </TabButton>
        <TabButton
          active={detailPanelTab === 'warnings'}
          onClick={() => setDetailPanelTab('warnings')}
          badge={unresolvedCount > 0 ? unresolvedCount : undefined}
          badgeType={errorCount > 0 ? 'error' : 'warning'}
          testId="detail-tab-warnings"
        >
          {t('detailPanel.tabs.warnings')}
        </TabButton>
      </div>

      {/* Tab content */}
      <div className="flex-1 overflow-auto">
        {detailPanelTab === 'details' ? (
          <DetailsContent
            selection={selection}
            project={project}
            stations={stations}
          />
        ) : (
          <WarningsContent
            warnings={warnings}
            project={project}
            onWarningClick={(warning) => {
              // Selecting the row highlights the problematic geometry on the map
              // but stays on the Warnings tab (the entity pills are the way to
              // jump to Details).
              setSource('detail-panel');
              setSelection({
                type: 'shot',
                lineId: warning.lineId,
                shotId: warning.shotId,
              });
            }}
          />
        )}
      </div>
    </div>
  );
}

interface TabButtonProps {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
  badge?: number | undefined;
  badgeType?: 'warning' | 'error' | undefined;
  testId?: string;
}

function TabButton({ active, onClick, children, badge, badgeType, testId }: TabButtonProps) {
  return (
    <button
      onClick={onClick}
      className={`
        px-4 py-2 text-sm font-medium border-b-2 transition-colors
        ${active
          ? 'border-accent text-text-primary'
          : 'border-transparent text-text-secondary hover:text-text-primary'
        }
      `}
      data-testid={testId}
    >
      {children}
      {badge !== undefined && badge > 0 && (
        <span
          className={`
            ml-2 px-1.5 py-0.5 text-xs rounded-full
            ${badgeType === 'error' ? 'bg-error-10 text-error' : 'bg-warning-10 text-warning'}
          `}
        >
          {badge}
        </span>
      )}
    </button>
  );
}

interface DetailsContentProps {
  selection: Selection | null;
  project: Project | null;
  stations: Station[];
}

function DetailsContent({ selection, project, stations }: DetailsContentProps) {
  const { t } = useTranslation();
  const { stats } = useProjectStore();
  // Several shots selected (and nothing else): edit their survey credit together.
  const multiShots = useSelectedShots(project);
  if (multiShots) return <MultiShotDetail picks={multiShots} />;

  if (!selection) {
    // Show project stats when nothing is selected
    return <ProjectStats project={project} stats={stats} stations={stations} />;
  }

  if (selection.type === 'station' && selection.stationRef) {
    const station = findStation(stations, selection.stationRef.line_id, selection.stationRef.station_index);

    if (station) {
      return <StationDetail station={station} />;
    }
  }

  if (selection.type === 'shot' && selection.lineId !== undefined && selection.shotId !== undefined) {
    const line = project?.lines.find((s) => s.id === selection.lineId);
    const shot = line?.shots.find((s) => s.id === selection.shotId);

    if (line && shot) {
      return <ShotDetail line={line} shot={shot} stations={stations} />;
    }
  }

  if (selection.type === 'line' && selection.lineId !== undefined) {
    const line = project?.lines.find((s) => s.id === selection.lineId);

    if (line) {
      return (
        <LineDetail
          line={line}
          stations={stations}
          totalProjectLength={stats?.total_length ?? 0}
        />
      );
    }
  }

  return (
    <div className="p-4">
      <div className="text-text-secondary">{t('detailPanel.selectionNotFound')}</div>
    </div>
  );
}

interface WarningsContentProps {
  warnings: ProjectWarning[];
  project: Project | null;
  onWarningClick: (warning: ProjectWarning) => void;
}

type WarningFlow =
  | { kind: 'list' }
  | { kind: 'resolve'; queue: ProjectWarning[]; index: number; resolveAll: boolean; resolved: number; skipped: number }
  | { kind: 'summary'; resolved: number; skipped: number; remaining: number };

// Identity of a warning for queue stepping + live-set rehydration. Include the
// datum index for datum warnings (`datum_gps_gap`, #741): two GPS fixes can map to
// the SAME shot (e.g. stations 0 and 1 both map to shot 0), sharing
// lineId/shotId/code but differing by datum - without this they collapse and
// the resolve flow would act on the wrong datum (issue #57 / Codex).
function WarningsContent({ warnings, project, onWarningClick }: WarningsContentProps) {
  const { t } = useTranslation();
  const { unresolveWarning } = useProjectStore(
    useShallow((s) => ({ unresolveWarning: s.unresolveWarning })),
  );
  const { setSelection, setDetailPanelTab, announceSelection, triggerCenterOnShot, triggerPanToShot, requestResolveWarning, clearResolveWarningRequest } = useUIStore(
    useShallow((s) => ({
      setSelection: s.setSelection,
      setDetailPanelTab: s.setDetailPanelTab,
      announceSelection: s.announceSelection,
      triggerCenterOnShot: s.triggerCenterOnShot,
      triggerPanToShot: s.triggerPanToShot,
      requestResolveWarning: s.requestResolveWarning,
      clearResolveWarningRequest: s.clearResolveWarningRequest,
    })),
  );
  // At the Full level the location subtitle is shown as entity pills (line +
  // shot), matching the rest of the detail panel; otherwise it stays plain text.
  const showPills = syntacticLevelAtLeast(useUIStore((s) => s.syntacticLevel), 'full');
  const [showResolved, setShowResolved] = useState(false);
  const [flow, setFlow] = useState<WarningFlow>({ kind: 'list' });

  const unresolvedWarnings = useMemo(() => warnings.filter((w) => !w.isResolved), [warnings]);
  const unresolvedCount = unresolvedWarnings.length;
  const resolvedCount = warnings.length - unresolvedCount;
  const displayedWarnings = showResolved ? warnings : unresolvedWarnings;

  // Virtualize the list so a large warning set (hundreds+) doesn't mount every
  // row - only the visible window renders, and a refreshWarnings() after each
  // resolve step reconciles a handful of rows instead of the whole list.
  const listParentRef = useRef<HTMLDivElement>(null);
  const rowVirtualizer = useVirtualizer({
    count: displayedWarnings.length,
    getScrollElement: () => listParentRef.current,
    estimateSize: () => 56,
    overscan: 8,
  });

  // The warning currently under resolution. Rehydrate it from the LIVE warnings
  // (matched by identity) so its shotIndex - which WarningResolution uses for
  // frozen-shot gating and the on-map preview endpoints - reflects edits an
  // earlier "Resolve all" step applied (e.g. a delete-shot renumbers later shots'
  // indices). Falls back to the queued copy if it's no longer present (#57 / Codex).
  const queuedWarning = flow.kind === 'resolve' ? flow.queue[flow.index] : undefined;
  const activeWarning = useMemo(() => {
    if (!queuedWarning) return undefined;
    const key = warningKey(queuedWarning);
    return warnings.find((w) => warningKey(w) === key) ?? queuedWarning;
  }, [queuedWarning, warnings]);
  const activeLine = activeWarning ? project?.lines.find((s) => s.id === activeWarning.lineId) : undefined;
  const activeShot = activeWarning && activeLine ? activeLine.shots.find((s) => s.id === activeWarning.shotId) : undefined;

  // Full focus on the shot under resolution (issue #57), for both a single
  // Resolve and every step of Resolve all. Selecting it does what clicking its
  // geometry link does anywhere else (highlight on the map + focus the details,
  // lines and data panels); centering brings it into view on the map. Keyed
  // on the warning identity so it re-fires as the queue steps. (The zoom button
  // lets the user close in further.)
  const activeKey = activeWarning ? warningKey(activeWarning) : undefined;
  useEffect(() => {
    if (activeWarning) {
      setSource('detail-panel');
      setSelection({ type: 'shot', lineId: activeWarning.lineId, shotId: activeWarning.shotId });
      triggerCenterOnShot(activeWarning.lineId, activeWarning.shotId);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeKey]);

  // Open the resolve flow when requested from outside (e.g. clicking the warning
  // indicator in the data table opens this tab and jumps straight to resolving
  // that shot's first outstanding issue).
  useEffect(() => {
    if (!requestResolveWarning) return;
    const { lineId, shotId } = requestResolveWarning;
    clearResolveWarningRequest();
    const target = warnings.find((w) => w.lineId === lineId && w.shotId === shotId && !w.isResolved);
    if (target) {
      setFlow({ kind: 'resolve', queue: [target], index: 0, resolveAll: false, resolved: 0, skipped: 0 });
    }
  }, [requestResolveWarning, warnings, clearResolveWarningRequest]);

  const labels = useIdentityLabels();
  const getLineName = labels.lineName;

  const handleUndoResolve = (e: React.MouseEvent, warning: ProjectWarning) => {
    e.stopPropagation();
    unresolveWarning(warning.lineId, warning.shotId, warning.code);
  };

  const startResolve = (e: React.MouseEvent, warning: ProjectWarning) => {
    e.stopPropagation();
    setFlow({ kind: 'resolve', queue: [warning], index: 0, resolveAll: false, resolved: 0, skipped: 0 });
  };

  const startResolveAll = () => {
    if (unresolvedWarnings.length === 0) return;
    setFlow({ kind: 'resolve', queue: unresolvedWarnings, index: 0, resolveAll: true, resolved: 0, skipped: 0 });
  };

  // Live unresolved warnings, read from the store so they reflect the refresh
  // that an applied fix already triggered.
  const liveUnresolved = () => useProjectStore.getState().warnings.filter((w) => !w.isResolved);

  // Find the next queue index that is still unresolved, skipping any item a
  // prior fix cleared as a side effect.
  const nextLiveIndex = (queue: ProjectWarning[], from: number, live: Set<string>) => {
    let next = from;
    while (next < queue.length && !live.has(warningKey(queue[next]!))) next++;
    return next;
  };

  // Advance after the user applied an option. Only count it as resolved if the
  // active warning actually cleared from the live set - a data fix can succeed
  // yet leave the warning firing (e.g. a manual length still below the minimum),
  // and that must not be reported as resolved.
  const advanceAfterApply = () => {
    setFlow((f) => {
      if (f.kind !== 'resolve') return f;
      const liveArr = liveUnresolved();
      const live = new Set(liveArr.map(warningKey));
      const cleared = !live.has(warningKey(f.queue[f.index]!));
      const resolved = f.resolved + (cleared ? 1 : 0);
      const next = nextLiveIndex(f.queue, f.index + 1, live);
      if (next >= f.queue.length) {
        return f.resolveAll ? { kind: 'summary', resolved, skipped: f.skipped, remaining: liveArr.length } : { kind: 'list' };
      }
      return { ...f, index: next, resolved };
    });
  };

  const advanceAfterSkip = () => {
    setFlow((f) => {
      if (f.kind !== 'resolve') return f;
      const liveArr = liveUnresolved();
      const live = new Set(liveArr.map(warningKey));
      const next = nextLiveIndex(f.queue, f.index + 1, live);
      const skipped = f.skipped + 1;
      if (next >= f.queue.length) {
        return f.resolveAll ? { kind: 'summary', resolved: f.resolved, skipped, remaining: liveArr.length } : { kind: 'list' };
      }
      return { ...f, index: next, skipped };
    });
  };

  const exitFlow = () => {
    setFlow((f) =>
      f.kind === 'resolve' && f.resolveAll
        ? { kind: 'summary', resolved: f.resolved, skipped: f.skipped, remaining: liveUnresolved().length }
        : { kind: 'list' },
    );
  };

  // --- Inline resolution view (reuses this panel) ---
  if (flow.kind === 'resolve' && activeWarning) {
    return (
      <WarningResolution
        warning={activeWarning}
        line={activeLine}
        shot={activeShot}
        position={flow.resolveAll ? { index: flow.index, total: flow.queue.length } : undefined}
        onExit={exitFlow}
        onResolved={advanceAfterApply}
        onSkip={advanceAfterSkip}
      />
    );
  }

  // --- "Resolve all" summary ---
  if (flow.kind === 'summary') {
    return (
      <div className="p-4 text-center text-text-secondary space-y-3" data-testid="warnings-resolve-summary">
        <div className="text-3xl flex items-center justify-center">
          <CheckCircleIcon size={28} />
        </div>
        <p>
          {t('detailPanel.warnings.resolveSummary', {
            resolved: flow.resolved,
            skipped: flow.skipped,
            remaining: flow.remaining,
          })}
        </p>
        <div className="flex items-center justify-center gap-2">
          {flow.remaining > 0 && (
            <Button size="sm" onClick={startResolveAll} data-testid="warnings-resolve-remaining">
              {t('detailPanel.warnings.resolveRemaining')}
            </Button>
          )}
          <Button size="sm" variant="primary" onClick={() => setFlow({ kind: 'list' })} data-testid="warnings-resolve-done">
            {t('detailPanel.warnings.resolveDone')}
          </Button>
        </div>
      </div>
    );
  }

  if (warnings.length === 0) {
    return (
      <div className="p-4 text-center text-text-secondary">
        <div className="text-3xl mb-2 flex items-center justify-center">
          <CheckCircleIcon size={28} />
        </div>
        <p>{t('detailPanel.warnings.empty')}</p>
      </div>
    );
  }

  return (
    <div className="p-2 h-full flex flex-col" data-testid="warnings-content">
      {(() => {
        const countText = (
          <span className="text-sm text-text-secondary">
            {t('detailPanel.warnings.count', {
              count: unresolvedCount,
              countFormatted: formatNumber(unresolvedCount),
            })}
            {resolvedCount > 0 && ` ${t('detailPanel.warnings.resolvedCount', {
              count: resolvedCount,
              countFormatted: formatNumber(resolvedCount),
            })}`}
          </span>
        );
        const actions = (
          <>
            {resolvedCount > 0 && (
              <button onClick={() => setShowResolved(!showResolved)}
                data-testid="warnings-show-resolved-toggle"
                className="text-xs text-accent hover:underline"
              >
                {showResolved ? t('detailPanel.warnings.hideResolved') : t('detailPanel.warnings.showResolved')}
              </button>
            )}
            {unresolvedCount > 0 && (
              <button
                onClick={startResolveAll}
                className="text-xs px-2 py-0.5 rounded bg-background-50 hover:bg-background text-text-secondary"
                data-testid="resolve-all-warnings"
                title={t('detailPanel.warnings.resolveAll')}
              >
                {t('detailPanel.warnings.resolveAll')}
              </button>
            )}
          </>
        );
        // When a "Show resolved" toggle is present, give the actions their own
        // row below the count so they don't crowd it; otherwise keep them inline.
        return resolvedCount > 0 ? (
          <div className="mb-2 px-2 shrink-0">
            <div>{countText}</div>
            <div className="flex items-center gap-2 mt-1">{actions}</div>
          </div>
        ) : (
          <div className="flex items-center justify-between mb-2 px-2 shrink-0">
            {countText}
            <div className="flex items-center gap-2">{actions}</div>
          </div>
        );
      })()}
      <div ref={listParentRef} className="flex-1 min-h-0 overflow-auto" role="list" aria-label={t('detailPanel.tabs.warnings')}>
        <div style={{ height: `${rowVirtualizer.getTotalSize()}px`, position: 'relative' }}>
          {rowVirtualizer.getVirtualItems().map((vItem) => {
            const warning = displayedWarnings[vItem.index]!;
            const index = vItem.index;
            return (
            <div
              key={warningKey(warning)}
              data-index={index}
              ref={rowVirtualizer.measureElement}
              role="listitem"
              className="absolute left-0 w-full pb-1"
              style={{ transform: `translateY(${vItem.start}px)` }}
            >
            <div
              className={`
              p-2 rounded transition-colors flow-root
              ${warning.isResolved ? 'opacity-50' : ''}
              hover:bg-background-50
            `}
            >
            {/* Action button floats right so the warning text wraps around it
                (issue #57) instead of the button reserving its own column. */}
            {warning.isResolved ? (
              <button onClick={(e) => handleUndoResolve(e, warning)}
                data-testid={warningUndoResolveTestId(warning)}
                className="float-right ml-2 text-xs px-2 py-0.5 rounded bg-background-50 hover:bg-background text-text-secondary"
                title={t('detailPanel.warnings.markUnresolved')}
              >
                {t('common.actions.undo')}
              </button>
            ) : (
              <button
                onClick={(e) => startResolve(e, warning)}
                className="float-right ml-2 text-xs px-2 py-0.5 rounded bg-background-50 hover:bg-background text-text-primary"
                title={t('detailPanel.warnings.resolve')}
                data-testid={warningResolveTestId(warning)}
              >
                {t('detailPanel.warnings.resolve')}
              </button>
            )}
            {/* Clickable message: single-click selects the shot, double-click
                pans to it. With pills (Full level) it stays a plain <div> so the
                inline shot pill <button> isn't nested in another interactive
                element; without pills there is no pill, so it becomes keyboard-
                operable (role=button + Enter/Space) to preserve select-by-keyboard.
                Either way it stays normal flow so the text wraps around the
                floated action button. */}
            {/* eslint-disable-next-line jsx-a11y/no-static-element-interactions, jsx-a11y/click-events-have-key-events */}
            <div
              {...(showPills
                ? {}
                : {
                    role: 'button' as const,
                    tabIndex: 0,
                    onKeyDown: (e: ReactKeyboardEvent) => {
                      if (e.key === 'Enter' || e.key === ' ') {
                        e.preventDefault();
                        onWarningClick(warning);
                      }
                    },
                  })}
              onClick={() => onWarningClick(warning)}
              onDoubleClick={() => triggerPanToShot(warning.lineId, warning.shotId)}
              className={`text-left cursor-pointer rounded${showPills ? '' : ' focus:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-inset'}`}
              data-testid={warningMessageTestId(warning)}
            >
              <span className={`mr-1.5 align-text-bottom inline-block ${warning.severity === 'error' ? 'text-error' : 'text-warning'}`}>
                {warning.isResolved
                  ? <CheckCircleIcon size={14} />
                  : (warning.severity === 'error' ? <AlertCircleIcon size={14} /> : <AlertTriangleIcon size={14} />)}
              </span>
              <span className={`text-sm ${warning.isResolved ? 'line-through' : ''}`}>
                {(() => {
                  // Survey messages open with a positional "Shot <line>:<pos>"
                  // prefix that is never shown: the shot is named by its
                  // stations instead - as a pill at the Full level.
                  const body = warningMessageBody(warning.message);
                  if (!showPills || body === warning.message) return warningMessageText(labels, warning);
                  const shot = labels.shotAt(warning.lineId, warning.shotIndex);
                  return (
                    <>
                      {/* The shot pill navigates to Details (select + switch tab),
                          its prior behaviour; stopPropagation keeps the row's
                          select-only handler from also firing. */}
                      <button type="button"
                        onClick={(e) => {
                          e.stopPropagation();
                          setSource('detail-panel');
                          setSelection({ type: 'shot', lineId: warning.lineId, shotId: warning.shotId });
                          setDetailPanelTab('details');
                        }}
                        className="inline align-middle p-0 cursor-pointer rounded focus:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-inset"
                        aria-label={t('detailPanel.warnings.openShotDetails', {
                          shot: labels.shotAt(warning.lineId, warning.shotIndex, true),
                        })}
                        data-testid={warningOpenShotTestId(warning)}
                      >
                        <EntityBlob entity="shot">{shot}</EntityBlob>
                      </button>
                      {' '}
                      {body}
                    </>
                  );
                })()}
              </span>
              {!showPills && (
                <div className="text-xs text-text-secondary mt-0.5">
                  {labels.shotAt(warning.lineId, warning.shotIndex, true)}
                </div>
              )}
            </div>
            {showPills && (
              <div className="mt-1 flex items-center gap-1.5 flex-wrap clear-both">
                <DetailLink
                  type="line"
                  target={{ lineId: warning.lineId }}
                  displayText={getLineName(warning.lineId)}
                  pillLabel={getLineName(warning.lineId)}
                  onSelect={() => {
                    setSelection({ type: 'line', lineId: warning.lineId });
                    setDetailPanelTab('details');
                  }}
                  onAnnounce={announceSelection}
                />
              </div>
            )}
            </div>
            </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}

function StationDetail({ station }: { station: Station }) {
  const { t } = useTranslation();
  const { project, stations, unmergeStations, unmergeStationPair } = useProjectStore();
  const { openDialog, setSelection, triggerCenterOnStation, announceSelection, setHoveredStation, setHoveredShot, setHoveredLineId } = useUIStore(
    useShallow((s) => ({ openDialog: s.openDialog, setSelection: s.setSelection, triggerCenterOnStation: s.triggerCenterOnStation, announceSelection: s.announceSelection, setHoveredStation: s.setHoveredStation, setHoveredShot: s.setHoveredShot, setHoveredLineId: s.setHoveredLineId }))
  );
  const precision = useDecimalPrecision();
  // Closure-adjusted view, Plan/Export mode, and a locked remote SpeleoDB project are
  // all read-only: disable mutation controls (edits happen in the measured survey view).
  const editingBlocked = useEditingBlocked();
  const distanceGradientOrigin = useUIStore((s) => s.distanceGradientOrigin);
  const showDistanceGradient = useUIStore(selectDistanceOverlayActive);
  // Show the active geometry's coordinates so inspection matches the map; falls
  // back to the passed (measured) station before the adjusted set is fetched.
  const activeStations = useActiveStations();
  const displayStation =
    findStation(activeStations, station.station_ref.line_id, station.station_ref.station_index) ?? station;
  const [unmergeTargetKey, setUnmergeTargetKey] = useState<string | null>(null);
  const line = project?.lines.find(
    (s) => s.id === station.station_ref.line_id
  );

  const labels = useIdentityLabels();
  const stationIndex = station.station_ref.station_index;
  const lineId = station.station_ref.line_id;

  // Distance-gradient origin (#34): flag this station when it's the chosen root.
  const isDistanceGradientRoot =
    showDistanceGradient &&
    distanceGradientOrigin != null &&
    distanceGradientOrigin.line_id === station.station_ref.line_id &&
    distanceGradientOrigin.station_index === station.station_ref.station_index;

  // Station-to-Shot mapping:
  // Station at station_index has:
  // - Previous shot: line.shots[station_index - 1] (display position = station_index)
  // - Next shot: line.shots[station_index] (display position = station_index + 1)
  const prevShotIndex = stationIndex - 1;
  const nextShotIndex = stationIndex;
  const prevShotPosition = stationIndex; // station_index for display
  const nextShotPosition = stationIndex + 1; // station_index + 1 for display

  // Check if shots exist
  const prevShotExists = line ? prevShotIndex >= 0 && prevShotIndex < line.shots.length : false;
  const nextShotExists = line ? nextShotIndex >= 0 && nextShotIndex < line.shots.length : false;

  // Determine if we should show prev/next shot links
  const isFirstStation = stationIndex === 0;
  const isLastStation = line ? stationIndex >= line.shots.length : true;

  const handleEdit = () => {
    setSource('detail-panel');
    openDialog('edit-station', { stationRef: station.station_ref });
  };

  const handleSelectPrevShot = () => {
    if (line && prevShotExists) {
      const shot = line.shots[prevShotIndex];
      if (shot) {
        setSource('detail-panel');
        setSelection({ type: 'shot', lineId, shotId: shot.id });
      }
    }
  };

  const handleSelectNextShot = () => {
    if (line && nextShotExists) {
      const shot = line.shots[nextShotIndex];
      if (shot) {
        setSource('detail-panel');
        setSelection({ type: 'shot', lineId, shotId: shot.id });
      }
    }
  };

  const handleSelectLine = () => {
    setSource('detail-panel');
    setSelection({ type: 'line', lineId });
  };

  const handleSelectMergedStation = (ref: { line_id: number; station_index: number }) => {
    setSource('detail-panel');
    setSelection({
      type: 'station',
      stationRef: { line_id: ref.line_id, station_index: ref.station_index },
    });
  };

  const handleCenterOnStation = () => {
    setSource('detail-panel');
    triggerCenterOnStation(station.station_ref);
  };

  const sameStationRef = (a: { line_id: number; station_index: number }, b: { line_id: number; station_index: number }): boolean => {
    return a.line_id === b.line_id && a.station_index === b.station_index;
  };

  const isDirectMerge = (ref: { line_id: number; station_index: number }): boolean => {
    return project?.station_merges.some((merge) => {
      const direct = sameStationRef(merge.station_a, station.station_ref) && sameStationRef(merge.station_b, ref);
      const reverse = sameStationRef(merge.station_a, ref) && sameStationRef(merge.station_b, station.station_ref);
      return direct || reverse;
    }) ?? false;
  };

  const handleUnmergeFromStation = async (ref: { line_id: number; station_index: number }) => {
    trackContextMenuAction('unmerge', 'detail-panel');
    const targetKey = `${ref.line_id}:${ref.station_index}`;
    setUnmergeTargetKey(targetKey);
    try {
      if (isDirectMerge(ref)) {
        await unmergeStationPair(station.station_ref, ref);
      } else {
        await unmergeStations(station.station_ref);
      }
    } finally {
      setUnmergeTargetKey(null);
    }
  };

  // Validate merged station targets exist
  const mergedStationExists = (ref: { line_id: number; station_index: number }): boolean =>
    findStation(stations, ref.line_id, ref.station_index) !== undefined;
  // A merge is where two lines meet, so a merged station is named with its line.
  const lineName = labels.lineName(lineId);
  const stationId = labels.stationId(station.station_ref);

  const centerButton = (
    <button
      type="button"
      onClick={handleCenterOnStation}
      className="p-1 text-text-secondary hover:text-text-primary rounded hover:bg-background-50 focus:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-inset"
      aria-label={t('detailPanel.station.center', {
        station: labels.station(station.station_ref, true),
      })}
      data-testid="center-station-button"
    >
      <CenterLocateIcon />
    </button>
  );

  return (
    <div className="h-full bg-panel overflow-auto" data-testid="station-detail">
      <Panel bare title={t('detailPanel.station.title')} headerActions={centerButton}>
        <div className="space-y-3">
          <DetailRow label={t('detailPanel.labels.name')} value={displayStationName(labels.stationName(station.station_ref))} testId="station-detail-name" />
          {stationId !== null && (
            <DetailRow label={t('identity.stationId')} value={String(stationId)} testId="station-detail-id" />
          )}
          <DetailRow label={t('detailPanel.labels.section')} value={lineName} testId="station-detail-line" />
          <DetailRow label={t('detailPanel.labels.x')} value={`${formatFixed(displayStation.coordinates.x, undefined, precision.coordinates)} ${t('common.units.meters')}`} />
          <DetailRow label={t('detailPanel.labels.y')} value={`${formatFixed(displayStation.coordinates.y, undefined, precision.coordinates)} ${t('common.units.meters')}`} />
          <DetailRow label={t('detailPanel.labels.depth')} value={`${formatFixed(displayStation.coordinates.z, undefined, precision.depth)} ${t('common.units.meters')}`} />

          {isDistanceGradientRoot && (
            <div className="pt-2 border-t border-border">
              <span className="text-accent" data-testid="distance-gradient-root">
                &#9678; {t('detailPanel.station.distanceGradientRoot')}
              </span>
            </div>
          )}

          {station.comment && (
            <div className="pt-2 border-t border-border">
              <span className="text-sm text-text-secondary">{t('detailPanel.station.comment')}:</span>
              <p className="mt-1 text-sm whitespace-pre-wrap">{station.comment}</p>
            </div>
          )}

          {station.is_zero_datum && (
            <StationDatumDetail
              datum={project?.zero_datums.find((d) => sameStationRef(d.station_ref, station.station_ref))}
              gps={station.gps_coordinates}
              gpsPrecision={precision.gps}
            />
          )}

          {station.merged_with.length > 0 && (
            <div className="pt-2 border-t border-border">
              <span className="text-sm text-text-secondary">{t('detailPanel.station.mergedWith')}:</span>
              <ul className="mt-1 space-y-1">
                {station.merged_with.map((ref, i) => (
                  <li key={i} className="text-sm flex items-center justify-between gap-2">
                    <div>
                      <DetailLink
                        type="station"
                        target={{ lineId: ref.line_id, position: ref.station_index }}
                        displayText={labels.station(ref, true)}
                        pillLabel={labels.station(ref)}
                        pillContext={labels.lineName(ref.line_id)}
                        onSelect={() => handleSelectMergedStation(ref)}
                        isMissing={!mergedStationExists(ref)}
                        onAnnounce={announceSelection}
                        onHoverStart={() => setHoveredStation({ line_id: ref.line_id, station_index: ref.station_index })}
                        onHoverEnd={() => setHoveredStation(null)}
                      />
                    </div>
                    <button
                      type="button"
                      onClick={() => {
                        void handleUnmergeFromStation(ref);
                      }}
                      className="p-1 rounded text-text-secondary hover:text-error hover:bg-error-10 focus:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-inset disabled:opacity-50 disabled:cursor-not-allowed"
                      title="Unmerge Stations"
                      aria-label={`Unmerge station ${labels.station(ref, true)}`}
                      data-testid={`detail-unmerge-${ref.line_id}-${ref.station_index}`}
                      disabled={editingBlocked || unmergeTargetKey === `${ref.line_id}:${ref.station_index}`}
                    >
                      <LinkOffIcon size={14} />
                    </button>
                  </li>
                ))}
              </ul>
            </div>
          )}

          {station.closures_with && station.closures_with.length > 0 && (
            <ClosuresSection
              closures={station.closures_with}
              loopClosures={project?.loop_closures ?? []}
              stations={stations}
              onSelectStation={handleSelectMergedStation}
              onAnnounce={announceSelection}
              onHoverStart={(ref) => setHoveredStation({ line_id: ref.line_id, station_index: ref.station_index })}
              onHoverEnd={() => setHoveredStation(null)}
            />
          )}

          {/* Connected line with hyperlinks */}
          <div className="pt-2 border-t border-border">
            <span className="text-sm text-text-secondary">{t('detailPanel.station.connected')}:</span>
            <div className="mt-1 space-y-1 text-sm">
              {!isFirstStation && (
                <div className="flex justify-between">
                  <span className="text-text-secondary">{t('detailPanel.station.previousShot')}:</span>
                  <DetailLink
                    type="shot"
                    target={{ lineId, position: prevShotPosition }}
                    displayText={labels.shotAt(lineId, prevShotIndex)}
                    fullText={labels.shotAt(lineId, prevShotIndex, true)}
                    onSelect={handleSelectPrevShot}
                    isMissing={!prevShotExists}
                    onAnnounce={announceSelection}
                    onHoverStart={() => {
                      const shot = line?.shots[prevShotIndex];
                      if (shot) setHoveredShot({ lineId, shotId: shot.id });
                    }}
                    onHoverEnd={() => setHoveredShot(null)}
                  />
                </div>
              )}
              {!isLastStation && (
                <div className="flex justify-between">
                  <span className="text-text-secondary">{t('detailPanel.station.nextShot')}:</span>
                  <DetailLink
                    type="shot"
                    target={{ lineId, position: nextShotPosition }}
                    displayText={labels.shotAt(lineId, nextShotIndex)}
                    fullText={labels.shotAt(lineId, nextShotIndex, true)}
                    onSelect={handleSelectNextShot}
                    isMissing={!nextShotExists}
                    onAnnounce={announceSelection}
                    onHoverStart={() => {
                      const shot = line?.shots[nextShotIndex];
                      if (shot) setHoveredShot({ lineId, shotId: shot.id });
                    }}
                    onHoverEnd={() => setHoveredShot(null)}
                  />
                </div>
              )}
              <div className="flex justify-between">
                <span className="text-text-secondary">{t('detailPanel.labels.section')}:</span>
                <DetailLink
                  type="line"
                  target={{ lineId }}
                  displayText={lineName}
                  onSelect={handleSelectLine}
                  onAnnounce={announceSelection}
                  onHoverStart={() => setHoveredLineId(lineId)}
                  onHoverEnd={() => setHoveredLineId(null)}
                />
              </div>
            </div>
          </div>
        </div>
      </Panel>

      <div className="p-3 border-t border-border">
        <Button size="sm" className="w-full" onClick={handleEdit} disabled={editingBlocked} data-testid="station-edit-button">
          {t('common.actions.edit')}
        </Button>
      </div>
    </div>
  );
}

/**
 * Compute inclination from depth change and shot length.
 * Positive = downward (depth increases), negative = upward, the same frame
 * the stored pitch is in, so this sits directly beside the recorded value
 * without either needing to be flipped to compare (issue #296).
 * Returns null if length <= 0.
 */
function computeInclination(depthIn: number, depthOut: number, length: number): number | null {
  if (length <= 0) return null;
  const ratio = (depthOut - depthIn) / length;
  const clamped = Math.max(-1, Math.min(1, ratio));
  return (Math.asin(clamped) * 180) / Math.PI;
}

function ShotDetail({ line, shot, stations }: { line: Line; shot: Shot; stations: Station[] }) {
  const { t } = useTranslation();
  const { setShotKind } = useProjectStore();
  const editingBlocked = useEditingBlocked();
  const { openDialog, setSelection, triggerCenterOnShot, announceSelection, setHoveredStation, setHoveredLineId } = useUIStore(
    useShallow((s) => ({ openDialog: s.openDialog, setSelection: s.setSelection, triggerCenterOnShot: s.triggerCenterOnShot, announceSelection: s.announceSelection, setHoveredStation: s.setHoveredStation, setHoveredLineId: s.setHoveredLineId }))
  );
  const precision = useDecimalPrecision();
  const labels = useIdentityLabels();

  // The shot at array index i runs from station i to station i + 1; the
  // indexes address them (and key their test ids), the names label them.
  const shotIndex = line.shots.indexOf(shot);
  const startStationIndex = shotIndex;
  const endStationIndex = shotIndex + 1;
  const startStationPosition = startStationIndex;
  const endStationPosition = endStationIndex;

  // Validate station targets exist
  const startStation = findStation(stations, line.id, startStationIndex);
  const endStation = findStation(stations, line.id, endStationIndex);
  const startStationExists = startStation !== undefined;
  const endStationExists = endStation !== undefined;

  const kindBadge = t('detailPanel.shot.kindBadge', { kind: t(SHOT_KIND_LABEL_KEY[shot.kind]) });
  const shotTargets = useMemo(() => ({ type: 'shots' as const, shots: [[line.id, shot.id] as [number, number]] }), [line.id, shot.id]);
  const shotCredits = useMemo(() => [shotCredit(shot)], [shot]);

  const handleCycleShotKind = async () => {
    trackContextMenuAction('cycle-shot-kind', 'detail-panel');
    await setShotKind(line.id, shot.id, nextShotKind(shot.kind));
  };

  const handleDelete = () => {
    setSource('detail-panel');
    const totalShots = line.shots.length;
    const isFirst = shotIndex === 0;
    const isLast = shotIndex === totalShots - 1;
    const { dialog, data } = getDeleteShotDialog(
      line.id,
      shot.id,
      isFirst,
      isLast,
      totalShots,
      labels.lineName(line.id),
    );
    openDialog(dialog, data);
  };

  const handleSelectStartStation = () => {
    setSource('detail-panel');
    setSelection({
      type: 'station',
      stationRef: { line_id: line.id, station_index: startStationIndex },
    });
  };

  const handleSelectEndStation = () => {
    setSource('detail-panel');
    setSelection({
      type: 'station',
      stationRef: { line_id: line.id, station_index: endStationIndex },
    });
  };

  const handleSelectLine = () => {
    setSource('detail-panel');
    setSelection({ type: 'line', lineId: line.id });
  };

  const handleCenterOnShot = () => {
    setSource('detail-panel');
    triggerCenterOnShot(line.id, shot.id);
  };

  const lineName = labels.lineName(line.id);
  const shotLabel = labels.shotAt(line.id, shotIndex);

  const centerButton = (
    <button
      type="button"
      onClick={handleCenterOnShot}
      className="p-1 text-text-secondary hover:text-text-primary rounded hover:bg-background-50 focus:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-inset"
      aria-label={t('detailPanel.shot.center', {
        shot: labels.shotAt(line.id, shotIndex, true),
      })}
      data-testid="center-shot-button"
    >
      <CenterLocateIcon />
    </button>
  );

  return (
    <div className="h-full bg-panel overflow-auto">
      <Panel bare title={t('detailPanel.shot.title')} headerActions={centerButton}>
        <div className="space-y-3">
          <DetailRow label={t('detailPanel.labels.shot')} value={shotLabel} testId="shot-detail-label" />
          <DetailRow label={t('detailPanel.labels.section')} value={lineName} />
          <DetailRow label={t('detailPanel.labels.length')} value={`${formatFixed(shot.length, undefined, precision.length)} ${t('common.units.meters')}`} />
          <DetailRow label={t('detailPanel.labels.azimuthIn')} value={`${formatFixed(shot.azimuth_in, undefined, precision.azimuth)}°`} />
          <DetailRow label={t('detailPanel.labels.azimuthOut')} value={`${formatFixed(shot.azimuth_out, undefined, precision.azimuth)}°`} />
          <DetailRow label={t('detailPanel.labels.depthIn')} value={`${formatFixed(shot.depth_in, undefined, precision.depth)} ${t('common.units.meters')}`} />
          <DetailRow label={t('detailPanel.labels.depthOut')} value={`${formatFixed(shot.depth_out, undefined, precision.depth)} ${t('common.units.meters')}`} />

          {(shot.pitch_in !== null || shot.pitch_out !== null || shot.length > 0) && (
            <div className="pt-2 border-t border-border">
              <span className="text-sm text-text-secondary">{t('detailPanel.shot.inclination')}:</span>
              <div className="mt-1 space-y-1 text-sm">
                {shot.pitch_in !== null && (
                  <DetailRow label={t('detailPanel.labels.pitchIn')} value={`${formatFixed(shot.pitch_in, undefined, precision.inclination)}°`} />
                )}
                {shot.pitch_out !== null && (
                  <DetailRow label={t('detailPanel.labels.pitchOut')} value={`${formatFixed(shot.pitch_out, undefined, precision.inclination)}°`} />
                )}
                {shot.length > 0 && (() => {
                  const incl = computeInclination(shot.depth_in, shot.depth_out, shot.length);
                  return incl !== null ? (
                    <DetailRow label={t('detailPanel.shot.computed')} value={`${formatFixed(incl, undefined, precision.inclination)}°`} />
                  ) : null;
                })()}
              </div>
            </div>
          )}

          {(shot.left !== null || shot.right !== null || shot.up !== null || shot.down !== null) && (
            <div className="pt-2 border-t border-border">
              <span className="text-sm text-text-secondary">{t('detailPanel.shot.lrud')}:</span>
              <div className="grid grid-cols-2 gap-2 mt-1 text-sm">
                <DetailRow label={t('detailPanel.labels.leftShort')} value={shot.left != null ? formatFixed(shot.left, undefined, precision.length) : t('detailPanel.placeholder.none')} />
                <DetailRow label={t('detailPanel.labels.rightShort')} value={shot.right != null ? formatFixed(shot.right, undefined, precision.length) : t('detailPanel.placeholder.none')} />
                <DetailRow label={t('detailPanel.labels.upShort')} value={shot.up != null ? formatFixed(shot.up, undefined, precision.length) : t('detailPanel.placeholder.none')} />
                <DetailRow label={t('detailPanel.labels.downShort')} value={shot.down != null ? formatFixed(shot.down, undefined, precision.length) : t('detailPanel.placeholder.none')} />
              </div>
            </div>
          )}

          {/* Connected line with hyperlinks */}
          <div className="pt-2 border-t border-border">
            <span className="text-sm text-text-secondary">{t('detailPanel.shot.connected')}:</span>
            <div className="mt-1 space-y-1 text-sm">
              <div className="flex justify-between">
                <span className="text-text-secondary">{t('detailPanel.shot.startStation')}:</span>
                <DetailLink
                  type="station"
                  target={{ lineId: line.id, position: startStationPosition }}
                  displayText={labels.station({ line_id: line.id, station_index: startStationIndex })}
                  fullText={labels.station({ line_id: line.id, station_index: startStationIndex }, true)}
                  onSelect={handleSelectStartStation}
                  isMissing={!startStationExists}
                  onAnnounce={announceSelection}
                  onHoverStart={() => setHoveredStation({ line_id: line.id, station_index: startStationIndex })}
                  onHoverEnd={() => setHoveredStation(null)}
                />
              </div>
              <div className="flex justify-between">
                <span className="text-text-secondary">{t('detailPanel.shot.endStation')}:</span>
                <DetailLink
                  type="station"
                  target={{ lineId: line.id, position: endStationPosition }}
                  displayText={labels.station({ line_id: line.id, station_index: endStationIndex })}
                  fullText={labels.station({ line_id: line.id, station_index: endStationIndex }, true)}
                  onSelect={handleSelectEndStation}
                  isMissing={!endStationExists}
                  onAnnounce={announceSelection}
                  onHoverStart={() => setHoveredStation({ line_id: line.id, station_index: endStationIndex })}
                  onHoverEnd={() => setHoveredStation(null)}
                />
              </div>
              <div className="flex justify-between">
                <span className="text-text-secondary">{t('detailPanel.labels.section')}:</span>
                <DetailLink
                  type="line"
                  target={{ lineId: line.id }}
                  displayText={lineName}
                  onSelect={handleSelectLine}
                  onAnnounce={announceSelection}
                  onHoverStart={() => setHoveredLineId(line.id)}
                  onHoverEnd={() => setHoveredLineId(null)}
                />
              </div>
            </div>
          </div>

          {shot.kind !== 'Normal' && <div className="text-warning text-sm">{kindBadge}</div>}

          {shot.comment && (
            <div className="pt-2 border-t border-border">
              <span className="text-sm text-text-secondary">{t('detailPanel.labels.comment')}:</span>
              <p className="text-sm mt-1">{shot.comment}</p>
            </div>
          )}

          <div className="pt-2 border-t border-border">
            <CreditDetail targets={shotTargets} credits={shotCredits} scope="shot" />
          </div>
        </div>
      </Panel>

      <div className="p-3 border-t border-border space-y-2">
        <Button
          size="sm"
          className="w-full"
          onClick={handleCycleShotKind}
          disabled={editingBlocked}
          data-testid="shot-kind-cycle-button"
        >
          {t('detailPanel.shot.markAs', { kind: t(SHOT_KIND_LABEL_KEY[nextShotKind(shot.kind)]) })}
        </Button>
        <Button size="sm" variant="danger" className="w-full" onClick={handleDelete} disabled={editingBlocked} data-testid="shot-delete-button">
          {t('common.actions.delete')}
        </Button>
      </div>
    </div>
  );
}

interface LineDetailProps {
  line: Line;
  stations: Station[];
  totalProjectLength: number;
}

function LineDetail({ line, stations, totalProjectLength }: LineDetailProps) {
  const { t } = useTranslation();
  const editingBlocked = useEditingBlocked();
  const { openDialog, setSelection, triggerZoomToLine, announceSelection, setHoveredStation } = useUIStore(
    useShallow((s) => ({ openDialog: s.openDialog, setSelection: s.setSelection, triggerZoomToLine: s.triggerZoomToLine, announceSelection: s.announceSelection, setHoveredStation: s.setHoveredStation }))
  );
  const precision = useDecimalPrecision();

  // Who surveyed the line and when: the aggregate over its shots, editable.
  const credits = useMemo(() => lineCredits(line), [line]);

  // Calculate line statistics
  // Length leaves out Reference shots, matching the project total it is a
  // share of (#689); the average is over the same shots.
  const { length: lineLength, shotCount: lengthShotCount } = surveyedLength(line.shots);
  const shotCount = line.shots.length;
  const referenceCount = line.shots.filter((s) => s.kind !== 'Normal').length;
  const avgShotLength = lengthShotCount > 0 ? lineLength / lengthShotCount : null;
  // Reference length is left out of the line length; say so, or a line of
  // Reference shots reads "0.0 m" with no reason next to a table of real lengths.
  const referenceLength = line.shots.reduce((sum, s) => (s.kind === 'Reference' ? sum + s.length : sum), 0);
  const contributionPercent = totalProjectLength > 0
    ? (lineLength / totalProjectLength) * 100
    : 0;

  // Get stations for this line and calculate depth range
  const lineStations = stations.filter(
    (s) => s.station_ref.line_id === line.id
  );
  const depths = lineStations.map((s) => s.coordinates.z);
  const minDepth = depths.length > 0 ? Math.min(...depths) : 0;
  const maxDepth = depths.length > 0 ? Math.max(...depths) : 0;

  // Endpoint stations for navigation links
  // First station: index 0, position 0 (0-indexed)
  // Last station: index = shots.length, position = shots.length (0-indexed)
  const firstStationIndex = 0;
  const firstStationPosition = 0;
  const lastStationIndex = line.shots.length; // Number of shots = last station index
  const lastStationPosition = lastStationIndex;

  // Single station case: when there are 0 shots, there's only station 0 (position 1)
  const hasSingleStation = line.shots.length === 0;

  // Validate station targets exist
  const firstStationExists = findStation(stations, line.id, firstStationIndex) !== undefined;
  const lastStationExists = findStation(stations, line.id, lastStationIndex) !== undefined;

  const handleSelectFirstStation = () => {
    setSource('detail-panel');
    setSelection({
      type: 'station',
      stationRef: { line_id: line.id, station_index: firstStationIndex },
    });
  };

  const handleSelectLastStation = () => {
    setSource('detail-panel');
    setSelection({
      type: 'station',
      stationRef: { line_id: line.id, station_index: lastStationIndex },
    });
  };

  const handleCenterOnLine = () => {
    setSource('detail-panel');
    triggerZoomToLine(line.id);
  };

  const handleEditLine = () => {
    setSource('detail-panel');
    openDialog('edit-line', { lineId: line.id });
  };

  const labels = useIdentityLabels();
  const lineDisplayName = labels.lineName(line.id);

  const centerButton = (
    <button
      type="button"
      onClick={handleCenterOnLine}
      className="p-1 text-text-secondary hover:text-text-primary rounded hover:bg-background-50 focus:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-inset"
      aria-label={t('detailPanel.section.center', { name: lineDisplayName })}
      data-testid="center-line-button"
    >
      <CenterLocateIcon />
    </button>
  );

  return (
    <div className="h-full bg-panel overflow-auto" data-testid="line-detail">
      <Panel bare title={t('detailPanel.section.title')} headerActions={centerButton}>
        <div className="space-y-4">
          {/* Line info */}
          <div className="space-y-2">
            <DetailRow label={t('common.labels.name')} value={lineDisplayName} />
            <DetailRow label={t('detailPanel.labels.color')} value={
              <div className="flex items-center gap-2">
                <span
                  className="w-4 h-4 rounded"
                  style={{ backgroundColor: line.color }}
                />
                <span>{line.color}</span>
              </div>
            } />
            <LineSectionDetailRow line={line} />
          </div>

          {/* Survey credit of every shot of the line */}
          <div className="pt-2 border-t border-border">
            <CreditDetail targets={{ type: 'line', line_id: line.id }} credits={credits} scope="line" />
          </div>

          {/* Survey statistics */}
          <div className="space-y-2 pt-2 border-t border-border">
            <h4 className="text-xs font-medium text-text-secondary uppercase tracking-wide">
              {t('detailPanel.section.statsTitle')}
            </h4>
            <div className="grid grid-cols-2 gap-x-4 gap-y-1">
              <StatCard label={t('detailPanel.section.totalLength')} value={`${formatFixed(lineLength, undefined, precision.length)} ${t('common.units.meters')}`} />
              <StatCard label={t('detailPanel.section.shotCount')} value={formatNumber(shotCount)} />
              <StatCard label={t('detailPanel.section.avgShot')} value={avgShotLength === null ? '-' : `${formatFixed(avgShotLength, undefined, precision.length)} ${t('common.units.meters')}`} />
              {referenceCount > 0 && (
                <StatCard label={t('detailPanel.section.referenceShots')} value={formatNumber(referenceCount)} />
              )}
            </div>
            {referenceLength > 0 && (
              <p className="text-xs text-text-secondary" data-testid="line-reference-length-note">
                {t('detailPanel.section.referenceLengthNote', {
                  length: `${formatFixed(referenceLength, undefined, precision.length)} ${t('common.units.meters')}`,
                })}
              </p>
            )}
          </div>

          {/* Depth range */}
          {lineStations.length > 0 && (
            <div className="space-y-2 pt-2 border-t border-border">
              <h4 className="text-xs font-medium text-text-secondary uppercase tracking-wide">
                {t('detailPanel.section.depthRange')}
              </h4>
              <div className="grid grid-cols-2 gap-x-4 gap-y-1">
                <StatCard label={t('detailPanel.section.shallowest')} value={`${formatFixed(minDepth, undefined, precision.depth)} ${t('common.units.meters')}`} />
                <StatCard label={t('detailPanel.section.deepest')} value={`${formatFixed(maxDepth, undefined, precision.depth)} ${t('common.units.meters')}`} />
              </div>
            </div>
          )}

          {/* Endpoints - first and last station links */}
          {lineStations.length > 0 && (
            <div className="space-y-2 pt-2 border-t border-border">
              <h4 className="text-xs font-medium text-text-secondary uppercase tracking-wide">
                {t('detailPanel.section.endpoints')}
              </h4>
              <div className="space-y-1 text-sm">
                {hasSingleStation ? (
                  // Single station case
                  <div className="flex justify-between">
                    <span className="text-text-secondary">{t('detailPanel.section.onlyStation')}:</span>
                    <DetailLink
                      type="station"
                      target={{ lineId: line.id, position: firstStationPosition }}
                      displayText={labels.station({ line_id: line.id, station_index: firstStationIndex })}
                      fullText={labels.station({ line_id: line.id, station_index: firstStationIndex }, true)}
                      onSelect={handleSelectFirstStation}
                      isMissing={!firstStationExists}
                      onAnnounce={announceSelection}
                      onHoverStart={() => setHoveredStation({ line_id: line.id, station_index: firstStationIndex })}
                      onHoverEnd={() => setHoveredStation(null)}
                    />
                  </div>
                ) : (
                  // Multiple stations case
                  <>
                    <div className="flex justify-between">
                      <span className="text-text-secondary">{t('detailPanel.section.firstStation')}:</span>
                      <DetailLink
                        type="station"
                        target={{ lineId: line.id, position: firstStationPosition }}
                        displayText={labels.station({ line_id: line.id, station_index: firstStationIndex })}
                      fullText={labels.station({ line_id: line.id, station_index: firstStationIndex }, true)}
                        onSelect={handleSelectFirstStation}
                        isMissing={!firstStationExists}
                        onAnnounce={announceSelection}
                        onHoverStart={() => setHoveredStation({ line_id: line.id, station_index: firstStationIndex })}
                        onHoverEnd={() => setHoveredStation(null)}
                      />
                    </div>
                    <div className="flex justify-between">
                      <span className="text-text-secondary">{t('detailPanel.section.lastStation')}:</span>
                      <DetailLink
                        type="station"
                        target={{ lineId: line.id, position: lastStationPosition }}
                        displayText={labels.station({ line_id: line.id, station_index: lastStationIndex })}
                        fullText={labels.station({ line_id: line.id, station_index: lastStationIndex }, true)}
                        onSelect={handleSelectLastStation}
                        isMissing={!lastStationExists}
                        onAnnounce={announceSelection}
                        onHoverStart={() => setHoveredStation({ line_id: line.id, station_index: lastStationIndex })}
                        onHoverEnd={() => setHoveredStation(null)}
                      />
                    </div>
                  </>
                )}
              </div>
            </div>
          )}

          {/* Contribution */}
          <div className="space-y-2 pt-2 border-t border-border">
            <h4 className="text-xs font-medium text-text-secondary uppercase tracking-wide">
              {t('detailPanel.section.contribution')}
            </h4>
            <div className="flex items-center gap-2">
              <div className="flex-1 h-2 bg-background rounded-full overflow-hidden">
                <div
                  className="h-full bg-accent transition-all"
                  style={{ width: `${Math.min(contributionPercent, 100)}%` }}
                />
              </div>
              <span className="text-sm font-medium w-12 text-right">
                {formatPercent(contributionPercent, undefined, 1)}
              </span>
            </div>
          </div>
        </div>
      </Panel>

      <div className="p-3 border-t border-border">
        <Button size="sm" className="w-full" onClick={handleEditLine} disabled={editingBlocked} data-testid="line-edit-button">
          {t('detailPanel.section.edit')}
        </Button>
      </div>
    </div>
  );
}

interface ProjectStatsProps {
  project: Project | null;
  stats: ProjectStatsType | null;
  stations: Station[];
}

function ProjectStats({ project, stats, stations }: ProjectStatsProps) {
  const { t } = useTranslation();
  const precision = useDecimalPrecision();
  if (!project || !stats) {
    return (
      <div className="p-4 text-center text-text-secondary">
        <p>{t('detailPanel.projectStats.empty')}</p>
      </div>
    );
  }

  // Calculate depth range from stations
  const depths = stations.map((s) => s.coordinates.z);
  const minDepth = depths.length > 0 ? Math.min(...depths) : 0;
  const maxDepth = depths.length > 0 ? Math.max(...depths) : 0;

  // The survey date range over every shot, computed by the backend so a
  // binary-loaded project's lazy lines are not materialized for it.
  const earliestDate = stats.first_survey_date ?? null;
  const latestDate = stats.last_survey_date ?? null;

    return (
      <div className="h-full bg-panel overflow-auto">
      {/* `bare`: no surrounding card border - the panel itself is the container.
          The title's border-b is the "Project stats / PROJECT" divider line.
          The invisible button-sized placeholder matches the entity-detail headers
          (which carry the center-on-map button), so the title divider sits at the
          same y in every Details state and the opener tab always aligns to it. */}
      <Panel
        bare
        title={t('detailPanel.projectStats.title')}
        headerActions={
          <button type="button" tabIndex={-1} aria-hidden="true" className="p-1 invisible" data-testid="project-stats-header-placeholder">
            <svg className="w-4 h-4" viewBox="0 0 24 24" />
          </button>
        }
      >
        <div className="space-y-4">
          {/* Project name + file path (#51: relocated from the toolbar center). */}
          <div className="space-y-1">
            <h4 className="text-xs font-medium text-text-secondary uppercase tracking-wide">
              {t('detailPanel.projectStats.project')}
            </h4>
            <div className="text-sm font-medium text-text-primary break-words" data-testid="project-stats-name">
              {project.name}
            </div>
            {project.file_path && (
              <div
                className="text-xs text-text-secondary break-all"
                data-testid="project-stats-path"
                title={project.file_path}
              >
                {project.file_path}
              </div>
            )}
          </div>

          {/* Survey overview */}
          <div className="space-y-2 pt-2 border-t border-border">
            <h4 className="text-xs font-medium text-text-secondary uppercase tracking-wide">
              {t('detailPanel.projectStats.overview')}
            </h4>
            <div className="grid grid-cols-2 gap-x-4 gap-y-1">
              <StatCard label={t('detailPanel.projectStats.totalLength')} value={`${formatFixed(stats.total_length, undefined, precision.length)} ${t('common.units.meters')}`} />
              <StatCard label={t('detailPanel.projectStats.penetration')} value={`${formatFixed(stats.penetration_distance, undefined, precision.length)} ${t('common.units.meters')}`} />
              <StatCard label={t('detailPanel.projectStats.sections')} value={formatNumber(stats.line_count)} />
              <StatCard label={t('detailPanel.projectStats.shots')} value={formatNumber(stats.shot_count)} />
              <StatCard label={t('detailPanel.projectStats.stations')} value={formatNumber(stats.station_count)} />
              {stats.loop_count > 0 && (
                <StatCard label={t('detailPanel.projectStats.loops')} value={formatNumber(stats.loop_count)} />
              )}
            </div>
          </div>

          {/* Depth information */}
          <div className="space-y-2 pt-2 border-t border-border">
            <h4 className="text-xs font-medium text-text-secondary uppercase tracking-wide">
              {t('detailPanel.projectStats.depthRange')}
            </h4>
            <div className="grid grid-cols-2 gap-x-4 gap-y-1">
              <StatCard label={t('detailPanel.projectStats.shallowest')} value={`${formatFixed(minDepth, undefined, precision.depth)} ${t('common.units.meters')}`} />
              <StatCard label={t('detailPanel.projectStats.deepest')} value={`${formatFixed(maxDepth, undefined, precision.depth)} ${t('common.units.meters')}`} />
              <StatCard label={t('detailPanel.projectStats.totalDepth')} value={`${formatFixed(maxDepth - minDepth, undefined, precision.depth)} ${t('common.units.meters')}`} />
            </div>
          </div>

          {/* Survey dates */}
          {(earliestDate || latestDate) && (
            <div className="space-y-2 pt-2 border-t border-border">
              <h4 className="text-xs font-medium text-text-secondary uppercase tracking-wide">
                {t('detailPanel.projectStats.surveyDates')}
              </h4>
              <div className="space-y-1">
                {earliestDate && (
                  <DetailRow
                    label={t('detailPanel.projectStats.firstSurvey')}
                    value={formatDate(new Date(earliestDate), undefined, { year: 'numeric', month: 'short', day: 'numeric' })}
                  />
                )}
                {latestDate && earliestDate !== latestDate && (
                  <DetailRow
                    label={t('detailPanel.projectStats.latestSurvey')}
                    value={formatDate(new Date(latestDate), undefined, { year: 'numeric', month: 'short', day: 'numeric' })}
                  />
                )}
              </div>
            </div>
          )}

          {/* Merges and datums */}
          <div className="space-y-2 pt-2 border-t border-border">
            <h4 className="text-xs font-medium text-text-secondary uppercase tracking-wide">
              {t('detailPanel.projectStats.connections')}
            </h4>
            <div className="grid grid-cols-2 gap-x-4 gap-y-1">
              <StatCard label={t('detailPanel.projectStats.stationMerges')} value={formatNumber(project.station_merges.length)} />
              <StatCard label={t('detailPanel.projectStats.zeroDatums')} value={formatNumber(project.zero_datums.length)} />
            </div>
          </div>

          {/* Per-project settings (local .stalx) + the SpeleoDB project actions. */}
          <ProjectStatsActions />
        </div>
      </Panel>
    </div>
  );
}

interface ClosuresSectionProps {
  closures: ClosureRef[];
  loopClosures: LoopClosure[];
  stations: Station[];
  onSelectStation: (ref: { line_id: number; station_index: number }) => void;
  onAnnounce: (message: string) => void;
  onHoverStart: (ref: { line_id: number; station_index: number }) => void;
  onHoverEnd: () => void;
}

function ClosuresSection({
  closures,
  loopClosures,
  stations,
  onSelectStation,
  onAnnounce,
  onHoverStart,
  onHoverEnd,
}: ClosuresSectionProps) {
  const { t } = useTranslation();
  const labels = useIdentityLabels();
  const removeLoopClosure = useProjectStore((s) => s.removeLoopClosure);
  const editingBlocked = useEditingBlocked();
  const precision = useDecimalPrecision();
  const [expanded, setExpanded] = useState(true);
  const [removingClosureId, setRemovingClosureId] = useState<bigint | null>(null);
  const contentId = 'closures-section-content';

  // Precompute station existence set for O(1) lookups
  const stationKeySet = useMemo(() => {
    const keys = new Set<string>();
    for (const s of stations) {
      keys.add(`${s.station_ref.line_id}:${s.station_ref.station_index}`);
    }
    return keys;
  }, [stations]);

  const stationExists = (ref: { line_id: number; station_index: number }): boolean => {
    return stationKeySet.has(`${ref.line_id}:${ref.station_index}`);
  };

  // Precompute loop closures map for O(1) lookups
  const closureMap = useMemo(() => {
    const map = new Map<bigint, LoopClosure>();
    for (const lc of loopClosures) {
      map.set(lc.id, lc);
    }
    return map;
  }, [loopClosures]);

  // Group closures by closure_id and sort deterministically
  const sortedGroups = useMemo(() => {
    const grouped = new Map<bigint, ClosureRef[]>();
    for (const c of closures) {
      const existing = grouped.get(c.closure_id);
      if (existing) {
        existing.push(c);
      } else {
        grouped.set(c.closure_id, [c]);
      }
    }

    // Sort: direct closures first, then transitive; tiebreak by closure_id ascending
    return Array.from(grouped.entries()).sort(([idA, refsA], [idB, refsB]) => {
      const aHasDirect = refsA.some((r) => !r.is_transitive);
      const bHasDirect = refsB.some((r) => !r.is_transitive);
      if (aHasDirect && !bHasDirect) return -1;
      if (!aHasDirect && bHasDirect) return 1;
      if (idA < idB) return -1;
      if (idA > idB) return 1;
      return 0;
    });
  }, [closures]);

  const handleRemoveClosure = async (closureId: bigint) => {
    trackContextMenuAction('remove-closure', 'detail-panel');
    setRemovingClosureId(closureId);
    try {
      await removeLoopClosure(closureId);
    } finally {
      setRemovingClosureId(null);
    }
  };

  return (
    <div className="pt-2 border-t border-border" data-testid="closures-section">
      <button
        type="button"
        onClick={() => setExpanded(!expanded)}
        className="flex items-center gap-1 text-sm text-text-secondary hover:text-text-primary w-full text-left"
        data-testid="closures-section-toggle"
        aria-expanded={expanded}
        aria-controls={contentId}
      >
        <span className="text-xs">
          {expanded ? <ChevronDownIcon size={12} /> : <ChevronRightIcon size={12} />}
        </span>
        <span>{t('detailPanel.closures.title')}</span>
      </button>
      {expanded && (
        <div className="mt-1 space-y-2" id={contentId} data-testid="closures-section-content">
          {sortedGroups.map(([closureId, refs]) => {
            const closureIdLabel = closureId.toString();
            const closure = closureMap.get(closureId);
            const directRefs = refs.filter((r) => !r.is_transitive);
            const transitiveRefs = refs.filter((r) => r.is_transitive);

            const isTransitiveOnly = directRefs.length === 0 && transitiveRefs.length > 0;

            return (
              <div key={closureIdLabel} className={`space-y-1${isTransitiveOnly ? ' opacity-50' : ''}`}>
                <div className="flex items-start justify-between gap-2">
                  <div className="space-y-1 min-w-0">
                    {directRefs.length > 0 && (
                      <div className="text-sm" data-testid={`closure-direct-${closureIdLabel}`}>
                        <span className="text-text-secondary">{t('detailPanel.closures.direct')} </span>
                        {directRefs.map((ref, i) => (
                          <span key={i}>
                            {i > 0 && ', '}
                            <DetailLink
                              type="station"
                              target={{ lineId: ref.station_ref.line_id, position: ref.station_ref.station_index }}
                              displayText={labels.station(ref.station_ref, true)}
                              pillLabel={labels.station(ref.station_ref)}
                              pillContext={labels.lineName(ref.station_ref.line_id)}
                              onSelect={() => onSelectStation(ref.station_ref)}
                              isMissing={!stationExists(ref.station_ref)}
                              onAnnounce={onAnnounce}
                              onHoverStart={() => onHoverStart(ref.station_ref)}
                              onHoverEnd={onHoverEnd}
                            />
                          </span>
                        ))}
                      </div>
                    )}
                    {transitiveRefs.length > 0 && (
                      <div className="text-sm" data-testid={`closure-transitive-${closureIdLabel}`}>
                        <span className="text-text-secondary">{t('detailPanel.closures.transitive')} </span>
                        {transitiveRefs.map((ref, i) => (
                          <span key={i}>
                            {i > 0 && ', '}
                            <DetailLink
                              type="station"
                              target={{ lineId: ref.station_ref.line_id, position: ref.station_ref.station_index }}
                              displayText={labels.station(ref.station_ref, true)}
                              pillLabel={labels.station(ref.station_ref)}
                              pillContext={labels.lineName(ref.station_ref.line_id)}
                              onSelect={() => onSelectStation(ref.station_ref)}
                              isMissing={!stationExists(ref.station_ref)}
                              onAnnounce={onAnnounce}
                              onHoverStart={() => onHoverStart(ref.station_ref)}
                              onHoverEnd={onHoverEnd}
                            />
                            {ref.via_station && (
                              <span>
                                {t('detailPanel.closures.viaPrefix')}
                                <DetailLink
                                  type="station"
                                  target={{ lineId: ref.via_station.line_id, position: ref.via_station.station_index }}
                                  displayText={labels.station(ref.via_station!, true)}
                              pillLabel={labels.station(ref.via_station!)}
                              pillContext={labels.lineName(ref.via_station.line_id)}
                                  onSelect={() => onSelectStation(ref.via_station!)}
                                  isMissing={!stationExists(ref.via_station)}
                                  onAnnounce={onAnnounce}
                                  onHoverStart={() => onHoverStart(ref.via_station!)}
                                  onHoverEnd={onHoverEnd}
                                />
                                {t('detailPanel.closures.viaSuffix')}
                              </span>
                            )}
                          </span>
                        ))}
                      </div>
                    )}
                  </div>
                  <button
                    type="button"
                    onClick={() => {
                      void handleRemoveClosure(closureId);
                    }}
                    className="p-1 rounded text-text-secondary hover:text-error hover:bg-error-10 focus:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-inset disabled:opacity-50 disabled:cursor-not-allowed"
                    title="Remove Closure"
                    aria-label={refs[0] ? `Remove closure with ${labels.station(refs[0].station_ref, true)}` : 'Remove closure'}
                    data-testid={`detail-remove-closure-${closureIdLabel}`}
                    disabled={editingBlocked || !closure || removingClosureId === closureId}
                  >
                    <LinkOffIcon size={14} />
                  </button>
                </div>
                {closure ? (
                  <div className="text-xs text-text-secondary" data-testid={`closure-metrics-${closureIdLabel}`}>
                    {t('detailPanel.closures.metrics', {
                      path: `${formatFixed(closure.path_length, undefined, precision.length)}${t('common.units.meters')}`,
                      error: `${formatFixed(closure.misclosure_distance, undefined, precision.length)}${t('common.units.meters')}`,
                      percent: formatPercent(closure.error_percentage, undefined, 2),
                    })}
                  </div>
                ) : (
                  <div className="text-xs text-text-secondary" data-testid={`closure-metrics-${closureId}`}>
                    {t('detailPanel.closures.unavailable')}
                  </div>
                )}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

