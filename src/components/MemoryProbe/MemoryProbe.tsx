/**
 * In-app memory probe overlay.
 *
 * Polls the backend `get_memory_stats` IPC for /proc-derived RSS of the
 * Tauri backend and WebKit children, and combines that with frontend
 * counters published from SurveyLayer/GlobalSurveyLayer/the Zustand stores.
 * Renders a compact bottom-left panel summarizing each retention bucket.
 *
 * Visibility is driven by the hamburger menu's Debug ▸ "Memory Stats" toggle
 * (`showMemoryStats`), or forced on via `config.debug.memory_probe`.
 *
 * The "Dump" button writes a JSON snapshot to the system temp dir via the
 * Tauri fs plugin, with a console.log fallback if the user denied fs.
 */
import { useEffect, useMemo, useRef, useState } from 'react';
import { invokeIpc } from '../../utils/ipc';
import type { MemoryStats } from '../../types/ipc/MemoryStats';
import {
  getFeatureCounts,
  estimateGeometryBytes,
  estimateMaplibreFeatureBytes,
  estimateFeatureBytes,
  getPerfTimings,
  startLongTaskObserver,
  type FeatureSourceKind,
} from '../../utils/memoryProbe';
import { useProjectStore } from '../../store/projectStore';
import { useGlobalViewStore } from '../../store/globalViewStore';
import { useUIStore } from '../../store/uiStore';

const POLL_INTERVAL_MS = 2000;
const MB = 1024 * 1024;
const KB = 1024;

function fmtBytes(b: number): string {
  if (!Number.isFinite(b) || b < 0) return '-';
  if (b >= MB * 100) return `${(b / MB).toFixed(0)} MB`;
  if (b >= MB) return `${(b / MB).toFixed(1)} MB`;
  if (b >= KB) return `${(b / KB).toFixed(0)} KB`;
  return `${b} B`;
}

function fmtKb(kb: bigint | number): string {
  const n = typeof kb === 'bigint' ? Number(kb) : kb;
  return fmtBytes(n * 1024);
}

interface Snapshot {
  ts: number;
  memory: MemoryStats | null;
  featureCounts: ReturnType<typeof getFeatureCounts>;
  geometryDataSize: number;
  geometryDataTotalShots: number;
  projectShots: number;
  projectStations: number;
  projectLines: number;
}

function takeSnapshot(memory: MemoryStats | null): Snapshot {
  const featureCounts = getFeatureCounts();
  const project = useProjectStore.getState().project;
  const surveyData = useProjectStore.getState().surveyData;
  const stations = useProjectStore.getState().stations;
  const geometryData = useGlobalViewStore.getState().geometryData;

  let geometryTotalShots = 0;
  for (const g of geometryData.values()) {
    geometryTotalShots += g.shots.length;
  }

  // Read shot count from the SoA store (typed-array length) so the probe
  // itself doesn't force-materialize every line's lazy `.shots` getter
  // on every tick. The legacy fallback only fires when no project is loaded
  // through the binary IPC path.
  let projectShots = 0;
  if (surveyData) {
    projectShots = surveyData.shotCount;
  } else if (project) {
    for (const s of project.lines) projectShots += s.shots.length;
  }

  return {
    ts: Date.now(),
    memory,
    featureCounts,
    geometryDataSize: geometryData.size,
    geometryDataTotalShots: geometryTotalShots,
    projectShots,
    projectStations: stations.length,
    projectLines: project?.lines.length ?? 0,
  };
}

