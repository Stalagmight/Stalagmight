import { test, expect, type Page } from './fixtures/coverage';
import { setupTauriMocks } from './mocks/tauri-ipc';
import { createProject } from './helpers/app';
import type { E2EWindow } from './helpers/e2eWindow';

/** Where keyboard focus is: the focused element's test id (or tag), and whether it is in the loading window. */
async function focusSpot(page: Page): Promise<{ id: string; inWindow: boolean }> {
  return page.evaluate(() => {
    const active = document.activeElement;
    const overlay = document.querySelector('[data-testid="project-loading-overlay"]');
    return {
      id: active?.getAttribute('data-testid') ?? active?.tagName.toLowerCase() ?? 'none',
      inWindow: overlay !== null && active !== null && overlay.contains(active),
    };
  });
}

/**
 * Press Tab `count` times, then Shift+Tab `count` times, and return every press
 * after which focus was NOT in the loading window (empty when it held focus).
 */
async function focusEscapes(page: Page, count: number): Promise<string[]> {
  const escapes: string[] = [];
  for (const key of ['Tab', 'Shift+Tab']) {
    for (let i = 1; i <= count; i++) {
      await page.keyboard.press(key);
      const spot = await focusSpot(page);
      if (!spot.inWindow) escapes.push(`${key} #${i} -> ${spot.id}`);
    }
  }
  return escapes;
}

/**
 * Note the target of every keydown and click from now on, so a test can show that
 * a key pressed during a load reached nothing behind the loading window.
 */
async function recordKeyTargets(page: Page): Promise<void> {
  await page.evaluate(() => {
    const w = window as unknown as { __loadingKeyTargets?: string[] };
    w.__loadingKeyTargets = [];
    const note = (e: Event): void => {
      const target = e.target instanceof Element ? e.target : null;
      const overlay = document.querySelector('[data-testid="project-loading-overlay"]');
      if (overlay !== null && target !== null && overlay.contains(target)) return;
      const id = target?.getAttribute('data-testid') ?? target?.tagName.toLowerCase() ?? 'none';
      w.__loadingKeyTargets?.push(`${e.type} on ${id}`);
    };
    document.addEventListener('keydown', note, true);
    document.addEventListener('click', note, true);
  });
}

/** The keydowns and clicks since `recordKeyTargets` that landed outside the loading window. */
async function keyTargetsBehind(page: Page): Promise<string[]> {
  return page.evaluate(() => (window as unknown as { __loadingKeyTargets?: string[] }).__loadingKeyTargets ?? []);
}

/** Hold a load in progress (nothing ends it but Cancel / Dismiss). */
async function holdLoad(page: Page): Promise<void> {
  await page.evaluate(() => {
    const e2e = (window as unknown as { __E2E_TEST__?: { simulateLoadingProgress?: (payload: Record<string, unknown>) => void } }).__E2E_TEST__;
    e2e?.simulateLoadingProgress?.({
      isLoading: true,
      projectName: 'CaveB',
      projectPath: '/caves/CaveB.stalx',
      phase: 'loading_project_data',
      overallProgress: 25,
    });
  });
}

/** Fail the held load: the error state with details, Retry, Dismiss and Copy. */
async function failLoad(page: Page): Promise<void> {
  await page.evaluate(() => {
    const e2e = (window as unknown as { __E2E_TEST__?: { simulateLoadingError?: (msg: string, details?: string) => void } }).__E2E_TEST__;
    e2e?.simulateLoadingError?.('Could not read the project file.', 'ENOENT: /caves/CaveB.stalx');
  });
}

