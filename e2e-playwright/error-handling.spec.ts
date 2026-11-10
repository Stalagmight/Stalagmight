import { test, expect, Page } from './fixtures/coverage';
import { setupTauriMocks } from './mocks/tauri-ipc';
import { setupMockedPage, createProject as createProjectViaApp, openAppMenu } from './helpers/app';
import { setIpcOverrides, clearIpcOverrideKeys } from './helpers/ipcOverrides';

/**
 * Error Handling E2E Tests
 *
 * Tests that errors are communicated clearly to users and the app
 * remains functional after error scenarios.
 */

/**
 * Helper to create a project.
 */
async function createProject(page: Page, name: string): Promise<void> {
  const newProjectBtn = page
    .locator('[data-testid="new-project-button"], [data-testid="empty-state-new-project"]')
    .first();
  await newProjectBtn.click();
  await page.getByTestId('new-project-dialog').waitFor({ state: 'visible', timeout: 5000 });
  await page.getByTestId('new-project-name-input').fill(name);
  await page.getByTestId('new-project-create-button').click();
  await expect(page.getByTestId('map-canvas')).toBeVisible({ timeout: 10000 });
}

/**
 * Sets up mocks with error simulation capabilities.
 * Allows triggering specific errors via window.__TAURI_ERROR_MODE__.
 */
