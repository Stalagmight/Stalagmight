/**
 * The hover card on a row name: everything about the station / shot that the
 * columns may not show, with the merges and closures spelled out and the
 * navigation actions at hand. Values reuse the column texts so they read
 * exactly like the cells. Its title names the section (the gutter shows only
 * the row ID), and the ID row adds the "<label>:<index>" name the link chips
 * use.
 */
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import type { StationRef } from '../../types/project';
import { SHOT_KIND_LABEL_KEY } from '../../utils/shotKind';
import { normalizeName } from '../../utils/people';
import { useDataModel } from './DataModelContext';
import { useDataNav } from './useDataNavigation';
import type { DataColumnKey, DataEntityRow, DataModelCtx } from './types';
import { isShotFrozen, misclosureText, rowWarnings, shotKindOf, stationDisplayName } from './model/columns';
import { shotCredit, shotName, stationName } from './model/ctx';
import { stationLinks, stationRefKey, shotKeyAt } from './model/links';
import { rowIdLabel, shotIdLabel } from './model/rowIds';
import { proseAround } from './model/linkItems';
import { dataRowPart } from './dataTestIds';

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <>
      <dt className="whitespace-nowrap">{label}</dt>
      <dd className="min-w-0 break-words text-text-primary">{children}</dd>
    </>
  );
}

const LINK_CLS = 'cursor-pointer text-left text-accent underline decoration-dotted underline-offset-2 hover:decoration-solid';

