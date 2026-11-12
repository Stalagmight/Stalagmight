import { Page } from '@playwright/test';
import { readFileSync } from 'node:fs';
import { dirname, resolve as pathResolve } from 'node:path';
import { createRequire } from 'node:module';
import type { TmlFile } from '../../src/types/ipc/TmlFile';
import type { TmlShot } from '../../src/types/ipc/TmlShot';

/** The wire shape of a ts-rs generated type: serde writes i64 as a JSON
 *  number, so fields the generated interfaces declare as `bigint` arrive as
 *  `number` at runtime. Recursive and type-only (erases at compile time) -
 *  its purpose is that a `gen:ipc-types` regen which adds a required field
 *  fails `typecheck:playwright` here instead of drifting silently. */
type TmlWire<T> = T extends bigint
  ? number
  : T extends Array<infer U>
    ? Array<TmlWire<U>>
    : T extends object
      ? { [K in keyof T]: TmlWire<T[K]> }
      : T;

declare global {
  interface Window {
    /** Installed by this mock; the app's IPC bridge. Also used to emit events. */
    __TAURI_INTERNALS__?: { invoke: (cmd: string, args?: unknown) => Promise<unknown> };
  }
}

// ----------------------------------------------------------------------------
// msgpack UMD bundle loader.
//
// The production frontend hits `load_project_full_binary` and expects a
// msgpack-encoded `BinaryEnvelope` (see src/utils/loadProjectBinary.ts).
// The mock runs inside the browser via `addInitScript` and can't import
// from npm, so before the mock attaches we inject the @msgpack/msgpack
// UMD bundle, which exposes `window.MessagePack.encode`.  The mock
// handler for `load_project_full_binary` builds the same fixture as
// `load_project_full`, repacks it into the binary wire format, and
// encodes it - exercising the real decoder end-to-end with no
// production-code fallbacks.
// ----------------------------------------------------------------------------
let cachedMsgpackUmd: string | null = null;
function loadMsgpackUmd(): string {
  if (cachedMsgpackUmd !== null) return cachedMsgpackUmd;
  const req = createRequire(import.meta.url);
  const cjsPath = req.resolve('@msgpack/msgpack');
  // dist.cjs/index.cjs → ../dist.umd/msgpack.min.js
  const pkgRoot = dirname(dirname(cjsPath));
  const umdPath = pathResolve(pkgRoot, 'dist.umd/msgpack.min.js');
  cachedMsgpackUmd = readFileSync(umdPath, 'utf8');
  return cachedMsgpackUmd;
}

/**
 * Mock data types matching the Stalagmight Rust backend models.
 */
interface MockProject {
  id: string;
  name: string;
  sections: MockSection[];
  station_merges: unknown[];
  zero_datums: unknown[];
  section_offsets: Record<string, [number, number]>;
}

interface MockSection {
  id: string;
  name: string;
  surveyor: string;
  date: string;
  color: string;
  shots: MockShot[];
}

interface MockShot {
  id: number;
  azimuth: number;
  length: number;
  depth: number;
  left?: number;
  right?: number;
  up?: number;
  down?: number;
  pitch_in?: number;
  pitch_out?: number;
  comment?: string;
}

/**
 * Create a mock project with optional custom data.
 */
function createMockProject(overrides?: Partial<MockProject>): MockProject {
  return {
    id: 'mock-project-001',
    name: 'Test Project',
    sections: [],
    station_merges: [],
    zero_datums: [],
    section_offsets: {},
    ...overrides,
  };
}

/**
 * Create a mock section with optional custom data.
 */
function createMockSection(overrides?: Partial<MockSection>): MockSection {
  return {
    id: 'mock-section-001',
    name: 'Test Section',
    surveyor: 'Test Surveyor',
    date: new Date().toISOString().split('T')[0] ?? '',
    color: '#3b82f6',
    shots: [],
    ...overrides,
  };
}

/**
 * Set up Tauri IPC mocks before the page loads.
 * This intercepts all Tauri API calls and returns mock data.
 */
/** 1x1 transparent PNG, enough for MapLibre to treat a tile as loaded. */
const STUB_TILE_PNG = Buffer.from(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==',
  'base64',
);

