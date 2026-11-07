import { test, expect, type Page } from './fixtures/coverage';
import { setupMockedPage, createProject } from './helpers/app';
import { defaultConfig } from '../src/types/ipc/defaultConfig';

/**
 * [#576 NF-W5I-1] The IPC mock answers in the backend's wire shapes, so a spec
 * exercises the app against what the real commands send:
 *
 * - `get_project_state` with no project open is `null` (the backend resolves the
 *   whole reply null), not a state whose `project` is null;
 * - `get_config` / `reload_config` are a WHOLE Config - every field
 *   `Config::default()` has (the generated `defaultConfig`), none it lacks; the
 *   fixture used to leave out fields the backend always sends (telemetry_enabled,
 *   shortcuts, cache providers, ...) and carry ones it never sends;
 * - every `import_dmp` section carries the wire's `version` and `content_hash`
 *   (the wizard keys LRUD and temperature on the version, so a versionless mock
 *   section showed them blank).
 */

type InvokeWindow = Window & {
  __TAURI_INTERNALS__: { invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown> };
};

const invoke = (page: Page, cmd: string, args?: Record<string, unknown>) =>
  page.evaluate(
    ([c, a]) => (window as unknown as InvokeWindow).__TAURI_INTERNALS__.invoke(c, a),
    [cmd, args] as const,
  );

/** Every key path of a plain-object tree (arrays and values are leaves). */
function keyPaths(value: unknown, prefix = ''): string[] {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return [];
  return Object.entries(value).flatMap(([key, child]) => {
    const path = prefix ? `${prefix}.${key}` : key;
    return [path, ...keyPaths(child, path)];
  });
}

function configDrift(answer: unknown): { missing: string[]; extra: string[] } {
  const want = new Set(keyPaths(defaultConfig));
  const got = new Set(keyPaths(answer));
  return {
    missing: [...want].filter((path) => !got.has(path)),
    extra: [...got].filter((path) => !want.has(path)),
  };
}

test.describe('IPC mock answers in wire shapes (#576 NF-W5I-1)', () => {
  test('get_project_state is null with no project open, a state once one is', async ({ page }) => {
    await setupMockedPage(page);
    const before = await invoke(page, 'get_project_state');
    console.log('get_project_state with no project open:', JSON.stringify(before)?.slice(0, 80));
    expect(before).toBeNull();

    await createProject(page, 'CaveA');
    const after = (await invoke(page, 'get_project_state')) as { project: { name: string } } | null;
    expect(after?.project.name).toBe('CaveA');
  });

  test('get_config and reload_config answer a whole Config', async ({ page }) => {
    await setupMockedPage(page);
    for (const cmd of ['get_config', 'reload_config']) {
      const drift = configDrift(await invoke(page, cmd));
      console.log(
        `${cmd}: ${drift.missing.length} field(s) missing, ${drift.extra.length} the wire lacks;`,
        `missing e.g. ${JSON.stringify(drift.missing.slice(0, 6))}; extra ${JSON.stringify(drift.extra)}`,
      );
      expect(drift, cmd).toEqual({ missing: [], extra: [] });
    }
  });

  test('import_dmp sections carry the wire version and content hash', async ({ page }) => {
    await setupMockedPage(page);
    const parsed = (await invoke(page, 'import_dmp', { path: '/mock/CaveA.dmp' })) as {
      version: number;
      sections: Array<{ name: string; version?: unknown; content_hash?: unknown }>;
    };
    const shapes = parsed.sections.map((s) => [s.name, typeof s.version, typeof s.content_hash]);
    console.log('import_dmp sections (name, typeof version, typeof content_hash):', JSON.stringify(shapes));
    expect(parsed.sections.length).toBeGreaterThan(0);
    for (const section of parsed.sections) {
      expect(section.version, section.name).toBe(parsed.version);
      expect(typeof section.content_hash, section.name).toBe('string');
    }
  });
});
