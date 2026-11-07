/**
 * [#576 RV6b-2] A display setting is written for the project it was made in.
 *
 * Grid spacing, the geometry mode and the display toggles are stored per project,
 * in whichever project database is open when `save_display_setting` runs. Since
 * SETTINGS-ORDER a write of one key waits for the previous write of that key to
 * settle, so a burst of clicks in project A followed by opening project B could
 * send A's queued writes after B opened - into B's file. Now:
 *  - a write carries the identity of the project it was made for - its
 *    `created_at` and, since NF-W8FE-3, the file it is open from (two copies of
 *    one file share `created_at`) - and the backend refuses it with
 *    PROJECT_CHANGED when another one is open;
 *  - a write still queued when the project is replaced by ANOTHER one (or a copy)
 *    is dropped, never sent - but one queued across a reload of the SAME project
 *    (same `created_at`, same file) is still sent: it was made in the project that
 *    is open (RV8b-3);
 *  - that refusal is the design working, so it is not logged as a failure.
 *
 * Store tier: the real stores and write chain; the IPC is mocked, and the first
 * write of a burst is held until the test releases it. All data is synthetic.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';

import { invokeIpcMock, stubGetErrorMessage, stringOfError } from '../../test/factories/ipc';

vi.mock('../../utils/ipc');
stubGetErrorMessage(stringOfError);

import { useUIStore } from '../uiStore';
import { useProjectStore } from '../projectStore';
import { noteProjectSessionChanged } from '../projectSession';
import type { Project } from '../../types/project';

const CAVE_A = '2026-01-01T10:00:00Z';
const CAVE_A_PATH = '/surveys/CaveA.stalx';
/** What a write made in CaveA names. */
const MADE_FOR_A = { createdAt: CAVE_A, filePath: CAVE_A_PATH };

/** The display-setting writes sent, in send order. */
let sent: Array<Record<string, unknown>>;
/** Releases the held first write. */
let releaseFirst: (() => void) | null;

beforeEach(() => {
  sent = [];
  releaseFirst = null;
  invokeIpcMock.mockReset();
  invokeIpcMock.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
    if (cmd === 'save_display_setting') {
      sent.push(args ?? {});
      if (sent.length === 1) {
        return new Promise<null>((resolve) => {
          releaseFirst = () => resolve(null);
        });
      }
      return Promise.resolve(null);
    }
    if (cmd === 'close_project') return Promise.resolve({ remote_link: null });
    return Promise.resolve(undefined);
  });
  useUIStore.setState({ appMode: 'survey', gridSpacing: 10, geometryMode: 'measured' });
  useProjectStore.setState({
    project: { name: 'CaveA', created_at: CAVE_A, file_path: CAVE_A_PATH, sections: [] } as unknown as Project,
    isClosing: false,
  });
});

async function until(predicate: () => boolean): Promise<void> {
  await vi.waitFor(() => expect(predicate()).toBe(true));
}