export async function setupTauriMocks(page: Page): Promise<void> {
  // Serve the custom tile protocol. In the real app Tauri registers
  // `stalagmight-tiles` and answers these; a plain browser has no such handler,
  // so every tile 404s through CORS and floods the console. That was invisible
  // while satellite defaulted off - now that it ships on, the map requests tiles
  // in every spec, and the ones asserting a clean console would fail on noise
  // that says nothing about the app.
  await page.route(/stalagmight-tiles(\.localhost)?/, (route) =>
    route.fulfill({
      status: 200,
      contentType: 'image/png',
      headers: { 'Access-Control-Allow-Origin': '*', 'Cache-Control': 'max-age=86400' },
      body: STUB_TILE_PNG,
    }),
  );

  // Note on external hosts (#417): the frontend never fetches the live esri
  // host in this env - tileConfig always emits the stalagmight-tiles protocol
  // stubbed above (the mock's arcgisonline url_template has no fetching
  // consumer). The #417 local-red/CI-green console noise was the dev-only
  // agent bridge's /__agent/token probe, now gated off under VITE_E2E in
  // main.tsx. The maplibre CSS is bundled from the installed package (#606),
  // so the page makes no external fetch at all.

  // Inject the msgpack UMD bundle first so the load_project_full_binary
  // mock can encode its result the same way the real backend does.
  // Exposes `window.MessagePack` synchronously before any app JS runs.
  await page.addInitScript({ content: loadMsgpackUmd() });

  await page.addInitScript(() => {
    // Event listener registry for mock Tauri events (plugin:event|listen/emit/unlisten)
    // This is defined early so it can be used by both mockHandler and __tauriEventMock
    interface EventListener {
      handler: number; // The callback ID (transformCallback ID)
      event: string;
    }
    const eventListeners = new Map<number, EventListener>();
    let eventListenerId = 0;
    // Callback registry: transformCallback stores callbacks as window[`_${id}`]
    const callbackRegistry = new Map<string, (arg: unknown) => void>();

    // Helper to emit events to all listeners
    const emitEvent = (eventName: string, payload: unknown) => {
      eventListeners.forEach((listener) => {
        if (listener.event === eventName) {
          const callbackId = listener.handler;
          // Callbacks are stored as window[`_${id}`] by transformCallback
          const callbackKey = `_${callbackId}`;
          const callback = (window as unknown as Record<string, unknown>)[callbackKey] as
            | ((arg: unknown) => void)
            | undefined;
          if (callback) {
            callback({ payload });
          } else {
            // Also check our local registry
            const localCallback = callbackRegistry.get(String(callbackId));
            if (localCallback) {
              localCallback({ payload });
            }
          }
        }
      });
    };

    // Store the mock command handler
    const mockHandler = (() => {
      let currentProject: {
        id: number;
        name: string;
        location: string | null;
        created_at: string;
        modified_at: string;
        file_path: string | null;
        sections: unknown[];
        station_merges: unknown[];
        zero_datums: unknown[];
        loop_closures: unknown[];
        section_offsets: Record<string, unknown>;
      } | null = null;
      let projectPath: string | null = null;
      // `BinaryEnvelope::is_remote` (#270): whether the loaded `.stalx` lives in
      // the SpeleoDB cache. The frontend resolves the read-only gate from this
      // SYNCHRONOUSLY at load, so leaving it off the envelope pinned every
      // Playwright load to `projectLinkKind: 'local'`. Local by default (what
      // almost every spec means); flip per-spec with __MOCK_SET_PROJECT_IS_REMOTE__.
      let projectIsRemote = false;
      // Window teardown calls, so an exit-guard spec can assert the POSITIVE
      // outcome of an unguarded close instead of only "no dialog appeared".
      let windowDestroyCount = 0;
      let windowCloseCount = 0;
      let sectionCounter = 0;
      // Shot ids are section-encoded, exactly as the backend allocates them:
      // `Section::new` seeds `next_shot_id = section.id * FEATURE_ID_STRIDE` and
      // increments from there (src-tauri/src/models/section.rs). Keeping the mock
      // on flat 1,2,3 ids made `Math.floor(fid / STRIDE)` accidentally correct and
      // hid a whole class of id-decoding bugs from Playwright (#301).
      const MOCK_FEATURE_ID_STRIDE = 100000;
      const mockShotId = (sectionId: number, seq: number): number =>
        sectionId * MOCK_FEATURE_ID_STRIDE + seq;

      // Per-shot survey credit, the shape the backend serves since surveyors,
      // explorers and the date moved onto every shot (a section keeps none).
      // Mock sections still carry the legacy section-level `surveyor`/`date`
      // (every fixture builds them that way); exactly like the backend's
      // migration, that is the credit of each shot that has none of its own.
      // `withShotCredit` applies it to every project this mock hands out, so the
      // app only ever sees the per-shot shape (`Shot.credit`, absent when empty).
      type MockPerson = { name: string; email?: string };
      type MockCredit = { surveyors: MockPerson[]; explorers: MockPerson[]; date: string | null };
      const mockPersonKey = (p: MockPerson): string =>
        `${p.name.trim().split(/\s+/).join(' ').toLowerCase()}\u0000${p.email?.trim().toLowerCase() ?? ''}`;
      const mockDedupe = (people: MockPerson[]): MockPerson[] => {
        const seen = new Set<string>();
        return people.filter((p) => {
          if (!p.name.trim()) return false;
          const key = mockPersonKey(p);
          if (seen.has(key)) return false;
          seen.add(key);
          return true;
        });
      };
      /** The backend's `credit::split_people`, for the mock's legacy text. */
      const mockSplitPeople = (half: string): MockPerson[] => {
        const text = half.trim();
        const parts = text.split(',').map((p) => p.trim());
        const lastFirst = !text.includes(';') && parts.length === 2
          && parts.every((p) => p !== '' && !/\s/.test(p));
        const pieces = lastFirst ? [text] : text.split(';').flatMap((piece) => piece.split(','));
        return mockDedupe(
          pieces
            .map((piece) => piece.trim())
            .filter((piece) => piece !== '' && !['unknown', 'none', '?'].includes(piece.toLowerCase()))
            .map((name) => ({ name })),
        );
      };
      /** `(explorers, surveyors)` of legacy text, as `credit::legacy_people`. */
      const mockLegacyPeople = (text: string): { explorers: MockPerson[]; surveyors: MockPerson[]; halves: boolean } => {
        const halves = text.split(' / ');
        return halves.length === 2
          ? { explorers: mockSplitPeople(halves[0] ?? ''), surveyors: mockSplitPeople(halves[1] ?? ''), halves: true }
          : { explorers: [], surveyors: mockSplitPeople(text), halves: false };
      };
      const mockLegacyCredit = (surveyor: unknown, date: unknown): MockCredit | null => {
        const people = typeof surveyor === 'string' ? mockLegacyPeople(surveyor) : { explorers: [], surveyors: [] };
        const day = typeof date === 'string' && date !== '' ? date : null;
        if (people.surveyors.length === 0 && people.explorers.length === 0 && day === null) return null;
        return { surveyors: people.surveyors, explorers: people.explorers, date: day };
      };
      const mockShotCredit = (
        section: Record<string, unknown>,
        shot: Record<string, unknown>,
      ): MockCredit => (shot.credit as MockCredit | undefined)
        ?? mockLegacyCredit(section.surveyor, section.date)
        ?? { surveyors: [], explorers: [], date: null };
      const mockStoreCredit = (shot: Record<string, unknown>, credit: MockCredit): void => {
        // An explicit empty credit stays EXPLICIT (it must not fall back to the
        // legacy section template); `withShotCredit` leaves it off the wire, as
        // the backend's `Shot::set_credit` does.
        if (credit.surveyors.length === 0 && credit.explorers.length === 0 && credit.date === null) {
          shot.credit = { surveyors: [], explorers: [], date: null };
        } else {
          shot.credit = credit;
        }
      };
      const withShotCredit = <P extends { sections: unknown[] } | null>(project: P): P => {
        if (!project) return project;
        const sections = (project.sections as Array<Record<string, unknown>>).map((section) => {
          const { surveyor, date, ...rest } = section;
          const template = mockLegacyCredit(surveyor, date);
          const shots = ((rest.shots as Array<Record<string, unknown>> | undefined) ?? []).map((shot) => {
            const own = shot.credit as MockCredit | undefined;
            const credit = own ?? template;
            const { credit: _dropped, ...bare } = shot;
            void _dropped;
            const empty = !credit
              || (credit.surveyors.length === 0 && credit.explorers.length === 0 && credit.date === null);
            return empty ? bare : { ...bare, credit: structuredClone(credit) };
          });
          return { ...rest, shots };
        });
        return { ...project, sections };
      };

      // #310: live plan pins are kept as opaque ANCHORS beside their positional refs, so a
      // survey edit that renumbers stations doesn't silently retarget them. The real backend
      // names a station by the shots on either side of it (core/state/dive_plan.rs's
      // `PlanAnchor`); this mock mirrors that NEIGHBOUR-PAIR semantics only - no reverse-parity
      // or terminal-shot-length tracking, since the mock only needs to survive the shot
      // add/delete an e2e can actually drive. The anchor shape below is this mock's own and is
      // opaque to the app (`PlanAnchor` is `unknown` on the TS side).
      interface MockPlanRefLite {
        section_id: number;
        station_index: number;
      }
      interface MockPlanAnchor {
        section_id: number;
        before_shot_uuid: string | null;
        after_shot_uuid: string | null;
      }
      interface MockShotRef {
        id: number;
        uuid?: string;
      }
      interface MockSectionRef {
        id: number;
        shots: MockShotRef[];
      }
      const sectionForAnchor = (sectionId: number): MockSectionRef | undefined =>
        (currentProject?.sections as MockSectionRef[] | undefined)?.find(
          (s) => s.id === sectionId,
        );
      /** Describe the station at `ref` by the uuids of its neighbouring shots (station `i` sits
       *  between shot `i-1` and shot `i`; a section end has no neighbour on that side). `null`
       *  when the section is gone or the index is out of range - mirrors the backend's
       *  `to_anchor`. */
      const refToAnchor = (ref: MockPlanRefLite | null | undefined): MockPlanAnchor | null => {
        if (!ref) return null;
        const section = sectionForAnchor(ref.section_id);
        if (!section || ref.station_index < 0 || ref.station_index > section.shots.length) {
          return null;
        }
        const before = ref.station_index > 0 ? section.shots[ref.station_index - 1] : undefined;
        const after =
          ref.station_index < section.shots.length ? section.shots[ref.station_index] : undefined;
        return {
          section_id: ref.section_id,
          before_shot_uuid: before?.uuid ?? null,
          after_shot_uuid: after?.uuid ?? null,
        };
      };
      /** Place an anchor against the project as it stands now, following the backend's
       *  `resolve_anchor` rules: with BOTH neighbours found the station is the boundary between
       *  them regardless of which now comes first (`pa < pb ? pa+1 : pa` - the order-agnosticism
       *  is what makes a reverse a no-op for interior stations); with one side that was NEVER
       *  recorded (a section END at capture), the station follows its sole shot to whichever end
       *  it currently touches (a reverse carries station 0 to station N and back); with one side
       *  recorded but deleted, the surviving side keeps its directional meaning. `null` only when
       *  NEITHER uuid is found anymore. */
      const anchorToRef = (anchor: MockPlanAnchor | null | undefined): MockPlanRefLite | null => {
        if (!anchor) return null;
        const section = sectionForAnchor(anchor.section_id);
        if (!section) return null;
        const find = (u: string | null) =>
          u ? section.shots.findIndex((s) => s.uuid === u) : -1;
        const pa = find(anchor.before_shot_uuid);
        const pb = find(anchor.after_shot_uuid);
        const n = section.shots.length;
        if (pa >= 0 && pb >= 0) {
          return { section_id: anchor.section_id, station_index: pa < pb ? pa + 1 : pa };
        }
        if (pa >= 0) {
          if (anchor.after_shot_uuid === null) {
            // Tail-end station at capture: stay on the end its shot touches now.
            const idx = pa === 0 ? 0 : n;
            return { section_id: anchor.section_id, station_index: idx };
          }
          return { section_id: anchor.section_id, station_index: pa + 1 };
        }
        if (pb >= 0) {
          if (anchor.before_shot_uuid === null) {
            // Head-end station at capture: stay on the end its shot touches now.
            const idx = pb === n - 1 ? n : 0;
            return { section_id: anchor.section_id, station_index: idx };
          }
          return { section_id: anchor.section_id, station_index: pb };
        }
        return null;
      };

      // `section_offsets` reaches the mock in TWO shapes, so nothing may index it
      // directly. `create_project_full` / `import_sections` / `execute_gpx_import`
      // build the JS `Record<sectionId, [dx, dy]>`, while the binary envelope
      // (`load_project_full_binary`, mirroring `core/load_binary.rs`) and anything
      // injected through `__E2E_MOCKS__.setCurrentProject` carry the backend's
      // `Array<[sectionId, [dx, dy]]>` pairs. `offsets["1"] = [dx, dy]` on the
      // array form sets ELEMENT 1 - silently clobbering a neighbouring section's
      // pair with a bare coordinate - so every read/write goes through these.
      const sectionOffsetPairs = (offsets: unknown): Array<[number, [number, number]]> => {
        if (Array.isArray(offsets)) {
          return (offsets as Array<[number, [number, number]]>).map(
            ([k, v]) => [Number(k), v] as [number, [number, number]],
          );
        }
        return Object.entries((offsets ?? {}) as Record<string, [number, number]>).map(
          ([k, v]) => [Number(k), v] as [number, [number, number]],
        );
      };
      const readSectionOffset = (offsets: unknown, sectionId: number): [number, number] => {
        if (Array.isArray(offsets)) {
          const entry = (offsets as Array<[number, [number, number]]>).find(
            ([k]) => Number(k) === sectionId,
          );
          return entry?.[1] ?? [0, 0];
        }
        return (
          (offsets as Record<string, [number, number] | undefined> | null | undefined)?.[
            String(sectionId)
          ] ?? [0, 0]
        );
      };
      const writeSectionOffset = (
        offsets: unknown,
        sectionId: number,
        value: [number, number],
      ): void => {
        if (Array.isArray(offsets)) {
          const pairs = offsets as Array<[number, [number, number]]>;
          const at = pairs.findIndex(([k]) => Number(k) === sectionId);
          if (at >= 0) pairs[at] = [sectionId, value];
          else pairs.push([sectionId, value]);
          return;
        }
        if (offsets && typeof offsets === 'object') {
          (offsets as Record<string, [number, number]>)[String(sectionId)] = value;
        }
      };

      // Cache mock state - tracks cache operations for verification
      // Use explicit types so providers can be empty and keys can be deleted
      interface MockProviderStats {
        tiles: bigint;
        bytes: bigint;
      }
      interface MockCacheStats {
        total_tiles: bigint;
        total_bytes: bigint;
        providers: Record<string, MockProviderStats>;
      }
      const defaultCacheStats: MockCacheStats = {
        total_tiles: 150n,
        total_bytes: 25600000n, // ~25MB
        providers: {
          esri: { tiles: 150n, bytes: 25600000n },
        },
      };
      let mockCacheStats: MockCacheStats = {
        total_tiles: defaultCacheStats.total_tiles,
        total_bytes: defaultCacheStats.total_bytes,
        providers: { ...defaultCacheStats.providers },
      };
      let mockNetworkStatus = true; // Online by default

      // Config override for testing config reload effects
      interface MockConfigOverrides {
        debug?: { map?: { show_hitboxes?: boolean; log_hit_detection?: boolean } };
        map?: {
          station?: { hit_area?: number };
          shot?: { hit_area?: number };
          animation?: { enabled?: boolean; duration?: number; point_zoom?: number };
        };
      }
      let mockConfigOverrides: MockConfigOverrides = {};

      // Expose reset function for tests via window
      (window as unknown as Record<string, unknown>).__MOCK_CACHE_RESET__ = () => {
        mockCacheStats = {
          total_tiles: defaultCacheStats.total_tiles,
          total_bytes: defaultCacheStats.total_bytes,
          providers: { ...defaultCacheStats.providers },
        };
        mockNetworkStatus = true;
      };
      (window as unknown as Record<string, unknown>).__MOCK_SET_NETWORK_STATUS__ = (status: boolean) => {
        mockNetworkStatus = status;
      };
      (window as unknown as Record<string, unknown>).__MOCK_GET_CACHE_STATS__ = () => mockCacheStats;

      // Geometry loading mock control
      // Paths listed here will fail geometry loading (throw error instead of returning data)
      const geometryErrorPaths = new Set<string>();
      // Paths listed here will delay geometry loading (resolve after delay ms)
      const geometryDelayPaths = new Map<string, number>();

      (window as unknown as Record<string, unknown>).__MOCK_SET_GEOMETRY_ERROR__ = (path: string) => {
        geometryErrorPaths.add(path);
      };
      (window as unknown as Record<string, unknown>).__MOCK_CLEAR_GEOMETRY_ERROR__ = (path: string) => {
        geometryErrorPaths.delete(path);
      };
      (window as unknown as Record<string, unknown>).__MOCK_SET_GEOMETRY_DELAY__ = (path: string, delayMs: number) => {
        geometryDelayPaths.set(path, delayMs);
      };
      (window as unknown as Record<string, unknown>).__MOCK_CLEAR_GEOMETRY_DELAY__ = (path: string) => {
        geometryDelayPaths.delete(path);
      };

      // Duplicate import mock state - configurable for hash-tracking tests
      let mockCheckDuplicateResult = false; // Default: no duplicate
      let trackImportedFileCalls: Array<{ hash: string }> = [];
      let mergeStationCalls: Array<{
        stationA: { section_id: number; station_index: number };
        stationB: { section_id: number; station_index: number };
      }> = [];
      let unmergeStationPairCalls: Array<{
        stationA: { section_id: number; station_index: number };
        stationB: { section_id: number; station_index: number };
      }> = [];
      let mockOrphanPlacementSuggestions: unknown[] = [];
      let mockImportPlacementSuggestions: unknown[] = [];
      // Records execute_walls_import args (esp. selectedSurveys) so the Walls
      // import dialog spec can assert the select-all→null / explicit-index path.
      let wallsImportCalls: Array<Record<string, unknown>> = [];
      // Records execute_tml_import args (includeExcluded, virtualShotMode, …) so
      // the Ariane import spec can assert what the dialog's toggles actually sent.
      let tmlImportCalls: Array<Record<string, unknown>> = [];
      // Dive-plan (.plnx) mock store (#244): an in-memory list of saved plans for the
      // open project. Reset per test via __MOCK_RESET_DIVE_PLANS__.
      let mockDivePlans: Array<{
        id: number;
        name: string;
        created_at: string;
        updated_at: string;
        snapshot: unknown;
      }> = [];
      let mockDivePlanNextId = 1;
      let mockDivePlanClock = 0;
      const nextDivePlanStamp = () => {
        mockDivePlanClock += 1;
        return `2026-07-06T10:00:${String(mockDivePlanClock).padStart(2, '0')}Z`;
      };

      (window as unknown as Record<string, unknown>).__MOCK_SET_CHECK_DUPLICATE_RESULT__ = (result: boolean) => {
        mockCheckDuplicateResult = result;
      };
      (window as unknown as Record<string, unknown>).__MOCK_GET_WALLS_IMPORT_CALLS__ = () => [...wallsImportCalls];
      (window as unknown as Record<string, unknown>).__MOCK_RESET_WALLS_IMPORT_CALLS__ = () => {
        wallsImportCalls = [];
      };
      (window as unknown as Record<string, unknown>).__MOCK_GET_TML_IMPORT_CALLS__ = () => [...tmlImportCalls];
      (window as unknown as Record<string, unknown>).__MOCK_RESET_TML_IMPORT_CALLS__ = () => {
        tmlImportCalls = [];
      };
      (window as unknown as Record<string, unknown>).__MOCK_RESET_DIVE_PLANS__ = () => {
        mockDivePlans = [];
        mockDivePlanNextId = 1;
        mockDivePlanClock = 0;
      };
      (window as unknown as Record<string, unknown>).__MOCK_GET_DIVE_PLANS__ = () => [...mockDivePlans];
      // Give the open project a file path (a saved project) so dive-plan save/list is
      // enabled - the bar gates on file_path AND databaseOpen (#275 B3). Call
      // refreshProjectState() after to pick up the file_path change; databaseOpen is
      // set here directly (refreshProjectState doesn't touch it) to mirror what the
      // real app does on load (projectStore.ts:727) and on save success (:1002): a
      // project with a path has an open, saved database. Clearing the path (null)
      // mirrors the unsaved/deferred state.
      (window as unknown as Record<string, unknown>).__MOCK_SET_PROJECT_PATH__ = (
        path: string | null,
      ) => {
        projectPath = path;
        if (currentProject) currentProject.file_path = path;
        (
          window as unknown as {
            __E2E_TEST__?: { projectStore?: { setState: (partial: { databaseOpen: boolean }) => void } };
          }
        ).__E2E_TEST__?.projectStore?.setState({ databaseOpen: path !== null });
      };
      // Make the next binary load report the project as a SpeleoDB-cached copy
      // (or back to local), so a spec can exercise the #270 read-only gate from
      // the load path itself rather than by poking the speleo store afterwards.
      (window as unknown as Record<string, unknown>).__MOCK_SET_PROJECT_IS_REMOTE__ = (
        isRemote: boolean,
      ) => {
        projectIsRemote = isRemote;
      };
      (window as unknown as Record<string, unknown>).__MOCK_GET_WINDOW_TEARDOWN_COUNTS__ = () => ({
        destroy: windowDestroyCount,
        close: windowCloseCount,
      });
      (window as unknown as Record<string, unknown>).__MOCK_RESET_WINDOW_TEARDOWN_COUNTS__ = () => {
        windowDestroyCount = 0;
        windowCloseCount = 0;
      };
      (window as unknown as Record<string, unknown>).__MOCK_GET_TRACK_IMPORTED_FILE_CALLS__ = () => [...trackImportedFileCalls];
      (window as unknown as Record<string, unknown>).__MOCK_RESET_TRACK_IMPORTED_FILE_CALLS__ = () => {
        trackImportedFileCalls = [];
      };
      (window as unknown as Record<string, unknown>).__MOCK_GET_MERGE_STATION_CALLS__ = () => [...mergeStationCalls];
      (window as unknown as Record<string, unknown>).__MOCK_RESET_MERGE_STATION_CALLS__ = () => {
        mergeStationCalls = [];
      };
      (window as unknown as Record<string, unknown>).__MOCK_GET_UNMERGE_STATION_PAIR_CALLS__ = () => [...unmergeStationPairCalls];
      (window as unknown as Record<string, unknown>).__MOCK_RESET_UNMERGE_STATION_PAIR_CALLS__ = () => {
        unmergeStationPairCalls = [];
      };
      // Records undo/redo invocations so specs can prove the shortcuts are inert in the
      // read-only workspaces (zero calls in Plan/Export, then one back in Survey).
      let undoRedoCalls: string[] = [];
      (window as unknown as Record<string, unknown>).__MOCK_GET_UNDO_REDO_CALLS__ = () => [...undoRedoCalls];
      (window as unknown as Record<string, unknown>).__MOCK_RESET_UNDO_REDO_CALLS__ = () => {
        undoRedoCalls = [];
      };
      // Survey-credit edits are undoable for real here (the backend's
      // EditShotCredit keeps each touched shot's before-credit): one step per
      // edit_shot_credit, or per execute_batch that carries any. Other commands
      // stay untracked, so `undo` with an empty credit stack is the old no-op.
      type MockCreditStep = Array<{ shot: Record<string, unknown>; before: unknown; after: unknown }>;
      const mockCreditUndo: MockCreditStep[] = [];
      const mockCreditRedo: MockCreditStep[] = [];
      // Set while execute_batch runs, so its credit sub-commands share one step.
      let mockCreditBatchStep: MockCreditStep | null = null;
      const mockRestoreCredit = (shot: Record<string, unknown>, credit: unknown): void => {
        if (credit === undefined) delete shot.credit;
        else shot.credit = structuredClone(credit);
      };
      // Counts native file-picker opens so a spec can prove the read-only-map guard
      // blocks Import (Ctrl+I) in Plan/Export before the picker even opens (#243).
      // Incremented inside the dialog `open` mock below (same init-script closure).
      let dialogOpenCount = 0;
      (window as unknown as Record<string, unknown>).__MOCK_GET_DIALOG_OPEN_COUNT__ = () => dialogOpenCount;
      (window as unknown as Record<string, unknown>).__MOCK_RESET_DIALOG_OPEN_COUNT__ = () => {
        dialogOpenCount = 0;
      };
      (window as unknown as Record<string, unknown>).__MOCK_SET_ORPHAN_SUGGESTIONS__ = (suggestions: unknown[]) => {
        mockOrphanPlacementSuggestions = Array.isArray(suggestions) ? [...suggestions] : [];
      };
      (window as unknown as Record<string, unknown>).__MOCK_RESET_ORPHAN_SUGGESTIONS__ = () => {
        mockOrphanPlacementSuggestions = [];
      };
      (window as unknown as Record<string, unknown>).__MOCK_SET_IMPORT_PLACEMENT_SUGGESTIONS__ = (suggestions: unknown[]) => {
        mockImportPlacementSuggestions = Array.isArray(suggestions) ? [...suggestions] : [];
      };
      (window as unknown as Record<string, unknown>).__MOCK_RESET_IMPORT_PLACEMENT_SUGGESTIONS__ = () => {
        mockImportPlacementSuggestions = [];
      };

      // Expose config override for testing config reload
      (window as unknown as Record<string, unknown>).__MOCK_SET_CONFIG_OVERRIDES__ = (overrides: MockConfigOverrides) => {
        mockConfigOverrides = overrides;
      };
      (window as unknown as Record<string, unknown>).__MOCK_RESET_CONFIG_OVERRIDES__ = () => {
        mockConfigOverrides = {};
      };

      // Store saved config for persistence testing
      let savedConfig: Record<string, unknown> | null = null;
      (window as unknown as Record<string, unknown>).__MOCK_GET_SAVED_CONFIG__ = () => savedConfig;
      (window as unknown as Record<string, unknown>).__MOCK_RESET_SAVED_CONFIG__ = () => {
        savedConfig = null;
      };

      // Data-table view state persisted via update_data_table_view, merged
      // back into get_config's panels: the frontend re-syncs its in-memory
      // config via loadConfig() after each save, so without the merge a
      // saved view change would silently revert on the next remount.
      //
      // Seeded from `window.__E2E_SEED_DATA_TABLE_VIEW__`, set by a test's OWN
      // `page.addInitScript` call registered before `setupMockedPage` (init
      // scripts run in registration order, before any app JS) - this is the
      // only way to have a seeded value present for the app's FIRST
      // `get_config` call at boot, which `update_data_table_view` (an
      // in-page IPC call, necessarily after boot) cannot reach in time.
      const mockDataTableView: Record<string, unknown> = {
        ...(((window as unknown as Record<string, unknown>).__E2E_SEED_DATA_TABLE_VIEW__ as
          | Record<string, unknown>
          | undefined) ?? {}),
      };

      // The new-section dialog's grid layout, persisted via
      // update_new_section_view. Its own store rather than more keys in
      // mockDataTableView, mirroring the Rust side where the two are separate
      // commands writing disjoint `panels` fields - a shared object would let one
      // view's partial write appear to carry the other's keys. Overlaid onto
      // get_config / reload_config alongside it, and seedable the same way via
      // `window.__E2E_SEED_NEW_SECTION_VIEW__`.
      const mockNewSectionView: Record<string, unknown> = {
        ...(((window as unknown as Record<string, unknown>).__E2E_SEED_NEW_SECTION_VIEW__ as
          | Record<string, unknown>
          | undefined) ?? {}),
      };

      // The edit-section dialog's shot grid, persisted via
      // update_edit_section_view. Third store for the third grid, matching the
      // Rust side's three disjoint `panels` field sets.
      const mockEditSectionView: Record<string, unknown> = {
        ...(((window as unknown as Record<string, unknown>).__E2E_SEED_EDIT_SECTION_VIEW__ as
          | Record<string, unknown>
          | undefined) ?? {}),
      };
      // Its OWN seed, deliberately not derived from the edit-section one. The
      // production change gave the import-time grids separate `import_shots_*`
      // fields precisely so reshaping columns mid-import cannot rewrite the
      // edit-section layout; seeding both from one hook would let a regression
      // that reads or writes the wrong slice look identical to correct
      // behaviour, in the mock that exists to prove they stay apart.
      const mockImportShotsView: Record<string, unknown> = {
        ...(((window as unknown as Record<string, unknown>).__E2E_SEED_IMPORT_SHOTS_VIEW__ as
          | Record<string, unknown>
          | undefined) ?? {}),
      };

      // `map.controls.show_navigation_tools` (#409): the Pan/Zoom toolbar group is
      // hidden unless the user opts in, so the mock's config reports false and a
      // spec that needs the tools seeds `window.__E2E_SEED_SHOW_NAVIGATION_TOOLS__`
      // via its OWN `page.addInitScript` before `setupMockedPage` (see
      // `seedNavigationTools` in helpers/app.ts) - same reason as the view seeds
      // above: the toolbar reads the value from the FIRST `get_config` at boot.
      const mockShowNavigationTools =
        (window as unknown as Record<string, unknown>).__E2E_SEED_SHOW_NAVIGATION_TOOLS__ === true;

      // The `[gas]` block persisted via update_gas_config, overlaid onto
      // get_config the same way. Null until something writes it, so an untouched
      // run keeps whatever gas config get_config would otherwise return.
      let mockGasConfig: Record<string, unknown> | null = null;

      // The `[map_palette]` block persisted via update_map_palette (the depth /
      // distance colour palettes and pins), overlaid onto get_config / reload_config
      // exactly like mockGasConfig.
      let mockMapPalette: Record<string, unknown> | null = null;

      // `[people].recent` persisted via remember_people: the people last added to
      // a shot's credit, most recent first, deduped by identity, capped at 200
      // (Rust `PeopleConfig::remember`). Overlaid onto get_config / reload_config
      // like mockMapPalette. Specs may seed it with window.__MOCK_RECENT_PEOPLE__
      // (read until the first remember_people write, so an init script that runs
      // after this one still seeds it).
      const MOCK_MAX_RECENT_PEOPLE = 200;
      let mockRecentPeopleWritten: Array<{ name: string; email?: string }> | null = null;
      const mockRecentPeople = (): Array<{ name: string; email?: string }> => {
        if (mockRecentPeopleWritten) return [...mockRecentPeopleWritten];
        const seeded = (window as unknown as Record<string, unknown>).__MOCK_RECENT_PEOPLE__;
        return Array.isArray(seeded)
          ? (seeded as Array<{ name: string; email?: string }>).slice(0, MOCK_MAX_RECENT_PEOPLE)
          : [];
      };

      // Station comments set via set_station_comment, keyed
      // "sectionId:stationIndex". get_project_state derives its station array
      // from the project's sections on every refresh, so comments must be
      // threaded back in from here or an edit would vanish on the next refresh.
      const mockStationComments: Record<string, string> = {};

      // Station names (the mNemo numbers a DMP import sends as `station_names`),
      // keyed "sectionId:stationIndex" and threaded into get_project_state's
      // derived stations like the comments above. Filled by import_sections;
      // tests can also seed one through __MOCK_SET_STATION_NAME__. The mock
      // never reverses an imported section, so the indexes are used as sent.
      const mockStationNames: Record<string, string> = {};
      (window as unknown as Record<string, unknown>).__MOCK_SET_STATION_NAME__ = (
        sectionId: number,
        stationIndex: number,
        name: string | null,
      ) => {
        const key = `${sectionId}:${stationIndex}`;
        if (name === null) delete mockStationNames[key];
        else mockStationNames[key] = name;
      };

      // Track last export_image call for test assertions
      let lastExportImageArgs: Record<string, unknown> | null = null;

      // Per-path project-name overrides applied by set_project_name_for_path,
      // so list_recent_projects reflects renames within a test.
      const recentNameOverrides: Record<string, string> = {};

      // Structural-integrity issues returned by check_project_integrity. Tests
      // seed these via __MOCK_SET_INTEGRITY_ISSUES__; repair fixes all but the
      // trailing `mockIntegrityRemaining` detect-only issues.
      let mockIntegrityIssues: string[] = [];
      let mockIntegrityRemaining = 0;
      // Whether the seeded issues block saving (unresolvable cross-references).
      // Default false so the banner stays dismissable in existing tests.
      let mockIntegritySaveBlocked = false;
      (window as unknown as Record<string, unknown>).__MOCK_SET_INTEGRITY_ISSUES__ = (
        issues: string[]
      ) => {
        mockIntegrityIssues = Array.isArray(issues) ? issues : [];
      };
      (window as unknown as Record<string, unknown>).__MOCK_SET_INTEGRITY_SAVE_BLOCKED__ = (
        blocked: boolean
      ) => {
        mockIntegritySaveBlocked = !!blocked;
      };
      // Seed how many of the issues are detect-only (survive repair), so tests
      // can exercise the "N need manual attention" path.
      (window as unknown as Record<string, unknown>).__MOCK_SET_INTEGRITY_REMAINING__ = (
        n: number
      ) => {
        mockIntegrityRemaining = Number.isFinite(n) ? Math.max(0, Math.floor(n)) : 0;
      };

      // [#473] Whether the loaded project's undo/redo history was unreadable and
      // discarded. Default false - every existing spec opens a project whose
      // history loads fine, and a banner appearing over them would be a false
      // failure. Seeded by __MOCK_SET_HISTORY_DISCARDED__.
      let mockHistoryDiscarded = false;
      // How many times the app has ASKED. A spec asserting the banner is absent needs
      // this: `toHaveCount(0)` passes on its first poll, so without a positive signal
      // that the post-load effect has actually run, the assertion is satisfied by the
      // effect not having happened yet and can never fail.
      let mockHistoryDiscardedCalls = 0;
      (window as unknown as Record<string, unknown>).__MOCK_SET_HISTORY_DISCARDED__ = (
        discarded: boolean
      ) => {
        mockHistoryDiscarded = !!discarded;
        mockHistoryDiscardedCalls = 0;
      };
      (window as unknown as Record<string, unknown>).__MOCK_HISTORY_DISCARDED_CALLS__ = () =>
        mockHistoryDiscardedCalls;

      // Expose currentProject setter for E2E tests (allows injecting test fixtures)
      (window as unknown as Record<string, unknown>).__E2E_MOCKS__ = {
        setCurrentProject: (project: typeof currentProject) => {
          currentProject = project;
        },
        getCurrentProject: () => currentProject,
        getLastExportImageArgs: () => lastExportImageArgs,
        clearLastExportImageArgs: () => { lastExportImageArgs = null; },
      };

      // Display settings persistence - in-memory storage for testing reload behavior
      const mockDisplaySettings: Record<string, string> = {
        showStations: 'true',
        showShots: 'true',
        showSectionNames: 'true',
        showDepthGradient: 'false',
      };

      interface MockGpxPoint {
        latitude: number;
        longitude: number;
        elevation: number | null;
        time: string | null;
      }

      const buildMockGpxFile = (path: string | null | undefined) => {
        const normalized = (path ?? '').toLowerCase();
        const now = new Date();
        const iso = now.toISOString();

        const waypointOnlyFile = {
          name: 'Waypoint Check Dive',
          waypoints: [
            { name: 'Entry Bell', comment: 'Main entry', latitude: 24.55, longitude: -81.78, elevation: 1.2, time: iso },
            { name: 'Jump Reel', comment: null, latitude: 24.5504, longitude: -81.7794, elevation: 3.4, time: iso },
          ],
          tracks: [],
          routes: [],
        };

        if (normalized.includes('waypoints-only')) {
          return waypointOnlyFile;
        }

        return {
          name: 'Reef Traverse',
          waypoints: waypointOnlyFile.waypoints,
          tracks: [
            {
              name: 'Main Line',
              comment: 'Primary track',
              segments: [
                {
                  points: [
                    { latitude: 24.55, longitude: -81.78, elevation: 2.0, time: iso },
                    { latitude: 24.5502, longitude: -81.7798, elevation: 2.6, time: iso },
                    { latitude: 24.55045, longitude: -81.7795, elevation: 3.1, time: iso },
                  ],
                },
              ],
            },
          ],
          routes: [
            {
              name: 'Exit Route',
              comment: null,
              points: [
                { name: 'Route Start', latitude: 24.55045, longitude: -81.7795, elevation: 3.1, time: iso },
                { name: 'Route Mid', latitude: 24.55065, longitude: -81.7792, elevation: 2.8, time: iso },
                { name: 'Route End', latitude: 24.5508, longitude: -81.7789, elevation: 2.5, time: iso },
              ],
            },
          ],
        };
      };

      // Synthetic Ariane TML preview (#411). Fully in-memory - no .tml fixture
      // is read - carrying one of each shot flavour the import dialog's optional
      // controls key off: an excluded shot, a commented shot and a virtual shot.
      const buildMockTmlShot = (
        id: number,
        fromId: number,
        section: string,
        depth: number,
        overrides: Partial<TmlWire<TmlShot>> = {}
      ): TmlWire<TmlShot> => ({
        id,
        from_id: fromId,
        shot_type: 'Real',
        azimuth: (id * 30) % 360,
        length: 10,
        depth,
        depth_in: depth - 1,
        inclination: 0,
        left: 1,
        right: 1,
        up: 1,
        down: 1,
        comment: null as string | null,
        explorer: 'Synthetic Surveyor',
        date: '2026-01-02',
        color: '#3b82f6',
        excluded: false,
        latitude: 0,
        longitude: 0,
        closure_to_id: 0,
        section,
        ...overrides,
      });

      // Preview closures derived the way the real parser's extract_closures
      // does (parsers/tml.rs): every Closure-type shot with closure_to_id >= 0
      // contributes one entry; "from" is the closure shot's own RAW position,
      // "to" is the target shot's RAW position (indices over the unfiltered
      // shot lists, including Closure/excluded rows). The real START-target
      // fallback is not modeled - this fixture has no START shots.
      const deriveTmlClosures = (
        sections: TmlWire<TmlFile>['sections']
      ): TmlWire<TmlFile>['closures'] => {
        const idToRawLoc = new Map<number, [number, number]>();
        sections.forEach((section, si) =>
          section.shots.forEach((shot, idx) => idToRawLoc.set(shot.id, [si, idx]))
        );
        const closures: TmlWire<TmlFile>['closures'] = [];
        sections.forEach((section, si) =>
          section.shots.forEach((shot, idx) => {
            if (shot.shot_type !== 'Closure' || shot.closure_to_id < 0) return;
            const to = idToRawLoc.get(shot.closure_to_id);
            if (to) {
              closures.push({
                from_section_index: si,
                from_station_index: idx,
                to_section_index: to[0],
                to_station_index: to[1],
              });
            }
          })
        );
        return closures;
      };

      // A path containing "imperial" yields an Imperial-unit file, so a spec can
      // drive the "converted from feet" branch of the dialog and the receipt.
      // The real parser converts ft->m BEFORE returning the preview (see
      // apply_unit_conversion in parsers/tml.rs: shot length/depth/depth_in and
      // LRUD scale by 0.3048; start_depth and first_start_absolute_elevation
      // deliberately mirror master and stay unscaled), so the imperial fixture's
      // canonical numbers are FEET and the parse output below is metric.
      const buildMockTmlFile = (path: string | null | undefined): TmlWire<TmlFile> => {
        const imperial = (path ?? '').toLowerCase().includes('imperial');
        const scale = imperial ? 0.3048 : 1;
        const scaleShot = (shot: TmlWire<TmlShot>): TmlWire<TmlShot> => ({
          ...shot,
          length: shot.length * scale,
          depth: shot.depth * scale,
          depth_in: shot.depth_in * scale,
          left: shot.left * scale,
          right: shot.right * scale,
          up: shot.up * scale,
          down: shot.down * scale,
        });
        // `.../crowded...` gives every shot a full credit - five surveyors, two
        // explorers, dates spanning a range - for the preview table's layout.
        const crowded = (path ?? '').toLowerCase().includes('crowded');
        const crowd = (date: string): Partial<TmlWire<TmlShot>> =>
          crowded
            ? {
                credit: {
                  surveyors: ['Ann Diver', 'Bo Lead', 'Cy Cave', 'Dee Deep', 'Eve Echo'].map((name) => ({ name })),
                  explorers: ['Fay Fathom', 'Gus Grotto'].map((name) => ({ name })),
                  date,
                },
              }
            : {};
        const sections: TmlWire<TmlFile>['sections'] = [
          {
            name: 'Main Line',
            shots: [
              buildMockTmlShot(1, 0, 'Main Line', 2, crowd('2026-03-02')),
              buildMockTmlShot(2, 1, 'Main Line', 4, { comment: 'Restriction at the T', ...crowd('2026-04-11') }),
              buildMockTmlShot(3, 2, 'Main Line', 6, { excluded: true, ...crowd('2026-04-11') }),
            ].map(scaleShot),
            surveyor: 'Synthetic Surveyor',
            date: '2026-01-02',
            color: '#3b82f6',
            is_branch_split: false,
            start_ariane_id: 0,
            end_ariane_id: 3,
            start_depth: 0,
          },
          {
            name: 'Side Passage',
            shots: [
              buildMockTmlShot(4, 1, 'Side Passage', 3, crowd('2026-03-02')),
              buildMockTmlShot(5, 4, 'Side Passage', 5, { shot_type: 'Virtual', ...crowd('2026-04-11') }),
              // The loop closure, as the real format stores it: a Closure-type
              // ROW near the end of the file. from_id=1 (Main Line station 1)
              // and closure_to_id=4 (Side Passage station 1) keep the previous
              // round's by-construction exclusion-safety: neither endpoint is
              // the excluded shot (id 3, Main Line's terminal) nor preceded by
              // it, so import-space indices agree under BOTH states of the
              // excluded-shots toggle.
              buildMockTmlShot(6, 1, 'Side Passage', 0, {
                shot_type: 'Closure',
                closure_to_id: 4,
                length: 0,
              }),
            ].map(scaleShot),
            surveyor: 'Synthetic Surveyor',
            date: '2026-01-03',
            color: '#22c55e',
            is_branch_split: true,
            start_ariane_id: 1,
            end_ariane_id: 5,
            start_depth: 2,
          },
        ];
        return {
          cave_name: 'Synthetic Cavern',
          unit: imperial ? 'Imperial' : 'Metric',
          use_magnetic_azimuth: true,
          first_start_absolute_elevation: 0,
          geo_coding: null,
          sections,
          closures: deriveTmlClosures(sections),
        };
      };

      const toShot = (
        pointA: MockGpxPoint,
        pointB: MockGpxPoint,
        shotId: number,
        depthInverted: boolean
      ) => {
        const dLat = pointB.latitude - pointA.latitude;
        const dLon = pointB.longitude - pointA.longitude;
        const horizontalMeters = Math.max(0.1, Math.hypot(dLat, dLon) * 111_000);
        const azimuth = ((Math.atan2(dLon, dLat) * 180) / Math.PI + 360) % 360;
        const toDepth = (value: number | null): number => {
          if (value == null || !Number.isFinite(value)) return 0;
          return depthInverted ? -Math.abs(value) : -value;
        };
        return {
          id: shotId,
          azimuth_in: azimuth,
          azimuth_out: azimuth,
          length: Number(horizontalMeters.toFixed(2)),
          depth_in: toDepth(pointA.elevation),
          depth_out: toDepth(pointB.elevation),
          pitch_in: null,
          pitch_out: null,
          left: null,
          right: null,
          up: null,
          down: null,
          comment: null,
          kind: 'Normal',
          marker_index: null,
          temperature: null,
        };
      };

      const EXIT_RECORDED_COMMANDS = new Set([
        'close_project', 'save_project', 'speleodb_done', 'speleodb_release_lock',
        // A project switch / quit settles the open project's lock first.
        'speleodb_settle_open_lock',
        'speleodb_discard_cached_copy', 'speleodb_logout', 'speleodb_login',
        'install_app_update_and_restart',
        // Home's remove-copy prompt and Settings' purge: specs assert the consent
        // flag each one carried.
        'speleodb_remove_cached_copy', 'speleodb_purge_cached_copies', 'speleodb_open_project',
      ]);

      return function mockCommand(
        cmd: string,
        args?: Record<string, unknown>
      ): unknown {
        // Skip logging for push_frontend_log to avoid infinite loop with console capture
        if (cmd !== 'push_frontend_log') {
          console.log(`[Mock IPC] Command: ${cmd}`, args);
        }

        // The exit-prompt / sign-out specs assert WHICH of these ran, with what, and in
        // what order - recorded before the override map so an overridden reply (an
        // upload that "fails", a release that throws) is still on the record.
        if (EXIT_RECORDED_COMMANDS.has(cmd)) {
          const win = window as unknown as Record<string, unknown>;
          const log = (win.__MOCK_EXIT_CALLS__ as Array<{ cmd: string; args: unknown }> | undefined) ?? [];
          log.push({ cmd, args: args ?? null });
          win.__MOCK_EXIT_CALLS__ = log;
        }

        const overrideMap = (window as unknown as Record<string, unknown>).__MOCK_IPC_OVERRIDES__ as
          | Record<string, unknown>
          | undefined;
        if (overrideMap && Object.prototype.hasOwnProperty.call(overrideMap, cmd)) {
          const override = overrideMap[cmd];
          if (override && typeof override === 'object' && '__throw' in override) {
            throw new Error(String((override as { __throw?: unknown }).__throw ?? 'Mock error'));
          }
          return override as unknown;
        }

        switch (cmd) {
          case 'update_project_settings': {
            // Per-project settings (#57): name + validation thresholds.
            console.log('[Mock IPC] update_project_settings');
            if (currentProject) {
              if (typeof args?.name === 'string') currentProject.name = args.name;
              if (args?.validation) {
                (currentProject as Record<string, unknown>).validation = args.validation;
              }
            }
            return { success: true, section_ids: [], project: currentProject };
          }

          case 'set_surface_elevation': {
            // Entrance elevation (#465): set or (given null/undefined) clear.
            // Real command returns a plain CommandResult (no `project` payload) -
            // the frontend re-fetches via get_project_state's own round trip.
            if (!currentProject) {
              return { success: false, error: 'No project open' };
            }
            // Mirrors the backend's guard_mutation: a remote project without the
            // write lock rejects with the EDIT_DISABLED coded failure. The mock
            // has no held-lock state, so `projectIsRemote` (set via
            // __MOCK_SET_PROJECT_IS_REMOTE__) is the closest read-only proxy -
            // exercised by the elevation read-only-gate spec.
            if (projectIsRemote) {
              return {
                success: false,
                error: 'EDIT_DISABLED: Set entrance elevation is disabled: this SpeleoDB project is open read-only. Acquire the lock to edit.',
              };
            }
            const elevation = args?.elevation;
            if (elevation !== null && elevation !== undefined && typeof elevation !== 'number') {
              return { success: false, error: 'Elevation must be a finite number' };
            }
            if (typeof elevation === 'number' && !Number.isFinite(elevation)) {
              return { success: false, error: 'Elevation must be a finite number' };
            }
            (currentProject as Record<string, unknown>).surface_elevation =
              typeof elevation === 'number' ? elevation : null;
            return { success: true, section_ids: [] };
          }

          case 'create_project_full': {
            // Consolidated create_project_full command for loading progress feature
            const name = (args?.name as string) ?? 'Untitled Project';
            const projectLocation = (args?.projectLocation as string | null | undefined) ?? null;
            const savePathArg = (args?.savePath as string | undefined) ?? undefined;
            const saveDirectoryArg = (args?.saveDirectory as string | undefined) ?? undefined;
            const now = new Date().toISOString();
            sectionCounter = 0;
            const sanitizedName = name.replace(/[^a-zA-Z0-9]/g, '_');
            const resolvedPath = savePathArg
              ?? (saveDirectoryArg
                ? `${saveDirectoryArg}/${sanitizedName}.stalx`
                : `/mock/Documents/stalagmight/${sanitizedName}.stalx`);
            projectPath = resolvedPath;

            currentProject = {
              id: 1,
              name,
              location: projectLocation,
              created_at: now,
              modified_at: now,
              file_path: projectPath,
              sections: [],
              station_merges: [],
              zero_datums: [],
              loop_closures: [],
              section_offsets: {},
            };

            // Return CreateProjectFullResult format
            return {
              loadingId: Date.now(),
              project: currentProject,
              displaySettings: {
                showStations: true,
                showSectionNames: true,
                showStationLabels: false,
                showStationNames: false,
                showShotData: false,
                showShotErrors: false,
                showReferenceShots: true,
                showZeroDatum: true,
                showGrid: false,
                showScaleBar: true,
                // Mirrors the backend's DisplaySettings default (loading.rs);
                // satellite ships on.
                showSatellite: true,
                showDepthGradient: false,
                gridSpacing: 10,
              },
            };
          }

          case 'load_project_full': {
            // Consolidated load_project_full command for loading progress feature
            projectPath = (args?.path as string) ?? null;
            const now = new Date().toISOString();
            sectionCounter = 0;

            // Generate a consistent loadingId for this load operation
            const loadingId = Date.now();

            // Emit progress events (these are received by useLoadingProgress hook)
            // Phase 1: Opening database
            emitEvent('loading-progress', {
              loadingId,
              phase: 'opening_database',
              phaseIndex: 0,
              phaseCount: 8,
              current: null,
              total: null,
              overallPercent: 5,
              isIndeterminate: true,
            });

            // Create a project with sections for testing
            const defaultSections = [
              {
                id: ++sectionCounter,
                name: 'Mock Tunnel A',
                surveyor: 'Test Surveyor',
                date: now.split('T')[0],
                color: '#4ECDC4',
                shots: [
                  { id: mockShotId(sectionCounter, 0), azimuth_in: 45, azimuth_out: 50, length: 10, depth_in: 0, depth_out: -2, kind: 'Normal' },
                  { id: mockShotId(sectionCounter, 1), azimuth_in: 90, azimuth_out: 95, length: 15, depth_in: -2, depth_out: -5, kind: 'Normal' },
                ],
              },
            ];

            currentProject = {
              id: 1,
              name: 'Test Project',
              location: null,
              created_at: now,
              modified_at: now,
              file_path: projectPath,
              sections: defaultSections,
              station_merges: [],
              zero_datums: [],
              loop_closures: [],
              section_offsets: {},
            };

            // Phase 2: Loading project data
            emitEvent('loading-progress', {
              loadingId,
              phase: 'loading_project_data',
              phaseIndex: 1,
              phaseCount: 8,
              current: 1,
              total: 1,
              overallPercent: 20,
              isIndeterminate: false,
            });

            // Generate mock stations from sections
            const mockStations: Array<{
              station_ref: { section_id: number; station_index: number };
              coordinates: { x: number; y: number; z: number };
              is_zero_datum: boolean;
              gps_coordinates: { latitude: number; longitude: number; depth_reference: number } | null;
              merged_with: Array<{ section_id: number; station_index: number }>;
              // Optional, exactly like the generated `Station` type: the backend
              // omits `closures_with` when empty (#441), so the mock must too -
              // a present-but-empty array is a wire shape the app never sees.
              closures_with?: Array<{
                closure_id: bigint;
                station_ref: { section_id: number; station_index: number };
                via_station: { section_id: number; station_index: number } | null;
                is_transitive: boolean;
              }>;
            }> = [];

            // Collect all section IDs for merged station setup
            const sectionIds = (currentProject.sections as Array<{ id: number }>).map(s => s.id);
            const currentMerges = (currentProject.station_merges ?? []) as Array<{
              station_a: { section_id: number; station_index: number };
              station_b: { section_id: number; station_index: number };
            }>;
            const currentClosures = (currentProject.loop_closures ?? []) as Array<{
              id: bigint;
              station_a: { section_id: number; station_index: number };
              station_b: { section_id: number; station_index: number };
            }>;

            // Build zero_datums lookup for deriving is_zero_datum and gps_coordinates
            const loadDatums = (currentProject.zero_datums ?? []) as Array<{
              station_ref: { section_id: number; station_index: number };
              gps: { latitude: number; longitude: number; depth_reference: number };
            }>;

            for (const section of currentProject.sections as Array<{ id: number; shots: Array<{ depth_in?: number; depth_out?: number }> }>) {
              const shots = section.shots ?? [];
              const depths: number[] = [];
              if (shots.length > 0) {
                const firstDepth = shots[0]?.depth_in ?? 0;
                depths.push(firstDepth);
                for (const shot of shots) {
                  depths.push(shot.depth_out ?? depths[depths.length - 1] ?? 0);
                }
              } else {
                depths.push(0);
              }

              for (let i = 0; i < depths.length; i++) {
                const stationRef = { section_id: section.id, station_index: i };
                let mergedWith: Array<{ section_id: number; station_index: number }> = currentMerges
                  .flatMap((merge) => {
                    const isA =
                      merge.station_a.section_id === stationRef.section_id &&
                      merge.station_a.station_index === stationRef.station_index;
                    const isB =
                      merge.station_b.section_id === stationRef.section_id &&
                      merge.station_b.station_index === stationRef.station_index;
                    if (isA) return [merge.station_b];
                    if (isB) return [merge.station_a];
                    return [];
                  });

                const sectionIndex = sectionIds.indexOf(section.id);
                const isLastStation = i === depths.length - 1;
                const isFirstStation = i === 0;

                // Backward-compatible fallback used by legacy tests when no explicit merges are configured.
                if (mergedWith.length === 0 && currentMerges.length === 0) {
                  if (sectionIndex === 0 && isLastStation && sectionIds.length > 1) {
                    mergedWith = [{ section_id: sectionIds[1]!, station_index: 0 }];
                  }
                  if (sectionIndex === 1 && isFirstStation && sectionIds.length > 1) {
                    const firstSection = currentProject.sections[0] as { shots: unknown[] } | undefined;
                    const firstSectionLastStationIndex = firstSection ? (firstSection.shots?.length ?? 0) : 0;
                    mergedWith = [{ section_id: sectionIds[0]!, station_index: firstSectionLastStationIndex }];
                  }
                }

                // Derive is_zero_datum and gps_coordinates from project zero_datums
                const loadDatum = loadDatums.find(
                  (d) => d.station_ref.section_id === section.id && d.station_ref.station_index === i
                );
                const closuresWith = currentClosures.flatMap((closure) => {
                  const isA =
                    closure.station_a.section_id === stationRef.section_id &&
                    closure.station_a.station_index === stationRef.station_index;
                  const isB =
                    closure.station_b.section_id === stationRef.section_id &&
                    closure.station_b.station_index === stationRef.station_index;
                  if (isA) {
                    return [{
                      closure_id: closure.id,
                      station_ref: closure.station_b,
                      via_station: null,
                      is_transitive: false,
                    }];
                  }
                  if (isB) {
                    return [{
                      closure_id: closure.id,
                      station_ref: closure.station_a,
                      via_station: null,
                      is_transitive: false,
                    }];
                  }
                  return [];
                });

                mockStations.push({
                  station_ref: stationRef,
                  coordinates: { x: i * 10, y: 0, z: depths[i] ?? 0 },
                  is_zero_datum: !!loadDatum,
                  gps_coordinates: loadDatum ? loadDatum.gps : null,
                  merged_with: mergedWith,
                  ...(closuresWith.length > 0 ? { closures_with: closuresWith } : {}),
                });
              }
            }

            // Phase 3-7: Computing stations, validating, etc.
            emitEvent('loading-progress', {
              loadingId,
              phase: 'computing_stations',
              phaseIndex: 3,
              phaseCount: 8,
              current: mockStations.length,
              total: mockStations.length,
              overallPercent: 60,
              isIndeterminate: false,
            });

            // Phase 8: Finalizing
            emitEvent('loading-progress', {
              loadingId,
              phase: 'finalizing',
              phaseIndex: 7,
              phaseCount: 8,
              current: null,
              total: null,
              overallPercent: 95,
              isIndeterminate: true,
            });

            const shotLengths = defaultSections.flatMap((section) =>
              (section.shots ?? []).map((shot: { length?: number }) => shot.length ?? 0)
            );
            const totalLength = shotLengths.reduce((sum, length) => sum + length, 0);
            // Absolute station z, as the backend fills it (#481) - not the raw
            // shot depths.
            const depthValues = mockStations.map((station) => station.coordinates.z);
            const minDepth = depthValues.length > 0 ? Math.min(...depthValues) : 0;
            const maxDepth = depthValues.length > 0 ? Math.max(...depthValues) : 0;
            const shotCount = defaultSections.reduce((count, section) => count + (section.shots?.length ?? 0), 0);

            // Return LoadProjectFullResult format
            return {
              loadingId,
              project: withShotCredit(currentProject),
              stations: mockStations,
              stats: {
                section_count: currentProject.sections.length,
                shot_count: shotCount,
                total_length: totalLength,
                min_depth: minDepth,
                max_depth: maxDepth,
                station_count: mockStations.length,
                loop_count: 0,
                penetration_distance: totalLength,
              },
              warnings: [],
              displaySettings: {
                showStations: true,
                showSectionNames: true,
                showStationLabels: false,
                showStationNames: false,
                showShotData: false,
                showShotErrors: false,
                showReferenceShots: true,
                showZeroDatum: true,
                showGrid: false,
                showScaleBar: true,
                // Mirrors the backend's DisplaySettings default (loading.rs);
                // satellite ships on.
                showSatellite: true,
                showDepthGradient: false,
                gridSpacing: 10,
              },
            };
          }

          case 'load_project_full_binary': {
            // Mirror the binary IPC wire format that the Rust backend
            // emits from `src-tauri/src/core/load_binary.rs::encode`:
            // build the same JSON fixture the legacy `load_project_full`
            // case produces, pack shots / stations into Uint8Array byte
            // buffers, msgpack-encode the BinaryEnvelope, and return
            // bytes as a number array (the frontend's `Array.isArray`
            // fallback in loadProjectBinary.ts converts that to a real
            // Uint8Array). The msgpack UMD was injected by the wrapping
            // setupTauriMocks before this script ran, so
            // `window.MessagePack.encode` is available synchronously.
            const jsonResult = mockHandler('load_project_full', args) as {
              loadingId: number;
              project: {
                id?: number; name?: string; location?: string | null;
                created_at?: string; modified_at?: string; file_path?: string | null;
                next_section_id?: number; next_closure_id?: number;
                sections: Array<{
                  id: number; name?: string | null; surveyor?: string | null;
                  date?: string | null; color?: string; direction?: string; quality?: number;
                  // The section's north reference (#438); undefined until an
                  // importer declares one.
                  azimuth_reference?: unknown | null;
                  shots?: Array<{
                    id: number; azimuth_in?: number; azimuth_out?: number;
                    length?: number; depth_in?: number; depth_out?: number;
                    pitch_in?: number | null; pitch_out?: number | null;
                    left?: number | null; right?: number | null;
                    up?: number | null; down?: number | null;
                    temperature?: number | null;
                    comment?: string | null;
                    kind?: string;
                    marker_index?: number | null;
                    credit?: MockCredit;
                  }>;
                }>;
                station_merges?: unknown[]; zero_datums?: unknown[];
                loop_closures?: unknown[]; section_offsets?: Record<string, [number, number]>;
                // Present once `update_project_settings` has stored per-project
                // thresholds on the mock project (#57).
                validation?: Record<string, number>;
                // Entrance elevation (#431); undefined until an import sets it.
                surface_elevation?: number | null;
                // The project-level north reference (#438); undefined until one
                // is declared.
                default_azimuth_reference?: unknown | null;
              };
              stations: Array<{
                station_ref: { section_id: number; station_index: number };
                coordinates?: { x: number; y: number; z: number };
                is_zero_datum?: boolean;
                gps_coordinates?: unknown | null;
                merged_with?: unknown[];
                closures_with?: unknown[];
                comment?: string | null;
                name?: string | null;
              }>;
              stats: Record<string, unknown>;
              warnings: unknown[];
              displaySettings: Record<string, unknown>;
            };

            const totalShots = jsonResult.project.sections.reduce(
              (n, s) => n + (s.shots?.length ?? 0),
              0,
            );
            const SHOT_FLOAT_STRIDE = 48;
            const SHOT_UINT_STRIDE = 12;
            const shotsFloats = new Uint8Array(totalShots * SHOT_FLOAT_STRIDE);
            const shotsUints = new Uint8Array(totalShots * SHOT_UINT_STRIDE);
            const shotsFlags = new Uint8Array(totalShots);
            const shotsFloatsView = new DataView(shotsFloats.buffer);
            const shotsUintsView = new DataView(shotsUints.buffer);
            const sectionsMeta: Array<Record<string, unknown>> = [];
            const shotComments: Array<[number, string]> = [];
            // Binary wire v7: one project-wide people table, each section's
            // distinct credits as indices into it, one u32 per shot (empty
            // buffer when no shot is credited) - `load_binary.rs::encode`.
            const creditPeople: MockPerson[] = [];
            const personIndex = new Map<string, number>();
            const internPerson = (p: MockPerson): number => {
              const key = JSON.stringify([p.name, p.email ?? null]);
              let idx = personIndex.get(key);
              if (idx === undefined) {
                idx = creditPeople.length;
                creditPeople.push(p);
                personIndex.set(key, idx);
              }
              return idx;
            };
            const shotsCreditIdx = new Uint32Array(totalShots).fill(0xffffffff);
            let anyCredit = false;
            let shotCursor = 0;

            for (const section of jsonResult.project.sections) {
              const shot_offset = shotCursor;
              const shot_count = section.shots?.length ?? 0;
              const sectionCredits: Array<{ surveyors: number[]; explorers: number[]; date: string | null }> = [];
              const creditKey = new Map<string, number>();
              for (const shot of section.shots ?? []) {
                if (shot.credit) {
                  const key = JSON.stringify(shot.credit);
                  let ci = creditKey.get(key);
                  if (ci === undefined) {
                    ci = sectionCredits.length;
                    sectionCredits.push({
                      surveyors: shot.credit.surveyors.map(internPerson),
                      explorers: shot.credit.explorers.map(internPerson),
                      date: shot.credit.date,
                    });
                    creditKey.set(key, ci);
                  }
                  shotsCreditIdx[shotCursor] = ci;
                  anyCredit = true;
                }
                const fOff = shotCursor * SHOT_FLOAT_STRIDE;
                const nullToNaN = (v: number | null | undefined): number =>
                  v === null || v === undefined ? NaN : v;
                shotsFloatsView.setFloat32(fOff + 0,  shot.azimuth_in ?? 0, true);
                shotsFloatsView.setFloat32(fOff + 4,  shot.azimuth_out ?? 0, true);
                shotsFloatsView.setFloat32(fOff + 8,  shot.length ?? 0, true);
                shotsFloatsView.setFloat32(fOff + 12, shot.depth_in ?? 0, true);
                shotsFloatsView.setFloat32(fOff + 16, shot.depth_out ?? 0, true);
                shotsFloatsView.setFloat32(fOff + 20, nullToNaN(shot.pitch_in), true);
                shotsFloatsView.setFloat32(fOff + 24, nullToNaN(shot.pitch_out), true);
                shotsFloatsView.setFloat32(fOff + 28, nullToNaN(shot.left), true);
                shotsFloatsView.setFloat32(fOff + 32, nullToNaN(shot.right), true);
                shotsFloatsView.setFloat32(fOff + 36, nullToNaN(shot.up), true);
                shotsFloatsView.setFloat32(fOff + 40, nullToNaN(shot.down), true);
                shotsFloatsView.setFloat32(fOff + 44, nullToNaN(shot.temperature), true);
                const uOff = shotCursor * SHOT_UINT_STRIDE;
                shotsUintsView.setUint32(uOff + 0, section.id >>> 0, true);
                shotsUintsView.setUint32(uOff + 4, shot.id >>> 0, true);
                shotsUintsView.setUint32(
                  uOff + 8,
                  shot.marker_index === null || shot.marker_index === undefined
                    ? 0xffffffff
                    : shot.marker_index >>> 0,
                  true,
                );
                // Binary wire v3: bits 0-1 carry the ShotKind discriminant (#27).
                shotsFlags[shotCursor] =
                  shot.kind === 'Jump' ? 1 : shot.kind === 'Reference' ? 2 : 0;
                if (shot.comment) shotComments.push([shotCursor, shot.comment]);
                shotCursor++;
              }
              sectionsMeta.push({
                id: section.id,
                name: section.name ?? null,
                color: section.color ?? '#888888',
                direction: section.direction ?? 'Forward',
                quality: section.quality ?? 1.0,
                shot_offset,
                shot_count,
                // The band this section's shot ids actually live in - a flat
                // cursor here would advertise an id the backend could never mint.
                next_shot_id: mockShotId(section.id as number, shot_count),
                // This section's own north reference (#438); `null` inherits the
                // project default above, which is what a section that declares
                // nothing carries on disk.
                azimuth_reference: section.azimuth_reference ?? null,
                credits: sectionCredits,
              });
            }
            const shotsCredit = anyCredit
              ? new Uint8Array(shotsCreditIdx.buffer)
              : new Uint8Array(0);

            const STATION_FLOAT_STRIDE = 24;
            const STATION_UINT_STRIDE = 8;
            const totalStations = jsonResult.stations.length;
            const stationsFloats = new Uint8Array(totalStations * STATION_FLOAT_STRIDE);
            const stationsUints = new Uint8Array(totalStations * STATION_UINT_STRIDE);
            const stationsFlags = new Uint8Array(totalStations);
            const stationsFloatsView = new DataView(stationsFloats.buffer);
            const stationsUintsView = new DataView(stationsUints.buffer);
            const stationGps: Array<[number, unknown]> = [];
            const stationMergedWith: Array<[number, unknown[]]> = [];
            const stationClosuresWith: Array<[number, unknown[]]> = [];
            const stationCommentsArr: Array<[number, string]> = [];
            const stationNamesArr: Array<[number, string]> = [];

            jsonResult.stations.forEach((station, idx) => {
              const fOff = idx * STATION_FLOAT_STRIDE;
              stationsFloatsView.setFloat64(fOff + 0,  station.coordinates?.x ?? 0, true);
              stationsFloatsView.setFloat64(fOff + 8,  station.coordinates?.y ?? 0, true);
              stationsFloatsView.setFloat64(fOff + 16, station.coordinates?.z ?? 0, true);
              const uOff = idx * STATION_UINT_STRIDE;
              stationsUintsView.setUint32(uOff + 0, station.station_ref.section_id >>> 0, true);
              stationsUintsView.setUint32(uOff + 4, station.station_ref.station_index >>> 0, true);
              stationsFlags[idx] = station.is_zero_datum ? 1 : 0;
              if (station.gps_coordinates) stationGps.push([idx, station.gps_coordinates]);
              if (station.merged_with?.length) stationMergedWith.push([idx, station.merged_with]);
              if (station.closures_with?.length) stationClosuresWith.push([idx, station.closures_with]);
              if (station.comment) stationCommentsArr.push([idx, station.comment]);
              if (station.name) stationNamesArr.push([idx, station.name]);
            });

            const envelope = {
              // Must match `core/load_binary.rs::BINARY_FORMAT_VERSION` and
              // `EXPECTED_VERSION` in `src/utils/loadProjectBinary.ts`; the
              // binaryFormatVersion tripwire pins the other two to each other.
              // Bumped from 3 to 4 when Ariane collaboration mode was removed:
              // the header no longer carries a `collab` field; 4 to 5 for the
              // #438 north references (a project-level default plus one per
              // section header); 6 to 7 for per-shot survey credit.
              version: 7,
              loading_id: jsonResult.loadingId,
              // Added to the envelope at v3 alongside the ShotKind bits
              // (`load_binary.rs`): the decoder reads it unvalidated, so omitting
              // it here made every mocked load resolve `isRemote: undefined` and
              // open the SpeleoDB read-only gate. The version tripwire pins the
              // NUMBER, not the field set, so it stayed green.
              is_remote: projectIsRemote,
              project_header: {
                name: jsonResult.project.name ?? 'Test Project',
                location: jsonResult.project.location ?? null,
                created_at: jsonResult.project.created_at ?? new Date().toISOString(),
                modified_at: jsonResult.project.modified_at ?? new Date().toISOString(),
                file_path: jsonResult.project.file_path ?? null,
                next_section_id: jsonResult.project.next_section_id
                  ?? (sectionsMeta.length + 1),
                next_closure_id: jsonResult.project.next_closure_id ?? 1,
                sections: sectionsMeta,
                station_merges: jsonResult.project.station_merges ?? [],
                zero_datums: jsonResult.project.zero_datums ?? [],
                loop_closures: jsonResult.project.loop_closures ?? [],
                section_offsets: sectionOffsetPairs(jsonResult.project.section_offsets),
                // The mock has no `station_offsets` state: no mocked command ever
                // writes one, so there is nothing for a reverse to mirror either
                // (the backend's `for_each_station_ref` does mirror them). Give
                // one a home here and `reverse_section` must mirror it too.
                station_offsets: [],
                station_comments_map: [],
                imported_file_hashes: [],
                imported_section_hashes: [],
                // Per-project validation thresholds (#57), sourced like every
                // sibling above: whatever `update_project_settings` last stored
                // on the mock project, else `ValidationConfig::default()` - what
                // a project that never customised its thresholds carries on
                // disk. Rust always sends this; omitting it made every mocked
                // load resolve `project.validation` as `undefined`, which the UI
                // quietly papers over with the GLOBAL config instead
                // (`ShotTableEditor` prefers the project's own thresholds), so
                // per-project thresholds stopped applying with nothing red.
                validation: jsonResult.project.validation ?? {
                  warn_shot_length_min: 0.3,
                  warn_shot_length_max: 50.0,
                  warn_depth_change_per_shot_max: 10.0,
                  warn_azimuth_deviation_max: 30.0,
                  azimuth_diff_threshold: 20.0,
                  pitch_diff_threshold: 10.0,
                  inclination_depth_diff_threshold: 15.0,
                },
                // Entrance elevation (#431), same provenance as `validation`
                // above: whatever the mock project currently carries, else
                // `null` (unknown) - what a project that never imported one
                // has on disk.
                surface_elevation: jsonResult.project.surface_elevation ?? null,
                // The project-level north reference (#438), same provenance as
                // the two above. `null` is "nothing recorded", which is what a
                // project that never declared one carries - and what every
                // section that declares none inherits.
                default_azimuth_reference:
                  jsonResult.project.default_azimuth_reference ?? null,
                // Every distinct person any shot credits, once (v7).
                credit_people: creditPeople,
              },
              shot_count: totalShots,
              shots_floats: shotsFloats,
              shots_uints: shotsUints,
              shots_flags: shotsFlags,
              shot_comments: shotComments,
              shots_credit: shotsCredit,
              station_count: totalStations,
              stations_floats: stationsFloats,
              stations_uints: stationsUints,
              stations_flags: stationsFlags,
              station_gps: stationGps,
              station_merged_with: stationMergedWith,
              station_closures_with: stationClosuresWith,
              station_comments: stationCommentsArr,
              station_names: stationNamesArr,
              stats: jsonResult.stats,
              warnings: jsonResult.warnings,
              display_settings: jsonResult.displaySettings,
              frozen_shots: [],
            };

            const MessagePack = (window as unknown as {
              MessagePack?: { encode: (v: unknown) => Uint8Array };
            }).MessagePack;
            if (!MessagePack) {
              throw new Error(
                'load_project_full_binary mock: window.MessagePack missing - setupTauriMocks must inject the msgpack UMD',
              );
            }
            const bytes = MessagePack.encode(envelope);
            // Return as plain number[] so the frontend's
            // `Array.isArray` fallback converts to Uint8Array without
            // any extra cross-realm dance.
            return Array.from(bytes);
          }

          case 'save_project': {
            // Mirror the backend's Save-a-Copy contract: an EXPLICIT destination
            // on a SpeleoDB-managed project writes a detached unlinked copy - the
            // session does NOT move (projectPath/file_path untouched) and the
            // result carries unlinked_copy_path instead of saved_path.
            if (projectIsRemote && args?.path) {
              return {
                success: true,
                unlinked_copy_path: args.path as string,
                error: null,
                warnings: [],
                section_ids: [],
              };
            }
            projectPath = (args?.path as string) ?? projectPath;
            if (currentProject && projectPath) {
              currentProject.file_path = projectPath;
            }
            return { success: true, saved_path: projectPath, error: null, warnings: [], section_ids: [] };
          }

          case 'close_project': {
            currentProject = null;
            projectPath = null;
            return { success: true, section_ids: [] };
          }

          // A project switch / quit hands the open project's lock back first. Nothing
          // is held in a mocked run, so nothing is released (no `lock_release`); specs
          // that drive a failed release override this reply.
          case 'speleodb_settle_open_lock':
            return { success: true, warnings: [], error: null, section_ids: [] };

          case 'cancel_loading': {
            // Idempotent - does nothing but shouldn't error
            return undefined;
          }

          case 'get_recent_logs':
            // DebugDialog expects string[] log lines prefixed with a level.
            return ['INFO [mock] app started', 'DEBUG [mock] project loaded'];

          case 'get_project':
          case 'get_project_state': {
            // Generate mock stations from sections with merged_with data
            const mockStations: Array<{
              station_ref: { section_id: number; station_index: number };
              coordinates: { x: number; y: number; z: number };
              is_zero_datum: boolean;
              gps_coordinates: { latitude: number; longitude: number; depth_reference: number } | null;
              merged_with: Array<{ section_id: number; station_index: number }>;
              // Optional, exactly like the generated `Station` type: the backend
              // omits `closures_with` when empty (#441), so the mock must too -
              // a present-but-empty array is a wire shape the app never sees.
              closures_with?: Array<{
                closure_id: bigint;
                station_ref: { section_id: number; station_index: number };
                via_station: { section_id: number; station_index: number } | null;
                is_transitive: boolean;
              }>;
              comment?: string;
            }> = [];

            // Collect all section IDs for merged station setup
            const sectionIdList = (currentProject?.sections as Array<{ id: number }> ?? []).map(s => s.id);
            const stateMerges = (currentProject?.station_merges ?? []) as Array<{
              station_a: { section_id: number; station_index: number };
              station_b: { section_id: number; station_index: number };
            }>;
            const stateClosures = (currentProject?.loop_closures ?? []) as Array<{
              id: bigint;
              station_a: { section_id: number; station_index: number };
              station_b: { section_id: number; station_index: number };
            }>;

            // Build zero_datums lookup for deriving is_zero_datum and gps_coordinates
            const zeroDatums = (currentProject?.zero_datums ?? []) as Array<{
              station_ref: { section_id: number; station_index: number };
              gps: { latitude: number; longitude: number; depth_reference: number };
            }>;

            if (currentProject?.sections) {
              for (const section of currentProject.sections as Array<{ id: number; shots: unknown[] }>) {
                // Each section gets (shots.length + 1) stations
                const shotCount = section.shots?.length ?? 0;
                const sectionIndex = sectionIdList.indexOf(section.id);

                for (let i = 0; i <= shotCount; i++) {
                  const stationRef = { section_id: section.id, station_index: i };
                  let mergedWith: Array<{ section_id: number; station_index: number }> = stateMerges
                    .flatMap((merge) => {
                      const isA =
                        merge.station_a.section_id === stationRef.section_id &&
                        merge.station_a.station_index === stationRef.station_index;
                      const isB =
                        merge.station_b.section_id === stationRef.section_id &&
                        merge.station_b.station_index === stationRef.station_index;
                      if (isA) return [merge.station_b];
                      if (isB) return [merge.station_a];
                      return [];
                    });
                  const isLastStation = i === shotCount;
                  const isFirstStation = i === 0;

                  // Backward-compatible fallback used by legacy tests when no explicit merges are configured.
                  if (mergedWith.length === 0 && stateMerges.length === 0) {
                    if (sectionIndex === 0 && isLastStation && sectionIdList.length > 1) {
                      mergedWith = [{ section_id: sectionIdList[1]!, station_index: 0 }];
                    }
                    if (sectionIndex === 1 && isFirstStation && sectionIdList.length > 1) {
                      const firstSection = currentProject.sections[0] as { shots?: unknown[] } | undefined;
                      const firstSectionLastStationIndex = firstSection?.shots?.length ?? 0;
                      mergedWith = [{ section_id: sectionIdList[0]!, station_index: firstSectionLastStationIndex }];
                    }
                  }

                  // Derive is_zero_datum and gps_coordinates from project zero_datums
                  const matchingDatum = zeroDatums.find(
                    (d) => d.station_ref.section_id === section.id && d.station_ref.station_index === i
                  );
                  const closuresWith = stateClosures.flatMap((closure) => {
                    const isA =
                      closure.station_a.section_id === stationRef.section_id &&
                      closure.station_a.station_index === stationRef.station_index;
                    const isB =
                      closure.station_b.section_id === stationRef.section_id &&
                      closure.station_b.station_index === stationRef.station_index;
                    if (isA) {
                      return [{
                        closure_id: closure.id,
                        station_ref: closure.station_b,
                        via_station: null,
                        is_transitive: false,
                      }];
                    }
                    if (isB) {
                      return [{
                        closure_id: closure.id,
                        station_ref: closure.station_a,
                        via_station: null,
                        is_transitive: false,
                      }];
                    }
                    return [];
                  });

                  const storedComment = mockStationComments[`${section.id}:${i}`];
                  const storedName = mockStationNames[`${section.id}:${i}`];
                  mockStations.push({
                    station_ref: stationRef,
                    coordinates: { x: i * 10, y: 0, z: 0 },
                    is_zero_datum: !!matchingDatum,
                    gps_coordinates: matchingDatum ? matchingDatum.gps : null,
                    merged_with: mergedWith,
                    ...(closuresWith.length > 0 ? { closures_with: closuresWith } : {}),
                    ...(storedComment !== undefined ? { comment: storedComment } : {}),
                    ...(storedName !== undefined ? { name: storedName } : {}),
                  });
                }
              }
            }

            const depthValues = mockStations.map((station) => station.coordinates.z);
            const minDepth = depthValues.length > 0 ? Math.min(...depthValues) : 0;
            const maxDepth = depthValues.length > 0 ? Math.max(...depthValues) : 0;
            const shotCount =
              currentProject?.sections.reduce(
                (count: number, section) =>
                  count + ((section as { shots?: Array<{ length?: number }> }).shots?.length ?? 0),
                0
              ) ?? 0;
            const totalLength =
              currentProject?.sections.reduce((total: number, section) => {
                const shots = (section as { shots?: Array<{ length?: number }> }).shots ?? [];
                return total + shots.reduce((sum, shot) => sum + (shot.length ?? 0), 0);
              }, 0) ?? 0;

            return {
              // Return a FRESH (deep-cloned) project each call, mirroring the real Tauri
              // IPC: the backend returns `project.clone()` serialized over IPC, so the
              // project AND every nested array (sections, zero_datums, loop_closures, …)
              // gets a new identity per call. The mock mutates those arrays in place
              // (e.g. sections.push in create_section, datums.push in set_zero_datum),
              // so returning the same reference froze #160's reference-keyed memos -
              // `useMemo([sections])` left the SectionsPanel empty even as fresh `stats`
              // showed the populated count. A deep clone keeps every consumer's
              // ref-identity check honest, not just the sections one. (Caused ~50
              // full-suite failures; smoke seeds once, so it never hit this.)
              project: currentProject ? withShotCredit(structuredClone(currentProject)) : currentProject,
              stations: mockStations,
              stats: {
                section_count: currentProject?.sections.length ?? 0,
                shot_count: shotCount,
                total_length: totalLength,
                min_depth: minDepth,
                max_depth: maxDepth,
                station_count: mockStations.length,
                loop_count: 0,
                penetration_distance: totalLength,
              },
              frozen_shots: [],
            };
          }

          case 'get_adjusted_stations': {
            // Closure-adjusted geometry view. The mock has no real least-squares
            // solver, so the adjusted geometry is identical to measured: reuse the
            // station array get_project_state builds.
            const state = mockHandler('get_project_state', args) as
              | { stations?: unknown[] }
              | null;
            return state?.stations ?? [];
          }

          case 'get_station_distances_from': {
            // Distance-gradient field (#34). No real graph in the mock - synthesize
            // distances from each station's mock x-coordinate (origin at x=0) so the
            // overlay + legend have finite values to render. `from` is ignored.
            const state = mockHandler('get_project_state', args) as
              | {
                  stations?: Array<{
                    station_ref: { section_id: number; station_index: number };
                    coordinates: { x: number; y: number; z: number };
                  }>;
                }
              | null;
            const stations = state?.stations ?? [];
            let max = 0;
            const distances = stations.map((s) => {
              const d = Math.abs(s.coordinates.x);
              if (d > max) max = d;
              return d;
            });
            return {
              distances,
              station_refs: stations.map((s) => s.station_ref),
              max_reachable: max,
            };
          }

          // Dive-plan persistence (#244). An in-memory mirror of the .plnx store.
          case 'list_dive_plans':
            return mockDivePlans.map((p) => ({
              id: p.id,
              name: p.name,
              created_at: p.created_at,
              updated_at: p.updated_at,
            }));
          case 'load_dive_plan': {
            const found = mockDivePlans.find((p) => p.id === args?.id);
            if (!found) throw new Error('That dive plan no longer exists.');
            // #310: this mock keeps a plan's refs POSITIONAL (unlike the real `.plnx`, which
            // canonicalizes to shot-uuid anchors on save), so the anchors below are derived
            // fresh at load time against the CURRENT survey - exactly what a real load would
            // hand back for a plan whose refs happen to still be current.
            const snap = (found.snapshot ?? {}) as {
              origin?: MockPlanRefLite | null;
              endpoint?: MockPlanRefLite | null;
              waypoints?: MockPlanRefLite[];
              marked_points?: Array<{ station: MockPlanRefLite; label?: string | null }>;
              stage_drop_overrides?: Array<MockPlanRefLite | null>;
            };
            return {
              meta: {
                id: found.id,
                name: found.name,
                created_at: found.created_at,
                updated_at: found.updated_at,
              },
              snapshot: found.snapshot,
              stale_ref_count: 0,
              anchors: {
                origin: refToAnchor(snap.origin ?? null),
                endpoint: refToAnchor(snap.endpoint ?? null),
                waypoints: (snap.waypoints ?? []).map((w) => refToAnchor(w)),
                marked_points: (snap.marked_points ?? []).map((m) => refToAnchor(m.station)),
                stage_drop_overrides: (snap.stage_drop_overrides ?? []).map((r) =>
                  refToAnchor(r ?? null),
                ),
              },
            };
          }
          case 'save_dive_plan': {
            const id = (args?.id ?? null) as number | null;
            const name = args?.name as string;
            const snapshot = args?.snapshot;
            const stamp = nextDivePlanStamp();
            if (id === null) {
              const plan = {
                id: mockDivePlanNextId,
                name,
                created_at: stamp,
                updated_at: stamp,
                snapshot,
              };
              mockDivePlanNextId += 1;
              mockDivePlans.push(plan);
              return {
                id: plan.id,
                name: plan.name,
                created_at: plan.created_at,
                updated_at: plan.updated_at,
              };
            }
            const existing = mockDivePlans.find((p) => p.id === id);
            if (!existing) throw new Error('That dive plan no longer exists.');
            existing.name = name;
            existing.updated_at = stamp;
            existing.snapshot = snapshot;
            return {
              id: existing.id,
              name: existing.name,
              created_at: existing.created_at,
              updated_at: existing.updated_at,
            };
          }
          case 'delete_dive_plan': {
            const before = mockDivePlans.length;
            mockDivePlans = mockDivePlans.filter((p) => p.id !== args?.id);
            if (mockDivePlans.length === before) {
              throw new Error('That dive plan no longer exists.');
            }
            return null;
          }

          // #310: anchor / re-resolve live plan pins across a survey edit. Recorded together in
          // one call log (both commands round-trip the same opaque anchor shape).
          case 'plan_anchors_for_refs': {
            const refs = (args?.refs ?? []) as Array<MockPlanRefLite | null>;
            const anchors = refs.map((ref) => refToAnchor(ref));
            const win = window as unknown as Record<string, unknown>;
            const calls = (win.__MOCK_ANCHOR_CALLS__ as unknown[]) ?? [];
            calls.push({ cmd: 'plan_anchors_for_refs', refs, anchors });
            win.__MOCK_ANCHOR_CALLS__ = calls;
            return anchors;
          }
          case 'plan_refs_for_anchors': {
            const anchors = (args?.anchors ?? []) as Array<MockPlanAnchor | null>;
            const refs = anchors.map((a) => anchorToRef(a));
            const win = window as unknown as Record<string, unknown>;
            const calls = (win.__MOCK_ANCHOR_CALLS__ as unknown[]) ?? [];
            calls.push({ cmd: 'plan_refs_for_anchors', anchors, refs });
            win.__MOCK_ANCHOR_CALLS__ = calls;
            return refs;
          }
          case 'compute_gas_reach_field': {
            // Record every call's inputs so drag e2e can assert the dragged station was sent as a
            // per-stage override (#246). Keyed on the camelCase arg the frontend actually invokes with.
            {
              const win = window as unknown as Record<string, unknown>;
              const calls = (win.__MOCK_GAS_REACH_CALLS__ as unknown[]) ?? [];
              calls.push({
                stageDropOverrides: args?.stageDropOverrides ?? [],
                endpoint: args?.endpoint ?? null,
                origin: args?.origin ?? null,
                // #298: the ordered way points, so an e2e can assert the pick reached the engine
                // and in what order.
                waypoints: args?.waypoints ?? [],
                // #317: the per-span speed + propulsion, so an e2e can assert a segment edit
                // reached the engine rather than only re-rendering the table.
                segments: args?.segments ?? [],
                // #317: the rig itself, so disabling it can be proven at the REQUEST - the
                // readout that gates on it also gates on the client-side `hasDpvRig` flag, which
                // flips a render tick before any recompute round-trips.
                dpv: (args?.gasConfig as { dpv?: unknown } | undefined)?.dpv ?? null,
              });
              win.__MOCK_GAS_REACH_CALLS__ = calls;
            }
            // Gas reachable-frontier field (#38). No real graph in the mock -
            // synthesize a field from each station's mock x-coordinate (origin at
            // x=0) so the overlay + legend + turn markers have finite values to
            // render. Diver/cylinders/gasConfig inputs are ignored; the budget is a
            // fixed plausible value so far-enough stations fall outside it.
            const state = mockHandler('get_project_state', args) as
              | {
                  stations?: Array<{
                    station_ref: { section_id: number; station_index: number };
                    coordinates: { x: number; y: number; z: number };
                  }>;
                }
              | null;
            const stations = state?.stations ?? [];
            const budget_litres = 1500;
            const gas_to_reach: (number | null)[] = stations.map((s) => Math.abs(s.coordinates.x) * 20);
            // #395: keep the LAST station genuinely UNCONNECTED (gas_to_reach null)
            // BEFORE computing `reachable` - mirrors the real backend invariant
            // (src-tauri/src/models/project/gas.rs: reachable requires g.is_finite())
            // so a null gas_to_reach can never pair with reachable:true. Specs can
            // then assert the three-way split: grey (unconnected, reachable:false,
            // gas_to_reach:null) vs dimmed-but-colored (beyond gas range,
            // reachable:false, gas_to_reach:non-null) vs reachable.
            if (gas_to_reach.length > 0) {
              gas_to_reach[gas_to_reach.length - 1] = null;
            }
            const reachable = gas_to_reach.map((g) => g !== null && g <= budget_litres);
            let max_reachable_gas = 0;
            for (let i = 0; i < gas_to_reach.length; i += 1) {
              const g = gas_to_reach[i];
              if (reachable[i] && g !== null && g !== undefined && g > max_reachable_gas) {
                max_reachable_gas = g;
              }
            }
            // Turn frontier: a reachable station whose next (by array order) is not
            // reachable - enough for the marker layer to render in the smoke test.
            const turn_frontier = stations
              .filter((_s, i) => reachable[i] && i + 1 < stations.length && !reachable[i + 1])
              .map((s) => s.station_ref);
            // Staged-strategy drops + the readout gauges (Half-Minus). Mirror the backend: only the
            // staged strategies carry drops, and only when the rig actually has enough Stage
            // cylinders (else it falls back to All-thirds). The endpoint, when given + reachable, is
            // the route target (else the deepest reachable station). Exact math is unit-tested in
            // Rust; the mock just produces plausible, shape-correct values for the UI wiring.
            const gasConfig = (args?.gasConfig ?? {}) as {
              default_reserve?: string;
              reserve_profile?: { stage_reserve_bar?: number; all_thirds_safety_margin_bar?: number };
              conservative_tank_gas?: boolean;
              swim_speed_m_per_min?: number;
              survey_speed_m_per_min?: number | null;
              dpv?: {
                primary?: { speed_m_per_min?: number; burn_minutes?: number };
                backup?: { speed_m_per_min?: number; burn_minutes?: number };
                range_reserve_fraction?: number;
              } | null;
            };
            const cylinders = (args?.cylinders ?? []) as Array<{
              role?: string;
              fill_bar?: number;
              water_volume_l?: number;
              rated_pressure_bar?: number;
              enabled?: boolean;
              usable_gas_bar?: number;
            }>;
            const requested = gasConfig.default_reserve ?? 'all_thirds';
            // Disabled tanks (#245) are excluded from the gauges/drops, mirroring the engine.
            const stageCyls = cylinders.filter((c) => c.role === 'stage' && c.enabled !== false);
            const required = requested === 'two_stage' ? 2 : requested === 'one_stage' ? 1 : 0;
            const effective = stageCyls.length >= required ? requested : 'all_thirds';
            const dropCount = effective === 'two_stage' ? 2 : effective === 'one_stage' ? 1 : 0;
            const reserve = gasConfig.reserve_profile?.stage_reserve_bar ?? 30;

            const reachableRefs = stations
              .filter((_s, i) => reachable[i])
              .map((s) => s.station_ref);
            let deepestIdx = -1;
            let deepestGas = -1;
            for (let i = 0; i < gas_to_reach.length; i += 1) {
              const g = gas_to_reach[i];
              if (reachable[i] && g !== null && g !== undefined && g > deepestGas) {
                deepestGas = g;
                deepestIdx = i;
              }
            }
            const endpoint = (args?.endpoint ?? null) as {
              section_id: number;
              station_index: number;
            } | null;
            const endpointReachable =
              endpoint != null &&
              reachableRefs.some(
                (r) =>
                  r.section_id === endpoint.section_id && r.station_index === endpoint.station_index,
              );
            const targetRef = endpointReachable
              ? endpoint
              : deepestIdx >= 0
                ? stations[deepestIdx].station_ref
                : null;
            const final_turn_ref = targetRef;
            // Mirrors the engine's own flag: true when the endpoint IS the target, false for the
            // deepest-in-budget fallback (no endpoint set, or one that cannot be used). The mock has
            // no partial walk toward an out-of-reach endpoint, so those two cases are all it can
            // distinguish - enough to keep the panel's two messages apart.
            const turn_toward_endpoint = endpointReachable && endpoint != null;
            // Drop stages: staged presets drop their first `dropCount` enabled stages; Free drops
            // every enabled stage with a positive usable (no cap). Each gets a ref, clamped to the
            // turnaround when the route is shorter - mirroring the engine so refs/bars/distances align.
            const dropSourceStages =
              effective === 'free'
                ? stageCyls.filter((c) => (c.usable_gas_bar ?? 0) > 0)
                : stageCyls.slice(0, dropCount);
            const stage_drop_refs = reachableRefs.length
              ? dropSourceStages.map(
                  (_c, i) => reachableRefs[Math.min(i + 1, reachableRefs.length - 1)],
                )
              : [];
            // Apply per-stage drop overrides (#246): pin a drop to its dragged station and flag it
            // over-reserve when that station sits DEEPER (more gas-to-reach) than the strategy drop.
            // ⚠ This is a DELIBERATELY simplified stand-in, NOT the engine's rule: the engine flags
            // over-reserve on `contribution > safe_in_litres` (with a pin-to-default no-op) and folds
            // the budget/turn/reach; the mock only moves the ref + sets a station-depth flag and keeps
            // the budget fixed. So the e2e asserts UI WIRING (colour, reset, warning) only; the flag
            // semantics + the recompute math are covered by the Rust unit tests, which are the source
            // of truth. A pin-to-default is a no-op here too (same station ⇒ not deeper ⇒ not flagged).
            const stageOverrides = (args?.stageDropOverrides ?? []) as Array<{
              section_id: number;
              station_index: number;
            } | null>;
            const gasAtRef = (r: { section_id: number; station_index: number }) => {
              const idx = stations.findIndex(
                (s) =>
                  s.station_ref.section_id === r.section_id &&
                  s.station_ref.station_index === r.station_index,
              );
              return idx >= 0 ? (gas_to_reach[idx] ?? 0) : 0;
            };
            const stage_drop_over_reserve = stage_drop_refs.map((defRef, i) => {
              const ov = stageOverrides[i];
              if (ov == null || defRef == null) return false;
              const over = gasAtRef(ov) > gasAtRef(defRef);
              stage_drop_refs[i] = ov;
              return over;
            });
            // Route legs to the target (consecutive reachable stations; the real graph path is
            // exercised in Rust). is_jump stays false - the seeded e2e sections carry no jumps.
            const route =
              targetRef != null
                ? reachableRefs
                    .slice(0, -1)
                    .map((from, i) => ({ from, to: reachableRefs[i + 1], is_jump: false }))
                : [];

            // Route physical metrics (#236) over the route's stations, in order (origin→turnaround),
            // derived from the route legs the mock returns - so it mirrors the backend's route_refs
            // CHAIN, not the full reachable set. NOTE: the seeded e2e cave is a single linear section
            // with no merge/closure ties, so this is an approximation for the linear seed, not a
            // re-implementation of the backend's tie-aware shortest path (exact math is Rust-tested).
            const coordByRef = new Map(
              stations.map((s) => [
                `${s.station_ref.section_id}:${s.station_ref.station_index}`,
                s.coordinates,
              ]),
            );
            const routeRefsOrdered =
              route.length > 0
                ? [route[0].from, ...route.map((leg) => leg.to)]
                : targetRef != null
                  ? [targetRef]
                  : [];
            const routePts = routeRefsOrdered
              .map((r) => coordByRef.get(`${r.section_id}:${r.station_index}`))
              .filter((c): c is { x: number; y: number; z: number } => c != null);
            let route_line_length_m = 0;
            let route_max_depth_m: number | null = null;
            let route_avg_depth_m: number | null = null;
            if (routePts.length > 0) {
              route_max_depth_m = Math.max(...routePts.map((c) => c.z));
              let depthLenSum = 0;
              for (let i = 1; i < routePts.length; i += 1) {
                const a = routePts[i - 1];
                const b = routePts[i];
                const seg = Math.hypot(b.x - a.x, b.y - a.y, b.z - a.z);
                route_line_length_m += seg;
                depthLenSum += seg * 0.5 * (a.z + b.z);
              }
              route_avg_depth_m =
                route_line_length_m > 0 ? depthLenSum / route_line_length_m : routePts[0].z;
            }

            const margin = gasConfig.reserve_profile?.all_thirds_safety_margin_bar ?? 0;
            const backCyls = cylinders.filter((c) => c.role === 'back_gas' && c.enabled !== false);
            const sumWv = backCyls.reduce((s, c) => s + (c.water_volume_l ?? 0), 0);
            const frac = effective === 'two_stage' ? 1 / 6 : effective === 'one_stage' ? 1 / 4 : 1 / 3;
            const avgStart =
              sumWv > 0
                ? backCyls.reduce((s, c) => s + (c.fill_bar ?? 0) * (c.water_volume_l ?? 0), 0) / sumWv
                : 0;
            // All-thirds turn rises by the safety margin (mock ignores Z: turn = avgStart·(1-⅓)+margin).
            const backgas_turn_bar =
              sumWv > 0
                ? Math.max(0, avgStart * (1 - frac) + (effective === 'all_thirds' ? margin : 0))
                : null;
            const stage_drop_bars = dropSourceStages.map((c) => {
              const fill = c.fill_bar ?? 0;
              const rated = c.rated_pressure_bar ?? fill;
              const basis = gasConfig.conservative_tank_gas !== false ? Math.min(fill, rated) : fill;
              if (effective === 'free') {
                const u = Math.max(0, Math.min(c.usable_gas_bar ?? 0, basis));
                return Math.max(0, fill - u);
              }
              return Math.min(basis, 0.5 * (basis + reserve));
            });
            // Cumulative swum distance to each drop ref along the route (aligned with stage_drop_refs).
            const cumDist: number[] = [];
            {
              let acc = 0;
              for (let i = 0; i < routePts.length; i += 1) {
                if (i > 0) {
                  const a = routePts[i - 1];
                  const b = routePts[i];
                  acc += Math.hypot(b.x - a.x, b.y - a.y, b.z - a.z);
                }
                cumDist.push(acc);
              }
            }
            const stage_drop_distances_m = stage_drop_refs.map((d) => {
              const j = routeRefsOrdered.findIndex(
                (r) => r.section_id === d.section_id && r.station_index === d.station_index,
              );
              return j >= 0 ? (cumDist[j] ?? 0) : 0;
            });
            // Per-row usable bar (#245), aligned 1:1 with INPUT cylinders (disabled/non-budget = 0).
            // The mock ignores the Z derate, so deliverable/water-vol ≈ fill. Rust unit tests pin
            // the exact math; this just gives the table shape-correct values.
            const isBudget = (r?: string) => r === 'back_gas' || r === 'stage';
            let enabledStageI = 0;
            const cylinder_usable_bars = cylinders.map((c) => {
              if (c.enabled === false) return 0;
              const fill = c.fill_bar ?? 0;
              const rated = c.rated_pressure_bar ?? fill;
              const basis = gasConfig.conservative_tank_gas !== false ? Math.min(fill, rated) : fill;
              if (effective === 'free') {
                // Clamp to `basis` (min(fill, rated) under conservative), matching the engine's
                // free_usable_bar - not raw fill - so the mock doesn't diverge on overfilled tanks.
                return isBudget(c.role) ? Math.max(0, Math.min(c.usable_gas_bar ?? 0, basis)) : 0;
              }
              if (effective === 'all_thirds') {
                // ⅓ less the safety margin (mock ignores Z), clamped ≥ 0 - mirrors the engine.
                return isBudget(c.role) ? Math.max(0, fill / 3 - margin) : 0;
              }
              if (c.role === 'back_gas') return frac * fill;
              if (c.role === 'stage') {
                const counted = enabledStageI < required;
                enabledStageI += 1;
                return counted ? Math.max(0, 0.5 * (basis - reserve)) : 0;
              }
              return 0;
            });
            // Cumulative gas at each way point (#298). Like the drop math above this is a
            // shape-correct stand-in, not the engine's multi-leg walk: it reports each way point's
            // own gas-to-reach (null when the station is not in the field at all, which is what
            // the panel's "unreachable" warning keys off) rather than accumulating the legs. The
            // real accumulation is unit-tested in Rust; the e2e asserts UI wiring only.
            const waypointArgs = (args?.waypoints ?? []) as Array<{
              section_id: number;
              station_index: number;
            }>;
            const waypoint_gas_litres = waypointArgs.map((w) => {
              const idx = stations.findIndex(
                (s) =>
                  s.station_ref.section_id === w.section_id &&
                  s.station_ref.station_index === w.station_index,
              );
              return idx >= 0 ? gas_to_reach[idx] : null;
            });
            // Round-trip dive profile (#299). Same standing as the metrics above: a shape-correct
            // stand-in over the linear e2e seed, not the engine's tie-aware, asymmetrically-priced
            // walk. It does honour the one property the UI is asserted on - the exit leg is swum at
            // `swim_speed_m_per_min` while the way in uses the survey speed when one is set - so a
            // spec can tell the two axes apart. The real engine additionally rides a SCOOTERED span
            // out at the span's own pace (#317); no spec asserts the exit half's numbers, so this
            // stand-in does not model that. The exact math is pinned by the Rust tests.
            const inSpeed = gasConfig.survey_speed_m_per_min ?? gasConfig.swim_speed_m_per_min ?? 10;
            const outSpeed = gasConfig.swim_speed_m_per_min ?? 10;
            const routeGasAt = (r: { section_id: number; station_index: number }) => {
              const idx = stations.findIndex(
                (s) =>
                  s.station_ref.section_id === r.section_id &&
                  s.station_ref.station_index === r.station_index,
              );
              return idx >= 0 ? (gas_to_reach[idx] ?? 0) : 0;
            };
            const outboundProfile = routeRefsOrdered.map((r, i) => ({
              route_index: i,
              station_ref: r,
              distance_m: cumDist[i] ?? 0,
              depth_m: routePts[i]?.z ?? 0,
              gas_l: routeGasAt(r),
              elapsed_min: (cumDist[i] ?? 0) / inSpeed,
              outbound: true,
            }));
            const turnPoint = outboundProfile[outboundProfile.length - 1];
            const totalOut = turnPoint?.distance_m ?? 0;
            const route_profile = [...outboundProfile];
            if (turnPoint) {
              let gas = turnPoint.gas_l;
              let elapsed = turnPoint.elapsed_min;
              for (let i = outboundProfile.length - 2; i >= 0; i -= 1) {
                const seg = outboundProfile[i + 1].distance_m - outboundProfile[i].distance_m;
                elapsed += seg / outSpeed;
                // Re-swim the leg's outbound cost, scaled by the speed ratio: faster out means less
                // time at depth, hence less gas. Enough for the UI, not the engine's real pricing.
                gas +=
                  (outboundProfile[i + 1].gas_l - outboundProfile[i].gas_l) * (inSpeed / outSpeed);
                route_profile.push({
                  ...outboundProfile[i],
                  distance_m: totalOut + (totalOut - outboundProfile[i].distance_m),
                  gas_l: gas,
                  elapsed_min: elapsed,
                  outbound: false,
                });
              }
            }
            // ⚠ Mock-only shortcut: a REF lookup, which is exactly the ambiguity the real engine's
            // route indices exist to remove (a way-point route revisits stations, and `findIndex`
            // matches the first pass). Adequate here because the seeded e2e cave is linear, so no
            // station appears twice - but do not read this as a model of the backend.
            // NOT filtered: the real engine maps `drop_indices` straight through, so this array is
            // always the same length as `stage_drop_refs` / `stage_drop_over_reserve`, and the
            // consumer indexes all three positionally. Dropping a miss would shift every later
            // entry and silently pin an over-reserve flag to the wrong stage. A miss surfaces as a
            // visibly wrong -1 instead, which is the direction a mock should fail in.
            const stage_drop_route_indices = stage_drop_refs.map((d) =>
              routeRefsOrdered.findIndex(
                (r) => r.section_id === d.section_id && r.station_index === d.station_index,
              ),
            );
            const waypoint_route_indices = waypointArgs.map((w) => {
              const j = routeRefsOrdered.findIndex(
                (r) => r.section_id === w.section_id && r.station_index === w.station_index,
              );
              return j >= 0 ? j : null;
            });
            // Scooter + per-segment readouts (#317). Same standing caveat as the drop math above:
            // a shape-correct stand-in for UI wiring, NOT the engine's rule. In particular the mock
            // does NOT enforce the range cap on `reachable` / `turn_frontier` - the engine ANDs it
            // into both masks, and the Rust unit tests are the source of truth for that. So an e2e
            // here may assert that the readout rows render the numbers it was handed; it may NOT
            // assert which stations grey out under a rig.
            //
            // With no rig (`dpv: null`, which is what both mock config blocks ship) every value
            // below is inert, so the 31 existing gas specs - all calibrated against the fixed
            // 1500 L budget and `|x| * 20` field - are untouched.
            const segmentArgs = (args?.segments ?? []) as Array<{
              speed_m_per_min?: number | null;
              on_scooter?: boolean;
            }>;
            const rig = gasConfig.dpv ?? null;
            const unitRange = (u?: { speed_m_per_min?: number; burn_minutes?: number }) =>
              (u?.speed_m_per_min ?? 0) * (u?.burn_minutes ?? 0);
            // Twin of `MAX_DPV_RANGE_RESERVE_FRACTION` (src-tauri/src/config/gas.rs) and its
            // frontend mirror (`MAX_DPV_RANGE_RESERVE_FRACTION` in GasSettings.tsx) - both clamp
            // the reserve fraction to [0, 0.9] before applying it.
            const clampedReserveFraction = Math.min(
              0.9,
              Math.max(0, rig?.range_reserve_fraction ?? 0),
            );
            const scooter_range_limit_m = rig
              ? Math.max(
                  0,
                  Math.min(unitRange(rig.primary), unitRange(rig.backup)) *
                    (1 - clampedReserveFraction),
                )
              : null;
            // Which segment each route leg belongs to: way points split the route, so leg j sits in
            // the segment named by how many way points the route has already passed at that point.
            const routeIndexOf = (r: { section_id: number; station_index: number }) =>
              routeRefsOrdered.findIndex(
                (x) => x.section_id === r.section_id && x.station_index === r.station_index,
              );
            const waypointRouteIdx = waypointArgs.map(routeIndexOf).filter((i) => i >= 0);
            const segmentAtLeg = (legIdx: number) =>
              waypointRouteIdx.filter((wi) => wi <= legIdx).length;
            // Twin of `MIN_SEGMENT_SPEED_M_PER_MIN`/`MAX_SEGMENT_SPEED_M_PER_MIN` in gas/scooter.rs
            // ([0.1, 1000] m/min) - the engine falls back to the plan speed outside this range
            // rather than accepting whatever was stored, and the mock must refuse the same values
            // or an e2e asserting an out-of-range speed's effect could pass against the mock while
            // diverging from the backend.
            const isSegmentSpeedInRange = (v: number | null | undefined): v is number =>
              typeof v === 'number' && v >= 0.1 && v <= 1000;
            let scooter_distance_m = 0;
            let route_runtime_min: number | null = null;
            if (routePts.length > 1) {
              let minutes = 0;
              for (let i = 1; i < routePts.length; i += 1) {
                const a = routePts[i - 1];
                const b = routePts[i];
                const seg = Math.hypot(b.x - a.x, b.y - a.y, b.z - a.z);
                const s = segmentArgs[segmentAtLeg(i - 1)];
                // A span with no speed of its own is flown at the PLAN's speed, not a hardcoded
                // one - mirroring `resolve_segments` (gas/scooter.rs). Getting this wrong is how
                // the real engine once stopped honouring the configured swim speed entirely.
                const speed = isSegmentSpeedInRange(s?.speed_m_per_min) ? s.speed_m_per_min : inSpeed;
                minutes += seg / speed;
                if (rig && s?.on_scooter) scooter_distance_m += seg;
              }
              route_runtime_min = minutes;
            }
            // The park point is DERIVED, never placed: the boundary after the last scootered
            // segment. Null when nothing is scootered, and when the units are carried all the way
            // to the turnaround (the last segment is scootered, so its boundary is the turn).
            let scooter_drop_ref: { section_id: number; station_index: number } | null = null;
            if (rig) {
              let lastOn = -1;
              for (let i = 0; i < segmentArgs.length; i += 1) {
                if (segmentArgs[i]?.on_scooter) lastOn = i;
              }
              if (lastOn >= 0 && lastOn + 1 < segmentArgs.length) {
                scooter_drop_ref = waypointArgs[lastOn] ?? null;
              }
            }
            // NOT `scooter_distance_m > scooter_range_limit_m` - that is verbatim the naive
            // per-route comparison `GasReachField::turn_on_scooter_range`'s own doc
            // (models/project/gas/model.rs) says the UI must not re-derive: the route already
            // stops at whichever cap bound first, so the two read as merely equal in exactly
            // the case being asked about. No spec asserts this value; hardcoded false rather
            // than guess at the real (cave-scoped) rule.
            const turn_on_scooter_range = false;
            return {
              station_refs: stations.map((s) => s.station_ref),
              gas_to_reach,
              reachable,
              turn_frontier,
              max_reachable_gas,
              budget_litres,
              route,
              stage_drop_refs,
              final_turn_ref,
              turn_toward_endpoint,
              backgas_turn_bar,
              stage_drop_bars,
              stage_drop_over_reserve,
              stage_drop_distances_m,
              cylinder_usable_bars,
              route_line_length_m,
              route_max_depth_m,
              route_avg_depth_m,
              waypoint_gas_litres,
              route_profile,
              stage_drop_route_indices,
              waypoint_route_indices,
              scooter_distance_m,
              scooter_range_limit_m,
              scooter_drop_ref,
              route_runtime_min,
              turn_on_scooter_range,
            };
          }

          case 'get_along_line_distance': {
            // Ruler along-line distance (#26). No real survey graph in the mock -
            // return a deterministic distance and tint the first couple of shots
            // of endpoint `a`'s section so the readout + tint have something to show.
            const endpointA = (args?.a ?? null) as { section_id?: number } | null;
            const sections = (currentProject?.sections ?? []) as Array<{ id: number; shots: Array<{ id: number }> }>;
            const sec = sections.find((s) => s.id === endpointA?.section_id) ?? sections[0];
            const path_shots = sec
              ? (sec.shots ?? []).slice(0, 2).map((sh) => ({ section_id: sec.id, shot_id: sh.id }))
              : [];
            // Stations bounding those shots (one more than the shot count).
            const path_stations = sec
              ? Array.from({ length: path_shots.length + 1 }, (_, i) => ({ section_id: sec.id, station_index: i }))
              : [];
            return { along_line: 12.34, path_shots, path_stations };
          }

          case 'create_section': {
            sectionCounter++;
            const rawShots = Array.isArray(args?.shots) ? (args?.shots as Array<Record<string, unknown>>) : [];
            const shots = rawShots.map((shot, index) => ({
              id: mockShotId(sectionCounter, index),
              // #310: a deterministic, stable uuid per shot - the neighbour-pair anchors above
              // key off this, exactly as the real backend keys off `Shot.uuid`.
              uuid: `mock-shot-uuid-${mockShotId(sectionCounter, index)}`,
              azimuth_in: (shot.azimuth_in as number) ?? 0,
              azimuth_out: (shot.azimuth_out as number) ?? 0,
              length: (shot.length as number) ?? 0,
              depth_in: (shot.depth_in as number) ?? 0,
              depth_out: (shot.depth_out as number) ?? 0,
              pitch_in: (shot.pitch_in as number | null | undefined) ?? null,
              pitch_out: (shot.pitch_out as number | null | undefined) ?? null,
              left: (shot.left as number | null | undefined) ?? null,
              right: (shot.right as number | null | undefined) ?? null,
              up: (shot.up as number | null | undefined) ?? null,
              down: (shot.down as number | null | undefined) ?? null,
              comment: (shot.comment as string | null | undefined) ?? null,
              kind: (shot.kind as string | undefined) ?? 'Normal',
              marker_index: (shot.marker_index as number | null | undefined) ?? null,
              temperature: (shot.temperature as number | null | undefined) ?? null,
              // Per-shot credit (New Section puts its pills on every shot).
              ...(shot.credit ? { credit: structuredClone(shot.credit) } : {}),
            }));
            // `SectionData.surveyor/date` are the backend's legacy TEMPLATE for
            // shots with no credit of their own; honour what the caller sent
            // (New Section sends null), defaulting only when the key is absent.
            const section = {
              id: sectionCounter,
              name: (args?.name as string) ?? `Section ${sectionCounter}`,
              surveyor: args && 'surveyor' in args ? ((args.surveyor as string | null) ?? null) : 'Test Surveyor',
              date: args && 'date' in args ? ((args.date as string | null) ?? null) : new Date().toISOString().split('T')[0],
              color: '#3b82f6',
              shots,
            };
            currentProject?.sections.push(section);
            return { success: true, section, section_id: sectionCounter, section_ids: [], warnings: [], error: null };
          }

          case 'import_sections': {
            console.log('[Mock IPC] import_sections called with:', args);
            const rawSections = Array.isArray(args?.sections) ? (args?.sections as Array<Record<string, unknown>>) : [];
            if (rawSections.length === 0) {
              // Mirrors the real backend's `sections.is_empty()` early return
              // (execute_import_sections, command_import.rs): no receipt when
              // nothing was imported.
              return { success: true, warnings: [], error: null, section_ids: [] };
            }
            if (!currentProject) {
              const now = new Date().toISOString();
              sectionCounter = 0;
              currentProject = {
                id: 1,
                name: 'Imported Project',
                location: null,
                created_at: now,
                modified_at: now,
                file_path: null,
                sections: [],
                station_merges: [],
                zero_datums: [],
                loop_closures: [],
                section_offsets: {},
              };
            }
            const importedSections = rawSections.map((sectionData) => {
              sectionCounter++;
              const rawShots = Array.isArray(sectionData.shots) ? (sectionData.shots as Array<Record<string, unknown>>) : [];
              // The backend's import overrides (`command_import.rs`): the dialog's
              // Surveyors / Explorers / date, when sent, REPLACE what each shot's
              // own credit said; otherwise the shot keeps the credit it arrived with.
              const setSurveyors = Array.isArray(sectionData.surveyors)
                ? mockDedupe(sectionData.surveyors as MockPerson[])
                : undefined;
              const setExplorers = Array.isArray(sectionData.explorers)
                ? mockDedupe(sectionData.explorers as MockPerson[])
                : undefined;
              const overrideDate = typeof sectionData.override_date === 'string' ? sectionData.override_date : undefined;
              const importedCredit = (shot: Record<string, unknown>): { credit?: MockCredit } => {
                const own = (shot.credit as MockCredit | undefined) ?? { surveyors: [], explorers: [], date: null };
                const credit: MockCredit = {
                  surveyors: setSurveyors ?? own.surveyors,
                  explorers: setExplorers ?? own.explorers,
                  date: overrideDate ?? own.date,
                };
                const empty = credit.surveyors.length === 0 && credit.explorers.length === 0 && credit.date === null;
                return empty ? {} : { credit: structuredClone(credit) };
              };
              const shots = rawShots.map((shot, index) => ({
                ...importedCredit(shot),
                id: mockShotId(sectionCounter, index),
                azimuth_in: (shot.azimuth_in as number) ?? 0,
                azimuth_out: (shot.azimuth_out as number) ?? 0,
                length: (shot.length as number) ?? 0,
                depth_in: (shot.depth_in as number) ?? 0,
                depth_out: (shot.depth_out as number) ?? 0,
                pitch_in: (shot.pitch_in as number | null | undefined) ?? null,
                pitch_out: (shot.pitch_out as number | null | undefined) ?? null,
                left: (shot.left as number | null | undefined) ?? null,
                right: (shot.right as number | null | undefined) ?? null,
                up: (shot.up as number | null | undefined) ?? null,
                down: (shot.down as number | null | undefined) ?? null,
                comment: (shot.comment as string | null | undefined) ?? null,
                kind: (shot.kind as string | undefined) ?? 'Normal',
                marker_index: (shot.marker_index as number | null | undefined) ?? null,
                temperature: (shot.temperature as number | null | undefined) ?? null,
              }));
              const section = {
                id: sectionCounter,
                name: (sectionData.name as string) ?? `Section ${sectionCounter}`,
                // The legacy template for uncredited shots. An explicit null (every
                // import dialog now sends one) means none; only a config built
                // without the key gets the fixture default.
                surveyor: sectionData.surveyor === undefined ? 'Test Surveyor' : (sectionData.surveyor as string | null),
                date: (sectionData.date as string) ?? new Date().toISOString().split('T')[0],
                color: (sectionData.color as string) ?? '#3b82f6',
                direction: (sectionData.direction as string) ?? 'in',
                shots,
              };
              currentProject?.sections.push(section);
              const stationNames = Array.isArray(sectionData.station_names)
                ? (sectionData.station_names as Array<[number, string]>)
                : [];
              for (const [stationIndex, name] of stationNames) {
                if (stationIndex <= shots.length) mockStationNames[`${section.id}:${stationIndex}`] = name;
              }
              return section;
            });
            const sectionIds = importedSections.map((section) => section.id as number);
            const importedShotCount = importedSections.reduce(
              (sum, section) => sum + (section.shots as unknown[]).length,
              0
            );
            const sourceLabel = (args?.sourceLabel as string | null | undefined) ?? null;
            const sourceFileName = (args?.sourceFileName as string | null | undefined) ?? null;
            const sectionsFoundArg = (args?.sectionsFound as number | null | undefined) ?? null;
            return {
              success: true,
              warnings: [],
              error: null,
              section_ids: sectionIds,
              ...(sectionIds[0] != null ? { section_id: sectionIds[0] } : {}),
              sections: importedSections,
              import_receipt: {
                source_format: sourceLabel ?? 'mNemo DMP',
                file_name: sourceFileName ?? '',
                sections_found: sectionsFoundArg ?? sectionIds.length,
                sections_imported: sectionIds.length,
                shot_count: importedShotCount,
                closures_requested: 0,
                closures_created: 0,
                unit_conversion: null,
                warnings: [],
                info: [],
              },
            };
          }

          case 'execute_walls_import': {
            // Record the import args (selectedSurveys, includeSplays, …) for
            // assertions, then return a one-section success so the dialog runs
            // its "imported" exit path (not the "nothing imported" branch).
            wallsImportCalls.push({ ...(args ?? {}) });
            const wallsPath = (args?.path as string | undefined) ?? '';
            const wallsFileName = wallsPath.split('/').pop() ?? wallsPath;
            return {
              success: true,
              warnings: [],
              error: null,
              section_ids: [1],
              section_id: 1,
              import_receipt: {
                source_format: 'Walls',
                file_name: wallsFileName,
                sections_found: 1,
                sections_imported: 1,
                shot_count: 0,
                closures_requested: 0,
                closures_created: 0,
                unit_conversion: null,
                warnings: [],
                info: [],
              },
            };
          }

          // File hash tracking for duplicate detection
          case 'calculate_file_hash':
            // Return a mock SHA256 hash based on path for consistency
            return `mock_hash_${(args?.path as string)?.split('/').pop() ?? 'unknown'}`;

          case 'check_duplicate_import':
            // Configurable via __MOCK_SET_CHECK_DUPLICATE_RESULT__
            return mockCheckDuplicateResult;

          case 'track_imported_file':
            console.log('[Mock IPC] track_imported_file called with:', args);
            trackImportedFileCalls.push({ hash: (args?.hash as string) ?? '' });
            return { success: true, section_ids: [] };

          case 'parse_gpx': {
            return buildMockGpxFile(args?.path as string | undefined);
          }

          case 'execute_gpx_import': {
            const importWaypoints = (args?.importWaypoints as boolean | undefined) ?? false;
            const importTracks = (args?.importTracks as boolean | undefined) ?? false;
            const importRoutes = (args?.importRoutes as boolean | undefined) ?? false;
            const depthInverted = (args?.depthInverted as boolean | undefined) ?? false;
            const filePath = (args?.path as string | undefined) ?? null;
            const sectionPlacements = (Array.isArray(args?.sectionPlacements) ? args?.sectionPlacements : []) as Array<{
              section_index?: number;
              placement?: {
                type?: string;
                station_index?: number;
                coordinates?: { latitude: number; longitude: number; depth_reference: number };
              };
              name?: string | null;
              color?: string;
            }>;

            if (!currentProject) {
              const now = new Date().toISOString();
              sectionCounter = 0;
              currentProject = {
                id: 1,
                name: 'Imported Project',
                location: null,
                created_at: now,
                modified_at: now,
                file_path: null,
                sections: [],
                station_merges: [],
                zero_datums: [],
                loop_closures: [],
                section_offsets: {},
              };
            }

            const gpxFile = buildMockGpxFile(filePath);
            const placementMap = new Map<number, typeof sectionPlacements[number]>();
            for (const placement of sectionPlacements) {
              const sectionIndex = placement.section_index;
              if (typeof sectionIndex === 'number') {
                placementMap.set(sectionIndex, placement);
              }
            }

            const createdSectionIds: number[] = [];
            let backendSectionIndex = 0;
            let gpxShotCount = 0;

            const appendSection = (
              name: string,
              shots: Array<Record<string, unknown>>,
              color: string,
              datum:
                | {
                    stationIndex: number;
                    gps: { latitude: number; longitude: number; depth_reference: number };
                  }
                | null
            ) => {
              sectionCounter += 1;
              const sectionId = sectionCounter;
              // toShot() numbers shots 1..n before the destination section is
              // known; re-key them onto this section's band so GPX-imported
              // sections carry the same section-encoded ids as every other path.
              const encodedShots = shots.map((shot, idx) => ({
                ...shot,
                id: mockShotId(sectionId, idx),
              }));
              const section = {
                id: sectionId,
                name,
                surveyor: 'GPX Import',
                date: new Date().toISOString().split('T')[0] ?? '',
                color,
                direction: 'in',
                shots: encodedShots,
              };
              currentProject?.sections.push(section);
              createdSectionIds.push(sectionId);
              gpxShotCount += encodedShots.length;
              if (datum) {
                (currentProject?.zero_datums as Array<{
                  station_ref: { section_id: number; station_index: number };
                  gps: { latitude: number; longitude: number; depth_reference: number };
                }>).push({
                  station_ref: { section_id: sectionId, station_index: datum.stationIndex },
                  gps: datum.gps,
                });
              }
            };

            if (importWaypoints) {
              gpxFile.waypoints.forEach((wpt, index) => {
                appendSection(
                  wpt.name ?? `Waypoint ${index + 1}`,
                  [],
                  '#4a90d9',
                  {
                    stationIndex: 0,
                    gps: {
                      latitude: wpt.latitude,
                      longitude: wpt.longitude,
                      depth_reference: 0,
                    },
                  }
                );
                backendSectionIndex += 1;
              });
            }

            if (importTracks) {
              gpxFile.tracks.forEach((track, trackIndex) => {
                track.segments.forEach((segment, segmentIndex) => {
                  const shots = segment.points.slice(1).map((point, idx) =>
                    toShot(segment.points[idx]!, point, idx + 1, depthInverted)
                  );
                  const placement = placementMap.get(backendSectionIndex);
                  const placementDatum =
                    placement?.placement?.type === 'zero_datum' && placement.placement.coordinates
                      ? {
                          stationIndex: placement.placement.station_index ?? 0,
                          gps: placement.placement.coordinates,
                        }
                      : segment.points[0]
                        ? {
                            stationIndex: 0,
                            gps: {
                              latitude: segment.points[0].latitude,
                              longitude: segment.points[0].longitude,
                              depth_reference: 0,
                            },
                          }
                        : null;

                  appendSection(
                    placement?.name ??
                      track.name ??
                      `Track ${trackIndex + 1}${track.segments.length > 1 ? ` - Seg ${segmentIndex + 1}` : ''}`,
                    shots,
                    placement?.color ?? '#3b82f6',
                    placementDatum
                  );
                  backendSectionIndex += 1;
                });
              });
            }

            if (importRoutes) {
              gpxFile.routes.forEach((route, routeIndex) => {
                const shots = route.points.slice(1).map((point, idx) =>
                  toShot(route.points[idx]!, point, idx + 1, depthInverted)
                );
                const placement = placementMap.get(backendSectionIndex);
                const placementDatum =
                  placement?.placement?.type === 'zero_datum' && placement.placement.coordinates
                    ? {
                        stationIndex: placement.placement.station_index ?? 0,
                        gps: placement.placement.coordinates,
                      }
                    : route.points[0]
                      ? {
                          stationIndex: 0,
                          gps: {
                            latitude: route.points[0].latitude,
                            longitude: route.points[0].longitude,
                            depth_reference: 0,
                          },
                        }
                      : null;

                appendSection(
                  placement?.name ?? route.name ?? `Route ${routeIndex + 1}`,
                  shots,
                  placement?.color ?? '#22c55e',
                  placementDatum
                );
                backendSectionIndex += 1;
              });
            }

            if (currentProject) {
              currentProject.modified_at = new Date().toISOString();
            }

            const gpxFileName = (filePath ?? '').split('/').pop() ?? filePath ?? '';
            return {
              success: true,
              warnings: [],
              error: null,
              section_ids: createdSectionIds,
              ...(createdSectionIds[0] != null ? { section_id: createdSectionIds[0] } : {}),
              import_receipt: {
                source_format: 'GPX',
                file_name: gpxFileName,
                sections_found: createdSectionIds.length,
                sections_imported: createdSectionIds.length,
                shot_count: gpxShotCount,
                closures_requested: 0,
                closures_created: 0,
                unit_conversion: null,
                warnings: [],
                info: [],
              },
            };
          }

          case 'parse_tml':
            return buildMockTmlFile(args?.path as string | undefined);

          case 'execute_tml_import': {
            tmlImportCalls.push({ ...(args ?? {}) });
            const tmlPath = (args?.path as string | undefined) ?? '';
            const includeExcluded = (args?.includeExcluded as boolean | undefined) ?? false;
            const virtualShotMode = (args?.virtualShotMode as string | null | undefined) ?? null;
            const tmlFile = buildMockTmlFile(tmlPath);

            if (!currentProject) {
              const now = new Date().toISOString();
              sectionCounter = 0;
              currentProject = {
                id: 1,
                name: 'Imported Project',
                location: null,
                created_at: now,
                modified_at: now,
                file_path: null,
                sections: [],
                station_merges: [],
                zero_datums: [],
                loop_closures: [],
                section_offsets: {},
              };
            }

            const commentsOnStations = (args?.commentsOnStations as boolean | undefined) ?? true;

            // Import-space station index per shot ID, mirroring the real
            // converter's id_to_import_loc (parsers/tml.rs): Closure rows and
            // (by default) excluded shots never enter the map; the shot at kept
            // index i ends at station i+1. Placements and closures below both
            // resolve through this, so exclusion renumbering behaves like the
            // real import.
            // #425: `null` selects every section; a list selects those indices.
            // Deselected sections contribute nothing to `idToImportLoc` either,
            // mirroring `build_selection_index` - which is what makes a
            // dependent section fall back to the origin instead of pointing at
            // a section that was never created.
            const rawSelection = args?.selectedSections as number[] | null | undefined;
            const selectedSections = rawSelection ? new Set(rawSelection) : null;
            const isSectionSelected = (i: number) =>
              selectedSections === null || selectedSections.has(i);
            const keptBySection: Array<{ tmlIndex: number; shots: typeof tmlFile.sections[number]['shots'] }> = [];
            const idToImportLoc = new Map<number, [number, number]>();
            tmlFile.sections.forEach((tmlSection, tmlIndex) => {
              if (!isSectionSelected(tmlIndex)) return;
              const kept = tmlSection.shots.filter(
                (shot) => shot.shot_type !== 'Closure' && (includeExcluded || !shot.excluded)
              );
              keptBySection.push({ tmlIndex, shots: kept });
              kept.forEach((shot, keptIdx) => idToImportLoc.set(shot.id, [tmlIndex, keptIdx + 1]));
            });

            // ZWSP marker split (core/comment_split.rs): lines prefixed with
            // U+200B are shot-comment lines (marker stripped); unmarked lines
            // are station comments routed to the shot's "to" station. A bare
            // Ariane comment has no markers, so it peels entirely to the
            // station when commentsOnStations is on.
            const splitComment = (raw: string | null): { shot: string | null; station: string | null } => {
              if (!raw) return { shot: null, station: null };
              const shotLines: string[] = [];
              const stationLines: string[] = [];
              for (const line of raw.split('\n')) {
                if (line.startsWith('\u200B')) shotLines.push(line.slice(1));
                else stationLines.push(line);
              }
              return {
                shot: shotLines.length ? shotLines.join('\n') : null,
                station: stationLines.length ? stationLines.join('\n') : null,
              };
            };

            const createdIdByTmlIndex = new Map<number, number>();
            const tmlSectionIds: number[] = [];
            let tmlShotCount = 0;
            for (const { tmlIndex, shots: keptShots } of keptBySection) {
              if (keptShots.length === 0) continue;
              const tmlSection = tmlFile.sections[tmlIndex];
              sectionCounter += 1;
              const sectionId = sectionCounter;
              currentProject?.sections.push({
                id: sectionId,
                name: tmlSection.name,
                surveyor: tmlSection.surveyor,
                date: tmlSection.date,
                color: tmlSection.color,
                direction: 'in',
                shots: keptShots.map((shot, index) => {
                  const split = commentsOnStations
                    ? splitComment(shot.comment)
                    : { shot: shot.comment, station: null };
                  if (split.station) {
                    // Station comments live in the mock's station-comment
                    // store, same as set_station_comment writes - the project
                    // state derives stations per refresh and threads these in.
                    mockStationComments[`${sectionId}:${index + 1}`] = split.station;
                  }
                  return {
                    id: mockShotId(sectionId, index),
                    // Same stable-uuid idiom as create_section above: the real
                    // backend never produces a uuid-less shot.
                    uuid: `mock-shot-uuid-${mockShotId(sectionId, index)}`,
                    azimuth_in: shot.azimuth,
                    azimuth_out: shot.azimuth,
                    length: shot.length,
                    // Depths pass through UNCHANGED: the real converter stores
                    // the parsed positive-down absolute depths verbatim in
                    // ShotData (normalize_tml_shot in parsers/tml.rs) - no
                    // sign flip, no rebasing.
                    depth_in: shot.depth_in,
                    depth_out: shot.depth,
                    // inclination_to_pitch (parsers/tml.rs): pitch is +down,
                    // Ariane inclination negates unless exactly zero; always
                    // Some(...) on the real path, so never null here.
                    pitch_in: shot.inclination === 0 ? 0 : -shot.inclination,
                    pitch_out: null,
                    left: shot.left,
                    right: shot.right,
                    up: shot.up,
                    down: shot.down,
                    comment: split.shot,
                    // DELIBERATELY UNMODELED: the real backend's Auto mode runs
                    // the #27 chain rule (classify_virtual_shots in
                    // src-tauri/src/models/project/classify.rs) over the merged
                    // station graph; this mock reduces it to a ternary, so under
                    // 'Auto' the kind below can DISAGREE with the real backend.
                    // Specs must not assert virtual-shot kinds under Auto - only
                    // under the unconditional overrides (AllJumps/AllReferences),
                    // which bypass the chain rule in the real backend too. Also
                    // unmodeled: the effective_start_id fallback and the
                    // topological re-sort of forward references (this fixture
                    // never produces either).
                    kind:
                      shot.shot_type === 'Virtual'
                        ? virtualShotMode === 'AllReferences'
                          ? 'Reference'
                          : 'Jump'
                        : 'Normal',
                    marker_index: null,
                    temperature: null,
                  };
                }),
              });
              createdIdByTmlIndex.set(tmlIndex, sectionId);
              tmlSectionIds.push(sectionId);
              tmlShotCount += keptShots.length;
            }

            // Placement, as determine_placement + FromImporting do it: section
            // 0 sits at the origin (no GPS in this fixture); every later
            // section resolves its start_ariane_id through id_to_import_loc
            // and, when the target lives in another section, station 0 of the
            // new section merges with the target station
            // (command_import.rs, ImportPlacement::FromImporting ->
            // merge_stations).
            for (const { tmlIndex } of keptBySection) {
              if (tmlIndex === 0) continue;
              const sectionId = createdIdByTmlIndex.get(tmlIndex);
              if (sectionId == null) continue;
              const startId = tmlFile.sections[tmlIndex].start_ariane_id;
              const target = startId >= 0 ? idToImportLoc.get(startId) : undefined;
              if (!target || target[0] === tmlIndex) continue;
              const targetSectionId = createdIdByTmlIndex.get(target[0]);
              if (targetSectionId == null) continue;
              (currentProject?.station_merges as Array<{
                station_a: { section_id: number; station_index: number };
                station_b: { section_id: number; station_index: number };
              }>).push({
                station_a: { section_id: sectionId, station_index: 0 },
                station_b: { section_id: targetSectionId, station_index: target[1] },
              });
            }

            // Loop closures, as the real import scan does it (parsers/tml.rs
            // closure extraction over id_to_import_loc): every Closure row
            // whose from_id AND closure_to_id both resolve to kept shots
            // becomes a loop closure; either endpoint filtered means the
            // closure is dropped. closures_requested mirrors the converter's
            // already-filtered list length, so requested == created here.
            let closuresCreated = 0;
            const loopClosures = (currentProject?.loop_closures ?? []) as Array<{
              id: bigint;
              station_a: { section_id: number; station_index: number };
              station_b: { section_id: number; station_index: number };
              path_length: number;
              misclosure_distance: number;
              error_percentage: number;
              horizontal_misclosure: number;
              vertical_misclosure: number;
              strategy: string;
              is_aligned: boolean;
            }>;
            for (const tmlSection of tmlFile.sections) {
              for (const shot of tmlSection.shots) {
                if (shot.shot_type !== 'Closure' || shot.closure_to_id < 0) continue;
                const from = idToImportLoc.get(shot.from_id);
                const to = idToImportLoc.get(shot.closure_to_id);
                if (!from || !to) continue;
                const fromSectionId = createdIdByTmlIndex.get(from[0]);
                const toSectionId = createdIdByTmlIndex.get(to[0]);
                if (fromSectionId == null || toSectionId == null) continue;
                const maxExistingId = loopClosures.reduce(
                  (maxId, closure) => (closure.id > maxId ? closure.id : maxId),
                  0n
                );
                loopClosures.push({
                  id: maxExistingId + 1n,
                  station_a: { section_id: fromSectionId, station_index: from[1] },
                  station_b: { section_id: toSectionId, station_index: to[1] },
                  path_length: 0,
                  misclosure_distance: 0,
                  error_percentage: 0,
                  horizontal_misclosure: 0,
                  vertical_misclosure: 0,
                  strategy: 'Batch',
                  is_aligned: false,
                });
                closuresCreated += 1;
              }
            }

            if (currentProject) {
              currentProject.modified_at = new Date().toISOString();
            }

            return {
              success: true,
              warnings: [],
              error: null,
              section_ids: tmlSectionIds,
              ...(tmlSectionIds[0] != null ? { section_id: tmlSectionIds[0] } : {}),
              import_receipt: {
                source_format: 'Ariane TML',
                file_name: tmlPath.split('/').pop() ?? tmlPath,
                sections_found: tmlFile.sections.length,
                sections_imported: tmlSectionIds.length,
                shot_count: tmlShotCount,
                closures_requested: closuresCreated,
                closures_created: closuresCreated,
                unit_conversion:
                  tmlFile.unit === 'Imperial' ? 'Lengths converted from feet to meters' : null,
                warnings: [],
              },
            };
          }

          case 'import_dmp':
          case 'parse_dmp': {
            return {
              version: 5,
              sections: [
                {
                  name: 'Main Passage',
                  direction: 'in',
                  date: new Date().toISOString().split('T')[0],
                  shots: [
                    {
                      shot_type: 'Standard',
                      heading_in: 45,
                      heading_out: 50, // both forward readings
                      length: 10.5,
                      depth_in: -2.0,
                      depth_out: -2.5,
                      pitch_in: 0,
                      pitch_out: 0,
                      left: 1.5,
                      right: 2.3,
                      up: 3.0,
                      down: 0.8,
                      temperature: 18.5,
                      hour: 10,
                      minute: 30,
                      second: 0,
                      marker_index: 0,
                      comment: 'Entrance point with good visibility',
                    },
                    {
                      shot_type: 'Standard',
                      heading_in: 90,
                      heading_out: 95, // both forward readings
                      length: 15.2,
                      depth_in: -2.5,
                      depth_out: -5.0,
                      pitch_in: 5,
                      pitch_out: 5,
                      left: 0.5,
                      right: 0.7,
                      up: 1.2,
                      down: 1.5,
                      temperature: 18.3,
                      hour: 10,
                      minute: 35,
                      second: 0,
                      marker_index: 1,
                      // No comment - testing undefined case
                    },
                    {
                      shot_type: 'Standard',
                      heading_in: 135,
                      heading_out: 140, // both forward readings
                      length: 8.0,
                      depth_in: -5.0,
                      depth_out: -3.0,
                      pitch_in: -10,
                      pitch_out: -10,
                      left: 4.2,
                      right: 3.8,
                      up: 5.0,
                      down: 2.0,
                      temperature: 18.0,
                      hour: 10,
                      minute: 40,
                      second: 0,
                      marker_index: 2,
                      comment: 'Large chamber with stalactites on ceiling, white formations visible on right wall',
                    },
                  ],
                },
                {
                  name: 'Side Tunnel',
                  direction: 'out',
                  date: new Date().toISOString().split('T')[0],
                  shots: [
                    {
                      shot_type: 'Standard',
                      heading_in: 270,
                      heading_out: 275, // both forward readings
                      length: 5.0,
                      depth_in: -3.0,
                      depth_out: -4.0,
                      pitch_in: 0,
                      pitch_out: 0,
                      left: 0,
                      right: 0,
                      up: 0,
                      down: 0,
                      temperature: 17.5,
                      hour: 11,
                      minute: 0,
                      second: 0,
                      marker_index: 0,
                      comment: 'Narrow restriction',
                    },
                    {
                      shot_type: 'Standard',
                      heading_in: 280,
                      heading_out: 285, // both forward readings
                      length: 7.5,
                      depth_in: -4.0,
                      depth_out: -6.0,
                      pitch_in: 0,
                      pitch_out: 0,
                      left: 0,
                      right: 0,
                      up: 0,
                      down: 0,
                      temperature: 17.2,
                      hour: 11,
                      minute: 5,
                      second: 0,
                      marker_index: 1,
                      // No comment
                    },
                  ],
                },
                {
                  name: 'Mixed LRUD Section',
                  direction: 'in',
                  date: new Date().toISOString().split('T')[0],
                  shots: [
                    {
                      shot_type: 'Standard',
                      heading_in: 180,
                      heading_out: 185, // both forward readings
                      length: 12.0,
                      depth_in: -1.0,
                      depth_out: -2.0,
                      pitch_in: 8.0,
                      pitch_out: 0,
                      left: 2.5,
                      right: 0,
                      up: 4.0,
                      down: 0,
                      temperature: 19.0,
                      hour: 12,
                      minute: 0,
                      second: 0,
                      marker_index: 0,
                    },
                    {
                      shot_type: 'Standard',
                      heading_in: 190,
                      heading_out: 195, // both forward readings
                      length: 9.0,
                      depth_in: -2.0,
                      depth_out: -3.0,
                      pitch_in: 0,
                      pitch_out: 12.0,
                      left: 0,
                      right: 1.8,
                      up: 0,
                      down: 3.2,
                      temperature: 18.8,
                      hour: 12,
                      minute: 5,
                      second: 0,
                      marker_index: 1,
                    },
                    {
                      shot_type: 'Standard',
                      heading_in: 200,
                      heading_out: 205, // both forward readings
                      length: 50.5,
                      depth_in: -3.0,
                      depth_out: -8.0,
                      pitch_in: 6.0,
                      pitch_out: 10.0,
                      left: 10.5,
                      right: 15.2,
                      up: 20.0,
                      down: 8.5,
                      temperature: 18.5,
                      hour: 12,
                      minute: 10,
                      second: 0,
                      marker_index: 2,
                    },
                  ],
                },
                {
                  name: 'Undefined LRUD Section',
                  direction: 'in',
                  date: new Date().toISOString().split('T')[0],
                  shots: [
                    {
                      shot_type: 'Standard',
                      heading_in: 90,
                      heading_out: 95, // both forward readings
                      length: 6.0,
                      depth_in: -2.0,
                      depth_out: -3.0,
                      pitch_in: 0,
                      pitch_out: 0,
                      // LRUD fields completely omitted (undefined)
                      temperature: 18.0,
                      hour: 14,
                      minute: 0,
                      second: 0,
                      marker_index: 0,
                    },
                    {
                      shot_type: 'Standard',
                      heading_in: 100,
                      heading_out: 105, // both forward readings
                      length: 8.0,
                      depth_in: -3.0,
                      depth_out: -4.0,
                      pitch_in: 0,
                      pitch_out: 0,
                      // Partial LRUD - only left and up defined
                      left: 1.2,
                      up: 2.5,
                      // right and down are undefined
                      temperature: 17.8,
                      hour: 14,
                      minute: 5,
                      second: 0,
                      marker_index: 1,
                    },
                  ],
                },
                {
                  name: 'Empty Section',
                  direction: 'in',
                  date: new Date().toISOString().split('T')[0],
                  shots: [],
                },
                {
                  name: 'Validation Test Section',
                  direction: 'in',
                  date: new Date().toISOString().split('T')[0],
                  shots: [
                    {
                      // Shot with warning: length exceeds max (50.0m)
                      shot_type: 'Standard',
                      heading_in: 45,
                      heading_out: 46,
                      length: 55.0,
                      depth_in: -1.0,
                      depth_out: -2.0,
                      pitch_in: 0,
                      pitch_out: 0,
                      left: 1.0,
                      right: 1.0,
                      up: 1.0,
                      down: 1.0,
                      temperature: 18.0,
                      hour: 15,
                      minute: 0,
                      second: 0,
                      marker_index: 0,
                      comment: 'Long shot - exceeds max length',
                    },
                    {
                      // Shot with error: azimuth out of range
                      shot_type: 'Standard',
                      heading_in: 400,
                      heading_out: 45,
                      length: 10.0,
                      depth_in: -2.0,
                      depth_out: -3.0,
                      pitch_in: 0,
                      pitch_out: 0,
                      left: 1.0,
                      right: 1.0,
                      up: 1.0,
                      down: 1.0,
                      temperature: 18.0,
                      hour: 15,
                      minute: 5,
                      second: 0,
                      marker_index: 1,
                      comment: 'Invalid azimuth - out of range',
                    },
                    {
                      // Shot with warning: excessive depth change
                      shot_type: 'Standard',
                      heading_in: 90,
                      heading_out: 91,
                      length: 8.0,
                      depth_in: -3.0,
                      depth_out: -15.0,
                      pitch_in: 0,
                      pitch_out: 0,
                      left: 1.0,
                      right: 1.0,
                      up: 1.0,
                      down: 1.0,
                      temperature: 18.0,
                      hour: 15,
                      minute: 10,
                      second: 0,
                      marker_index: 2,
                      comment: 'Excessive depth change',
                    },
                    {
                      // Shot with warning: LRUD exceeds max
                      shot_type: 'Standard',
                      heading_in: 120,
                      heading_out: 121,
                      length: 6.0,
                      depth_in: -15.0,
                      depth_out: -16.0,
                      pitch_in: 0,
                      pitch_out: 0,
                      left: 60.0,
                      right: 1.0,
                      up: 1.0,
                      down: 1.0,
                      temperature: 18.0,
                      hour: 15,
                      minute: 15,
                      second: 0,
                      marker_index: 3,
                      comment: 'Large LRUD value',
                    },
                  ],
                },
              ],
            };
          }

          case 'merge_stations': {
            console.log('[Mock IPC] merge_stations called with:', args);
            const stationA = (args?.stationA ?? args?.station_a) as
              | { section_id: number; station_index: number }
              | undefined;
            const stationB = (args?.stationB ?? args?.station_b) as
              | { section_id: number; station_index: number }
              | undefined;
            if (stationA && stationB) {
              mergeStationCalls.push({ stationA, stationB });
              if (currentProject) {
                (currentProject.station_merges as Array<{
                  station_a: { section_id: number; station_index: number };
                  station_b: { section_id: number; station_index: number };
                }>).push({
                  station_a: stationA,
                  station_b: stationB,
                });
              }
            }
            return { success: true, section_ids: [] };
          }

          case 'execute_batch': {
            // Mock execute batch - track calls for assertions and return success
            console.log('[Mock IPC] execute_batch called with:', args);
            const win = window as unknown as Record<string, unknown>;
            const existing = win.__MOCK_EXECUTE_BATCH_CALLS__ as unknown[] | undefined;
            const calls = Array.isArray(existing) ? existing : [];
            calls.push(args?.commands ?? null);
            win.__MOCK_EXECUTE_BATCH_CALLS__ = calls;
            // Apply contained update_shot commands to currentProject (reusing
            // the update_shot mutation) so a multi-cell paste/fill survives
            // the store's post-command refresh. Nested batch payloads bypass
            // Tauri's top-level camelCase conversion and stay snake_case.
            const commands = (Array.isArray(args?.commands) ? args?.commands : []) as Array<
              Record<string, unknown>
            >;
            // The real execute_batch is all-or-nothing: it snapshots the WHOLE
            // project (`capture_project_snapshot`) and restores on any
            // sub-command failure. Snapshot the whole thing here too - a
            // sections-only snapshot would leave `section_offsets`, the station
            // refs and the comment map half-mirrored after a failed multi-section
            // reverse (#24), i.e. exactly the half-applied state the batching
            // exists to prevent, in the mock that is supposed to pin it.
            const projectBefore = currentProject
              ? (JSON.parse(JSON.stringify(currentProject)) as typeof currentProject)
              : null;
            const commentsBefore = { ...mockStationComments };
            // #24: entries accumulate ACROSS sub-commands, in step order and with repeats -
            // each carries the shot count in force at its own step, which is what lets the
            // frontend compose rather than replay. A batch reversing the same section twice
            // must therefore emit two entries, not one.
            const reversedSections: Array<{ section_id: number; shot_count: number }> = [];
            const creditStep: MockCreditStep = [];
            mockCreditBatchStep = creditStep;
            for (const command of commands) {
              let result:
                | {
                    success?: boolean;
                    error?: string;
                    // Always present on a real CommandResult (#441), unlike
                    // `reversed_sections` which the wire genuinely omits.
                    section_ids?: number[];
                    reversed_sections?: Array<{ section_id: number; shot_count: number }>;
                  }
                | undefined;
              if (command?.type === 'update_shot') {
                result = mockHandler('update_shot', {
                  sectionId: command.section_id,
                  shotId: command.shot_id,
                  updates: command.updates,
                }) as typeof result;
              } else if (command?.type === 'untrack_import') {
                // Rides the withdrawal batch (#394) so a re-import of the same
                // file is not reported as a duplicate. This mock does not keep
                // hash SETS (`check_section_hashes` answers false for everything
                // and `check_duplicate_import` reads one configurable flag), so
                // the faithful replay is to clear that flag and drop the
                // recorded track calls the withdrawal has just undone.
                mockCheckDuplicateResult = false;
                trackImportedFileCalls = [];
                result = { success: true, section_ids: [] };
              } else if (command?.type === 'delete_section') {
                // Withdrawing an abandoned import (#394) removes N sections in
                // ONE batch, so the batch has to replay it or a cancelled DMP
                // import would look like it kept everything.
                result = mockHandler('delete_section', {
                  sectionId: command.section_id,
                }) as typeof result;
              } else if (command?.type === 'update_section_metadata') {
                // Edit Section's Save: a rename beside the credit edit, one step.
                result = mockHandler('update_section', {
                  sectionId: command.section_id,
                  updates: command.updates,
                }) as typeof result;
              } else if (command?.type === 'edit_shot_credit') {
                // Data-table paste / fill of the credit columns (one per shot).
                result = mockHandler('edit_shot_credit', {
                  targets: command.targets,
                  edit: command.edit,
                }) as typeof result;
              } else if (command?.type === 'reverse_section') {
                // Multi-section reverse (#24) is batched so it is atomic and
                // costs one undo, so the batch has to replay it too.
                result = mockHandler('reverse_section', {
                  sectionId: command.section_id,
                  swapLeftRight: command.swap_left_right,
                }) as typeof result;
                if (result?.reversed_sections) {
                  reversedSections.push(...result.reversed_sections);
                }
              }
              if (result && result.success === false) {
                mockCreditBatchStep = null;
                if (currentProject && projectBefore) {
                  Object.assign(currentProject, projectBefore);
                }
                for (const key of Object.keys(mockStationComments)) {
                  delete mockStationComments[key];
                }
                Object.assign(mockStationComments, commentsBefore);
                return result;
              }
            }
            mockCreditBatchStep = null;
            if (creditStep.length > 0) {
              mockCreditUndo.push(creditStep);
              mockCreditRedo.length = 0;
            }
            return {
              success: true,
              section_ids: [],
              ...(reversedSections.length > 0 ? { reversed_sections: reversedSections } : {}),
            };
          }

          case 'check_merge_connectivity': {
            // Mock connectivity check
            // Default: not connected but merge allowed (opens merge dialog)
            // Tests can override via __MOCK_CONNECTIVITY_RESULT__
            console.log('[Mock IPC] check_merge_connectivity called with:', args);
            const mockConnectivity = (window as unknown as Record<string, unknown>).__MOCK_CONNECTIVITY_RESULT__ as {
              connected: boolean;
              preview: unknown | null;
            } | undefined;
            if (mockConnectivity) {
              return mockConnectivity;
            }
            return { connected: false, preview: null, can_merge: true, reason: null };
          }

          case 'create_loop_closure': {
            console.log('[Mock IPC] create_loop_closure called with:', args);
            const stationA = (args?.stationA ?? args?.station_a) as
              | { section_id: number; station_index: number }
              | undefined;
            const stationB = (args?.stationB ?? args?.station_b) as
              | { section_id: number; station_index: number }
              | undefined;
            const closureStrategy = (args?.closureStrategy ?? args?.closure_strategy) as string | undefined;
            if (!stationA || !stationB) {
              return { success: false, error: 'Missing closure stations' };
            }

            if (currentProject) {
              const loopClosures = currentProject.loop_closures as Array<{
                id: bigint;
                station_a: { section_id: number; station_index: number };
                station_b: { section_id: number; station_index: number };
                path_length: number;
                misclosure_distance: number;
                error_percentage: number;
                horizontal_misclosure: number;
                vertical_misclosure: number;
                strategy: string;
                is_aligned: boolean;
              }>;

              const alreadyExists = loopClosures.some((closure) => {
                const direct =
                  closure.station_a.section_id === stationA.section_id &&
                  closure.station_a.station_index === stationA.station_index &&
                  closure.station_b.section_id === stationB.section_id &&
                  closure.station_b.station_index === stationB.station_index;
                const reverse =
                  closure.station_a.section_id === stationB.section_id &&
                  closure.station_a.station_index === stationB.station_index &&
                  closure.station_b.section_id === stationA.section_id &&
                  closure.station_b.station_index === stationA.station_index;
                return direct || reverse;
              });
              if (alreadyExists) {
                return { success: false, error: 'A loop closure already exists between these stations.' };
              }

              const maxExistingId = loopClosures.reduce(
                (maxId, closure) => (closure.id > maxId ? closure.id : maxId),
                0n
              );
              const nextId = maxExistingId + 1n;

              loopClosures.push({
                id: nextId,
                station_a: stationA,
                station_b: stationB,
                path_length: 0,
                misclosure_distance: 0,
                error_percentage: 0,
                horizontal_misclosure: 0,
                vertical_misclosure: 0,
                strategy: closureStrategy ?? 'Batch',
                is_aligned: false,
              });
            }

            return { success: true, section_ids: [] };
          }

          case 'remove_loop_closure': {
            console.log('[Mock IPC] remove_loop_closure called with:', args);
            const closureIdRaw = args?.closureId ?? args?.closure_id;
            if (closureIdRaw === undefined || closureIdRaw === null) {
              return { success: false, error: 'Missing closure ID' };
            }
            const closureId =
              typeof closureIdRaw === 'bigint'
                ? closureIdRaw
                : typeof closureIdRaw === 'number'
                  ? BigInt(closureIdRaw)
                  : typeof closureIdRaw === 'string'
                    ? BigInt(closureIdRaw)
                    : null;
            if (closureId === null) {
              return { success: false, error: 'Invalid closure ID' };
            }

            if (!currentProject) {
              return { success: false, error: 'No project open' };
            }

            const loopClosures = currentProject.loop_closures as Array<{ id: bigint }>;
            const closureIndex = loopClosures.findIndex((closure) => closure.id === closureId);
            if (closureIndex < 0) {
              return { success: false, error: 'Loop closure not found' };
            }

            loopClosures.splice(closureIndex, 1);
            return { success: true, section_ids: [] };
          }

          case 'unmerge_stations':
            // Mock unmerge stations - return success
            console.log('[Mock IPC] unmerge_stations called with:', args);
            if (currentProject && args) {
              const stationRef = (args.stationRef ?? args.station_ref) as { section_id: number; station_index: number } | undefined;
              if (stationRef) {
                const stationMerges = currentProject.station_merges as Array<{
                  station_a: { section_id: number; station_index: number };
                  station_b: { section_id: number; station_index: number };
                }>;

                const removedPairs: Array<{
                  station_a: { section_id: number; station_index: number };
                  station_b: { section_id: number; station_index: number };
                }> = [];

                for (let i = stationMerges.length - 1; i >= 0; i -= 1) {
                  const merge = stationMerges[i]!;
                  const touchesStation =
                    (merge.station_a.section_id === stationRef.section_id && merge.station_a.station_index === stationRef.station_index) ||
                    (merge.station_b.section_id === stationRef.section_id && merge.station_b.station_index === stationRef.station_index);
                  if (touchesStation) {
                    removedPairs.push(merge);
                    stationMerges.splice(i, 1);
                  }
                }

                if (removedPairs.length > 0 && Array.isArray(currentProject.loop_closures)) {
                  const loopClosures = currentProject.loop_closures as Array<Record<string, unknown>>;
                  currentProject.loop_closures = loopClosures.filter((closure) => {
                    const stationA = closure.station_a as { section_id: number; station_index: number } | undefined;
                    const stationB = closure.station_b as { section_id: number; station_index: number } | undefined;
                    if (!stationA || !stationB) {
                      return true;
                    }

                    const matchesRemovedPair = removedPairs.some((pair) => {
                      const direct =
                        pair.station_a.section_id === stationA.section_id &&
                        pair.station_a.station_index === stationA.station_index &&
                        pair.station_b.section_id === stationB.section_id &&
                        pair.station_b.station_index === stationB.station_index;
                      const reverse =
                        pair.station_b.section_id === stationA.section_id &&
                        pair.station_b.station_index === stationA.station_index &&
                        pair.station_a.section_id === stationB.section_id &&
                        pair.station_a.station_index === stationB.station_index;
                      return direct || reverse;
                    });

                    return !matchesRemovedPair;
                  });
                }
              }
            }
            return { success: true, section_ids: [] };

          case 'unmerge_station_pair': {
            console.log('[Mock IPC] unmerge_station_pair called with:', args);
            const stationA = (args?.stationA ?? args?.station_a) as { section_id: number; station_index: number } | undefined;
            const stationB = (args?.stationB ?? args?.station_b) as { section_id: number; station_index: number } | undefined;
            if (!stationA || !stationB) {
              return { success: false, error: 'Missing station pair' };
            }

            unmergeStationPairCalls.push({ stationA, stationB });

            if (currentProject) {
              const stationMerges = currentProject.station_merges as Array<{
                station_a: { section_id: number; station_index: number };
                station_b: { section_id: number; station_index: number };
              }>;

              const matchIndex = stationMerges.findIndex((merge) => {
                const direct =
                  merge.station_a.section_id === stationA.section_id &&
                  merge.station_a.station_index === stationA.station_index &&
                  merge.station_b.section_id === stationB.section_id &&
                  merge.station_b.station_index === stationB.station_index;
                const reverse =
                  merge.station_b.section_id === stationA.section_id &&
                  merge.station_b.station_index === stationA.station_index &&
                  merge.station_a.section_id === stationB.section_id &&
                  merge.station_a.station_index === stationB.station_index;
                return direct || reverse;
              });

              if (matchIndex >= 0) {
                stationMerges.splice(matchIndex, 1);

                if (Array.isArray(currentProject.loop_closures)) {
                  const loopClosures = currentProject.loop_closures as Array<Record<string, unknown>>;
                  currentProject.loop_closures = loopClosures.filter((closure) => {
                    const closureA = closure.station_a as { section_id: number; station_index: number } | undefined;
                    const closureB = closure.station_b as { section_id: number; station_index: number } | undefined;
                    if (!closureA || !closureB) {
                      return true;
                    }
                    const direct =
                      closureA.section_id === stationA.section_id &&
                      closureA.station_index === stationA.station_index &&
                      closureB.section_id === stationB.section_id &&
                      closureB.station_index === stationB.station_index;
                    const reverse =
                      closureA.section_id === stationB.section_id &&
                      closureA.station_index === stationB.station_index &&
                      closureB.section_id === stationA.section_id &&
                      closureB.station_index === stationA.station_index;
                    return !(direct || reverse);
                  });
                }
                return { success: true, section_ids: [] };
              }
            }

            return { success: false, error: 'Stations are not directly merged' };
          }

          case 'move_segment': {
            // Record the dispatched offset for commit-assertion tests. These handlers
            // don't mutate currentProject (refreshProjectState refetches generated
            // coords), so tests assert on the committed offset, not a coord read-back.
            console.log('[Mock IPC] move_segment called with:', args);
            const win = window as unknown as Record<string, unknown>;
            const calls = (win.__MOCK_MOVE_CALLS__ as unknown[]) ?? [];
            calls.push({ command: 'move_segment', args: args ?? null });
            win.__MOCK_MOVE_CALLS__ = calls;
            return { success: true, section_ids: [] };
          }

          case 'move_section': {
            // Record the dispatched offset (mirrors move_segment/move_station) so
            // commit-assertion tests can check the offset that reached the backend.
            console.log('[Mock IPC] move_section called with:', args);
            const win = window as unknown as Record<string, unknown>;
            const calls = (win.__MOCK_MOVE_CALLS__ as unknown[]) ?? [];
            calls.push({ command: 'move_section', args: args ?? null });
            win.__MOCK_MOVE_CALLS__ = calls;
            return { success: true, section_ids: [] };
          }

          case 'move_station': {
            console.log('[Mock IPC] move_station called with:', args);
            const win = window as unknown as Record<string, unknown>;
            const calls = (win.__MOCK_MOVE_CALLS__ as unknown[]) ?? [];
            calls.push({ command: 'move_station', args: args ?? null });
            win.__MOCK_MOVE_CALLS__ = calls;
            return { success: true };
          }

          case 'move_shot':
            // Mock move shot - just return success
            console.log('[Mock IPC] move_shot called with:', args);
            return { success: true };

          case 'get_anchored_section_ids':
            // Return section IDs that have zero datums
            console.log('[Mock IPC] get_anchored_section_ids called');
            if (currentProject) {
              const datums = (currentProject.zero_datums ?? []) as Array<{
                station_ref: { section_id: number; station_index: number };
              }>;
              const anchoredIds = [...new Set(datums.map(d => d.station_ref.section_id))];
              anchoredIds.sort((a, b) => a - b);
              return anchoredIds;
            }
            return [];

          case 'set_zero_datum': {
            console.log('[Mock IPC] set_zero_datum called with:', args);
            // Update currentProject's zero_datums to reflect the change
            if (currentProject && args) {
              const stationRef = args.stationRef as { section_id: number; station_index: number };
              const coordinates = args.coordinates as { latitude: number; longitude: number; depth_reference: number };
              const datums = currentProject.zero_datums as Array<{
                station_ref: { section_id: number; station_index: number };
                gps: { latitude: number; longitude: number; depth_reference: number };
              }>;
              // Update in-place if exists, otherwise push
              const existing = datums.find(
                (d) =>
                  d.station_ref.section_id === stationRef.section_id &&
                  d.station_ref.station_index === stationRef.station_index
              );
              if (existing) {
                existing.gps = coordinates;
              } else {
                // In-place mutation (same array identity) is load-bearing for the
                // deep-clone reactivity guard: datum-closure-reactivity.spec.ts only
                // catches a shallow-clone regression because the fresh array identity
                // comes from get_project_state's structuredClone, NOT from this write.
                // If this is ever changed to fresh-copy zero_datums, update that spec.
                datums.push({ station_ref: stationRef, gps: coordinates });
              }
            }
            return { success: true, section_ids: [] };
          }

          case 'remove_zero_datum': {
            console.log('[Mock IPC] remove_zero_datum called with:', args);
            // Remove datum from currentProject's zero_datums
            if (currentProject && args) {
              const stationRef = args.stationRef as { section_id: number; station_index: number };
              const datums = currentProject.zero_datums as Array<{
                station_ref: { section_id: number; station_index: number };
              }>;
              const idx = datums.findIndex(
                (d) =>
                  d.station_ref.section_id === stationRef.section_id &&
                  d.station_ref.station_index === stationRef.station_index
              );
              if (idx >= 0) {
                datums.splice(idx, 1);
              }
            }
            return { success: true, section_ids: [] };
          }

          case 'set_station_comment': {
            console.log('[Mock IPC] set_station_comment called with:', args);
            const stationRef = args?.stationRef as { section_id: number; station_index: number };
            const comment = (args?.comment as string | null | undefined) ?? null;
            const key = `${stationRef.section_id}:${stationRef.station_index}`;
            if (comment === null) {
              delete mockStationComments[key];
            } else {
              mockStationComments[key] = comment;
            }
            return { success: true, section_ids: [] };
          }

          case 'check_section_hashes': {
            const hashes = (Array.isArray(args?.hashes) ? args?.hashes : []) as string[];
            return hashes.map(() => false);
          }

          case 'suggest_import_placement':
            return [];

          case 'suggest_all_import_placements':
            return [...mockImportPlacementSuggestions];

          case 'suggest_orphan_placements':
            return [...mockOrphanPlacementSuggestions];

          case 'count_unanchored_sections':
            return 0;

          case 'get_segment_geometry':
            return {
              sections: [
                {
                  section_id: (args?.sectionId as number | undefined) ?? 1,
                  color: '#4a90d9',
                  shot_lines: [[[0, 0], [10, 0]]],
                },
              ],
            };

          case 'undo':
          case 'redo': {
            undoRedoCalls.push(cmd);
            const from = cmd === 'undo' ? mockCreditUndo : mockCreditRedo;
            const to = cmd === 'undo' ? mockCreditRedo : mockCreditUndo;
            const step = from.pop();
            if (step) {
              for (const entry of step) mockRestoreCredit(entry.shot, cmd === 'undo' ? entry.before : entry.after);
              to.push(step);
            }
            return { success: true, section_ids: [] };
          }

          case 'can_undo':
            return mockCreditUndo.length > 0;

          case 'can_redo':
            return mockCreditRedo.length > 0;

          case 'get_undo_state':
            return { can_undo: false, can_redo: false };

          case 'get_config':
            // Return saved config if available (for testing persistence),
            // overlaying any view state stored via update_data_table_view and any
            // `[gas]` block stored via update_gas_config.
            if (savedConfig) {
              return {
                ...savedConfig,
                panels: {
                  ...((savedConfig.panels as Record<string, unknown> | undefined) ?? {}),
                  ...mockDataTableView,
                  ...mockNewSectionView,
                  ...mockEditSectionView,
                  ...mockImportShotsView,
                ...mockImportShotsView,
                },
                ...(mockGasConfig ? { gas: mockGasConfig } : {}),
                ...(mockMapPalette ? { map_palette: mockMapPalette } : {}),
                people: { recent: mockRecentPeople() },
              };
            }
            return {
              general: {
                auto_save: true,
                auto_save_interval_seconds: 30,
                recent_projects_max: 20,
                recent_projects: [],
                undo_history_limit: 100,
                undo_persist_limit: 20,
                shortcut_feedback: 'non_obvious',
                locale: 'system',
                number_format: 'locale',
                date_format: 'locale',
                splash_screen: false,
                // The real defaults (#631: the opt-out gates the background
                // download, so leaving it unset silently reads as "off").
                update_check_enabled: true,
                update_track: 'stable',
                // Keep onboarding disabled in E2E to avoid overlay interception.
                onboarding_enabled: false,
                onboarding_completed_flows: [],
              },
              units: {
                length: 'meters',
                depth: 'meters',
                angle: 'degrees',
                decimal_precision: { length: 1, coordinates: 2, depth: 1, azimuth: 1, inclination: 1, gps: 6 },
              },
              gas: {
                units: 'metric',
                default_reserve: 'all_thirds',
                reserve_profile: {
                  stage_reserve_bar: 30,
                  all_thirds_safety_margin_bar: 0,
                  recalc_hold_stage2_bar: 100,
                  recalc_hold_stage1_bar: 70,
                },
                swim_speed_m_per_min: 10,
                survey_speed_m_per_min: null,
                survey_rmv_multiplier: 1,
                stressed_rmv_multiplier: 1.5,
                jump_rmv_multiplier: 1.5,
                conservative_tank_gas: true,
                dpv: null,
              },
              display: {
                grid_color: '#333366',
                grid_spacing: 10,
                font_size: 12,
                font_color: '#f5f5f5',
              },
              section_colors: {
                palette: ['#4ecdc4', '#ff6b6b', '#95e1d3', '#f38181', '#aa96da', '#fcbad3', '#a8d8ea', '#ffcb77'],
              },
              map: {
                satellite_provider: 'esri',
                satellite_enabled: false,
                satellite_gps_origin: {
                  latitude: 24.55,
                  longitude: -81.78,
                },
                zoom_default: 15,
                zoom_min: 1,
                zoom_max: 22,
                zoom_step: 1.0,
                controls: {
                  pan_speed: 1.0,
                  zoom_speed: 0.55,
                  invert_scroll: false,
                  show_navigation_tools: mockShowNavigationTools,
                },
                station: {
                  radius: 4.0,
                  hit_area: 10.0,
                  hover_radius: 6.0,
                  selection_radius: 5.0,
                },
                shot: {
                  width: 2.0,
                  hit_area: 8.0,
                  hover_width: 4.0,
                  selection_width: 3.0,
                },
                snap: {
                  threshold: 10.0,
                  enabled_draw: true,
                  enabled_merge: true,
                  enabled_measure: true,
                },
                animation: {
                  enabled: true,
                  duration: 300,
                  point_zoom: 18,
                },
              },
              validation: {
                warn_shot_length_min: 0.1,
                warn_shot_length_max: 50.0,
                warn_depth_change_per_shot_max: 10.0,
                warn_azimuth_deviation_max: 30.0,
                azimuth_diff_threshold: 20.0,
                pitch_diff_threshold: 5.0,
              },
              closure_adjustment: {
                base_sigma_length_m: 0.05,
                base_sigma_azimuth_deg: 1.0,
                base_sigma_depth_m: 0.05,
                transit_vertical_mode: 'ignore',
                sequential_distribution_default: 'ProportionalByLength',
              },
              export: {
                default_image_format: 'png',
                default_dpi: 300,
                default_include_satellite: true,
                default_include_grid: false,
                default_include_labels: true,
              },
              logging: {
                level: 'info',
                max_directory_size_mb: 128,
              },
              shortcuts: {
                // File
                import_dmp: ['Ctrl+I'],
                export: ['Ctrl+E'],
                new_file: ['Ctrl+N'],
                open_file: ['Ctrl+O'],
                save: ['Ctrl+S'],
                save_as: ['Ctrl+Shift+S'],
                // Tools
                select: ['S'],
                pan: ['P'],
                zoom: ['Z'],
                merge: ['M'],
                measure: ['R'],
                move_segment: ['V'],
                move_shot: ['G'],
                move_station: ['T'],
                // Edit
                undo: ['Ctrl+Z'],
                redo: ['Ctrl+Y', 'Ctrl+Shift+Z'],
                // View
                toggle_depth_gradient: ['Shift+D'],
                toggle_grid: ['Shift+G'],
                toggle_satellite: ['B'],
                toggle_station_labels: ['L'],
                toggle_shot_labels: ['Shift+L'],
                toggle_section_labels: ['Ctrl+L'],
                fit_to_bounds: ['F'],
                zoom_in: ['Equal', 'Plus'],
                zoom_out: ['Minus'],
                // Panels (tests use Ctrl+ prefix)
                toggle_sections_panel: ['Ctrl+1'],
                toggle_detail_panel: ['Ctrl+2'],
                toggle_data_table: ['Ctrl+3'],
              },
              panels: {
                sections_panel_width: 250,
                detail_panel_width: 300,
                data_table_height: 200,
                wizard_map_panel_width: 480,
                data_table_shot_column_order: [],
                data_table_shot_hidden_columns: ['section_id', 'shot_id', 'surveyors', 'explorers', 'survey_date'],
                data_table_shot_columns_seen: ['surveyors', 'explorers', 'survey_date'],
                data_table_shot_column_widths: {},
                data_table_azimuth_mode: 'avg',
                data_table_inclination_mode: 'off',
                data_table_station_column_order: [],
                data_table_station_hidden_columns: ['_x', '_y', '_lat', '_long', 'name'],
                data_table_station_columns_seen: ['name'],
                data_table_station_column_widths: {},
                // The new-section dialog's grid layout. Present (not omitted) for
                // the same reason as the reveal state below: get_config on a real
                // backend always serializes these, and the dialog treats an ABSENT
                // hidden list as "never stored, use the built-in defaults" but a
                // present empty one as "the user unhid everything".
                new_section_column_order: [],
                new_section_hidden_columns: [
                  'azimuth_in',
                  'azimuth_out',
                  '_pitch_avg',
                  'pitch_in',
                  'pitch_out',
                  'left',
                  'right',
                  'up',
                  'down',
                ],
                new_section_column_widths: {},
                edit_section_column_order: [],
                edit_section_hidden_columns: [
                  'azimuth_in',
                  'azimuth_out',
                  '_pitch_avg',
                  'pitch_in',
                  'pitch_out',
                ],
                import_shots_column_order: [],
                import_shots_hidden_columns: [
                  'azimuth_in',
                  'azimuth_out',
                  '_pitch_avg',
                  'pitch_in',
                  'pitch_out',
                ],
                import_shots_column_widths: {},
                edit_section_column_widths: {},
                // Concrete reveal state so initPanels never falls back to its
                // defaults. Deliberately NOT the shipped default any more: a fresh
                // install opens the data table, but most specs want the map
                // unobstructed, so the fixture keeps it closed and the specs that
                // care about it open it themselves. The shipped default is pinned
                // in Rust (config::tests) and in uiStore.initPanels.test.ts.
                sections_panel_open: true,
                sections_panel_mode: 'pin',
                detail_panel_open: true,
                detail_panel_mode: 'pin',
                data_table_open: false,
                data_table_mode: 'pin',
                ...mockDataTableView,
                ...mockNewSectionView,
                ...mockEditSectionView,
                ...mockImportShotsView,
              },
              cache: {
                tile_cache_enabled: true,
                tile_cache_max_mb: 500,
                tile_ttl_days: 30,
              },
              debug: {
                map: {
                  show_hitboxes: false,
                  log_hit_detection: false,
                },
              },
              global_view: {
                fps_threshold: 30,
                tolerance: 0.5,
                throttle_tolerance: 1.0,
              },
              appearance: {
                theme: 'dark',
                day_color_theme: 'light',
                night_color_theme: 'cave',
              },
              syntactic_highlighting: {
                level: 'full',
                preset: 'default',
                overrides: {},
              },
              // The real get_config always sends these sections (Rust Config::default,
              // mirrored in src/types/ipc/defaultConfig.ts). Without them, Settings
              // fills them in on the first edit and reports them as changed.
              import: {
                overlap_mode: 'import_data',
              },
              placement_suggestion: {
                azimuth_sigma: 18.0,
                length_sigma: 0.8,
                depth_sigma: 0.35,
                azimuth_weight: 0.35,
                length_weight: 0.25,
                depth_in_weight: 0.2,
                depth_out_weight: 0.2,
                min_shot_score: 0.15,
                min_aggregate_score: 0.45,
                max_reference_shots: 5,
                max_suggestions: 10,
                min_partial_ratio: 0.15,
                short_shot_threshold: 0.5,
                single_shot_penalty: 0.6,
                max_azimuth_error_full: 25.0,
                max_azimuth_error_partial: 35.0,
                max_depth_error: 0.3,
                max_length_error_fraction: 0.03,
                max_length_error_floor: 0.4,
              },
              // SpeleoDB config block: the real get_config always returns this
              // (Rust SpeleoDbConfig::default); Settings reads it directly.
              speleodb: {
                endpoint: null,
                email: null,
                insecure_http: false,
              },
              // Spread LAST so a stored `[gas]` block wins over the literal's own.
              ...(mockGasConfig ? { gas: mockGasConfig } : {}),
              ...(mockMapPalette ? { map_palette: mockMapPalette } : {}),
              people: { recent: mockRecentPeople() },
            };

          case 'update_config':
            console.log('[Mock IPC] update_config called with:', args);
            savedConfig = args?.config as Record<string, unknown> | undefined ?? null;
            return null;

          case 'update_gas_config': {
            // Partial update of the `[gas]` block only, mirroring the Rust impl.
            // Overlaid in get_config the same way mockDataTableView is, rather
            // than written into savedConfig: savedConfig is null until something
            // posts a whole Config, and assigning a lone `{ gas }` to it would
            // make get_config return a config with no general/display/panels at
            // all. Keeping the two separate is also what lets a test assert that
            // a gas write left the OTHER fields untouched (#357 item 2).
            console.log('[Mock IPC] update_gas_config called with:', args);
            const gas = args?.gas as Record<string, unknown> | undefined;
            if (gas) mockGasConfig = gas;
            return null;
          }

          case 'remember_people': {
            // Partial `[people].recent` write, mirroring `PeopleConfig::remember`:
            // the given people move to the front in order (a known identity takes
            // the new spelling), then dedupe + cap. Empty input writes nothing.
            console.log('[Mock IPC] remember_people called with:', args);
            const rawPeople = args?.people;
            const given = Array.isArray(rawPeople) ? (rawPeople as MockPerson[]) : [];
            const fresh = mockDedupe(
              given
                .filter((p) => typeof p?.name === 'string')
                .map((p) => {
                  const email = typeof p.email === 'string' ? p.email.trim() : '';
                  return email ? { name: p.name.trim(), email } : { name: p.name.trim() };
                })
                .filter((p) => p.name !== ''),
            );
            if (fresh.length > 0) {
              const freshKeys = new Set(fresh.map(mockPersonKey));
              mockRecentPeopleWritten = mockDedupe([
                ...fresh,
                ...mockRecentPeople().filter((p) => !freshKeys.has(mockPersonKey(p))),
              ]).slice(0, MOCK_MAX_RECENT_PEOPLE);
            }
            const win = window as unknown as Record<string, unknown>;
            const calls = (win.__MOCK_REMEMBER_PEOPLE_CALLS__ as unknown[] | undefined) ?? [];
            calls.push(given);
            win.__MOCK_REMEMBER_PEOPLE_CALLS__ = calls;
            return mockRecentPeople();
          }

          case 'update_map_palette': {
            // Partial update of `[map_palette]` only, mirroring the Rust impl;
            // overlaid in get_config like update_gas_config (see there).
            console.log('[Mock IPC] update_map_palette called with:', args);
            const palette = args?.palette as Record<string, unknown> | undefined;
            if (palette) mockMapPalette = palette;
            const win = window as unknown as Record<string, unknown>;
            const calls = (win.__MOCK_MAP_PALETTE_CALLS__ as unknown[] | undefined) ?? [];
            calls.push(palette ?? null);
            win.__MOCK_MAP_PALETTE_CALLS__ = calls;
            return null;
          }

          case 'update_data_table_view': {
            console.log('[Mock IPC] update_data_table_view called with:', args);
            // Record calls for assertions (debounced view-persistence payloads).
            const win = window as unknown as Record<string, unknown>;
            const existing = win.__MOCK_DATA_TABLE_VIEW_CALLS__ as unknown[] | undefined;
            const calls = Array.isArray(existing) ? existing : [];
            calls.push(args ?? null);
            win.__MOCK_DATA_TABLE_VIEW_CALLS__ = calls;
            // Mirror the Rust impl's partial-update semantics: only provided
            // fields overwrite the stored view state (merged into get_config's
            // panels above).
            const columnOrder = (args?.columnOrder ?? args?.column_order) as string[] | undefined;
            if (columnOrder != null) mockDataTableView.data_table_shot_column_order = columnOrder;
            const hiddenColumns = (args?.hiddenColumns ?? args?.hidden_columns) as string[] | undefined;
            if (hiddenColumns != null) mockDataTableView.data_table_shot_hidden_columns = hiddenColumns;
            const azimuthMode = (args?.azimuthMode ?? args?.azimuth_mode) as string | undefined;
            if (azimuthMode != null) mockDataTableView.data_table_azimuth_mode = azimuthMode;
            const inclinationMode = (args?.inclinationMode ?? args?.inclination_mode) as string | undefined;
            if (inclinationMode != null) mockDataTableView.data_table_inclination_mode = inclinationMode;
            const stationColumnOrder = (args?.stationColumnOrder ?? args?.station_column_order) as string[] | undefined;
            if (stationColumnOrder != null) mockDataTableView.data_table_station_column_order = stationColumnOrder;
            const stationHiddenColumns = (args?.stationHiddenColumns ?? args?.station_hidden_columns) as string[] | undefined;
            if (stationHiddenColumns != null) mockDataTableView.data_table_station_hidden_columns = stationHiddenColumns;
            const columnWidths = (args?.columnWidths ?? args?.column_widths) as
              | Record<string, number>
              | undefined;
            if (columnWidths != null) mockDataTableView.data_table_shot_column_widths = columnWidths;
            const stationColumnWidths = (args?.stationColumnWidths ?? args?.station_column_widths) as
              | Record<string, number>
              | undefined;
            if (stationColumnWidths != null) {
              mockDataTableView.data_table_station_column_widths = stationColumnWidths;
            }
            return null; // Tauri serializes () as null
          }

          case 'update_new_section_view': {
            console.log('[Mock IPC] update_new_section_view called with:', args);
            // Record calls for assertions, same shape as the data-table one.
            const win = window as unknown as Record<string, unknown>;
            const existing = win.__MOCK_NEW_SECTION_VIEW_CALLS__ as unknown[] | undefined;
            const calls = Array.isArray(existing) ? existing : [];
            calls.push(args ?? null);
            win.__MOCK_NEW_SECTION_VIEW_CALLS__ = calls;
            // Partial-update semantics, mirroring update_new_section_view_impl:
            // only provided fields overwrite the stored layout.
            const columnOrder = (args?.columnOrder ?? args?.column_order) as string[] | undefined;
            if (columnOrder != null) mockNewSectionView.new_section_column_order = columnOrder;
            const hiddenColumns = (args?.hiddenColumns ?? args?.hidden_columns) as
              | string[]
              | undefined;
            if (hiddenColumns != null) mockNewSectionView.new_section_hidden_columns = hiddenColumns;
            const columnWidths = (args?.columnWidths ?? args?.column_widths) as
              | Record<string, number>
              | undefined;
            if (columnWidths != null) mockNewSectionView.new_section_column_widths = columnWidths;
            return null; // Tauri serializes () as null
          }

          case 'update_edit_section_view': {
            console.log('[Mock IPC] update_edit_section_view called with:', args);
            const win = window as unknown as Record<string, unknown>;
            const existing = win.__MOCK_EDIT_SECTION_VIEW_CALLS__ as unknown[] | undefined;
            const calls = Array.isArray(existing) ? existing : [];
            calls.push(args ?? null);
            win.__MOCK_EDIT_SECTION_VIEW_CALLS__ = calls;
            const columnOrder = (args?.columnOrder ?? args?.column_order) as string[] | undefined;
            if (columnOrder != null) mockEditSectionView.edit_section_column_order = columnOrder;
            const hiddenColumns = (args?.hiddenColumns ?? args?.hidden_columns) as
              | string[]
              | undefined;
            if (hiddenColumns != null) {
              mockEditSectionView.edit_section_hidden_columns = hiddenColumns;
            }
            const columnWidths = (args?.columnWidths ?? args?.column_widths) as
              | Record<string, number>
              | undefined;
            if (columnWidths != null) mockEditSectionView.edit_section_column_widths = columnWidths;
            return null; // Tauri serializes () as null
          }
          case 'update_import_shots_view': {
            console.log('[Mock IPC] update_import_shots_view called with:', args);
            const win = window as unknown as Record<string, unknown>;
            // Its OWN call list. Recording import saves under the edit-section key
            // made an import-grid save indistinguishable from an edit-section one,
            // so no spec could assert that either slice wrote only its own fields -
            // which is the whole point of the separate `import_shots_*` config.
            const existing = win.__MOCK_IMPORT_SHOTS_VIEW_CALLS__ as unknown[] | undefined;
            const calls = Array.isArray(existing) ? existing : [];
            calls.push(args ?? null);
            win.__MOCK_IMPORT_SHOTS_VIEW_CALLS__ = calls;
            const columnOrder = (args?.columnOrder ?? args?.column_order) as string[] | undefined;
            if (columnOrder != null) mockImportShotsView.import_shots_column_order = columnOrder;
            const hiddenColumns = (args?.hiddenColumns ?? args?.hidden_columns) as
              | string[]
              | undefined;
            if (hiddenColumns != null) {
              mockImportShotsView.import_shots_hidden_columns = hiddenColumns;
            }
            const columnWidths = (args?.columnWidths ?? args?.column_widths) as
              | Record<string, number>
              | undefined;
            if (columnWidths != null) mockImportShotsView.import_shots_column_widths = columnWidths;
            return null; // Tauri serializes () as null
          }

          case 'reload_config': {
            // Return saved config if available (for testing persistence),
            // overlaying the same two stores get_config does. Without the
            // overlay, Settings > Advanced "Reload from disk" handed back a
            // config with neither the view state stored via
            // update_data_table_view nor the `[gas]` block stored via
            // update_gas_config - so a reload discarded state the mock
            // otherwise treats as persisted, and a reload-after-save test would
            // have asserted against the wrong baseline.
            if (savedConfig) {
              return [{
                ...savedConfig,
                panels: {
                  ...((savedConfig.panels as Record<string, unknown> | undefined) ?? {}),
                  ...mockDataTableView,
                  ...mockNewSectionView,
                  ...mockEditSectionView,
                  ...mockImportShotsView,
                ...mockImportShotsView,
                },
                ...(mockGasConfig ? { gas: mockGasConfig } : {}),
                ...(mockMapPalette ? { map_palette: mockMapPalette } : {}),
                people: { recent: mockRecentPeople() },
              }, false];
            }
            // Return mock config with any test overrides applied
            // This allows tests to modify config values via __MOCK_SET_CONFIG_OVERRIDES__
            const baseConfig = {
              general: {
                auto_save: true,
                auto_save_interval_seconds: 30,
                recent_projects_max: 20,
                recent_projects: [],
                undo_history_limit: 100,
                undo_persist_limit: 20,
                shortcut_feedback: 'non_obvious',
                locale: 'system',
                number_format: 'locale',
                date_format: 'locale',
                splash_screen: false,
                // The real defaults (#631: the opt-out gates the background
                // download, so leaving it unset silently reads as "off").
                update_check_enabled: true,
                update_track: 'stable',
                // Keep onboarding disabled in E2E to avoid overlay interception.
                onboarding_enabled: false,
                onboarding_completed_flows: [],
              },
              units: {
                length: 'meters',
                depth: 'meters',
                angle: 'degrees',
                decimal_precision: { length: 1, coordinates: 2, depth: 1, azimuth: 1, inclination: 1, gps: 6 },
              },
              gas: {
                units: 'metric',
                default_reserve: 'all_thirds',
                reserve_profile: {
                  stage_reserve_bar: 30,
                  all_thirds_safety_margin_bar: 0,
                  recalc_hold_stage2_bar: 100,
                  recalc_hold_stage1_bar: 70,
                },
                swim_speed_m_per_min: 10,
                survey_speed_m_per_min: null,
                survey_rmv_multiplier: 1,
                stressed_rmv_multiplier: 1.5,
                jump_rmv_multiplier: 1.5,
                conservative_tank_gas: true,
                dpv: null,
              },
              display: {
                grid_color: '#333366',
                grid_spacing: 10,
                font_size: 12,
                font_color: '#f5f5f5',
              },
              section_colors: {
                palette: ['#4ecdc4', '#ff6b6b', '#95e1d3', '#f38181', '#aa96da', '#fcbad3', '#a8d8ea', '#ffcb77'],
              },
              map: {
                satellite_provider: 'esri',
                satellite_enabled: false,
                satellite_gps_origin: {
                  latitude: 24.55,
                  longitude: -81.78,
                },
                zoom_default: 15,
                zoom_min: 1,
                zoom_max: 22,
                zoom_step: 1.0,
                controls: {
                  pan_speed: 1.0,
                  zoom_speed: 0.55,
                  invert_scroll: false,
                  show_navigation_tools: mockShowNavigationTools,
                },
                station: {
                  radius: 4.0,
                  hit_area: mockConfigOverrides.map?.station?.hit_area ?? 10.0,
                  hover_radius: 6.0,
                  selection_radius: 5.0,
                },
                shot: {
                  width: 2.0,
                  hit_area: mockConfigOverrides.map?.shot?.hit_area ?? 8.0,
                  hover_width: 4.0,
                  selection_width: 3.0,
                },
                snap: {
                  threshold: 10.0,
                  enabled_draw: true,
                  enabled_merge: true,
                  enabled_measure: true,
                },
                animation: {
                  enabled: mockConfigOverrides.map?.animation?.enabled ?? true,
                  duration: mockConfigOverrides.map?.animation?.duration ?? 300,
                  point_zoom: mockConfigOverrides.map?.animation?.point_zoom ?? 18,
                },
              },
              validation: {
                warn_shot_length_min: 0.1,
                warn_shot_length_max: 50.0,
                warn_depth_change_per_shot_max: 10.0,
                warn_azimuth_deviation_max: 30.0,
                azimuth_diff_threshold: 20.0,
                pitch_diff_threshold: 5.0,
              },
              closure_adjustment: {
                base_sigma_length_m: 0.05,
                base_sigma_azimuth_deg: 1.0,
                base_sigma_depth_m: 0.05,
                transit_vertical_mode: 'ignore',
                sequential_distribution_default: 'ProportionalByLength',
              },
              export: {
                default_image_format: 'png',
                default_dpi: 300,
                default_include_satellite: true,
                default_include_grid: false,
                default_include_labels: true,
              },
              logging: {
                level: 'info',
                max_directory_size_mb: 128,
              },
              shortcuts: {
                // File
                import_dmp: ['Ctrl+I'],
                export: ['Ctrl+E'],
                new_file: ['Ctrl+N'],
                open_file: ['Ctrl+O'],
                save: ['Ctrl+S'],
                save_as: ['Ctrl+Shift+S'],
                // Tools
                select: ['S'],
                pan: ['P'],
                zoom: ['Z'],
                merge: ['M'],
                measure: ['R'],
                move_segment: ['V'],
                move_shot: ['G'],
                move_station: ['T'],
                // Edit
                undo: ['Ctrl+Z'],
                redo: ['Ctrl+Y', 'Ctrl+Shift+Z'],
                // View
                toggle_depth_gradient: ['Shift+D'],
                toggle_grid: ['Shift+G'],
                toggle_satellite: ['B'],
                toggle_station_labels: ['L'],
                toggle_shot_labels: ['Shift+L'],
                toggle_section_labels: ['Ctrl+L'],
                fit_to_bounds: ['F'],
                zoom_in: ['Equal', 'Plus'],
                zoom_out: ['Minus'],
                // Panels (tests use Ctrl+ prefix)
                toggle_sections_panel: ['Ctrl+1'],
                toggle_detail_panel: ['Ctrl+2'],
                toggle_data_table: ['Ctrl+3'],
              },
              panels: {
                sections_panel_width: 250,
                detail_panel_width: 300,
                data_table_height: 200,
                wizard_map_panel_width: 480,
                data_table_shot_column_order: [],
                data_table_shot_hidden_columns: ['section_id', 'shot_id', 'surveyors', 'explorers', 'survey_date'],
                data_table_shot_columns_seen: ['surveyors', 'explorers', 'survey_date'],
                data_table_shot_column_widths: {},
                data_table_azimuth_mode: 'avg',
                data_table_inclination_mode: 'off',
                data_table_station_column_order: [],
                data_table_station_hidden_columns: ['_x', '_y', '_lat', '_long', 'name'],
                data_table_station_columns_seen: ['name'],
                data_table_station_column_widths: {},
                // The new-section dialog's grid layout. Present (not omitted) for
                // the same reason as the reveal state below: get_config on a real
                // backend always serializes these, and the dialog treats an ABSENT
                // hidden list as "never stored, use the built-in defaults" but a
                // present empty one as "the user unhid everything".
                new_section_column_order: [],
                new_section_hidden_columns: [
                  'azimuth_in',
                  'azimuth_out',
                  '_pitch_avg',
                  'pitch_in',
                  'pitch_out',
                  'left',
                  'right',
                  'up',
                  'down',
                ],
                new_section_column_widths: {},
                edit_section_column_order: [],
                edit_section_hidden_columns: [
                  'azimuth_in',
                  'azimuth_out',
                  '_pitch_avg',
                  'pitch_in',
                  'pitch_out',
                ],
                import_shots_column_order: [],
                import_shots_hidden_columns: [
                  'azimuth_in',
                  'azimuth_out',
                  '_pitch_avg',
                  'pitch_in',
                  'pitch_out',
                ],
                import_shots_column_widths: {},
                edit_section_column_widths: {},
                // Concrete reveal state so initPanels never falls back to its
                // defaults. Deliberately NOT the shipped default any more: a fresh
                // install opens the data table, but most specs want the map
                // unobstructed, so the fixture keeps it closed and the specs that
                // care about it open it themselves. The shipped default is pinned
                // in Rust (config::tests) and in uiStore.initPanels.test.ts.
                sections_panel_open: true,
                sections_panel_mode: 'pin',
                detail_panel_open: true,
                detail_panel_mode: 'pin',
                data_table_open: false,
                data_table_mode: 'pin',
                ...mockDataTableView,
                ...mockNewSectionView,
                ...mockEditSectionView,
                ...mockImportShotsView,
              },
              cache: {
                tile_cache_enabled: true,
                tile_cache_max_mb: 500,
                tile_ttl_days: 30,
              },
              debug: {
                map: {
                  show_hitboxes: mockConfigOverrides.debug?.map?.show_hitboxes ?? false,
                  log_hit_detection: mockConfigOverrides.debug?.map?.log_hit_detection ?? false,
                },
              },
              global_view: {
                fps_threshold: 30,
                tolerance: 0.5,
                throttle_tolerance: 1.0,
              },
              appearance: {
                theme: 'dark',
                day_color_theme: 'light',
                night_color_theme: 'cave',
              },
              syntactic_highlighting: {
                level: 'full',
                preset: 'default',
                overrides: {},
              },
              // Spread LAST so a stored `[gas]` block wins over the literal's
              // own, exactly as in get_config. (`panels` already picks up
              // mockDataTableView above.)
              ...(mockGasConfig ? { gas: mockGasConfig } : {}),
              ...(mockMapPalette ? { map_palette: mockMapPalette } : {}),
              people: { recent: mockRecentPeople() },
            };
            return [baseConfig, false];
          }

          case 'get_recent_projects':
          case 'list_recent_projects':
            return [
              {
                name: 'Cave System Alpha',
                location: 'Yucatan, Mexico',
                file_path: '/projects/cave-alpha.stalx',
                modified_at: new Date().toISOString(),
                section_count: 5,
                total_length: 1250.5,
                gps_center: [-87.4631, 20.2137],
                gps_bounds: {
                  north: 20.2200,
                  south: 20.2074,
                  east: -87.4568,
                  west: -87.4694,
                },
              },
              {
                name: 'Underwater Cave Beta',
                location: 'Florida, USA',
                file_path: '/projects/cave-beta.stalx',
                modified_at: new Date(Date.now() - 86400000).toISOString(),
                section_count: 3,
                total_length: 820.0,
                gps_center: [-82.4584, 29.6516],
                gps_bounds: {
                  north: 29.6550,
                  south: 29.6482,
                  east: -82.4521,
                  west: -82.4647,
                },
              },
              {
                name: 'Deep Cave Gamma',
                location: 'Bahamas',
                file_path: '/projects/cave-gamma.stalx',
                modified_at: new Date(Date.now() - 172800000).toISOString(),
                section_count: 8,
                total_length: 2100.3,
                gps_center: [-77.3963, 25.0343],
                gps_bounds: null,
              },
              {
                name: 'Local Survey (No GPS)',
                location: 'Unknown',
                file_path: '/projects/local-survey.stalx',
                modified_at: new Date(Date.now() - 259200000).toISOString(),
                section_count: 2,
                total_length: 350.0,
                gps_center: null,
                gps_bounds: null,
              },
            ].map((p) =>
              recentNameOverrides[p.file_path]
                ? { ...p, name: recentNameOverrides[p.file_path] }
                : p
            );

          case 'get_warnings':
          case 'get_project_warnings':
            return [];

          case 'check_project_integrity':
            return { messages: mockIntegrityIssues, save_blocked: mockIntegritySaveBlocked };

          case 'history_discarded':
            // #473: whether the open project's edit history was unreadable and
            // replaced by an empty stack. A peek, not a take - the real command
            // keeps answering the same thing while the project is open, so the
            // mock must too or a re-render would clear the banner.
            mockHistoryDiscardedCalls += 1;
            return mockHistoryDiscarded;

          case 'repair_project_integrity': {
            const total = mockIntegrityIssues.length;
            const remaining = Math.min(mockIntegrityRemaining, total);
            const fixed = total - remaining;
            // Keep the trailing `remaining` issues as detect-only residue.
            mockIntegrityIssues = mockIntegrityIssues.slice(total - remaining);
            return { fixed, remaining };
          }

          case 'remove_from_recent_projects':
            // Mock remove from recent - just return success
            console.log('[Mock IPC] remove_from_recent_projects called with:', args);
            return { success: true, section_ids: [] };

          case 'delete_project':
            // Mock destructive delete - acknowledge success. We don't mutate the
            // hardcoded recents list here (would leak across tests in a worker);
            // the store + Rust tests cover the recents-removal side effect.
            console.log('[Mock IPC] delete_project called with:', args);
            return { success: true, section_ids: [] };

          case 'set_project_name_for_path': {
            const p = String(args?.path ?? '');
            const name = String(args?.name ?? '').trim();
            if (!name) return { success: false, error: 'Project name cannot be empty' };
            recentNameOverrides[p] = name;
            return { success: true, section_ids: [] };
          }

          // Display settings (per-project) - uses in-memory storage for persistence testing
          case 'load_display_settings':
            // Return current display settings from in-memory storage
            return { ...mockDisplaySettings };

          case 'save_display_setting': {
            // Persist setting to in-memory storage
            const key = args?.key as string;
            const value = args?.value as string;
            if (key && value !== undefined) {
              mockDisplaySettings[key] = value;
              console.log(`[Mock IPC] save_display_setting: ${key}=${value}`);
            }
            return undefined;
          }

          // Window plugin commands
          case 'plugin:window|set_title':
          case 'plugin:window|current':
            return { label: 'main' };

          // The real teardown the exit flow ends in (`useBeforeUnload`'s
          // closeWindowNow -> getCurrentWindow().destroy()). Counted, not just
          // swallowed, so a spec can assert that an UNGUARDED close actually
          // PROCEEDED - "no dialog appeared" alone also holds when the close
          // handler silently did nothing.
          case 'plugin:window|destroy':
            windowDestroyCount += 1;
            return null;

          // `performQuit()` asks for a close; the real backend prevents it and
          // re-emits `tauri://close-requested`, which the app turns into the
          // destroy above. Counted for the same reason.
          case 'plugin:window|close':
            windowCloseCount += 1;
            return null;

          case 'get_stations':
            return [];

          // Plugin commands
          case 'plugin:dialog|open':
            // Count picker opens so a spec can prove the read-only-map guard (#243)
            // blocks Import before the picker even opens.
            dialogOpenCount += 1;
            // Return a mock file path for testing
            return '/mock/test-survey.dmp';
          case 'plugin:dialog|save':
            return '/mock/saved-project.stalx';

          // Path plugin: resolve a base directory to an absolute mock path.
          // Used by documentDir()/downloadDir() in the saveDialog() helper.
          case 'plugin:path|resolve_directory': {
            const baseDirs: Record<number, string> = {
              6: '/mock/documents', // BaseDirectory.Document
              7: '/mock/downloads', // BaseDirectory.Download
            };
            const dir = args?.directory as number | undefined;
            return (dir !== undefined && baseDirs[dir]) || '/mock/home';
          }

          case 'plugin:fs|read':
          case 'plugin:fs|readFile':
            return new Uint8Array([]);

          case 'plugin:fs|write':
          case 'plugin:fs|writeFile':
            return null;

          case 'plugin:fs|exists':
            return false;

          // Opener plugin commands
          case 'plugin:opener|reveal_item_in_dir':
            console.log('[Mock IPC] reveal_item_in_dir called with:', args);
            return undefined;

          case 'plugin:opener|open_path':
            console.log('[Mock IPC] open_path called with:', args);
            return undefined;

          // Export commands
          case 'export_csv':
            console.log('[Mock IPC] export_csv called with:', args);
            return { success: true, path: args?.path };

          case 'export_json':
            console.log('[Mock IPC] export_json called with:', args);
            return { success: true, path: args?.path };

          // Dive-profile export (#299). Records the call so a spec can assert the plan INPUTS
          // reached the backend - the real command recomputes the reach field from them rather
          // than trusting a client-held one, so sending them is the contract under test.
          case 'export_dive_profile': {
            const win = window as unknown as Record<string, unknown>;
            const calls = (win.__MOCK_DIVE_PROFILE_EXPORTS__ as unknown[]) ?? [];
            calls.push({ path: args?.path, format: args?.format, request: args?.request });
            win.__MOCK_DIVE_PROFILE_EXPORTS__ = calls;
            return undefined;
          }

          case 'render_dive_profile_text':
            return [
              'Mock Cave - planned dive profile',
              'Planned penetration profile from Stalagmight. NOT a decompression schedule.',
              '',
              '   Dist(m)   Depth(m)   Runtime      Gas(L)  Leg',
              '       0.0        0.0      0:00           0  in',
            ].join('\n');

          case 'export_value_histogram': {
            // A fixed, plausible histogram so the export panel's Levels slider
            // renders: depth over 0..40 m, distance over 0..250 m.
            const kind = args?.kind as string | undefined;
            const bins = (args?.bins as number | undefined) ?? 64;
            const counts = Array.from({ length: bins }, (_, k) => Math.round(20 * Math.exp(-((k - bins / 3) ** 2) / (bins * 2))));
            return kind === 'distance' ? { min: 0, max: 250, counts } : { min: 0, max: 40, counts };
          }

          case 'preview_svg_export': {
            // Stick-map preview lives in the same backend command pipeline; the
            // mock returns a tiny but valid SVG so the preview component can
            // render without hitting Rust during e2e tests.
            const opts = args?.options as Record<string, unknown> | undefined;
            const threshold = (opts?.branching_threshold as number | undefined) ?? 0;
            // Encode the threshold in a data attribute so tests can verify
            // the preview re-fetches when the slider changes; likewise the
            // colour scales the export would draw (palette head + levels).
            const scaleTag = (sc: unknown): string => {
              const v = sc as { palette?: string[]; levels?: { min: number; max: number; gamma: number } } | undefined;
              if (!v) return 'none';
              return `${v.palette?.[0] ?? 'theme'}:${v.levels ? `${v.levels.min}-${v.levels.max}-${v.levels.gamma}` : 'auto'}`;
            };
            return `<?xml version="1.0" encoding="UTF-8"?>\n<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" data-threshold="${threshold}" data-depth-scale="${scaleTag(opts?.depth_scale)}" data-distance-scale="${scaleTag(opts?.distance_scale)}"><rect width="100" height="100" fill="#ffffff"/><line x1="10" y1="50" x2="90" y2="50" stroke="#000000" stroke-width="1"/></svg>`;
          }

          case 'export_image': {
            console.log('[Mock IPC] export_image called with:', args);
            // Store args for test assertions via __E2E_MOCKS__.getLastExportImageArgs()
            const exportOptions = args?.options as Record<string, unknown> | undefined;

            // Validate the slimmed ExportImageOptions shape (matches the new
            // backend struct): format + dpi + branching_threshold only.
            if (exportOptions) {
              if (exportOptions.format === undefined) {
                throw new Error('[Mock IPC] export_image: missing required option "format"');
              }
              if (exportOptions.dpi === undefined) {
                throw new Error('[Mock IPC] export_image: missing required option "dpi"');
              }
              if (exportOptions.branching_threshold === undefined) {
                throw new Error('[Mock IPC] export_image: missing required option "branching_threshold"');
              }
              const validFormats = ['png', 'svg', 'pdf'];
              if (!validFormats.includes(exportOptions.format as string)) {
                throw new Error(`[Mock IPC] export_image: invalid format "${exportOptions.format}", must be one of: ${validFormats.join(', ')}`);
              }
            }

            lastExportImageArgs = {
              path: args?.path,
              options: exportOptions ? { ...exportOptions } : null,
            };
            console.log('[Mock IPC] export_image options:', JSON.stringify(exportOptions));
            // Generate a unique loadingId for this export (matches ProgressEmitter behavior)
            const exportLoadingId = Date.now();
            // Emit loading-progress events using shared infrastructure format
            // Use longer delays to allow tests to observe progress states
            setTimeout(() => {
              window.__TAURI_INTERNALS__?.invoke('plugin:event|emit', {
                event: 'loading-progress',
                payload: {
                  loadingId: exportLoadingId,
                  phase: 'export_generating',
                  phaseIndex: 1,
                  phaseCount: 4,
                  current: null,
                  total: null,
                  overallPercent: 30,
                  isIndeterminate: true,
                },
              }).catch(() => {});
            }, 100);
            setTimeout(() => {
              window.__TAURI_INTERNALS__?.invoke('plugin:event|emit', {
                event: 'loading-progress',
                payload: {
                  loadingId: exportLoadingId,
                  phase: 'export_writing',
                  phaseIndex: 2,
                  phaseCount: 4,
                  current: 1,
                  total: 1,
                  overallPercent: 90,
                  isIndeterminate: false,
                },
              }).catch(() => {});
            }, 300);
            setTimeout(() => {
              window.__TAURI_INTERNALS__?.invoke('plugin:event|emit', {
                event: 'loading-progress',
                payload: {
                  loadingId: exportLoadingId,
                  phase: 'export_complete',
                  phaseIndex: 3,
                  phaseCount: 4,
                  current: 1,
                  total: 1,
                  overallPercent: 100,
                  isIndeterminate: false,
                },
              }).catch(() => {});
            }, 400);
            // Return after delay to allow progress events to be received and observed
            return new Promise(resolve => setTimeout(() => resolve({ success: true, path: args?.path }), 500));
          }

          // Section and shot editing commands
          case 'update_shot': {
            console.log('[Mock IPC] update_shot called with:', args);
            // Mutate currentProject to support persistence testing
            // Note: Tauri invoke passes camelCase from JS side
            if (currentProject && args) {
              const sectionId = args.sectionId as number;
              const shotId = args.shotId as number;
              const updates = args.updates as Record<string, unknown>;
              const section = (currentProject.sections as Array<{ id: number; shots: Array<{ id: number } & Record<string, unknown>> }>)
                .find(s => s.id === sectionId);
              if (section) {
                const shot = section.shots.find(sh => sh.id === shotId);
                if (shot && updates) {
                  Object.assign(shot, updates);
                  console.log('[Mock IPC] update_shot: applied updates to shot', shotId, updates);
                }
              }
            }
            return { success: true, section_ids: [] };
          }

          case 'delete_shot': {
            console.log('[Mock IPC] delete_shot called with:', args);
            // #310: a plain no-op response left every station index unchanged, so nothing
            // could ever be renumbered to test a live anchored pin against. Splice the shot
            // out in place - the real backend's `remove_shot_with_rewire` shortens the section
            // by one shot the same way, shifting every later station down by one index.
            if (currentProject && args) {
              const sectionId = args.sectionId as number;
              const shotId = args.shotId as number;
              const section = (
                currentProject.sections as Array<{ id: number; shots: Array<{ id: number }> }>
              ).find((s) => s.id === sectionId);
              if (section) {
                const idx = section.shots.findIndex((sh) => sh.id === shotId);
                if (idx >= 0) section.shots.splice(idx, 1);
              }
            }
            return { success: true, section_ids: [] };
          }

          case 'delete_last_shot_and_section':
            console.log('[Mock IPC] delete_last_shot_and_section called with:', args);
            return { success: true, section_ids: [] };

          case 'preview_delete_shot': {
            console.log('[Mock IPC] preview_delete_shot called with:', args);
            const sectionId = (args?.sectionId as number | undefined) ?? 1;
            return {
              removed_station: { section_id: sectionId, station_index: 0 },
              removed_merges: [
                {
                  station_a: { section_id: sectionId, station_index: 0 },
                  station_b: { section_id: sectionId, station_index: 1 },
                },
              ],
              removed_closures: [
                {
                  closure_id: 1n,
                  station_a: { section_id: sectionId, station_index: 0 },
                  station_b: { section_id: sectionId, station_index: 2 },
                },
              ],
              replacement_merges: [],
              replacement_closure_updates: [],
              closure_to_merge: [],
              merge_rewire_skipped: false,
              closure_rewire_skipped: true,
              shift_blocked: false,
              shift_blocked_reasons: [],
            };
          }

          case 'delete_shot_and_split':
            console.log('[Mock IPC] delete_shot_and_split called with:', args);
            return { success: true, section_ids: [] };

          case 'preview_delete_shot_range': {
            console.log('[Mock IPC] preview_delete_shot_range called with:', args);
            const sectionId = (args?.sectionId as number | undefined) ?? 1;
            return {
              section_id: sectionId,
              start_pos: 1,
              end_pos: 2,
              kind: 'interior',
              removed_stations: [{ section_id: sectionId, station_index: 2 }],
              removed_merges: [
                {
                  station_a: { section_id: sectionId, station_index: 1 },
                  station_b: { section_id: sectionId, station_index: 2 },
                },
              ],
              removed_closures: [],
              replacement_merges: [],
              replacement_closure_updates: [],
              closure_to_merge: [],
              merge_rewire_skipped: false,
              closure_rewire_skipped: false,
              shift_blocked: false,
              shift_blocked_reasons: [],
              removed_datums: [],
              removed_comments: [],
            };
          }

          case 'delete_shot_range_split':
            console.log('[Mock IPC] delete_shot_range_split called with:', args);
            return { success: true, section_ids: [] };

          case 'delete_shot_range_shift':
            console.log('[Mock IPC] delete_shot_range_shift called with:', args);
            return { success: true, section_ids: [] };

          case 'update_section': {
            console.log('[Mock IPC] update_section called with:', args);
            // Mutate currentProject to support persistence testing
            // Note: Tauri invoke passes camelCase from JS side
            if (currentProject && args) {
              const sectionId = args.sectionId as number;
              const updates = args.updates as Record<string, unknown>;
              const section = (currentProject.sections as Array<{ id: number } & Record<string, unknown>>)
                .find(s => s.id === sectionId);
              if (section && updates) {
                // The legacy surveyor/date land on EVERY shot, as the backend's
                // `SectionUpdates` does: the surveyors (and, for the Ariane
                // "explorers / surveyors" form, the explorers) from the text.
                const shots = (section.shots as Array<Record<string, unknown>> | undefined) ?? [];
                for (const shot of shots) {
                  const credit = structuredClone(mockShotCredit(section, shot));
                  if ('surveyor' in updates) {
                    const people = mockLegacyPeople((updates.surveyor as string | null) ?? '');
                    credit.surveyors = people.surveyors;
                    if (people.halves) credit.explorers = people.explorers;
                  }
                  if ('date' in updates) credit.date = (updates.date as string | null) ?? null;
                  if ('surveyor' in updates || 'date' in updates) mockStoreCredit(shot, credit);
                }
                Object.assign(section, updates);
                console.log('[Mock IPC] update_section: applied updates to section', sectionId, updates);
              }
            }
            return { success: true, section_ids: [] };
          }

          case 'delete_section': {
            console.log('[Mock IPC] delete_section called with:', args);
            const sectionId = (args?.sectionId ?? args?.section_id) as number;
            if (currentProject && sectionId !== undefined) {
              currentProject.sections = (currentProject.sections as Array<{ id: number }>).filter(
                (s) => s.id !== sectionId,
              );
            }
            return { success: true, section_ids: [] };
          }

          case 'reverse_section': {
            console.log('[Mock IPC] reverse_section called with:', args);
            const sectionId = (args?.sectionId ?? args?.section_id) as number;
            const swapLr = (args?.swapLeftRight ?? args?.swap_left_right ?? false) as boolean;
            const section = (currentProject?.sections as Array<{
              id: number;
              shots: Array<Record<string, unknown>>;
            }> | undefined)?.find((s) => s.id === sectionId);
            if (!section) {
              return { success: false, error: `Section ${sectionId} not found` };
            }
            const shotCount = section.shots.length;
            // Mirror the backend transform (#24, models/project/reverse.rs):
            // reverse the order, normalise both readings to [0, 360), reciprocate
            // AND swap the azimuths (they are two FORWARD readings, so swapping
            // alone is a no-op), swap depths, swap and negate pitch, optionally
            // swap L/R. Ids stay attached to positions; everything else travels.
            const ids = section.shots.map((sh) => sh.id);
            const num = (v: unknown) => (typeof v === 'number' ? v : 0);
            // Every wrap here is `rem_euclid`, matching `reverse.rs::reciprocal`
            // and `models::shot::average_azimuth_of`. A bare `%` truncates toward
            // zero and returns the ANTIPODE once the operand goes below -360 -
            // exactly the bug #24 fixed in `average_azimuth_of`, and reachable
            // because validation only WARNS on an out-of-range azimuth.
            const mod360 = (a: number) => ((a % 360) + 360) % 360;
            const recip = (a: number) => mod360(a + 180);
            const reversed = [...section.shots].reverse().map((sh) => {
              const next: Record<string, unknown> = { ...sh };
              // Normalise BEFORE the degenerate test, like `reverse_shots`: that
              // branch swaps its operands verbatim, so an out-of-range reading
              // would otherwise be written straight back onto the record and stay
              // out of range for every later consumer.
              const azIn = mod360(num(sh.azimuth_in));
              const azOut = mod360(num(sh.azimuth_out));
              if (mod360(azOut - azIn) === 180) {
                // Readings exactly opposite: the circular mean is undefined and
                // reciprocating maps the pair to itself, so the backend swaps
                // them instead to turn the bearing around.
                next.azimuth_in = azOut;
                next.azimuth_out = azIn;
              } else {
                next.azimuth_in = recip(azOut);
                next.azimuth_out = recip(azIn);
              }
              next.depth_in = sh.depth_out;
              next.depth_out = sh.depth_in;
              const pitchIn = sh.pitch_in as number | null | undefined;
              const pitchOut = sh.pitch_out as number | null | undefined;
              next.pitch_in = pitchOut == null ? pitchOut : -pitchOut;
              next.pitch_out = pitchIn == null ? pitchIn : -pitchIn;
              if (swapLr) {
                next.left = sh.right;
                next.right = sh.left;
              }
              return next;
            });
            reversed.forEach((sh, i) => {
              sh.id = ids[i];
            });

            // Station 0 is now the far end, so the section anchor gains the
            // displacement the section used to span or the whole passage
            // teleports. Measured on the PRE-reverse order, like the backend.
            let dx = 0;
            let dy = 0;
            for (const sh of section.shots) {
              const azIn = num(sh.azimuth_in);
              const azOut = num(sh.azimuth_out);
              // Circular mean of two FORWARD readings (models::average_azimuth_of),
              // wrapped with rem_euclid for the reason above - the truncated form
              // returns the antipode for a pair more than 540 apart, which would
              // send the offset compensation the wrong way and move the cave.
              const diff = mod360(azOut - azIn + 540) - 180;
              const meanRad = (mod360(azIn + diff / 2) * Math.PI) / 180;
              const dz = num(sh.depth_out) - num(sh.depth_in);
              const lenSq = num(sh.length) ** 2 - dz ** 2;
              const horizontal = lenSq <= 0 ? 0 : Math.sqrt(lenSq);
              dx += horizontal * Math.sin(meanRad);
              dy += horizontal * Math.cos(meanRad);
            }
            section.shots = reversed;

            // Finiteness first, like `reverse_section`: `NaN !== 0` is TRUE, so a
            // bare `!== 0` admits a non-finite displacement into `section_offsets`,
            // which has no other finite guard - and `save_project` rewrites a
            // non-finite offset to 0.0, teleporting the section to the origin. See
            // the backend's `reverse_refuses_to_write_a_non_finite_offset`.
            if (Number.isFinite(dx) && Number.isFinite(dy) && (dx !== 0 || dy !== 0)) {
              const offsets = currentProject!.section_offsets;
              const [ox, oy] = readSectionOffset(offsets, sectionId);
              writeSectionOffset(offsets, sectionId, [ox + dx, oy + dy]);
            }

            // Station i became station shotCount - i, so every external ref into
            // this section moves with it - otherwise a merge, closure or datum
            // silently ends up on the opposite physical end. The mock stores
            // refs positionally (the backend's anchor keys survive the reverse
            // on their own), so mirror the index here.
            const mirror = (ref: { section_id: number; station_index: number } | undefined) => {
              if (ref && ref.section_id === sectionId) {
                ref.station_index = shotCount - ref.station_index;
              }
            };
            for (const m of (currentProject!.station_merges ?? []) as Array<{
              station_a: { section_id: number; station_index: number };
              station_b: { section_id: number; station_index: number };
            }>) {
              mirror(m.station_a);
              mirror(m.station_b);
            }
            for (const c of (currentProject!.loop_closures ?? []) as Array<{
              station_a: { section_id: number; station_index: number };
              station_b: { section_id: number; station_index: number };
            }>) {
              mirror(c.station_a);
              mirror(c.station_b);
            }
            for (const zd of (currentProject!.zero_datums ?? []) as Array<{
              station_ref: { section_id: number; station_index: number };
            }>) {
              mirror(zd.station_ref);
            }
            // Comments are keyed "sectionId:stationIndex"; rebuild the section's
            // keys in one pass so a mirror can't collide with a key it just wrote.
            const movedComments: Array<[string, string]> = [];
            for (const key of Object.keys(mockStationComments)) {
              const [sid, stationIndex] = key.split(':');
              if (Number(sid) !== sectionId) continue;
              const text = mockStationComments[key]!;
              delete mockStationComments[key];
              movedComments.push([`${sectionId}:${shotCount - Number(stationIndex)}`, text]);
            }
            for (const [key, text] of movedComments) {
              mockStationComments[key] = text;
            }

            // #24: the frontend's own POSITIONAL refs (gas-plan pins, the loaded-plan
            // baseline, the distance-gradient origin) are mirrored off this field, not
            // inferred from geometry - a reverse keeps every station's world position, so
            // a position diff reports nothing. Omitting it here made the whole forward
            // aftermath a silent no-op under the mock while the specs asserted the mirror.
            // Matches the backend: `{section_id, shot_count}` with the count IN FORCE at
            // this step, and a shot-less section is DROPPED (nothing was renumbered, and
            // it is the divisor for the frontend's mirror).
            return {
              success: true,
              section_ids: [],
              section_id: sectionId,
              ...(shotCount > 0
                ? { reversed_sections: [{ section_id: sectionId, shot_count: shotCount }] }
                : {}),
            };
          }

          case 'edit_shot_credit': {
            console.log('[Mock IPC] edit_shot_credit called with:', args);
            // The backend's `CreditEdit::apply` per targeted shot: per role, set,
            // then remove, then add (by identity); `set_date` sets or clears.
            const win = window as unknown as Record<string, unknown>;
            const existing = win.__MOCK_EDIT_SHOT_CREDIT_CALLS__ as unknown[] | undefined;
            const calls = Array.isArray(existing) ? existing : [];
            calls.push(args ?? null);
            win.__MOCK_EDIT_SHOT_CREDIT_CALLS__ = calls;
            const targets = (args?.targets ?? null) as
              | { type: 'section'; section_id: number }
              | { type: 'shots'; shots: Array<[number, number]> }
              | null;
            const edit = (args?.edit ?? {}) as Record<string, unknown>;
            const sections = (currentProject?.sections ?? []) as Array<Record<string, unknown> & { id: number }>;
            const picked: Array<[Record<string, unknown>, Record<string, unknown>]> = [];
            if (targets?.type === 'section') {
              const section = sections.find((s) => s.id === targets.section_id);
              if (!section) return { success: false, error: `Section ${targets.section_id} not found` };
              for (const shot of (section.shots as Array<Record<string, unknown>>) ?? []) picked.push([section, shot]);
            } else if (targets?.type === 'shots') {
              for (const [sid, shid] of targets.shots) {
                const section = sections.find((s) => s.id === sid);
                const shot = ((section?.shots as Array<Record<string, unknown>> | undefined) ?? []).find((x) => x.id === shid);
                if (!section || !shot) return { success: false, error: `Shot ${shid} not found in section ${sid}` };
                picked.push([section, shot]);
              }
            } else {
              return { success: false, error: 'edit_shot_credit: missing targets' };
            }
            const editRole = (current: MockPerson[], role: 'surveyors' | 'explorers'): MockPerson[] => {
              const set = edit[`set_${role}`] as MockPerson[] | undefined;
              const remove = (edit[`remove_${role}`] as MockPerson[] | undefined) ?? [];
              const add = (edit[`add_${role}`] as MockPerson[] | undefined) ?? [];
              let list = set ? mockDedupe(set) : [...current];
              const removed = new Set(remove.map(mockPersonKey));
              list = list.filter((p) => !removed.has(mockPersonKey(p)));
              return mockDedupe([...list, ...add]);
            };
            const touched: number[] = [];
            const step: MockCreditStep = [];
            for (const [section, shot] of picked) {
              const stored = shot.credit === undefined ? undefined : structuredClone(shot.credit);
              const before = mockShotCredit(section, shot);
              const after: MockCredit = {
                surveyors: editRole(before.surveyors, 'surveyors'),
                explorers: editRole(before.explorers, 'explorers'),
                date: 'set_date' in edit ? ((edit.set_date as string | null) ?? null) : before.date,
              };
              mockStoreCredit(shot, after);
              step.push({ shot, before: stored, after: structuredClone(shot.credit) });
              const sid = section.id as number;
              if (!touched.includes(sid)) touched.push(sid);
            }
            if (mockCreditBatchStep) mockCreditBatchStep.push(...step);
            else if (step.length > 0) {
              mockCreditUndo.push(step);
              mockCreditRedo.length = 0;
            }
            return { success: true, section_ids: touched };
          }

          case 'create_shot': {
            console.log('[Mock IPC] create_shot called with:', args);
            // Record calls for assertions (anchor args), mirroring execute_batch.
            const win = window as unknown as Record<string, unknown>;
            const existing = win.__MOCK_CREATE_SHOT_CALLS__ as unknown[] | undefined;
            const calls = Array.isArray(existing) ? existing : [];
            calls.push(args ?? null);
            win.__MOCK_CREATE_SHOT_CALLS__ = calls;
            // Mutate currentProject so the committed shot survives the store's
            // refresh: insert after/before the anchor shot (append by default)
            // with a section-monotonic id.
            // Note: Tauri invoke passes camelCase from JS side
            const sectionId = (args?.sectionId ?? args?.section_id) as number;
            const section = (currentProject?.sections as Array<{
              id: number;
              shots: Array<{ id: number } & Record<string, unknown>>;
            }> | undefined)?.find((s) => s.id === sectionId);
            if (!section) {
              return { success: false, error: `Section ${sectionId} not found` };
            }
            const shotData = (args?.shot ?? {}) as Record<string, unknown>;
            const afterShotId = (args?.afterShotId ?? args?.after_shot_id ?? null) as number | null;
            const beforeShotId = (args?.beforeShotId ?? args?.before_shot_id ?? null) as number | null;
            const nextId =
              section.shots.reduce(
                (max, s) => Math.max(max, s.id),
                mockShotId(section.id as number, 0) - 1,
              ) + 1;
            const newShot = {
              id: nextId,
              azimuth_in: (shotData.azimuth_in as number) ?? 0,
              azimuth_out: (shotData.azimuth_out as number) ?? 0,
              length: (shotData.length as number) ?? 0,
              depth_in: (shotData.depth_in as number) ?? 0,
              depth_out: (shotData.depth_out as number) ?? 0,
              pitch_in: (shotData.pitch_in as number | null | undefined) ?? null,
              pitch_out: (shotData.pitch_out as number | null | undefined) ?? null,
              left: (shotData.left as number | null | undefined) ?? null,
              right: (shotData.right as number | null | undefined) ?? null,
              up: (shotData.up as number | null | undefined) ?? null,
              down: (shotData.down as number | null | undefined) ?? null,
              comment: (shotData.comment as string | null | undefined) ?? null,
              kind: (shotData.kind as string | undefined) ?? 'Normal',
              marker_index: (shotData.marker_index as number | null | undefined) ?? null,
              temperature: (shotData.temperature as number | null | undefined) ?? null,
            };
            let insertAt = section.shots.length;
            if (afterShotId !== null) {
              const i = section.shots.findIndex((s) => s.id === afterShotId);
              if (i >= 0) insertAt = i + 1;
            } else if (beforeShotId !== null) {
              const i = section.shots.findIndex((s) => s.id === beforeShotId);
              if (i >= 0) insertAt = i;
            }
            // A new shot with no credit of its own takes its neighbour's - the
            // one before it, or the next when inserting at the start - as the
            // backend's `create_shot` does.
            if (shotData.credit !== undefined) {
              (newShot as Record<string, unknown>).credit = shotData.credit;
            } else {
              const neighbour = insertAt > 0 ? section.shots[insertAt - 1] : section.shots[0];
              if (neighbour?.credit !== undefined) {
                (newShot as Record<string, unknown>).credit = structuredClone(neighbour.credit);
              }
            }
            section.shots.splice(insertAt, 0, newShot);
            return { success: true, section_ids: [], shot_id: nextId };
          }

          case 'split_shot_at_distance': {
            console.log('[Mock IPC] split_shot_at_distance called with:', args);
            // Record calls for assertions (exact args), mirroring create_shot.
            const win = window as unknown as Record<string, unknown>;
            const existing = win.__MOCK_SPLIT_SHOT_CALLS__ as unknown[] | undefined;
            const calls = Array.isArray(existing) ? existing : [];
            calls.push(args ?? null);
            win.__MOCK_SPLIT_SHOT_CALLS__ = calls;

            const sectionId = (args?.sectionId ?? args?.section_id) as number;
            const shotId = (args?.shotId ?? args?.shot_id) as number;
            const distance = args?.distance as number;
            const section = (currentProject?.sections as Array<{
              id: number;
              shots: Array<{ id: number } & Record<string, unknown>>;
            }> | undefined)?.find((s) => s.id === sectionId);
            if (!section) {
              return { success: false, error: `Section ${sectionId} not found` };
            }
            const idx = section.shots.findIndex((s) => s.id === shotId);
            if (idx < 0) {
              return { success: false, error: `Shot ${shotId} not found in section ${sectionId}` };
            }
            const target = section.shots[idx] as Record<string, unknown>;
            const length = (target.length as number) ?? 0;
            // Mirror the backend floor: both children must be at least
            // MIN_SPLIT_CHILD_LENGTH_M (src/utils/shotSplit.ts + models/project.rs),
            // so the mock cannot accept a distance the real command would reject.
            const MIN_SPLIT_CHILD_LENGTH_M = 0.01;
            if (
              !Number.isFinite(distance) ||
              distance < MIN_SPLIT_CHILD_LENGTH_M ||
              length - distance < MIN_SPLIT_CHILD_LENGTH_M
            ) {
              return {
                success: false,
                error: `Both sides of the split must be at least ${MIN_SPLIT_CHILD_LENGTH_M.toFixed(
                  2,
                )}m (shot length ${length.toFixed(2)}m)`,
              };
            }
            // Mirrors the backend's collinear split: shared azimuth/pitch/LRUD,
            // depth linearly interpolated at the split point. The tail is
            // mutated in place (keeps its id); the head is a fresh shot
            // inserted before it, matching split_shot_at_distance's semantics
            // closely enough for UI assertions.
            const depthIn = (target.depth_in as number) ?? 0;
            const depthOut = (target.depth_out as number) ?? 0;
            const midDepth = depthIn + (distance / length) * (depthOut - depthIn);
            const nextId =
              section.shots.reduce(
                (max, s) => Math.max(max, s.id),
                mockShotId(section.id as number, 0) - 1,
              ) + 1;
            const headShot = {
              ...target,
              id: nextId,
              length: distance,
              depth_out: midDepth,
              comment: null,
              marker_index: null,
            };
            target.length = length - distance;
            target.depth_in = midDepth;
            section.shots.splice(idx, 0, headShot);
            return { success: true, section_ids: [], shot_id: nextId };
          }

          case 'merge_projects':
            console.log('[Mock IPC] merge_projects called with:', args);
            return { success: true, section_ids: [] };

          case 'add_project_file': {
            console.log('[Mock IPC] add_project_file called with:', args);
            const win = window as unknown as Record<string, unknown>;
            const existing = win.__MOCK_ADD_PROJECT_FILE_CALLS__ as unknown[] | undefined;
            const calls = Array.isArray(existing) ? existing : [];
            calls.push(args ?? null);
            win.__MOCK_ADD_PROJECT_FILE_CALLS__ = calls;
            if (!currentProject) {
              return { success: false, warnings: [], error: 'No project is open to add the file to', section_ids: [] };
            }
            const addedPath = (args?.path as string | undefined) ?? '';
            const addedFileName = addedPath.split('/').pop() ?? addedPath;
            // Mirrors `execute`'s guard_mutation, as set_surface_elevation does:
            // `projectIsRemote` is the mock's read-only proxy.
            if (projectIsRemote) {
              return {
                success: false,
                warnings: [],
                error: `EDIT_DISABLED: Add '${addedFileName}' to project is disabled: this SpeleoDB project is open read-only. Acquire the lock to edit.`,
                section_ids: [],
              };
            }
            sectionCounter++;
            const addedSection = {
              id: sectionCounter,
              name: `Added ${sectionCounter}`,
              surveyor: 'Test Surveyor',
              date: new Date().toISOString().split('T')[0],
              color: '#3b82f6',
              direction: 'in',
              shots: [
                {
                  id: mockShotId(sectionCounter, 0),
                  azimuth_in: 0,
                  azimuth_out: 0,
                  length: 5,
                  depth_in: 0,
                  depth_out: 0,
                  pitch_in: null,
                  pitch_out: null,
                  left: null,
                  right: null,
                  up: null,
                  down: null,
                  comment: null,
                  kind: 'Normal',
                  marker_index: null,
                  temperature: null,
                },
              ],
            };
            currentProject.sections.push(addedSection);
            return {
              success: true,
              warnings: [],
              error: null,
              section_ids: [addedSection.id],
              // Shape of `add_receipt` (crates/core/src/core/state/project_add.rs).
              import_receipt: {
                source_format: 'Stalagmight',
                file_name: addedFileName,
                sections_found: 1,
                sections_imported: 1,
                shot_count: 1,
                closures_requested: 0,
                closures_created: 0,
                unit_conversion: null,
                warnings: [],
                info: [],
                zero_datum_created: null,
              },
            };
          }

          case 'set_shot_kind': {
            console.log('[Mock IPC] set_shot_kind called with:', args);
            const sectionId = (args?.sectionId ?? args?.section_id) as number;
            const shotId = (args?.shotId ?? args?.shot_id) as number;
            const kind = (args?.kind ?? 'Normal') as string;
            if (currentProject && sectionId && shotId !== undefined) {
              const section = (
                currentProject.sections as Array<{ id: number; shots: Array<{ id: number }> }>
              ).find((s) => s.id === sectionId);
              if (section) {
                const shot = section.shots.find((s) => s.id === shotId);
                if (shot) {
                  (shot as { kind?: string }).kind = kind;
                }
              }
            }
            return { success: true, section_ids: [] };
          }

          // Raw sqlite browsing (data table fullscreen)
          case 'get_raw_table_names': {
            console.log('[Mock IPC] get_raw_table_names called');
            if (!currentProject) {
              throw new Error('No project database; save the project to browse raw data');
            }
            // Alphabetical, mirroring list_raw_table_names' ORDER BY name.
            return ['project', 'sections', 'shots', 'undo_stack'];
          }

          case 'get_raw_table': {
            console.log('[Mock IPC] get_raw_table called with:', args);
            if (!currentProject) {
              throw new Error('No project database; save the project to browse raw data');
            }
            const project = currentProject;
            const table = (args?.table as string) ?? '';
            const limit = Math.min((args?.limit as number) ?? 1000, 10_000);
            const offset = (args?.offset as number) ?? 0;
            // Values are stringified "server-side" like Database::read_raw_table:
            // NULL → "NULL", blobs → a size placeholder like "<blob 4.2 KB>".
            const str = (value: unknown): string =>
              value === null || value === undefined ? 'NULL' : String(value);
            const rawSections = project.sections as Array<{
              id: number;
              name?: string | null;
              surveyor?: string | null;
              date?: string | null;
              color?: string | null;
              direction?: string;
              shots?: Array<{ id: number } & Record<string, unknown>>;
            }>;
            let columns: string[];
            let allRows: string[][];
            switch (table) {
              case 'project':
                columns = ['id', 'name', 'location', 'created_at', 'modified_at', 'schema_version', 'next_closure_id'];
                allRows = [[
                  '1',
                  project.name,
                  str(project.location),
                  project.created_at,
                  project.modified_at,
                  '7',
                  '1',
                ]];
                break;
              case 'sections':
                columns = ['id', 'name', 'surveyor', 'date', 'color', 'direction', 'quality', 'created_at'];
                allRows = rawSections.map((section) => [
                  String(section.id),
                  str(section.name),
                  str(section.surveyor),
                  str(section.date),
                  str(section.color),
                  section.direction ?? 'in',
                  '1',
                  project.created_at,
                ]);
                break;
              case 'shots':
                columns = [
                  'id', 'section_id', 'sequence_order', 'azimuth_in', 'azimuth_out', 'length',
                  'depth_in', 'depth_out', 'pitch_in', 'pitch_out', 'left_distance', 'right_distance',
                  'up_distance', 'down_distance', 'comment', 'kind', 'marker_index', 'temperature', 'uuid',
                ];
                allRows = rawSections.flatMap((section) =>
                  (section.shots ?? []).map((shot, index) => [
                    String(shot.id),
                    String(section.id),
                    String(index),
                    str(shot.azimuth_in),
                    str(shot.azimuth_out),
                    str(shot.length),
                    str(shot.depth_in),
                    str(shot.depth_out),
                    str(shot.pitch_in),
                    str(shot.pitch_out),
                    str(shot.left),
                    str(shot.right),
                    str(shot.up),
                    str(shot.down),
                    str(shot.comment),
                    shot.kind === 'Jump' ? '1' : shot.kind === 'Reference' ? '2' : '0',
                    str(shot.marker_index),
                    str(shot.temperature),
                    'NULL',
                  ])
                );
                break;
              case 'undo_stack':
                // BLOB columns render as size placeholders, like the real backend.
                columns = ['id', 'sequence', 'command_json', 'undo_data_json', 'executed_at'];
                allRows = [['1', '1', '<blob 1.2 KB>', '<blob 4.2 KB>', project.modified_at]];
                break;
              default:
                throw new Error(`Unknown table: ${table}`);
            }
            return {
              columns,
              rows: allRows.slice(offset, offset + limit),
              total_rows: allRows.length,
            };
          }

          // Cache commands
          case 'get_cache_stats':
            console.log('[Mock IPC] get_cache_stats called');
            return mockCacheStats;

          case 'clear_tile_cache': {
            // Normalize provider: could be string, null, or undefined from frontend
            const provider = typeof args?.provider === 'string' ? args.provider : undefined;
            console.log('[Mock IPC] clear_tile_cache called, provider:', provider);
            if (provider) {
              // Clear specific provider
              delete mockCacheStats.providers[provider];
              // Recalculate totals
              let totalTiles = 0n;
              let totalBytes = 0n;
              for (const p of Object.values(mockCacheStats.providers)) {
                totalTiles += p.tiles;
                totalBytes += p.bytes;
              }
              mockCacheStats.total_tiles = totalTiles;
              mockCacheStats.total_bytes = totalBytes;
            } else {
              // Clear all
              mockCacheStats = { total_tiles: 0n, total_bytes: 0n, providers: {} };
            }
            return null; // Tauri serializes () as null
          }

          case 'get_network_status':
            console.log('[Mock IPC] get_network_status called');
            return mockNetworkStatus;

          case 'get_tile_providers':
            // Return TileProviderInfo[] matching Rust struct shape
            console.log('[Mock IPC] get_tile_providers called');
            return [
              {
                name: 'esri',
                display_label: 'ESRI World Imagery',
                url_template: 'https://server.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/{z}/{y}/{x}',
                attribution: 'Source: Esri, Vantor, Earthstar Geographics, and the GIS User Community',
                attribution_notice: 'Powered by Esri',
                attribution_url: 'https://www.esri.com',
                service_info_url:
                  'https://server.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer?f=json',
                tile_size: 256,
                min_zoom: 1,
                max_zoom: 22,
                content_type_hint: 'image/jpeg',
                api_key_env: null,
                auth_header: null,
                available: true,
              },
            ];

          case 'get_tile_attribution':
            // Mirrors the offline path of the real command: specs run with no
            // network, so the backend would answer with its compiled-in source
            // names and live: false. Asserting on THAT is the point - the map
            // must still be attributed when the fetch cannot happen.
            console.log('[Mock IPC] get_tile_attribution called');
            return {
              provider: (args?.provider as string) ?? 'esri',
              text: 'Source: Esri, Vantor, Earthstar Geographics, and the GIS User Community',
              notice: 'Powered by Esri',
              notice_url: 'https://www.esri.com',
              live: false,
            };

          // Plugin: event system handlers for @tauri-apps/api/event
          case 'plugin:event|listen': {
            const event = args?.event as string;
            const handler = args?.handler as number;
            console.log('[Mock IPC] plugin:event|listen', event, 'handler:', handler);
            const listenerId = ++eventListenerId;
            eventListeners.set(listenerId, { handler, event });
            return listenerId;
          }

          case 'plugin:event|unlisten': {
            // Real Tauri API passes { event: eventName, eventId: listenerId }
            const eventId = args?.eventId as number;
            console.log('[Mock IPC] plugin:event|unlisten', 'eventId:', eventId);
            eventListeners.delete(eventId);
            return undefined;
          }

          case 'plugin:event|emit': {
            const event = args?.event as string;
            const payload = args?.payload;
            console.log('[Mock IPC] plugin:event|emit', event, payload);
            emitEvent(event, payload);
            return undefined;
          }

          case 'get_survey_geometry_for_file': {
            const filePath = args?.path as string;
            console.log('[Mock IPC] get_survey_geometry_for_file called for:', filePath);

            // Check if this path should simulate an error
            if (geometryErrorPaths.has(filePath)) {
              throw new Error(`Failed to load geometry for ${filePath}`);
            }

            // Check if this path should be delayed
            const delay = geometryDelayPaths.get(filePath);
            // Return a SurveyGeometry-conformant payload matching src/types/survey.ts
            const mockGeometry = {
              origin: [-82.46, 24.93] as [number, number],
              shots: [
                { from: [-82.46, 24.93] as [number, number], to: [-82.4599, 24.9301] as [number, number], kind: 'Normal', section_id: 1 },
                { from: [-82.4599, 24.9301] as [number, number], to: [-82.4598, 24.9302] as [number, number], kind: 'Normal', section_id: 1 },
              ],
            };

            if (delay && delay > 0) {
              return new Promise((resolve) => setTimeout(() => resolve(mockGeometry), delay));
            }

            return mockGeometry;
          }

          case 'take_pending_launch_files':
            // #424: no OS handed this browser-hosted app a file. Present so the
            // startup drain resolves to an empty list instead of falling through
            // to the `null` default, which `for (const p of null)` would throw on.
            return [];

          case 'take_unrecognised_shot_kinds':
            // #27: no unreadable kinds in the mocked fixtures. Present so the
            // post-load call resolves instead of throwing into the catch, which
            // would mask a real regression in that path.
            return [];

          // In-app updater (#579). The default is "nothing on offer", so the
          // launch check every spec triggers resolves quietly instead of
          // parking an update banner over whatever that spec is asserting on.
          // A spec that wants the banner overrides this case.
          case 'check_for_app_update':
            return { status: { kind: 'up_to_date' }, current_version: '0.5.0', installs_on_quit: true };

          case 'arm_forced_exit':
            // The explicit "Quit now" past a running SpeleoDB transfer: the
            // backend half (cut instead of wait) is covered in Rust.
            (window as unknown as { __forcedExitArmed?: number }).__forcedExitArmed =
              ((window as unknown as { __forcedExitArmed?: number }).__forcedExitArmed ?? 0) + 1;
            return null;
          case 'disarm_forced_exit':
            // A "Quit now" whose window destroy failed takes the arm back; the
            // backend half is covered in Rust.
            (window as unknown as { __forcedExitDisarmed?: number }).__forcedExitDisarmed =
              ((window as unknown as { __forcedExitDisarmed?: number }).__forcedExitDisarmed ?? 0) + 1;
            return null;
          case 'arm_app_update_restart':
          case 'unarm_app_update_restart':
            // The backend's one-shot install consent. Succeeds here so a spec
            // that seeds an available update can drive Restart through the real
            // exit guards; the refusal cases are covered in Rust.
            return null;

          case 'discard_app_update_download': {
            // #645: idempotent in the backend, and the mock parks nothing to
            // drop - so it only counts the call, for a spec to assert it.
            const win = window as unknown as Record<string, unknown>;
            win.__MOCK_DISCARD_UPDATE_CALLS__ = ((win.__MOCK_DISCARD_UPDATE_CALLS__ as number | undefined) ?? 0) + 1;
            return null;
          }

          case 'download_app_update':
          case 'install_app_update_and_restart':
            // Unreachable from the default state above (both refuse without a
            // checked update), and a real install would end the browser session.
            // Present so the contract holds and an accidental call is visible as
            // a failed assertion rather than an unhandled null.
            throw new Error(`${cmd} is not driven by the default mock state`);

          // Silent no-ops: commands that should succeed without side-effects
          case 'push_frontend_log':
            return null;

          // SpeleoDB (#203/#204/#205): deterministic logged-out / local defaults so
          // the settings tab + browser render without a live server. No e2e spec
          // drives the hand-off yet; these keep the mounts from erroring on null.
          case 'speleodb_session':
            return {
              logged_in: false,
              user_email: null,
              endpoint: null,
              };
          case 'speleodb_link_status':
            // Full SpeleoLinkStatus shape (remote_state / access_denied / server_error
            // included) so a spec that flips is_remote:true still matches the DTO.
            return {
              is_remote: false,
              edit_enabled: true,
              has_unpushed_edits: false,
              permission: null,
              lock_holder: null,
              remote_state: 'unknown',
              access_denied: false,
              server_error: null,
              // [#303] Dirty ONLY because a chain step walked this copy back to an older
              // revision - the user has edited nothing. Every consumer composes it with
              // has_unpushed_edits (`isUnwoundNotEdited`), and it is what makes the close
              // and quit prompts stop calling an old copy "unpushed changes" and stop
              // pre-focusing the push. It was missing here, so it read as `undefined`
              // rather than false and no spec could reach the state at all - the state
              // this whole feature is built around.
              unwound: false,
              // [#303] The hybrid undo stack's policy fields. Full is the default, and
              // not overridden, so the settings control renders "following the global
              // default" - the shape a spec sees unless it overrides this.
              undo_mode: 'full',
              undo_mode_overridden: false,
              // A spec opts into a different link state by assigning this before the app
              // asks; anything it carries wins over the defaults above.
              ...((window as unknown as Record<string, unknown>)
                .__speleoLinkStatusOverrides as Record<string, unknown> | undefined),
            };
          case 'speleodb_list_projects':
            return [];
          // #270: local (un-pushed) state per project id, keyed by id. Nothing is
          // cached in a mocked run, so every list renders without a local-edit
          // accent; specs that need one override this per spec.
          case 'speleodb_local_project_states':
            return {};
          case 'speleodb_take_remote':
            // Returns the reloaded .stalx path (#198); no e2e spec drives it yet.
            return '/mock/project.stalx';
          case 'speleodb_logout':
          case 'speleodb_unlink':
          // Strip half of the managed-file prompt: deletes a foreign copy's
          // speleo_link row. No payload; specs assert the invocation itself.
          case 'speleodb_remove_link_at':
          case 'speleodb_release_lock':
          // [#258 review] The provenance-scoped unwind. No payload, and no argument:
          // it releases whatever THIS process recorded acquiring, or nothing.
          case 'speleodb_release_lock_acquired_here':
          case 'speleodb_project_access_at':
          case 'speleodb_release_current_lock':
          // [#303] Which revision a chain step would load, backward and forward. `null`
          // by default - a mocked project has no revision history to walk - so neither
          // dropdown shows its step entry unless a spec overrides this.
          case 'speleodb_unwind_target':
          case 'speleodb_rewind_target':
            return null;
          // [#303] Discard the local copy and re-lock. Mocked as a plain successful
          // acquire: the specs that reach it care that the app returns to an editing
          // state, not about the download it does on the way.
          case 'speleodb_discard_changes':
            return { kind: 'editing' };
          // Sign-out's discard of a cached copy that is not open. Specs assert the
          // invocation (via __MOCK_EXIT_CALLS__). Like the backend, it refuses
          // without the explicit consent flag (the mock's copies all hold
          // un-uploaded work: that is why the sign-out asks about them).
          case 'speleodb_discard_cached_copy':
            if (args?.discardUnpushed !== true) {
              return {
                success: false,
                warnings: [],
                error: 'This copy has changes that were never uploaded to SpeleoDB, or its state could not be read. Nothing was removed.',
                section_ids: [],
              };
            }
            return { success: true, warnings: [], error: null, section_ids: [] };
          // Cached SpeleoDB copies on the home screen. Nothing is cached in a mocked
          // run; specs that need copies (owner-classified) override the listing.
          case 'speleodb_cached_projects':
            return [];
          case 'speleodb_remove_cached_copy':
            return { success: true, warnings: [], error: null, section_ids: [] };
          case 'speleodb_purge_cached_copies':
            return { removed: 0, kept_unpushed: [], kept_open: [], failed: [], locks_released: [], locks_unchecked: [] };
          case 'speleodb_get_project_metadata':
            return { name: 'Mock Cave', description: '', country: 'US', visibility: 'PRIVATE', latitude: null, longitude: null, exclude_geojson: false };
          case 'speleodb_project_collaborators': {
            // The linked project's collaborators as survey-credit suggestions;
            // specs override via __MOCK_SPELEODB_COLLABORATORS__.
            const override = (window as unknown as Record<string, unknown>).__MOCK_SPELEODB_COLLABORATORS__;
            if (Array.isArray(override)) return override;
            return [
              { name: 'Mock Collaborator', email: 'collaborator@example.org', level: 'READ_AND_WRITE' },
              { name: null, email: 'test@example.com', level: 'ADMIN' },
            ];
          }
          case 'speleodb_publish':
            // #207 publish result: the reloaded (now-remote) local .stalx path.
            return {
              project_id: 'mock-project-id',
              endpoint: 'https://speleodb.example',
              local_path: '/mock/project.stalx',
            };
          case 'speleodb_login':
            return {
              user_email: 'test@example.com',
              endpoint: 'https://speleodb.example',
            };

          default:
            console.warn(`[Mock IPC] Unhandled command: ${cmd}`);
            return null;
        }
      };
    })();

    // Mock the Tauri internals (Tauri 2.x API)
    const tauriInternals = {
      invoke: async (cmd: string, args?: Record<string, unknown>) => {
        return mockHandler(cmd, args);
      },
      transformCallback: (callback: () => void, once?: boolean) => {
        const id = Math.random().toString(36).substring(7);
        (window as unknown as Record<string, unknown>)[`_${id}`] = callback;
        return id;
      },
      convertFileSrc: (filePath: string, protocol?: string) => {
        return `asset://localhost/${filePath}`;
      },
      // sep()/delimiter() in @tauri-apps/api/path read these synchronously.
      plugins: {
        path: { sep: '/', delimiter: ':' },
      },
      metadata: {
        currentWindow: { label: 'main' },
        currentWebview: { label: 'main' },
      },
    };

    (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = tauriInternals;

    // Mock window.__TAURI__ for older API usage
    (window as unknown as { __TAURI__: unknown }).__TAURI__ = {
      invoke: tauriInternals.invoke,
      convertFileSrc: tauriInternals.convertFileSrc,
      transformCallback: tauriInternals.transformCallback,
    };

    // Mock window API for getCurrentWindow
    (window as unknown as { __TAURI_INTERNALS__: typeof tauriInternals & { windows?: unknown } }).__TAURI_INTERNALS__.metadata = {
      currentWindow: {
        label: 'main',
      },
      currentWebview: {
        label: 'main',
      },
    };

    // Mock Tauri dialog plugin for dynamic import
    // This is needed because @tauri-apps/plugin-dialog uses its own invoke mechanism
    const mockDialogModule = {
      open: async (_options?: unknown) => {
        console.log('[Mock Dialog] open called');
        return '/mock/test-survey.dmp';
      },
      save: async (_options?: unknown) => {
        console.log('[Mock Dialog] save called');
        return '/mock/saved-project.stalx';
      },
      message: async (_message: string, _options?: unknown) => {
        console.log('[Mock Dialog] message called');
        return undefined;
      },
      ask: async (_message: string, _options?: unknown) => {
        console.log('[Mock Dialog] ask called');
        return true;
      },
      confirm: async (_message: string, _options?: unknown) => {
        console.log('[Mock Dialog] confirm called');
        return true;
      },
    };

    // Store the mock module for potential interception
    (window as unknown as Record<string, unknown>).__TAURI_DIALOG_MOCK__ = mockDialogModule;

    // Mock __TAURI_EVENT_PLUGIN_INTERNALS__ for @tauri-apps/api/event cleanup
    // The real Tauri event API calls unregisterListener on unlisten
    interface TauriEventPluginInternals {
      unregisterListener: (eventId: number) => void;
    }
    const tauriEventPluginInternals: TauriEventPluginInternals = {
      unregisterListener: (eventId: number) => {
        console.log('[Mock Event] unregisterListener:', eventId);
        eventListeners.delete(eventId);
      },
    };
    (window as unknown as Record<string, unknown>).__TAURI_EVENT_PLUGIN_INTERNALS__ = tauriEventPluginInternals;

    // Mock event system for E2E tests (loading progress events, etc.)
    // This is wired to the plugin:event system so @tauri-apps/api/event.listen() works
    interface TauriEventMock {
      listen: (event: string, handler: (e: { payload: unknown }) => void) => Promise<() => void>;
      unlisten: (eventId: number) => void;
      emit: (event: string, payload: unknown) => void;
    }
    const tauriEventMock: TauriEventMock = {
      // Listen for events - returns an unlisten function
      listen: async (event: string, handler: (e: { payload: unknown }) => void) => {
        const listenerId = ++eventListenerId;
        const callbackId = `mock_cb_${listenerId}`;
        (window as unknown as Record<string, unknown>)[`_${callbackId}`] = handler;
        eventListeners.set(listenerId, { handler: callbackId as unknown as number, event });
        console.log('[Mock Event] listen:', event, 'listenerId:', listenerId);
        return () => {
          eventListeners.delete(listenerId);
          delete (window as unknown as Record<string, unknown>)[`_${callbackId}`];
        };
      },
      // Unlisten by eventId
      unlisten: (eventId: number) => {
        console.log('[Mock Event] unlisten:', eventId);
        eventListeners.delete(eventId);
      },
      // Emit events through the same system that @tauri-apps/api/event.listen uses
      emit: (event: string, payload: unknown) => {
        console.log('[Mock Event] emitEvent:', event, payload);
        emitEvent(event, payload);
      },
    };
    (window as unknown as Record<string, unknown>).__tauriEventMock = tauriEventMock;

    console.log('[Mock IPC] Tauri mocks installed');
  });
}

/**
 * Set up mocks with a custom project already loaded.
 */
export async function setupTauriMocksWithProject(
  page: Page,
  project: Partial<MockProject>
): Promise<void> {
  await page.addInitScript(
    (projectData) => {
      const currentProject = {
        id: projectData.id ?? 'mock-project-001',
        name: projectData.name ?? 'Test Project',
        sections: projectData.sections ?? [],
        station_merges: projectData.station_merges ?? [],
        zero_datums: projectData.zero_datums ?? [],
        section_offsets: projectData.section_offsets ?? {},
      };

      const mockHandler = (cmd: string, args?: Record<string, unknown>): unknown => {
        if (cmd !== 'push_frontend_log') {
          console.log(`[Mock IPC] Command: ${cmd}`, args);
        }

        switch (cmd) {
          case 'get_project':
          case 'load_project':
            return currentProject;
          case 'save_project':
            return { success: true, section_ids: [] };
          case 'push_frontend_log':
            return null;
          default:
            return null;
        }
      };

      (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {
        invoke: async (cmd: string, args?: Record<string, unknown>) => {
          return mockHandler(cmd, args);
        },
        transformCallback: (callback: () => void) => {
          const id = Math.random().toString(36).substring(7);
          (window as unknown as Record<string, unknown>)[`_${id}`] = callback;
          return id;
        },
        // sep()/delimiter() in @tauri-apps/api/path read these synchronously.
        plugins: {
          path: { sep: '/', delimiter: ':' },
        },
      };

      console.log('[Mock IPC] Tauri mocks with project installed');
    },
    project
  );
}

export { createMockProject, createMockSection };
export type { MockProject, MockSection, MockShot };