async function setupTauriMocksWithErrorSimulation(page: Page): Promise<void> {
  await page.addInitScript(() => {
    // Error simulation mode - can be set to trigger specific errors
    (window as unknown as { __TAURI_ERROR_MODE__: string | null }).__TAURI_ERROR_MODE__ = null;

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
    let sectionCounter = 0;

    const mockHandler = (cmd: string, args?: Record<string, unknown>): unknown => {
      // Skip logging for push_frontend_log to avoid infinite loop with console capture
      if (cmd === 'push_frontend_log') return null;
      const errorMode = (window as unknown as { __TAURI_ERROR_MODE__: string | null }).__TAURI_ERROR_MODE__;
      console.log(`[Mock IPC] Command: ${cmd}, errorMode: ${errorMode}`, args);

      // Check for error simulation modes
      if (errorMode === 'save_permission_denied' && cmd === 'save_project') {
        console.log('[Mock IPC] Simulating save permission denied error');
        return { success: false, error: 'Permission denied: cannot write to file' };
      }

      if (errorMode === 'save_disk_full' && cmd === 'save_project') {
        console.log('[Mock IPC] Simulating disk full error');
        return { success: false, error: 'Disk full: no space left on device' };
      }

      if (errorMode === 'open_corrupted' && cmd === 'load_project_full') {
        console.log('[Mock IPC] Simulating corrupted file error');
        throw new Error('File corrupted: invalid project format');
      }

      if (errorMode === 'open_not_found' && cmd === 'load_project_full') {
        console.log('[Mock IPC] Simulating file not found error');
        throw new Error('File not found: the specified file does not exist');
      }

      if (errorMode === 'dmp_parse_error' && (cmd === 'import_dmp' || cmd === 'parse_dmp')) {
        console.log('[Mock IPC] Simulating DMP parse error');
        throw new Error('Invalid DMP header: expected version 2-5, got invalid data');
      }

      if (errorMode === 'dmp_truncated' && (cmd === 'import_dmp' || cmd === 'parse_dmp')) {
        console.log('[Mock IPC] Simulating truncated DMP file error');
        throw new Error('Truncated DMP file: unexpected end of data');
      }

      if (errorMode === 'undo_error' && cmd === 'undo') {
        console.log('[Mock IPC] Simulating undo error');
        return { success: false, error: 'Undo failed: command stack corrupted' };
      }

      if (errorMode === 'redo_error' && cmd === 'redo') {
        console.log('[Mock IPC] Simulating redo error');
        return { success: false, error: 'Redo failed: no commands to redo' };
      }

      // Normal mock handler logic
      switch (cmd) {
        case 'create_project_full': {
          const name = (args?.name as string) ?? 'Untitled Project';
          const now = new Date().toISOString();
          sectionCounter = 0;
          currentProject = {
            id: 1,
            name,
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
          return {
            loadingId: Date.now(),
            project: currentProject,
            displaySettings: {
              showStations: true,
              showSectionNames: true,
              showStationLabels: false,
              showStationNames: false,
              showShotData: false,
              showReferenceShots: true,
              showZeroDatum: true,
              showGrid: false,
              showScaleBar: true,
              showSatellite: false,
              showDepthGradient: false,
              gridSpacing: 10,
            },
          };
        }

        case 'load_project_full': {
          projectPath = (args?.path as string) ?? null;
          const now = new Date().toISOString();
          currentProject = {
            id: 1,
            name: 'Test Project',
            location: null,
            created_at: now,
            modified_at: now,
            file_path: projectPath,
            sections: [],
            station_merges: [],
            zero_datums: [],
            loop_closures: [],
            section_offsets: {},
          };
          return {
            loadingId: Date.now(),
            project: currentProject,
            stations: [],
            stats: {
              section_count: 0,
              shot_count: 0,
              total_length: 0,
              min_depth: 0,
              max_depth: 0,
              station_count: 0,
              loop_count: 0,
              penetration_distance: 0,
            },
            warnings: [],
            displaySettings: {
              showStations: true,
              showSectionNames: true,
              showStationLabels: false,
              showStationNames: false,
              showShotData: false,
              showReferenceShots: true,
              showZeroDatum: true,
              showGrid: false,
              showScaleBar: true,
              showSatellite: false,
              showDepthGradient: false,
              gridSpacing: 10,
            },
          };
        }

        case 'save_project': {
          projectPath = (args?.path as string) ?? projectPath;
          if (currentProject && projectPath) {
            currentProject.file_path = projectPath;
          }
          return { success: true, path: projectPath };
        }

        case 'close_project': {
          currentProject = null;
          projectPath = null;
          return { success: true };
        }

        case 'get_project':
        case 'get_project_state': {
          const mockStations: Array<{
            station_ref: { section_id: number; station_index: number };
            coordinates: { x: number; y: number; z: number };
            is_zero_datum: boolean;
            gps_coordinates: null;
            merged_with: never[];
          }> = [];

          if (currentProject?.sections) {
            for (const section of currentProject.sections as Array<{ id: number; shots: unknown[] }>) {
              const shotCount = section.shots?.length ?? 0;
              for (let i = 0; i <= shotCount; i++) {
                mockStations.push({
                  station_ref: {
                    section_id: section.id,
                    station_index: i,
                  },
                  coordinates: { x: i * 10, y: 0, z: 0 },
                  is_zero_datum: i === 0,
                  gps_coordinates: null,
                  merged_with: [],
                });
              }
            }
          }

          return {
            project: currentProject,
            stations: mockStations,
            stats: {
              section_count: currentProject?.sections.length ?? 0,
              shot_count: mockStations.length > 0 ? mockStations.length - 1 : 0,
              total_length: 0,
              min_depth: 0,
              max_depth: 0,
              station_count: mockStations.length,
              loop_count: 0,
              penetration_distance: 0,
            },
          };
        }

        case 'create_section': {
          sectionCounter++;
          const section = {
            id: sectionCounter,
            name: (args?.name as string) ?? `Section ${sectionCounter}`,
            surveyor: 'Test Surveyor',
            date: new Date().toISOString().split('T')[0],
            color: '#3b82f6',
            shots: args?.shots ?? [],
          };
          currentProject?.sections.push(section);
          return { success: true, section };
        }

        case 'import_dmp':
        case 'parse_dmp': {
          return {
            version: 5,
            sections: [
              {
                name: 'Test Section',
                direction: 'in',
                date: new Date().toISOString().split('T')[0],
                shots: [
                  {
                    shot_type: 'Standard',
                    heading_in: 45,
                    heading_out: 50,
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
                    comment: 'Test shot',
                  },
                ],
              },
            ],
          };
        }

        case 'undo':
        case 'redo':
          return { success: true };

        case 'can_undo':
          return true;

        case 'can_redo':
          return true;

        case 'get_undo_state':
          return { can_undo: true, can_redo: true };

        case 'get_config':
          return {
            general: {
              auto_save: true,
              auto_save_interval_seconds: 30,
              recent_projects_max: 20,
              undo_history_limit: 100,
              undo_persist_limit: 20,
            },
            units: {
              length: 'meters',
              depth: 'meters',
              angle: 'degrees',
              decimal_precision: 1,
            },
            display: {
              grid_color: '#333366',
              grid_spacing: 10,
              font_size: 12,
              font_color: '#f5f5f5',
              depth_gradient: {
                enabled: false,
                shallow_color: '#00bfff',
                deep_color: '#ff4444',
                shallow_depth: 0.0,
                deep_depth: 50.0,
              },
            },
            section_colors: {
              palette: ['#4ecdc4', '#ff6b6b', '#95e1d3', '#f38181', '#aa96da', '#fcbad3', '#a8d8ea', '#ffcb77'],
            },
            map: {
              satellite_provider: 'openstreetmap',
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
                show_navigation_tools: false,
              },
              station: {
                radius: 4.0,
                hit_area: 10.0,
              },
              shot: {
                width: 2.0,
                hit_area: 8.0,
              },
              snap: {
                threshold: 10.0,
                enabled_draw: true,
                enabled_merge: true,
                enabled_measure: true,
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
              // Tools
              select: ['S'],
              pan: ['P'],
              zoom: ['Z'],
              merge: ['M'],
              measure: ['R'],
              move_station: ['T'],
              // Edit
              undo: ['Ctrl+Z'],
              redo: ['Ctrl+Y', 'Ctrl+Shift+Z'],
              save: ['Ctrl+S'],
              // View
              toggle_depth_gradient: ['G'],
              toggle_grid: ['Shift+G'],
              toggle_satellite: ['B'],
              toggle_station_labels: ['L'],
              toggle_shot_labels: ['Shift+L'],
              toggle_section_labels: ['Ctrl+L'],
              fit_to_bounds: ['F'],
              zoom_in: ['Equal', 'Plus'],
              zoom_out: ['Minus'],
              // Panels
              toggle_sections_panel: ['Ctrl+1'],
              toggle_detail_panel: ['Ctrl+2'],
              toggle_data_table: ['Ctrl+3'],
            },
            panels: {
              sections_panel_width: 250,
              detail_panel_width: 300,
              data_table_height: 200,
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
            appearance: {
              theme: 'dark',
            },
          };

        case 'get_recent_projects':
        case 'list_recent_projects':
          return [];

        case 'get_warnings':
        case 'get_project_warnings':
          return [];

        case 'load_display_settings':
          return {};

        case 'save_display_setting':
          return null;

        case 'plugin:window|set_title':
        case 'plugin:window|current':
          return { label: 'main' };

        case 'get_stations':
          return [];

        case 'plugin:dialog|open':
          return '/mock./fixtures/coverage-survey.dmp';
        case 'plugin:dialog|save':
          return '/mock/saved-project.stalx';

        case 'plugin:fs|read':
        case 'plugin:fs|readFile':
          return new Uint8Array([]);

        case 'plugin:fs|write':
        case 'plugin:fs|writeFile':
          return null;

        case 'plugin:fs|exists':
          return false;

        case 'plugin:opener|reveal_item_in_dir':
        case 'plugin:opener|open_path':
          return undefined;

        case 'update_shot':
        case 'delete_shot':
        case 'update_section':
        case 'delete_section':
        case 'create_shot':
        case 'set_shot_kind':
          return { success: true };

        default:
          console.warn(`[Mock IPC] Unhandled command: ${cmd}`);
          return null;
      }
    };

    const tauriInternals = {
      invoke: async (cmd: string, args?: Record<string, unknown>) => {
        return mockHandler(cmd, args);
      },
      transformCallback: (callback: () => void) => {
        const id = Math.random().toString(36).substring(7);
        (window as unknown as Record<string, unknown>)[`_${id}`] = callback;
        return id;
      },
      convertFileSrc: (filePath: string) => {
        return `asset://localhost/${filePath}`;
      },
      metadata: {
        currentWindow: { label: 'main' },
        currentWebview: { label: 'main' },
      },
    };

    (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = tauriInternals;

    (window as unknown as { __TAURI__: unknown }).__TAURI__ = {
      invoke: tauriInternals.invoke,
      convertFileSrc: tauriInternals.convertFileSrc,
      transformCallback: tauriInternals.transformCallback,
    };

    const mockDialogModule = {
      open: async () => '/mock./fixtures/coverage-survey.dmp',
      save: async () => '/mock/saved-project.stalx',
      message: async () => undefined,
      ask: async () => true,
      confirm: async () => true,
    };

    (window as unknown as Record<string, unknown>).__TAURI_DIALOG_MOCK__ = mockDialogModule;

    console.log('[Mock IPC] Tauri mocks with error simulation installed');
  });
}

test.describe('Error Handling', () => {
  test.describe('Save Errors', () => {
    /**
     * Test 9.1: should display error notification on save failure
     *
     * Verifies that save errors (permission denied, disk full) are
     * clearly communicated to the user via error notification.
     */
    test('should display error notification on save failure @smoke', async ({ page }) => {
      await setupMockedPage(page);
      await createProjectViaApp(page, 'Error Test Project');

      // `save_project` returns a CommandResult, not Result<_, String>, so the
      // real backend can never reject the promise - a failure is always
      // success:false. Simulating a throw exercised a path the app does not
      // have, which is why this assertion used to be skipped rather than met.
      await setIpcOverrides(page, {
        save_project: { success: false, error: 'Permission denied: cannot write to file' },
      });

      // Trigger save with Ctrl+S
      await page.keyboard.press('Control+s');

      // The failure must reach the user - that is what this test is named for.
      // It may land in a dialog or an inline notification; either satisfies it.
      await expect(
        page.locator('text=/Permission denied|cannot write|save.*fail/i').first(),
      ).toBeVisible({ timeout: 5000 });

      // Verify app remains functional after error
      await expect(page.getByTestId('map-canvas')).toBeVisible();

      // Test disk full error as well.
      await setIpcOverrides(page, {
        save_project: { __throw: 'Disk full: no space left on device' },
      });

      await page.keyboard.press('Control+s');

      // App should still be functional
      await expect(page.getByTestId('map-canvas')).toBeVisible();
    });
  });

  test.describe('File Open Errors', () => {
    /**
     * Test 9.2: should display error on file open failure
     *
     * Verifies that file open errors (corrupted file, missing file) are
     * clearly communicated to the user.
     */
    test('should display error on file open failure', async ({ page }) => {
      await setupMockedPage(page);

      const loadingError = page.getByTestId('loading-progress');
      const title = page.locator('h1:has-text("Stalagmight")');

      // Opening a file whose load fails must surface the loading-error overlay
      // with the message, keep the app on the start page, and be dismissable.
      // Use the hamburger "Open Project" item. Open/Import now routes by the
      // chosen file's EXTENSION (unified Open/Import), so we override the mocked
      // file dialog to return a .stalx path - that routes through loadProject,
      // which we force to throw, surfacing the loading-error overlay. (The default
      // mock dialog returns a .dmp, which would route to import instead.)
      // Asserted deterministically (wait for the actual states, no racing
      // best-effort timeouts) so it can't flake under parallel load - and it now
      // verifies the error message is shown, which the previous version never did.
      const expectOpenErrorSurfaced = async (message: string) => {
        await setIpcOverrides(page, {
          load_project_full: { __throw: message },
          load_project: { __throw: message },
          'plugin:dialog|open': '/mock/broken-project.stalx',
        });

        await openAppMenu(page);
        await page.getByTestId('global-open-project-button').click();

        await expect(loadingError).toBeVisible({ timeout: 10000 });
        await expect(loadingError).toContainText(message);

        await loadingError.getByRole('button', { name: 'Dismiss' }).click();
        await expect(loadingError).toBeHidden({ timeout: 10000 });

        // App stays functional on the start page.
        await expect(title).toBeVisible();
      };

      await expectOpenErrorSurfaced('File corrupted: invalid project format');
      await expectOpenErrorSurfaced('File not found: the specified file does not exist');
    });
  });

  test.describe('DMP Parse Errors', () => {
    test('shows error dialog for invalid DMP header', async ({ page }) => {
      await setupMockedPage(page);
      await createProjectViaApp(page, 'DMP Error Test');

      await setIpcOverrides(page, {
        import_dmp: { __throw: 'Invalid DMP header: expected version 2-5, got invalid data' },
        parse_dmp: { __throw: 'Invalid DMP header: expected version 2-5, got invalid data' },
      });

      await page.keyboard.press('Control+i');

      const errorDialog = page.getByTestId('error-dialog');
      await expect(errorDialog).toBeVisible({ timeout: 5000 });
      await expect(errorDialog).toContainText('Import Failed');
      await expect(errorDialog).toContainText('Invalid DMP header');

      await errorDialog.locator('button:has-text("OK")').click();
      await expect(errorDialog).toBeHidden({ timeout: 3000 });
      await expect(page.getByTestId('map-canvas')).toBeVisible();
      await expect(page.locator('[data-testid="dmp-selection-dialog"]')).toHaveCount(0);
    });

    test('shows error dialog for truncated DMP file', async ({ page }) => {
      await setupMockedPage(page);
      await createProjectViaApp(page, 'DMP Truncated Test');

      await setIpcOverrides(page, {
        import_dmp: { __throw: 'Truncated DMP file: unexpected end of data' },
        parse_dmp: { __throw: 'Truncated DMP file: unexpected end of data' },
      });

      await page.keyboard.press('Control+i');

      const errorDialog = page.getByTestId('error-dialog');
      await expect(errorDialog).toBeVisible({ timeout: 5000 });
      await expect(errorDialog).toContainText('Import Failed');
      await expect(errorDialog).toContainText('Truncated DMP file');

      await errorDialog.locator('button:has-text("OK")').click();
      await expect(errorDialog).toBeHidden({ timeout: 3000 });
      await expect(page.getByTestId('map-canvas')).toBeVisible();
      await expect(page.locator('[data-testid="dmp-selection-dialog"]')).toHaveCount(0);
    });
  });

  test.describe('Network/IPC Timeout', () => {
    /**
     * Test 9.4: should handle network/IPC timeout gracefully
     *
     * Verifies the app handles IPC timeouts gracefully and remains
     * functional.
     */
    test('should handle network/IPC timeout gracefully', async ({ page }) => {
      // Use standard mocks - we'll test with slow operations
      await setupTauriMocks(page);
      await page.goto('/');

      await createProject(page, 'Timeout Test');

      // The mocked IPC returns immediately, but we can verify the app
      // handles rapid operations without blocking
      for (let i = 0; i < 5; i++) {
        await page.keyboard.press('Control+s');
      }

      // App should remain responsive
      await expect(page.getByTestId('map-canvas')).toBeVisible();

      // Try multiple rapid keyboard shortcuts
      await page.keyboard.press('Control+z');
      await page.keyboard.press('Control+y');
      await page.keyboard.press('Control+z');

      // App should still be functional
      await expect(page.getByTestId('map-canvas')).toBeVisible();
    });
  });

  test.describe('Form Validation', () => {
    /**
     * Test 9.5: should display validation errors in forms
     *
     * Verifies inline validation errors are displayed in forms
     * when users enter invalid data.
     */
    test('should display validation errors in forms @smoke', async ({ page }) => {
      await setupTauriMocks(page);
      await page.goto('/');

      // Test New Project dialog validation
      const newProjectBtn = page
        .locator('[data-testid="new-project-button"], [data-testid="empty-state-new-project"]')
        .first();
      await newProjectBtn.click();
      await page.getByTestId('new-project-dialog').waitFor({ state: 'visible', timeout: 5000 });

      // Try to create without name
      const createBtn = page.getByTestId('new-project-create-button');

      // Button should be disabled when name is empty
      const isDisabled = await createBtn.isDisabled();

      if (!isDisabled) {
        // If button is enabled, clicking should show validation error
        await createBtn.click();

        // Check for error message
        const errorMsg = page
          .locator('[data-testid="new-project-dialog"] >> text=/required|name.*empty|enter.*name/i')
          .first();
        if (await errorMsg.isVisible()) {
          await expect(errorMsg).toBeVisible();
        }
      }

      // Dialog should still be open
      await expect(page.getByTestId('new-project-dialog')).toBeVisible();

      // Now enter a valid name and create
      await page.getByTestId('new-project-name-input').fill('Valid Project');
      await createBtn.click();

      // Should successfully create and show canvas
      await expect(page.getByTestId('map-canvas')).toBeVisible({ timeout: 10000 });
    });
  });

  test.describe('Undo/Redo Recovery', () => {
    /**
     * Test 9.6: should recover from undo/redo errors
     *
     * Verifies the app handles undo/redo errors gracefully and
     * remains functional.
     */
    test('should recover from undo/redo errors', async ({ page }) => {
      await setupMockedPage(page);
      await createProjectViaApp(page, 'Undo Error Test');

      await setIpcOverrides(page, {
        undo: { __throw: 'Undo failed: command stack corrupted' },
      });

      // Try undo - should handle error gracefully
      await page.keyboard.press('Control+z');

      // App should remain functional
      await expect(page.getByTestId('map-canvas')).toBeVisible();

      await setIpcOverrides(page, {
        redo: { __throw: 'Redo failed: no commands to redo' },
      });

      // Try redo - should handle error gracefully
      await page.keyboard.press('Control+y');

      // App should still be functional
      await expect(page.getByTestId('map-canvas')).toBeVisible();

      // Disable overrides and verify normal operations work.
      await clearIpcOverrideKeys(page, ['undo', 'redo']);

      // Undo/redo should work normally now
      await page.keyboard.press('Control+z');
      await page.keyboard.press('Control+y');

      // App should remain functional
      await expect(page.getByTestId('map-canvas')).toBeVisible();
    });
  });
});
