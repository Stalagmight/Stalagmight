import { useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { CylinderTableActions, CylindersTable } from './CylindersTable';
import { DpvRigEditor } from './DpvRigEditor';
import { ProfileChart } from './ProfileChart';
import { SegmentsTable } from './SegmentsTable';

type PlanTab = 'cylinders' | 'dpv' | 'segments' | 'profile';

/**
 * The Plan-mode bottom panel (#38): tabbed tabular data that replaces the survey data
 * table while planning. "Cylinders" is the tank list, which fills the pane on its own;
 * "DPV" is the optional rig editor (#317, moved out of Settings so the rig lives alongside
 * the rest of the dive's gear, and split off the tank list so neither has to share the
 * fold); "Segments" (#317) is the per-span pace/propulsion editor; "Profile" is the
 * anticipated depth/distance/runtime profile of the planned route (#299), drawn from the
 * round-trip series the gas engine publishes on `GasReachField.route_profile`. Tab state is
 * local + transient (never persisted).
 *
 * The Cylinders tab's two non-row controls (add a tank, conservative gas) ride at the right
 * end of the tab strip rather than under the grid - see {@link CylinderTableActions}.
 */
export function PlanTablesPanel() {
  const { t } = useTranslation();
  const [tab, setTab] = useState<PlanTab>('cylinders');
  return (
    <div className="h-full bg-panel flex flex-col" data-testid="plan-tables-panel">
      <div className="flex items-center border-b border-border shrink-0">
        <TabButton
          active={tab === 'cylinders'}
          onClick={() => setTab('cylinders')}
          testId="plan-tab-cylinders"
        >
          {t('planTables.tabs.cylinders')}
        </TabButton>
        <TabButton active={tab === 'dpv'} onClick={() => setTab('dpv')} testId="plan-tab-dpv">
          {t('planTables.tabs.dpv')}
        </TabButton>
        <TabButton active={tab === 'segments'} onClick={() => setTab('segments')} testId="plan-tab-segments">
          {t('planTables.tabs.segments')}
        </TabButton>
        <TabButton active={tab === 'profile'} onClick={() => setTab('profile')} testId="plan-tab-profile">
          {t('planTables.tabs.profile')}
        </TabButton>
        {tab === 'cylinders' && (
          <div className="ml-auto flex items-center pl-4 pr-3">
            <CylinderTableActions />
          </div>
        )}
      </div>
      {/* Only the rig editor scrolls its pane: the two grids own their own scroller (a second
          one around them would double up the scrollbars) and the chart sizes itself to the
          pane, so it clips instead. */}
      <div className={`flex-1 min-h-0 ${tab === 'dpv' ? 'overflow-auto' : 'overflow-hidden'}`}>
        {tab === 'cylinders' ? (
          <CylindersTable />
        ) : tab === 'dpv' ? (
          <DpvRigEditor />
        ) : tab === 'segments' ? (
          <SegmentsTable />
        ) : (
          <ProfileChart />
        )}
      </div>
    </div>
  );
}

function TabButton({
  active,
  onClick,
  children,
  testId,
}: {
  active: boolean;
  onClick: () => void;
  children: ReactNode;
  testId?: string;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-pressed={active}
      className={`px-4 py-2 text-sm font-medium border-b-2 transition-colors ${
        active
          ? 'border-accent text-text-primary'
          : 'border-transparent text-text-secondary hover:text-text-primary'
      }`}
      data-testid={testId}
    >
      {children}
    </button>
  );
}