describe('display settings are scoped to their project (#576 RV6b-2)', () => {
  it('a write carries the identity of the project it was made in', async () => {
    useUIStore.getState().setGridSpacing(20);
    await until(() => sent.length === 1);
    releaseFirst?.();
    console.log('[RV6b-2] save_display_setting args:', JSON.stringify(sent[0]));
    expect(sent[0]).toEqual({ key: 'gridSpacing', value: '20', madeFor: MADE_FOR_A });
  });

  it('a write names the copy it was made in, not only the project (#576 NF-W8FE-3)', async () => {
    // Two copies of one project file share created_at: only the file tells the
    // backend which one a write was made for.
    useUIStore.getState().setGridSpacing(20);
    await until(() => sent.length === 1);
    releaseFirst?.();
    const copyPath = '/surveys/CaveA copy.stalx';
    useProjectStore.setState({
      project: { name: 'CaveA', created_at: CAVE_A, file_path: copyPath, sections: [] } as unknown as Project,
    });
    useUIStore.getState().saveDisplaySetting('showGrid', true);
    await until(() => sent.length === 2);
    console.log('[NF-W8FE-3] writes made in CaveA, then in its copy:', JSON.stringify(sent));
    expect(sent.map((args) => args.madeFor)).toEqual([
      MADE_FOR_A,
      { createdAt: CAVE_A, filePath: copyPath },
    ]);
  });

  it('a never-saved project names no file', async () => {
    useProjectStore.setState({
      project: { name: 'CaveA', created_at: CAVE_A, file_path: null, sections: [] } as unknown as Project,
    });
    useUIStore.getState().setGridSpacing(20);
    await until(() => sent.length === 1);
    releaseFirst?.();
    expect(sent[0]).toEqual({ key: 'gridSpacing', value: '20', madeFor: { createdAt: CAVE_A, filePath: null } });
  });

  it('a write still queued when the project closes is dropped, not sent to the next one', async () => {
    const { setGridSpacing, saveDisplaySetting, setGeometryMode } = useUIStore.getState();
    setGridSpacing(20);
    await until(() => sent.length === 1);
    // Queued behind the held first write of the same key.
    setGridSpacing(50);
    setGeometryMode('closureAdjusted');
    const toggle = saveDisplaySetting('showGrid', true);

    const closed = await useProjectStore.getState().closeProject();
    releaseFirst?.();
    await toggle;
    await new Promise((resolve) => setTimeout(resolve, 20));

    console.log(
      `[RV6b-2] close: success=${closed.success}; writes sent: ${JSON.stringify(sent)} ` +
        '(expected: no second gridSpacing - the 50 queued behind the held 20 is dropped)',
    );
    expect(closed.success).toBe(true);
    // The geometry mode and the toggle had no write ahead of them, so they were
    // sent at once - with A's identity, for the backend to refuse once B is open.
    const queuedGrid = sent.filter((args) => args.key === 'gridSpacing');
    expect(queuedGrid).toEqual([{ key: 'gridSpacing', value: '20', madeFor: MADE_FOR_A }]);
    expect(sent.every((args) => JSON.stringify(args.madeFor) === JSON.stringify(MADE_FOR_A))).toBe(true);
  });

  it('a write queued across a reload of the same project is still sent to it (#576 RV8b-3)', async () => {
    // A dialog-driven reload of the open file (publish, a SpeleoDB pull, the
    // recovery reopen) replaces the project session, as any load does. A write
    // queued behind an in-flight one of the same key was made in THIS project, and
    // the project open after the reload is the same one: it lands.
    const { setGridSpacing } = useUIStore.getState();
    setGridSpacing(20);
    await until(() => sent.length === 1);
    setGridSpacing(50);
    // What a load of the same file does: a new session, the same project.
    noteProjectSessionChanged();
    useProjectStore.setState({
      project: { name: 'CaveA', created_at: CAVE_A, file_path: CAVE_A_PATH, sections: [] } as unknown as Project,
    });
    releaseFirst?.();
    await new Promise((resolve) => setTimeout(resolve, 20));
    console.log('[RV8b-3] writes across a same-file reload:', JSON.stringify(sent));
    expect(sent).toEqual([
      { key: 'gridSpacing', value: '20', madeFor: MADE_FOR_A },
      { key: 'gridSpacing', value: '50', madeFor: MADE_FOR_A },
    ]);
  });

  it('a write queued across a switch to another copy of the file is dropped (#576 RV8b-3)', async () => {
    const { setGridSpacing } = useUIStore.getState();
    setGridSpacing(20);
    await until(() => sent.length === 1);
    setGridSpacing(50);
    noteProjectSessionChanged();
    useProjectStore.setState({
      project: {
        name: 'CaveA',
        created_at: CAVE_A,
        file_path: '/surveys/CaveA copy.stalx',
        sections: [],
      } as unknown as Project,
    });
    releaseFirst?.();
    await new Promise((resolve) => setTimeout(resolve, 20));
    console.log('[RV8b-3] writes across a switch to a copy:', JSON.stringify(sent));
    expect(sent).toEqual([{ key: 'gridSpacing', value: '20', madeFor: MADE_FOR_A }]);
  });

  // [#576 RV9a-2] A Save As (or a never-saved project's first save) moves the open
  // project to another file without a new session; the backend still takes a write
  // made just before it (`opened_from`). Comparing files alone dropped it.
  it.each([
    ['a Save As', CAVE_A_PATH],
    ["a never-saved project's first save", null],
  ] as const)('a write queued across %s is still sent to the project', async (_what, openFrom) => {
    const savedAs = '/surveys/CaveA renamed.stalx';
    useProjectStore.setState({
      project: { name: 'CaveA', created_at: CAVE_A, file_path: openFrom, sections: [] } as unknown as Project,
    });
    const fallback = invokeIpcMock.getMockImplementation();
    invokeIpcMock.mockImplementation((cmd: string, args?: Record<string, unknown>) =>
      cmd === 'save_project'
        ? Promise.resolve({ success: true, saved_path: savedAs, warnings: [] })
        : cmd === 'check_project_integrity'
          ? Promise.resolve({ messages: [] })
          : fallback!(cmd, args),
    );
    const { setGridSpacing } = useUIStore.getState();
    setGridSpacing(20);
    await until(() => sent.length === 1);
    setGridSpacing(50);

    const saved = await useProjectStore.getState().saveProject(savedAs);
    expect(saved.success).toBe(true);
    expect(useProjectStore.getState().project?.file_path).toBe(savedAs);
    releaseFirst?.();
    await new Promise((resolve) => setTimeout(resolve, 20));
    console.log('[RV9a-2] writes across', _what, JSON.stringify(sent));
    const madeFor = { createdAt: CAVE_A, filePath: openFrom };
    expect(sent).toEqual([
      { key: 'gridSpacing', value: '20', madeFor },
      { key: 'gridSpacing', value: '50', madeFor },
    ]);
  });

  it("the backend's PROJECT_CHANGED refusal is not reported as a failure", async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    invokeIpcMock.mockImplementation((cmd: string) =>
      cmd === 'save_display_setting'
        ? Promise.reject({ code: 'PROJECT_CHANGED', message: 'another project is open' })
        : Promise.resolve(undefined),
    );
    await useUIStore.getState().saveDisplaySetting('showGrid', true);
    invokeIpcMock.mockImplementation((cmd: string) =>
      cmd === 'save_display_setting'
        ? Promise.reject({ code: 'FAILED', message: 'disk full' })
        : Promise.resolve(undefined),
    );
    await useUIStore.getState().saveDisplaySetting('showGrid', false);
    const warned = warn.mock.calls.map(([line]) => String(line));
    warn.mockRestore();
    expect(warned).toEqual(['Failed to save display setting:']);
  });
});