export function MemoryProbe(): JSX.Element | null {
  const configEnabled = useProjectStore((s) => s.config?.debug?.memory_probe ?? false);
  const showMemoryStats = useUIStore((s) => s.showMemoryStats);
  const visible = showMemoryStats || configEnabled;

  const [memory, setMemory] = useState<MemoryStats | null>(null);
  const [snapshot, setSnapshot] = useState<Snapshot>(() => takeSnapshot(null));
  const [collapsed, setCollapsed] = useState(false);
  const prevWebRssRef = useRef<number | null>(null);
  const [rssDelta, setRssDelta] = useState<number | null>(null);
  /** Last dump result shown to the user; cleared after a few seconds. */
  const [dumpStatus, setDumpStatus] = useState<string | null>(null);

  // Long-task PerformanceObserver - runs even when the probe is hidden
  // so we capture freezes the user didn't expect.
  useEffect(() => {
    startLongTaskObserver();
  }, []);

  // Poll backend + take frontend snapshot at a fixed interval. Stop when hidden
  // to avoid the IPC churn polluting the very thing we're measuring.
  useEffect(() => {
    if (!visible) return;
    let cancelled = false;

    async function tick() {
      try {
        const stats = await invokeIpc<MemoryStats>('get_memory_stats', undefined, {
          purpose: 'memory probe poll',
        });
        if (cancelled) return;

        // Compute delta vs previous WebProcess RSS so we can see growth at a glance.
        const web = stats.webkit_processes.find((p) => p.role === 'webkit-web');
        if (web) {
          const cur = Number(web.rss_kb);
          if (prevWebRssRef.current !== null) {
            setRssDelta(cur - prevWebRssRef.current);
          }
          prevWebRssRef.current = cur;
        }

        setMemory(stats);
        setSnapshot(takeSnapshot(stats));
      } catch (e) {
        // Probe is best-effort; swallow errors so it never crashes the app.
        if (!cancelled) console.warn('[MemoryProbe] poll failed:', e);
      }
    }

    void tick();
    const id = window.setInterval(() => void tick(), POLL_INTERVAL_MS);
    return () => {
      cancelled = true;
      window.clearInterval(id);
    };
  }, [visible]);

  // Geometry-cache bytes from the GlobalView store. Walk it lazily since the
  // user may have a few hundred systems but each estimate is O(1).
  const geometryBytes = useMemo(() => {
    let total = 0;
    for (const g of useGlobalViewStore.getState().geometryData.values()) {
      total += estimateGeometryBytes(g);
    }
    return total;
  }, [snapshot.geometryDataSize, snapshot.geometryDataTotalShots]);

  const featureBytes = useMemo(() => estimateMaplibreFeatureBytes(), [snapshot.featureCounts]);

  if (!visible) return null;

  const web = memory?.webkit_processes.find((p) => p.role === 'webkit-web');
  const net = memory?.webkit_processes.find((p) => p.role === 'webkit-network');
  const gpu = memory?.webkit_processes.find((p) => p.role === 'webkit-gpu');

  return (
    <div
      data-testid="memory-probe-panel"
      style={{
        position: 'fixed',
        bottom: 8,
        left: 8,
        zIndex: 9999,
        backgroundColor: 'rgba(20, 20, 20, 0.88)',
        color: '#eee',
        font: '11px/1.35 ui-monospace, SFMono-Regular, Menlo, monospace',
        padding: collapsed ? '4px 8px' : '8px 10px',
        borderRadius: 4,
        border: '1px solid #444',
        minWidth: collapsed ? 0 : 280,
        maxWidth: 360,
        boxShadow: '0 2px 8px rgba(0,0,0,0.5)',
        pointerEvents: 'auto',
      }}
    >
      <div
        style={{ display: 'flex', alignItems: 'center', gap: 8, cursor: 'pointer', userSelect: 'none' }}
        onClick={() => setCollapsed((v) => !v)}
        data-testid="memory-probe-toggle"
      >
        <strong style={{ flex: 1 }}>mem probe {collapsed ? '▸' : '▾'}</strong>
        {!collapsed && (
          <>
            <button
              data-testid="memory-probe-copy-snapshot"
              onClick={(e) => {
                e.stopPropagation();
                void dumpSnapshot(snapshot, featureBytes, geometryBytes, setDumpStatus);
              }}
              style={btnStyle}
              title="Copy JSON snapshot to clipboard"
            >
              {dumpStatus ?? 'dump'}
            </button>
          </>
        )}
      </div>

      {!collapsed && (
        <>
          <ProbeGroup title="processes (RSS from /proc)">
            <Row label="backend" value={fmtKb(memory?.backend.rss_kb ?? 0)} />
            <Row
              label="WebKitWebProcess"
              value={fmtKb(web?.rss_kb ?? 0)}
              tone={web && Number(web.rss_kb) > 1024 * 1024 * 2 ? 'warn' : 'normal'}
              suffix={rssDelta !== null ? formatDelta(rssDelta) : ''}
            />
            <Row label="WebKitNetworkProcess" value={fmtKb(net?.rss_kb ?? 0)} />
            {gpu && <Row label="WebKitGPUProcess" value={fmtKb(gpu.rss_kb)} />}
            <Row label="total" value={fmtKb(memory?.total_rss_kb ?? 0)} bold />
          </ProbeGroup>

          <ProbeGroup title="JS heap retention (est.)">
            <Row label="GlobalView geometryData" value={fmtBytes(geometryBytes)} suffix={`${snapshot.geometryDataSize} sys · ${snapshot.geometryDataTotalShots} shots`} />
            <Row
              label="MapLibre feature copies"
              value={fmtBytes(featureBytes)}
              suffix="main+worker"
              tone={featureBytes > 200 * MB ? 'warn' : 'normal'}
            />
          </ProbeGroup>

          <ProbeGroup title="MapLibre source counts">
            {(Object.keys(snapshot.featureCounts.perSource) as FeatureSourceKind[])
              .filter((k) => snapshot.featureCounts.perSource[k] > 0)
              .map((k) => {
                const n = snapshot.featureCounts.perSource[k];
                const bytes = n * estimateFeatureBytes(k);
                const calls = snapshot.featureCounts.setDataCalls[k];
                return (
                  <Row
                    key={k}
                    label={k}
                    value={`${n.toLocaleString()}`}
                    suffix={`${fmtBytes(bytes)} · ${calls} calls`}
                    tone={bytes > 50 * MB ? 'warn' : 'normal'}
                  />
                );
              })}
            {Object.values(snapshot.featureCounts.perSource).every((v) => v === 0) && (
              <div style={{ color: '#888' }}>(no map sources active)</div>
            )}
          </ProbeGroup>

          <ProbeGroup title="projectStore">
            <Row label="lines" value={snapshot.projectLines.toLocaleString()} />
            <Row label="shots" value={snapshot.projectShots.toLocaleString()} />
            <Row label="stations" value={snapshot.projectStations.toLocaleString()} />
          </ProbeGroup>

          {Object.keys(getPerfTimings()).length > 0 && (
            <ProbeGroup title="recent perf">
              {Object.entries(getPerfTimings()).map(([label, t]) => (
                <Row
                  key={label}
                  label={label}
                  value={`${t.ms} ms`}
                  tone={t.ms > 500 ? 'warn' : 'normal'}
                  suffix={`${Math.max(0, Math.round((Date.now() - t.ts) / 1000))}s ago`}
                />
              ))}
            </ProbeGroup>
          )}

          <div style={{ fontSize: 10, color: '#888', marginTop: 6 }}>
            Debug ▸ Memory Stats toggles · estimates ±2× - see backend RSS for truth
          </div>
        </>
      )}
    </div>
  );
}