test.describe('Loading Progress', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    // Wait for GlobalView to load with recent projects
    await page.waitForSelector('[data-testid="global-view"]', { timeout: 10000 });
  });

  test.describe('User Flow Tests (open project via double-click)', () => {
    test('double-clicking project card triggers load_project_full', async ({ page }) => {
      // Wait for project list to be visible first
      await page.waitForSelector('h3:has-text("Cave System Alpha")', { timeout: 5000 });

      // Track IPC calls - must be set up before the action
      const ipcCalls: string[] = [];
      page.on('console', (msg) => {
        const text = msg.text();
        if (text.includes('load_project_full')) {
          ipcCalls.push(text);
        }
      });

      // Double-click on a project card
      await page.locator('h3:has-text("Cave System Alpha")').dblclick();

      // Verify load_project_full was called (poll until the IPC log arrives)
      await expect.poll(() => ipcCalls.length).toBeGreaterThan(0);
    });

    test('load_project_full emits progress events', async ({ page }) => {
      // Wait for project list
      await page.waitForSelector('h3:has-text("Cave System Alpha")', { timeout: 5000 });

      // Track event emissions - look for any loading-progress related log
      const eventLogs: string[] = [];
      page.on('console', (msg) => {
        const text = msg.text();
        // Capture any console log mentioning loading-progress (from emitEvent calls)
        if (text.includes('loading-progress')) {
          eventLogs.push(text);
        }
      });

      // Double-click to start loading
      await page.locator('h3:has-text("Cave System Alpha")').dblclick();

      // Verify loading-progress events were emitted during load_project_full
      await expect.poll(() => eventLogs.length).toBeGreaterThan(0);
    });

    test('clientOpId increments on each project open', async ({ page }) => {
      // Wait for project list
      await page.waitForSelector('h3:has-text("Cave System Alpha")', { timeout: 5000 });

      // Get initial clientOpId
      const idBefore = await page.evaluate(() => {
        const e2e = (window as unknown as { __E2E_TEST__?: { getLoadingId?: () => number } }).__E2E_TEST__;
        return e2e?.getLoadingId?.() ?? 0;
      });

      // Open a project
      await page.locator('h3:has-text("Cave System Alpha")').dblclick();

      // clientOpId should have incremented from the load (poll the store)
      await expect.poll(() => page.evaluate(() => {
        const e2e = (window as unknown as { __E2E_TEST__?: { getLoadingId?: () => number } }).__E2E_TEST__;
        return e2e?.getLoadingId?.() ?? 0;
      })).toBeGreaterThan(idBefore);
    });
  });

  test.describe('Event System Tests', () => {
    test('plugin:event|listen is called when app starts listening', async ({ page }) => {
      // Verify plugin:event|listen is called when useLoadingProgress registers
      const consoleLogs: string[] = [];
      page.on('console', (msg) => {
        if (msg.text().includes('[Mock IPC] plugin:event|listen')) {
          consoleLogs.push(msg.text());
        }
      });

      // Trigger a loading state which activates useLoadingProgress hook
      await page.evaluate(() => {
        const e2e = (window as unknown as { __E2E_TEST__?: { simulateLoadingProgress?: (payload: Record<string, unknown>) => void } }).__E2E_TEST__;
        e2e?.simulateLoadingProgress?.({
          isLoading: true,
          projectName: 'Test Project',
          phase: 'loading_project_data',
          phaseIndex: 1,
          phaseCount: 8,
          overallProgress: 25,
        });
      });

      // Verify plugin:event|listen was called for loading-progress
      // (poll until React has registered the listener)
      await expect.poll(() => consoleLogs.some(log =>
        log.includes('loading-progress')
      )).toBe(true);
    });

    test('event system correctly routes loading-progress events', async ({ page }) => {
      // Verify the event mock emit function works with the plugin:event system
      const eventResult = await page.evaluate(() => {
        return new Promise<{ received: boolean; payload: unknown }>((resolve) => {
          // Register a listener using the plugin:event|listen pathway
          const handler = (event: { payload: unknown }) => {
            resolve({ received: true, payload: event.payload });
          };

          // Store callback for transformCallback
          const callbackId = Math.random().toString(36).substring(7);
          (window as unknown as Record<string, unknown>)[`_${callbackId}`] = handler;

          // Simulate plugin:event|listen
          const tauriInternals = (window as unknown as { __TAURI_INTERNALS__: { invoke: (cmd: string, args: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__;
          tauriInternals.invoke('plugin:event|listen', { event: 'test-event', handler: callbackId });

          // Emit via __tauriEventMock
          setTimeout(() => {
            const mock = (window as unknown as { __tauriEventMock?: { emit: (event: string, payload: unknown) => void } }).__tauriEventMock;
            if (mock) {
              mock.emit('test-event', { test: 'data' });
            }
          }, 50);

          // Timeout fallback
          setTimeout(() => resolve({ received: false, payload: null }), 500);
        });
      });

      expect(eventResult.received).toBe(true);
      expect(eventResult.payload).toEqual({ test: 'data' });
    });
  });

  test.describe('Direct State Manipulation Tests', () => {
    test('shows progress UI when simulated via E2E hook', async ({ page }) => {
      // Use E2E hooks to manually trigger loading state
      await page.evaluate(() => {
        const e2e = (window as unknown as { __E2E_TEST__?: { simulateLoadingProgress?: (payload: Record<string, unknown>) => void } }).__E2E_TEST__;
        e2e?.simulateLoadingProgress?.({
          isLoading: true,
          projectName: 'Test Project',
          phase: 'loading_project_data',
          phaseIndex: 1,
          phaseCount: 8,
          overallProgress: 25,
        });
      });

      // Progress UI should appear
      await expect(page.locator('[data-testid="loading-progress"]')).toBeVisible();
    });

    test('shows project name in loading title', async ({ page }) => {
      await page.evaluate(() => {
        const e2e = (window as unknown as { __E2E_TEST__?: { simulateLoadingProgress?: (payload: Record<string, unknown>) => void } }).__E2E_TEST__;
        e2e?.simulateLoadingProgress?.({
          isLoading: true,
          projectName: 'Test Cave Project',
          phase: 'loading_project_data',
          phaseIndex: 1,
          phaseCount: 8,
          overallProgress: 25,
        });
      });

      await expect(page.getByText(/Loading Test Cave Project\.\.\./)).toBeVisible();
    });

    test('shows phase updates with step numbers', async ({ page }) => {
      await page.evaluate(() => {
        const e2e = (window as unknown as { __E2E_TEST__?: { simulateLoadingProgress?: (payload: Record<string, unknown>) => void } }).__E2E_TEST__;
        e2e?.simulateLoadingProgress?.({
          isLoading: true,
          projectName: 'Test Project',
          phase: 'computing_stations',
          phaseIndex: 3,
          phaseCount: 8,
          overallProgress: 50,
        });
      });

      // Phase text should show "Step N of M:" format (4 of 8 because phaseIndex is 0-based)
      await expect(page.getByText(/Step 4 of 8:/)).toBeVisible();
    });

    test('shows phase label during loading', async ({ page }) => {
      await page.evaluate(() => {
        const e2e = (window as unknown as { __E2E_TEST__?: { simulateLoadingProgress?: (payload: Record<string, unknown>) => void } }).__E2E_TEST__;
        e2e?.simulateLoadingProgress?.({
          isLoading: true,
          projectName: 'Test Project',
          phase: 'opening_database',
          phaseIndex: 0,
          phaseCount: 8,
          overallProgress: 5,
        });
      });

      // Phase text should be visible (Opening database... formatted)
      await expect(page.getByText(/Opening database/)).toBeVisible();
    });

    test('cancel button dismisses loading UI', async ({ page }) => {
      await page.evaluate(() => {
        const e2e = (window as unknown as { __E2E_TEST__?: { simulateLoadingProgress?: (payload: Record<string, unknown>) => void } }).__E2E_TEST__;
        e2e?.simulateLoadingProgress?.({
          isLoading: true,
          projectName: 'Test Project',
          phase: 'loading_project_data',
          phaseIndex: 1,
          phaseCount: 8,
          overallProgress: 25,
        });
      });

      await expect(page.locator('[data-testid="loading-progress"]')).toBeVisible();
      await page.getByText('Cancel').click();

      await expect(page.locator('[data-testid="loading-progress"]')).not.toBeVisible();
      await expect(page.locator('[data-testid="global-view"]')).toBeVisible();
    });

    test('stale events are ignored after cancel', async ({ page }) => {
      // Start a load with a specific loadingId
      const loadingId = Date.now();
      await page.evaluate((id) => {
        const e2e = (window as unknown as { __E2E_TEST__?: { simulateLoadingProgress?: (payload: Record<string, unknown>) => void } }).__E2E_TEST__;
        e2e?.simulateLoadingProgress?.({
          isLoading: true,
          loadingId: id,
          clientOpId: 1,
          projectName: 'Test Project',
          phase: 'loading_project_data',
          phaseIndex: 1,
          phaseCount: 8,
          overallProgress: 25,
        });
      }, loadingId);

      await expect(page.locator('[data-testid="loading-progress"]')).toBeVisible();

      // Cancel/reset loading
      await page.evaluate(() => {
        const e2e = (window as unknown as { __E2E_TEST__?: { resetLoading?: () => void } }).__E2E_TEST__;
        e2e?.resetLoading?.();
      });

      await expect(page.locator('[data-testid="loading-progress"]')).not.toBeVisible();

      // Emit stale event with the old loadingId - should be ignored since not loading
      await page.evaluate((id) => {
        const mock = (window as unknown as { __tauriEventMock?: { emit: (event: string, payload: unknown) => void } }).__tauriEventMock;
        if (mock) {
          mock.emit('loading-progress', {
            loadingId: id,
            phase: 'finalizing',
            phaseIndex: 7,
            phaseCount: 8,
            current: null,
            total: null,
            overallPercent: 100,
            isIndeterminate: true,
          });
        }
      }, loadingId);

      // Should still be at GlobalView with no loading UI
      await expect(page.locator('[data-testid="global-view"]')).toBeVisible();
      await expect(page.locator('[data-testid="loading-progress"]')).not.toBeVisible();
    });

    test('clientOpId can be set via simulateLoadingProgress', async ({ page }) => {
      const idBefore = await page.evaluate(() => {
        const e2e = (window as unknown as { __E2E_TEST__?: { getLoadingId?: () => number } }).__E2E_TEST__;
        return e2e?.getLoadingId?.() ?? 0;
      });

      await page.evaluate(() => {
        const e2e = (window as unknown as { __E2E_TEST__?: { simulateLoadingProgress?: (payload: Record<string, unknown>) => void } }).__E2E_TEST__;
        e2e?.simulateLoadingProgress?.({
          isLoading: true,
          clientOpId: 42,
          projectName: 'Test Project',
          phase: 'loading_project_data',
          overallProgress: 25,
        });
      });

      const idAfter = await page.evaluate(() => {
        const e2e = (window as unknown as { __E2E_TEST__?: { getLoadingId?: () => number } }).__E2E_TEST__;
        return e2e?.getLoadingId?.() ?? 0;
      });

      expect(idAfter).toBe(42);
      expect(idAfter).toBeGreaterThan(idBefore);
    });

    test('error state shows retry and dismiss buttons', async ({ page }) => {
      await page.evaluate(() => {
        const e2e = (window as unknown as { __E2E_TEST__?: { simulateLoadingProgress?: (payload: Record<string, unknown>) => void } }).__E2E_TEST__;
        e2e?.simulateLoadingProgress?.({
          isLoading: true,
          projectName: 'Test Project',
          projectPath: './fixtures/coverage/path.stalx',
          phase: 'loading_project_data',
          overallProgress: 25,
        });
      });

      await expect(page.locator('[data-testid="loading-progress"]')).toBeVisible();

      // Simulate error
      await page.evaluate(() => {
        const e2e = (window as unknown as { __E2E_TEST__?: { simulateLoadingError?: (msg: string, details?: string) => void } }).__E2E_TEST__;
        e2e?.simulateLoadingError?.('Test error', 'Error details here');
      });

      await expect(page.getByText('Retry')).toBeVisible();
      await expect(page.getByText('Dismiss')).toBeVisible();
    });
  });

  // [#576 LOAD-VIEW-D1] A load started from the project view (Discard in the
  // unsaved-changes prompt) shows the same overlay, over the project: it blocks
  // the Display menu and the shortcuts, and a failed load's error is on screen
  // until dismissed. Before, the overlay lived in the project list only.
  test.describe('In the project view', () => {
    test('a load shows the overlay over the project, blocks the menus and shortcuts, and its error can be dismissed', async ({ page }) => {
      await createProject(page, 'CaveA');
      await page.evaluate(() => {
        const e2e = (window as unknown as { __E2E_TEST__?: { simulateLoadingProgress?: (payload: Record<string, unknown>) => void } }).__E2E_TEST__;
        e2e?.simulateLoadingProgress?.({
          isLoading: true,
          projectName: 'CaveB',
          projectPath: '/caves/CaveB.stalx',
          phase: 'loading_project_data',
          overallProgress: 25,
        });
      });

      await expect(page.getByTestId('loading-progress')).toBeVisible();
      await expect(page.getByText('Loading CaveB')).toBeVisible();
      await expect(page.getByTestId('single-system-view')).toBeVisible();

      // The Display menu button is under the overlay: a click there lands on the overlay.
      const displayMenuReachable = await page.evaluate(() => {
        const button = document.querySelector('[data-testid="display-menu-button"]');
        if (!button) return null;
        const r = button.getBoundingClientRect();
        const top = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
        return top !== null && button.contains(top);
      });
      expect(displayMenuReachable).toBe(false);

      // The shortcuts stand down: '?' would open the keyboard-shortcuts dialog.
      await page.keyboard.press('?');
      expect(await page.evaluate(() => (window as E2EWindow).__E2E_TEST__.uiStore.getState().screenOccupants)).toBeGreaterThan(0);
      expect(await page.evaluate(() => (window as E2EWindow).__E2E_TEST__.uiStore.getState().activeDialog)).toBeNull();

      await page.evaluate(() => {
        const e2e = (window as unknown as { __E2E_TEST__?: { simulateLoadingError?: (msg: string, details?: string) => void } }).__E2E_TEST__;
        e2e?.simulateLoadingError?.('Could not read the project file.');
      });
      await expect(page.getByText('Could not read the project file.')).toBeVisible();
      await expect(page.getByTestId('loading-error-retry')).toBeVisible();

      await page.getByTestId('loading-error-dismiss').click();
      await expect(page.getByTestId('loading-progress')).toHaveCount(0);
      expect(await page.evaluate(() => (window as E2EWindow).__E2E_TEST__.projectStore.getState().loading.isLoading)).toBe(false);
      expect(await page.evaluate(() => (window as E2EWindow).__E2E_TEST__.uiStore.getState().screenOccupants)).toBe(0);
      await expect(page.getByTestId('single-system-view')).toBeVisible();
    });

    // [W71-NF1] The loading window is modal for the keyboard as well: Tab and
    // Shift+Tab used to walk out of it onto the toolbar and the panels under it,
    // where Enter / Space acted. Focus now moves into it, stays there (in progress
    // and in its error state), and goes back where it was when the window closes.
    test('the loading window holds keyboard focus, in progress and in its error state, and gives it back when it closes', async ({ page }) => {
      await createProject(page, 'CaveA');
      const trigger = page.getByTestId('display-menu-button');
      await trigger.focus();
      await expect(trigger).toBeFocused();

      await holdLoad(page);
      await expect(page.getByTestId('loading-progress')).toBeVisible();
      await expect(page.getByTestId('loading-progress-cancel')).toBeFocused();
      await recordKeyTargets(page);
      // More presses each way than the window has controls.
      expect(await focusEscapes(page, 4)).toEqual([]);

      // Space presses the focused Cancel: the load ends and focus is back on the toolbar.
      await page.keyboard.press('Space');
      await expect(page.getByTestId('project-loading-overlay')).toHaveCount(0);
      await expect(trigger).toBeFocused();
      expect(await keyTargetsBehind(page)).toEqual([]);
      await expect(page.getByTestId('display-menu')).toHaveCount(0);

      // The error state: focus moves to its first control when it replaces the progress.
      await holdLoad(page);
      await expect(page.getByTestId('loading-progress-cancel')).toBeFocused();
      await failLoad(page);
      await expect(page.getByTestId('loading-error-details-toggle')).toBeFocused();
      await recordKeyTargets(page);
      expect(await focusEscapes(page, 6)).toEqual([]);
      // Details, Retry, Dismiss: Enter on Dismiss closes it and focus is back on the toolbar.
      await page.keyboard.press('Tab');
      await page.keyboard.press('Tab');
      await expect(page.getByTestId('loading-error-dismiss')).toBeFocused();
      await page.keyboard.press('Enter');
      await expect(page.getByTestId('project-loading-overlay')).toHaveCount(0);
      await expect(trigger).toBeFocused();
      expect(await keyTargetsBehind(page)).toEqual([]);
      await expect(page.getByTestId('display-menu')).toHaveCount(0);
      await expect(page.getByTestId('single-system-view')).toBeVisible();
    });
  });

  test.describe('In the project list', () => {
    // [#576 Q-W60-1-D1] The project list's loading window holds the screen the way the
    // project view's does (LOAD-VIEW-D1): menus, shortcuts and an OS-launched file all
    // wait until the load ends or is cancelled.
    test('a load holds the screen until it ends: menus, shortcuts and a launched file wait', async ({ page }) => {
      await page.evaluate(() => {
        const w = window as unknown as {
          __MOCK_IPC_OVERRIDES__?: Record<string, unknown>;
          __pickerCalls?: number;
          __launchFetches?: number;
        };
        const overrides = { ...(w.__MOCK_IPC_OVERRIDES__ ?? {}) };
        Object.defineProperty(overrides, 'plugin:dialog|open', {
          enumerable: true,
          configurable: true,
          get() {
            w.__pickerCalls = (w.__pickerCalls ?? 0) + 1;
            return null;
          },
        });
        Object.defineProperty(overrides, 'take_pending_launch_files', {
          enumerable: true,
          configurable: true,
          get() {
            w.__launchFetches = (w.__launchFetches ?? 0) + 1;
            return [];
          },
        });
        w.__MOCK_IPC_OVERRIDES__ = overrides;
        const e2e = (window as unknown as { __E2E_TEST__?: { simulateLoadingProgress?: (payload: Record<string, unknown>) => void } }).__E2E_TEST__;
        e2e?.simulateLoadingProgress?.({
          isLoading: true,
          projectName: 'CaveB',
          projectPath: '/caves/CaveB.stalx',
          phase: 'loading_project_data',
          overallProgress: 25,
        });
      });
      const counters = () =>
        page.evaluate(() => {
          const w = window as unknown as E2EWindow & { __pickerCalls?: number; __launchFetches?: number };
          return {
            occupants: w.__E2E_TEST__.uiStore.getState().screenOccupants as number,
            activeDialog: w.__E2E_TEST__.uiStore.getState().activeDialog,
            pickerCalls: w.__pickerCalls ?? 0,
            launchFetches: w.__launchFetches ?? 0,
          };
        });

      await expect(page.getByTestId('loading-progress')).toBeVisible();
      await expect(page.getByText('Loading CaveB')).toBeVisible();
      await expect(page.getByTestId('global-view')).toBeVisible();

      // The toolbar's Open / Import button is under the overlay: a click there lands on the overlay.
      const openButtonReachable = await page.evaluate(() => {
        const button = document.querySelector('[data-testid="global-open-import-button"]');
        if (!button) return null;
        const r = button.getBoundingClientRect();
        const top = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
        return top !== null && button.contains(top);
      });
      expect(openButtonReachable).toBe(false);

      // The loading window holds the screen, so the shortcuts stand down and an
      // OS-launched file stays in the launch queue instead of replacing the load.
      await page.keyboard.press('Control+o');
      await page.keyboard.press('Control+n');
      await page.evaluate(() =>
        (window as unknown as { __tauriEventMock: { emit: (e: string, p: unknown) => void } }).__tauriEventMock.emit('launch-file-queued', null),
      );
      expect(await counters()).toEqual({ occupants: 1, activeDialog: null, pickerCalls: 0, launchFetches: 0 });

      // The load fails; its error stays up (and holds the screen) until dismissed.
      await page.evaluate(() => {
        const e2e = (window as unknown as { __E2E_TEST__?: { simulateLoadingError?: (msg: string, details?: string) => void } }).__E2E_TEST__;
        e2e?.simulateLoadingError?.('Could not read the project file.');
      });
      await expect(page.getByText('Could not read the project file.')).toBeVisible();
      expect((await counters()).occupants).toBe(1);

      await page.getByTestId('loading-error-dismiss').click();
      await expect(page.getByTestId('loading-progress')).toHaveCount(0);
      // The screen is free: the launched file that waited is taken now.
      await expect.poll(async () => (await counters()).launchFetches).toBeGreaterThan(0);
      expect((await counters()).occupants).toBe(0);
    });

    // [W71-NF1] Tab and Shift+Tab used to walk out of the loading window onto the
    // cards, the search field and the sort / density controls under it, and Enter /
    // Space acted there. Focus now moves into the window, stays there, and goes back
    // to where it was when the load is cancelled.
    test('the loading window holds keyboard focus and gives it back when the load is cancelled', async ({ page }) => {
      const selection = () =>
        page.evaluate(() =>
          (window as unknown as { __E2E_TEST__: { globalViewStore: { getState: () => { selectedSystemIds: unknown } } } })
            .__E2E_TEST__.globalViewStore.getState().selectedSystemIds,
        );
      const card = page.locator('[role="button"][data-testid^="system-card-"]').first();
      await card.focus();
      await expect(card).toBeFocused();
      const selectedBefore = await selection();

      await holdLoad(page);
      await expect(page.getByTestId('loading-progress')).toBeVisible();
      await expect(page.getByTestId('loading-progress-cancel')).toBeFocused();
      await recordKeyTargets(page);
      // More presses each way than the window has controls (and than the list has cards).
      expect(await focusEscapes(page, 6)).toEqual([]);

      // Enter presses the focused Cancel: the load ends and focus is back on the card.
      await page.keyboard.press('Enter');
      await expect(page.getByTestId('project-loading-overlay')).toHaveCount(0);
      await expect(card).toBeFocused();
      expect(await keyTargetsBehind(page)).toEqual([]);
      expect(await selection()).toEqual(selectedBefore);
      expect(await page.evaluate(() => (window as E2EWindow).__E2E_TEST__.uiStore.getState().screenOccupants)).toBe(0);
    });

    test('the loading window holds keyboard focus in its error state, and Dismiss gives it back', async ({ page }) => {
      const search = page.getByTestId('globalview-search-input');
      await search.focus();
      await expect(search).toBeFocused();

      await holdLoad(page);
      await failLoad(page);
      await expect(page.getByTestId('loading-error-details-toggle')).toBeFocused();
      await recordKeyTargets(page);
      expect(await focusEscapes(page, 6)).toEqual([]);

      // Enter and Space act on the focused control in the window, nothing behind it.
      const details = page.getByTestId('loading-error-details-toggle');
      await expect(details).toBeFocused();
      await page.keyboard.press('Enter');
      await expect(details).toHaveAttribute('aria-expanded', 'true');
      await page.keyboard.press('Space');
      await expect(details).toHaveAttribute('aria-expanded', 'false');
      expect(await keyTargetsBehind(page)).toEqual([]);
      await expect(search).toHaveValue('');

      // Details, Retry, Dismiss: Enter on Dismiss closes it and focus is back in the search field.
      await page.keyboard.press('Tab');
      await page.keyboard.press('Tab');
      await expect(page.getByTestId('loading-error-dismiss')).toBeFocused();
      await page.keyboard.press('Enter');
      await expect(page.getByTestId('project-loading-overlay')).toHaveCount(0);
      await expect(search).toBeFocused();
      expect(await keyTargetsBehind(page)).toEqual([]);
    });

    // A dialog keeps its place above the loading window whichever came first (the
    // survey-file imports raise the window under their own open dialog), and focus
    // goes back to the window when a dialog raised over it closes.
    test('a dialog stays above the loading window, and focus goes back to the window when it closes', async ({ page }) => {
      const openShortcuts = () =>
        page.evaluate(() => (window as E2EWindow).__E2E_TEST__.uiStore.getState().openDialog('keyboard-shortcuts'));
      const dialogOnTop = () =>
        page.evaluate(() => {
          const dialog = document.querySelector('[data-testid="keyboard-shortcuts-dialog"]');
          const overlay = document.querySelector('[data-testid="project-loading-overlay"]');
          if (!dialog || !overlay) return null;
          const r = dialog.getBoundingClientRect();
          const top = document.elementFromPoint(r.left + r.width / 2, r.top + 8);
          return {
            painted: top !== null && dialog.contains(top),
            focusInDialog: dialog.contains(document.activeElement),
            windowInert: overlay.closest('[inert]') !== null,
          };
        });

      // The dialog first, then a load: the dialog stays on top and keeps focus.
      await openShortcuts();
      await expect(page.getByTestId('keyboard-shortcuts-dialog')).toBeVisible();
      await expect.poll(() => focusSpot(page)).toEqual({ id: 'keyboard-shortcuts-search', inWindow: false });
      await holdLoad(page);
      await expect(page.getByTestId('project-loading-overlay')).toBeAttached();
      expect(await dialogOnTop()).toEqual({ painted: true, focusInDialog: true, windowInert: true });
      await page.evaluate(() => (window as unknown as { __E2E_TEST__: { resetLoading: () => void } }).__E2E_TEST__.resetLoading());
      await expect(page.getByTestId('project-loading-overlay')).toHaveCount(0);
      expect((await focusSpot(page)).id).toBe('keyboard-shortcuts-search');
      await page.keyboard.press('Escape');
      await expect(page.getByTestId('keyboard-shortcuts-dialog')).toHaveCount(0);

      // A load, then a dialog over it: the dialog is on top and has focus; closing it
      // hands focus back to the window, which still holds it.
      await holdLoad(page);
      await expect(page.getByTestId('loading-progress-cancel')).toBeFocused();
      await openShortcuts();
      await expect(page.getByTestId('keyboard-shortcuts-dialog')).toBeVisible();
      await expect.poll(dialogOnTop).toEqual({ painted: true, focusInDialog: true, windowInert: true });
      await page.keyboard.press('Escape');
      await expect(page.getByTestId('keyboard-shortcuts-dialog')).toHaveCount(0);
      await expect(page.getByTestId('loading-progress-cancel')).toBeFocused();
      expect(await focusEscapes(page, 3)).toEqual([]);
    });
  });

  test.describe('E2E Hook Availability', () => {
    test('loading E2E hooks are exposed globally (always, not DEV-gated)', async ({ page }) => {
      // These hooks are exposed for E2E tests even in production builds
      const hooks = await page.evaluate(() => {
        const e2e = (window as unknown as { __E2E_TEST__?: Record<string, unknown> }).__E2E_TEST__;
        return {
          hasGetLoadingId: typeof e2e?.getLoadingId === 'function',
          hasGetBackendLoadingId: typeof e2e?.getBackendLoadingId === 'function',
          hasSimulateLoadingProgress: typeof e2e?.simulateLoadingProgress === 'function',
          hasSimulateLoadingError: typeof e2e?.simulateLoadingError === 'function',
          hasResetLoading: typeof e2e?.resetLoading === 'function',
        };
      });

      expect(hooks.hasGetLoadingId).toBe(true);
      expect(hooks.hasGetBackendLoadingId).toBe(true);
      expect(hooks.hasSimulateLoadingProgress).toBe(true);
      expect(hooks.hasSimulateLoadingError).toBe(true);
      expect(hooks.hasResetLoading).toBe(true);
    });

    test('event mock listen/unlisten/emit functions are available', async ({ page }) => {
      const hasEventMock = await page.evaluate(() => {
        const mock = (window as unknown as { __tauriEventMock?: { listen?: unknown; unlisten?: unknown; emit?: unknown } }).__tauriEventMock;
        return {
          hasListen: typeof mock?.listen === 'function',
          hasUnlisten: typeof mock?.unlisten === 'function',
          hasEmit: typeof mock?.emit === 'function',
        };
      });

      expect(hasEventMock.hasListen).toBe(true);
      expect(hasEventMock.hasUnlisten).toBe(true);
      expect(hasEventMock.hasEmit).toBe(true);
    });

    test('__TAURI_EVENT_PLUGIN_INTERNALS__ is available for cleanup', async ({ page }) => {
      const hasInternals = await page.evaluate(() => {
        const internals = (window as unknown as { __TAURI_EVENT_PLUGIN_INTERNALS__?: { unregisterListener?: unknown } }).__TAURI_EVENT_PLUGIN_INTERNALS__;
        return {
          hasUnregisterListener: typeof internals?.unregisterListener === 'function',
        };
      });

      expect(hasInternals.hasUnregisterListener).toBe(true);
    });

    test('plugin:event|listen/emit/unlisten commands are handled with correct API shape', async ({ page }) => {
      const result = await page.evaluate(async () => {
        const tauriInternals = (window as unknown as { __TAURI_INTERNALS__: { invoke: (cmd: string, args?: unknown) => Promise<unknown> } }).__TAURI_INTERNALS__;

        // Test listen returns a listener ID (eventId)
        const eventId = await tauriInternals.invoke('plugin:event|listen', { event: 'test', handler: 'cb1' });

        // Test unlisten with correct API shape: { event: eventName, eventId: listenerId }
        await tauriInternals.invoke('plugin:event|unlisten', { event: 'test', eventId });

        return { eventIdIsNumber: typeof eventId === 'number' };
      });

      expect(result.eventIdIsNumber).toBe(true);
    });
  });
});