export function DataHoverCard({ row }: { row: DataEntityRow }) {
  const { t } = useTranslation();
  const model = useDataModel();
  const nav = useDataNav();
  if (!model || !nav) return null;
  const { ctx } = model;
  const part = dataRowPart(row);
  const text = (key: DataColumnKey) => model.columnByKey.get(key)?.text(row, ctx) ?? '';
  const section = ctx.sectionsById.get(row.sectionId);
  const fullName = ctx.sectionNames.get(row.sectionId) ?? String(row.sectionId);

  const goTo = (ref: StationRef, label: ReactNode, testSuffix: string) => (
    <button
      type="button"
      className={LINK_CLS}
      onMouseDown={(e) => e.stopPropagation()}
      onClick={(e) => {
        e.stopPropagation();
        nav.goToRow(stationRefKey(ref));
      }}
      data-testid={`data-hover-link-${part}-${testSuffix}`}
    >
      {label}
    </button>
  );

  const header = (
    <div className="mb-1.5 flex items-center gap-1.5 text-sm">
      <span className="h-2.5 w-2.5 shrink-0 rounded-full" style={{ backgroundColor: section?.color }} aria-hidden="true" />
      <span className="truncate font-semibold">{fullName}</span>
      <span className="shrink-0 text-text-secondary">{`#${row.sectionId}`}</span>
    </div>
  );

  let body: ReactNode;
  if (row.kind === 'station') {
    const links = stationLinks(ctx, row.sectionId, row.stationIndex, row.g);
    const name = stationDisplayName(ctx, row.g);
    const shotIn = shotKeyAt(ctx, row.sectionId, row.stationIndex - 1);
    const shotOut = shotKeyAt(ctx, row.sectionId, row.stationIndex);
    const shotLabel = (key: string | null, position: number) => {
      const r = key ? model.rowByKey.get(key) : undefined;
      if (!r || r.kind !== 'shot') return null;
      const len = model.columnByKey.get('length')?.text(r, ctx) ?? '';
      return `${shotIdLabel(row.sectionId, position)} · ${len} m`;
    };
    const issue = stationIssueLevel(ctx, row);
    // "Closes to <name>": the prose normal, the station name bold (as the chips).
    const closesTo = proseAround(ctx, 'dataWorkspace.links.closesTo', 'Closes to {{name}}');
    body = (
      <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5 text-xs">
        <Row label={t('dataWorkspace.hover.station')}>
          {`${rowIdLabel(row)} (${stationName(ctx, row.sectionId, row.stationIndex)})`}
        </Row>
        {name && <Row label={t('dataWorkspace.hover.name')}>{name}</Row>}
        <Row label={t('dataWorkspace.hover.xy')}>{`${text('x')} / ${text('y')} m`}</Row>
        <Row label={t('dataWorkspace.hover.depth')}>{`${text('depth')} m`}</Row>
        {text('lat') && <Row label={t('dataWorkspace.hover.gps')}>{`${text('lat')}, ${text('long')}`}</Row>}
        {links.zeroDatum && <Row label={t('dataWorkspace.hover.zeroDatum')}>{t('dataWorkspace.hover.yes')}</Row>}
        {text('comment') && <Row label={t('dataWorkspace.hover.comment')}>{text('comment')}</Row>}
        {links.merges.length > 0 && (
          <Row label={t('dataWorkspace.hover.merges')}>
            <span className="flex flex-wrap gap-x-2" data-testid={`data-hover-merges-${part}`}>
              {links.merges.map((m) => (
                <span key={`${m.section_id}-${m.station_index}`}>
                  {goTo(
                    m,
                    <span className="font-semibold">{stationName(ctx, m.section_id, m.station_index)}</span>,
                    `${m.section_id}-${m.station_index}`,
                  )}
                </span>
              ))}
            </span>
          </Row>
        )}
        {links.closures.length > 0 && (
          <Row label={t('dataWorkspace.hover.closures')}>
            <span className="flex flex-col" data-testid={`data-hover-closures-${part}`}>
              {links.closures.map((c) => (
                <span key={`${c.closureId}-${c.partner.section_id}-${c.partner.station_index}`}>
                  {goTo(
                    c.partner,
                    <>
                      {closesTo[0]}
                      <span className="font-semibold">{stationName(ctx, c.partner.section_id, c.partner.station_index)}</span>
                      {closesTo[1]}
                    </>,
                    `${c.partner.section_id}-${c.partner.station_index}`,
                  )}
                  {misclosureText(ctx, c.misclosure, c.errorPct) && (
                    <span className="ml-1.5 tabular-nums text-text-secondary">{misclosureText(ctx, c.misclosure, c.errorPct)}</span>
                  )}
                </span>
              ))}
            </span>
          </Row>
        )}
        {shotLabel(shotIn, row.stationIndex) && <Row label={t('dataWorkspace.hover.incoming')}>{shotLabel(shotIn, row.stationIndex)}</Row>}
        {shotLabel(shotOut, row.stationIndex + 1) && (
          <Row label={t('dataWorkspace.hover.outgoing')}>{shotLabel(shotOut, row.stationIndex + 1)}</Row>
        )}
        <Row label={t('dataWorkspace.hover.issues')}>
          <span className={issue === 'error' ? 'text-error' : issue === 'warning' ? 'text-warning' : ''}>
            {t(issue === 'error' ? 'dataWorkspace.hover.issueLevelError' : issue === 'warning' ? 'dataWorkspace.hover.issueLevelWarning' : 'dataWorkspace.hover.issueLevelNone')}
          </span>
        </Row>
      </dl>
    );
  } else {
    const kind = shotKindOf(ctx, row);
    const warnings = rowWarnings(ctx, row);
    const credit = shotCredit(ctx, row);
    const names = (people: readonly { name: string }[]) => people.map((p) => normalizeName(p.name)).join(', ');
    body = (
      <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5 text-xs">
        <Row label={t('dataWorkspace.hover.shot')}>{`${rowIdLabel(row)} (${shotName(ctx, row.sectionId, row.position)})`}</Row>
        <Row label={t('dataWorkspace.hover.fromTo')}>{text('fromTo')}</Row>
        <Row label={t('dataWorkspace.columns.length')}>{text('length')}</Row>
        <Row label={t('dataWorkspace.columns.azimuth')}>{`${text('azimuth')} (${text('azimuth_in')} / ${text('azimuth_out')})`}</Row>
        {text('inclination') && <Row label={t('dataWorkspace.columns.inclination')}>{text('inclination')}</Row>}
        <Row label={t('dataWorkspace.hover.depth')}>{`${text('depth_in')} → ${text('depth_out')} m`}</Row>
        <Row label={t('dataTable.columnsMenu.lrud')}>{['left', 'right', 'up', 'down'].map((k) => text(k as DataColumnKey) || '-').join(' / ')}</Row>
        <Row label={t('dataWorkspace.hover.kind')}>{t(SHOT_KIND_LABEL_KEY[kind])}</Row>
        {isShotFrozen(ctx, row) && <Row label={t('dataWorkspace.hover.frozen')}>{t('dataWorkspace.hover.frozenText')}</Row>}
        {warnings.length > 0 && (
          <Row label={t('dataWorkspace.hover.warnings')}>
            <ul className="flex flex-col" data-testid={`data-hover-warnings-${part}`}>
              {warnings.map((w, i) => (
                <li
                  key={`${w.code}-${i}`}
                  className={w.isResolved ? 'text-text-secondary line-through' : w.severity === 'error' ? 'text-error' : 'text-warning'}
                >
                  {w.isResolved ? `${w.message} (${t('dataWorkspace.hover.ignored')})` : w.message}
                </li>
              ))}
            </ul>
          </Row>
        )}
        {credit && (credit.surveyors.length > 0 || credit.explorers.length > 0 || credit.date) && (
          <Row label={t('dataWorkspace.hover.credit')}>
            <span className="flex flex-col">
              {credit.surveyors.length > 0 && <span>{t('dataWorkspace.hover.surveyedBy', { names: names(credit.surveyors) })}</span>}
              {credit.explorers.length > 0 && <span>{t('dataWorkspace.hover.exploredBy', { names: names(credit.explorers) })}</span>}
              {credit.date && <span>{t('dataWorkspace.hover.onDate', { date: credit.date })}</span>}
            </span>
          </Row>
        )}
        {text('comment') && <Row label={t('dataWorkspace.hover.comment')}>{text('comment')}</Row>}
      </dl>
    );
  }

  return (
    <div className="flex flex-col" data-testid={`data-hover-card-${part}`} onMouseDown={(e) => e.stopPropagation()}>
      {header}
      {body}
      <div className="mt-2 flex gap-2 border-t border-border pt-1.5 text-xs">
        <button
          type="button"
          className="rounded px-1.5 py-0.5 text-accent hover:bg-accent-10"
          onClick={(e) => {
            e.stopPropagation();
            nav.showOnMap(row);
          }}
          data-testid={`data-hover-show-on-map-${part}`}
        >
          {`${t('dataWorkspace.menu.showOnMap')} (${t('dataWorkspace.shortcuts.showOnMap')})`}
        </button>
        <button
          type="button"
          className="rounded px-1.5 py-0.5 text-accent hover:bg-accent-10"
          onClick={(e) => {
            e.stopPropagation();
            nav.openInSurvey(row);
          }}
          data-testid={`data-hover-open-in-survey-${part}`}
        >
          {`${t('dataWorkspace.menu.openInSurvey')} (${t('dataWorkspace.shortcuts.openInSurvey')})`}
        </button>
      </div>
    </div>
  );
}

/** The worst unresolved issue on the shots touching a station, or its own. */
export function stationIssueLevel(ctx: DataModelCtx, row: Extract<DataEntityRow, { kind: 'station' }>): 'error' | 'warning' | null {
  let level: 'error' | 'warning' | null = null;
  const consider = (list: readonly { severity: string; isResolved: boolean }[]) => {
    for (const w of list) {
      if (w.isResolved) continue;
      if (w.severity === 'error') level = 'error';
      else if (level === null) level = 'warning';
    }
  };
  consider(rowWarnings(ctx, row));
  const sec = ctx.sectionsById.get(row.sectionId);
  if (sec) {
    for (const i of [row.stationIndex - 1, row.stationIndex]) {
      if (i < 0 || i >= sec.shotCount) continue;
      consider(ctx.warningsByShot.get(`${row.sectionId}:${ctx.data.shotIds[sec.shotOffset + i]}`) ?? []);
    }
  }
  return level;
}