const btnStyle: React.CSSProperties = {
  fontSize: 10,
  padding: '2px 6px',
  background: '#333',
  color: '#eee',
  border: '1px solid #555',
  borderRadius: 3,
  cursor: 'pointer',
};

function ProbeGroup({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div style={{ marginTop: 6 }}>
      <div style={{ fontSize: 10, color: '#aaa', textTransform: 'uppercase', letterSpacing: 0.3 }}>{title}</div>
      <div>{children}</div>
    </div>
  );
}

function Row({
  label,
  value,
  suffix,
  bold,
  tone,
}: {
  label: string;
  value: string;
  suffix?: string;
  bold?: boolean;
  tone?: 'normal' | 'warn';
}) {
  const valueColor = tone === 'warn' ? '#ff9a4a' : '#eee';
  return (
    <div style={{ display: 'flex', gap: 6, fontWeight: bold ? 600 : 400 }}>
      <span style={{ flex: 1, color: '#bbb', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
        {label}
      </span>
      <span style={{ color: valueColor }}>{value}</span>
      {suffix && <span style={{ color: '#888', fontSize: 10 }}>{suffix}</span>}
    </div>
  );
}

function formatDelta(deltaKb: number): string {
  const sign = deltaKb >= 0 ? '+' : '-';
  const abs = Math.abs(deltaKb);
  if (abs < 50) return ''; // hide noise
  return ` ${sign}${(abs / 1024).toFixed(1)}MB`;
}

async function dumpSnapshot(
  snap: Snapshot,
  featureBytes: number,
  geometryBytes: number,
  setStatus: (s: string | null) => void,
): Promise<void> {
  const payload = {
    ts: new Date(snap.ts).toISOString(),
    backend_rss_kb: snap.memory?.backend.rss_kb?.toString(),
    backend_vm_peak_kb: snap.memory?.backend.vm_peak_kb?.toString(),
    webkit_processes: snap.memory?.webkit_processes.map((p) => ({
      pid: p.pid,
      name: p.name,
      role: p.role,
      rss_kb: p.rss_kb.toString(),
      vm_size_kb: p.vm_size_kb.toString(),
      vm_peak_kb: p.vm_peak_kb.toString(),
      vm_rss_anon_kb: p.vm_rss_anon_kb.toString(),
    })),
    total_rss_kb: snap.memory?.total_rss_kb?.toString(),
    estimates: {
      maplibre_feature_bytes: featureBytes,
      global_geometry_bytes: geometryBytes,
    },
    feature_counts: snap.featureCounts,
    project: {
      lines: snap.projectLines,
      shots: snap.projectShots,
      stations: snap.projectStations,
    },
    global_view: {
      systems: snap.geometryDataSize,
      total_shots: snap.geometryDataTotalShots,
    },
  };
  const text = JSON.stringify(payload, null, 2);
  console.log('[MemoryProbe snapshot]\n' + text);
  try {
    await navigator.clipboard.writeText(text);
    setStatus('copied!');
  } catch {
    setStatus('clipboard failed');
  }
  // Auto-clear the badge so subsequent dumps re-confirm.
  window.setTimeout(() => setStatus(null), 1800);
}

export default MemoryProbe;
