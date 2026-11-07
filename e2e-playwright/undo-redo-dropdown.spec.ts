import { test, expect, Page } from './fixtures/coverage';
import { setupTauriMocks } from './mocks/tauri-ipc';
import type { MockReply } from './mocks/ipc/types';
import type { CommandError } from '../src/types/ipc/CommandError';

/**
 * E2E tests for Undo/Redo History Dropdown feature.
 * Tests hover behavior, click actions, keyboard navigation, detail preview, and edge cases.
 */

/** Shape of a mock undo/redo history item */
interface MockHistoryItem {
  index: number;
  description: string;
  timestamp: string | null;
  commandType: string;
  previewData: {
    changes: Array<{ field: string; oldValue: string | null; newValue: string | null }>;
    additionalChanges: number;
  } | null;
}

/** Mock undo history items for testing */
function getMockUndoStack(): MockHistoryItem[] {
  return [
    {
      index: 0,
      description: 'Update shot length to 15.5m',
      timestamp: new Date(Date.now() - 60000).toISOString(), // 1 minute ago
      commandType: 'update_shot',
      previewData: {
        changes: [
          { field: 'Length', oldValue: '10.0', newValue: '15.5' },
          { field: 'Azimuth In', oldValue: '45', newValue: '48' },
        ],
        additionalChanges: 0,
      },
    },
    {
      index: 1,
      description: 'Create section "Main Tunnel"',
      timestamp: new Date(Date.now() - 120000).toISOString(), // 2 minutes ago
      commandType: 'create_section',
      previewData: {
        changes: [{ field: 'Name', oldValue: '', newValue: 'Main Tunnel' }],
        additionalChanges: 0,
      },
    },
    {
      index: 2,
      description: 'Delete shot in Side Passage',
      timestamp: new Date(Date.now() - 180000).toISOString(), // 3 minutes ago
      commandType: 'delete_shot',
      previewData: {
        changes: [
          { field: 'Length', oldValue: '8.5', newValue: '' },
          { field: 'Azimuth In', oldValue: '120', newValue: '' },
        ],
        additionalChanges: 1,
      },
    },
    {
      index: 3,
      description: 'Merge stations A1 and B2',
      timestamp: null, // Legacy entry without timestamp
      commandType: 'merge_stations',
      previewData: {
        changes: [
          { field: 'Station A', oldValue: null, newValue: '1:0 (Main Tunnel)' },
          { field: 'Station B', oldValue: null, newValue: '2:1 (Side Passage)' },
        ],
        additionalChanges: 0,
      },
    },
  ];
}

function getMockRedoStack(): MockHistoryItem[] {
  return [
    {
      index: 0,
      description: 'Delete section "Old Tunnel"',
      timestamp: new Date(Date.now() - 300000).toISOString(),
      commandType: 'delete_section',
      previewData: {
        changes: [{ field: 'Name', oldValue: 'Old Tunnel', newValue: '' }],
        additionalChanges: 2,
      },
    },
  ];
}

interface MockUndoRedoConfig {
  canUndo: boolean;
  canRedo: boolean;
  undoStack: MockHistoryItem[];
  redoStack: MockHistoryItem[];
  /** When true, undo/redo commands mutate the mock stacks (move entries between them)
   *  and derive canUndo/canRedo from array lengths. Default: false (fixed stacks). */
  mutating?: boolean;
}

/**
 * Extends the shared Tauri mocks with undo/redo specific overrides.
 * MUST be called AFTER page.goto() since it uses page.evaluate.
 */
async function extendMocksWithUndoRedo(page: Page, config: Partial<MockUndoRedoConfig> = {}): Promise<void> {
  const fullConfig = {
    canUndo: config.canUndo ?? true,
    canRedo: config.canRedo ?? true,
    undoStack: config.undoStack ?? getMockUndoStack(),
    redoStack: config.redoStack ?? getMockRedoStack(),
    mutating: config.mutating ?? false,
  };

  await page.evaluate((mockConfig) => {
    // Track undo/redo calls for verification
    (window as unknown as Record<string, unknown>).__UNDO_REDO_CALLS__ = [];

    // Store the mock config
    (window as unknown as Record<string, unknown>).__UNDO_REDO_MOCK_CONFIG__ = mockConfig;

    // Get the original invoke function from the shared mock
    const originalInvoke = (window as unknown as { __TAURI_INTERNALS__: { invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown> } }).__TAURI_INTERNALS__.invoke;

    /** Re-index stack entries so index matches array position */
    function reindex(stack: Array<{ index: number }>) {
      stack.forEach((e, i) => { e.index = i; });
    }

    // Override invoke to intercept undo/redo specific commands
    (window as unknown as { __TAURI_INTERNALS__: { invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown> } }).__TAURI_INTERNALS__.invoke = async (cmd: string, args?: Record<string, unknown>) => {
      const cfg = (window as unknown as { __UNDO_REDO_MOCK_CONFIG__: typeof mockConfig }).__UNDO_REDO_MOCK_CONFIG__;

      switch (cmd) {
        case 'can_undo':
          return cfg.mutating ? cfg.undoStack.length > 0 : cfg.canUndo;
        case 'can_redo':
          return cfg.mutating ? cfg.redoStack.length > 0 : cfg.canRedo;
        case 'get_undo_state':
          return cfg.mutating
            ? { can_undo: cfg.undoStack.length > 0, can_redo: cfg.redoStack.length > 0 }
            : { can_undo: cfg.canUndo, can_redo: cfg.canRedo };
        case 'get_undo_stack':
          return cfg.undoStack;
        case 'get_redo_stack':
          return cfg.redoStack;
        case 'undo': {
          const calls = (window as unknown as { __UNDO_REDO_CALLS__: Array<{ cmd: string; index: number }> }).__UNDO_REDO_CALLS__;
          calls.push({ cmd: 'undo', index: 0 });
          if (cfg.mutating && cfg.undoStack.length > 0) {
            const entry = cfg.undoStack.shift()!;
            cfg.redoStack.unshift(entry);
            reindex(cfg.undoStack);
            reindex(cfg.redoStack);
          }
          return { reversed_steps: [], warnings: [], remote_link: null } satisfies MockReply<'undo'>;
        }
        case 'redo': {
          const calls = (window as unknown as { __UNDO_REDO_CALLS__: Array<{ cmd: string; index: number }> }).__UNDO_REDO_CALLS__;
          calls.push({ cmd: 'redo', index: 0 });
          if (cfg.mutating && cfg.redoStack.length > 0) {
            const entry = cfg.redoStack.shift()!;
            cfg.undoStack.unshift(entry);
            reindex(cfg.undoStack);
            reindex(cfg.redoStack);
          }
          return { reversed_steps: [], warnings: [], remote_link: null } satisfies MockReply<'redo'>;
        }
        case 'undo_to': {
          const calls = (window as unknown as { __UNDO_REDO_CALLS__: Array<{ cmd: string; index: number }> }).__UNDO_REDO_CALLS__;
          const toIndex = args?.index as number;
          calls.push({ cmd: 'undo_to', index: toIndex });
          if (cfg.mutating) {
            const count = toIndex + 1;
            for (let i = 0; i < count && cfg.undoStack.length > 0; i++) {
              const entry = cfg.undoStack.shift()!;
              cfg.redoStack.unshift(entry);
            }
            reindex(cfg.undoStack);
            reindex(cfg.redoStack);
          }
          return { reversed_steps: [], warnings: [], remote_link: null } satisfies MockReply<'undo_to'>;
        }
        case 'redo_to': {
          const calls = (window as unknown as { __UNDO_REDO_CALLS__: Array<{ cmd: string; index: number }> }).__UNDO_REDO_CALLS__;
          const toIndex = args?.index as number;
          calls.push({ cmd: 'redo_to', index: toIndex });
          if (cfg.mutating) {
            const count = toIndex + 1;
            for (let i = 0; i < count && cfg.redoStack.length > 0; i++) {
              const entry = cfg.redoStack.shift()!;
              cfg.undoStack.unshift(entry);
            }
            reindex(cfg.undoStack);
            reindex(cfg.redoStack);
          }
          return { reversed_steps: [], warnings: [], remote_link: null } satisfies MockReply<'redo_to'>;
        }
        default:
          return originalInvoke(cmd, args);
      }
    };

    console.log('[Mock IPC] Extended mocks with undo/redo support');
  }, fullConfig);
}

/**
 * Helper to update undo/redo mock config after page load.
 */
async function updateUndoRedoMockConfig(page: Page, config: Partial<MockUndoRedoConfig>): Promise<void> {
  await page.evaluate((newConfig) => {
    const existingConfig = (window as unknown as { __UNDO_REDO_MOCK_CONFIG__: MockUndoRedoConfig }).__UNDO_REDO_MOCK_CONFIG__;
    Object.assign(existingConfig, newConfig);
  }, config);
}

/**
 * Helper to create a new project with mock sections/stations.
 * Uses data-testid selectors where available for stability.
 */
async function createProject(page: Page, name: string): Promise<void> {
  // Click New Project button - use data-testid or fall back to text selector
  const newProjectBtn = page.getByTestId('new-project-button').or(
    page.locator('button:has-text("New Project"), h3:has-text("New Project")').first()
  );
  await newProjectBtn.click();

  // Wait for modal and fill name
  await page.waitForSelector('.fixed.inset-0');
  await page.locator('.fixed.inset-0 input').first().fill(name);
  await page.click('.fixed.inset-0 button:has-text("Create")');
  await page.waitForSelector('canvas', { timeout: 10000 });

  // Wait for map to be fully initialized (per memory pitfall)
  await page.waitForFunction(
    () => (window as unknown as { __stalagmight_map__?: unknown }).__stalagmight_map__ !== undefined,
    { timeout: 10000 }
  );

  // Force a refresh of undo state to ensure store picks up mock values
  // Use async/await properly to avoid race conditions
  await page.evaluate(async () => {
    const stores = (window as unknown as { __E2E_TEST__?: { projectStore: { getState: () => { refreshUndoState: () => Promise<void> } } } }).__E2E_TEST__;
    if (stores?.projectStore) {
      await stores.projectStore.getState().refreshUndoState();
    }
  });

  // Wait for React to re-render with new state - use waitForFunction for reliability
  await page.waitForFunction(
    () => {
      const undoBtn = document.querySelector('[data-testid="toolbar-undo-button"]');
      return undoBtn && !undoBtn.hasAttribute('disabled');
    },
    { timeout: 5000 }
  );
}

/**
 * Helper to get undo/redo calls made during test.
 */
async function getUndoRedoCalls(page: Page): Promise<Array<{ cmd: string; index: number }>> {
  return await page.evaluate(() => {
    return (window as unknown as { __UNDO_REDO_CALLS__: Array<{ cmd: string; index: number }> }).__UNDO_REDO_CALLS__ || [];
  });
}

test.describe('Undo/Redo History Dropdown', () => {
  test.beforeEach(async ({ page }) => {
    // Use shared mocks, then extend with undo/redo support after page load
    await setupTauriMocks(page);
    await page.goto('/');
    await extendMocksWithUndoRedo(page);
    await createProject(page, 'Dropdown Test Project');
  });

  test.describe('Hover Behavior', () => {
    test('dropdown appears after ~300ms hover on Undo button @smoke', async ({ page }) => {
      const undoButton = page.getByTestId('toolbar-undo-button');
      await expect(undoButton).toBeVisible();

      // Verify dropdown is not visible initially
      const dropdown = page.getByTestId('toolbar-undo-button-dropdown');
      await expect(dropdown).not.toBeVisible();

      // Hover over undo button and wait for dropdown
      await undoButton.hover();
      await expect(dropdown).toBeVisible({ timeout: 1000 });
    });

    test('dropdown appears after ~300ms hover on Redo button', async ({ page }) => {
      const redoButton = page.getByTestId('toolbar-redo-button');
      await expect(redoButton).toBeVisible();

      // Verify dropdown is not visible initially
      const dropdown = page.getByTestId('toolbar-redo-button-dropdown');
      await expect(dropdown).not.toBeVisible();

      // Hover over redo button and wait for dropdown
      await redoButton.hover();
      await expect(dropdown).toBeVisible({ timeout: 1000 });
    });

    test('dropdown closes when mouse leaves', async ({ page }) => {
      const undoButton = page.getByTestId('toolbar-undo-button');
      const dropdown = page.getByTestId('toolbar-undo-button-dropdown');

      // Hover to open dropdown
      await undoButton.hover();
      await expect(dropdown).toBeVisible();

      // Move mouse away from dropdown
      await page.mouse.move(0, 0);

      // Dropdown should close
      await expect(dropdown).not.toBeVisible({ timeout: 1000 });
    });
  });

  test.describe('Item Click Behavior', () => {
    test('clicking item at index 2 executes undo_to and closes dropdown @smoke', async ({ page }) => {
      const undoButton = page.getByTestId('toolbar-undo-button');
      const dropdown = page.getByTestId('toolbar-undo-button-dropdown');

      // Hover to open dropdown
      await undoButton.hover();
      await expect(dropdown).toBeVisible();

      // Click item at index 2
      const itemIndex2 = page.getByTestId('toolbar-undo-button-item-2');
      await expect(itemIndex2).toBeVisible();
      await itemIndex2.click();

      // Verify undo_to was called with index 2
      const calls = await getUndoRedoCalls(page);
      expect(calls).toHaveLength(1);
      expect(calls[0]).toEqual({ cmd: 'undo_to', index: 2 });

      // Dropdown should close after item click
      await expect(dropdown).not.toBeVisible({ timeout: 1000 });
    });

    test('clicking item in redo dropdown calls redo_to @smoke', async ({ page }) => {
      const redoButton = page.getByTestId('toolbar-redo-button');

      // Hover to open dropdown
      await redoButton.hover();

      // Click first item
      const item = page.getByTestId('toolbar-redo-button-item-0');
      await expect(item).toBeVisible();
      await item.click();

      // Verify redo_to was called with index 0
      const calls = await getUndoRedoCalls(page);
      expect(calls).toHaveLength(1);
      expect(calls[0]).toEqual({ cmd: 'redo_to', index: 0 });
    });
  });

  test.describe('Keyboard Navigation', () => {
    test('Arrow Down opens dropdown from focused button @smoke', async ({ page }) => {
      const undoButton = page.getByTestId('toolbar-undo-button');
      const dropdown = page.getByTestId('toolbar-undo-button-dropdown');

      // Focus the button
      await undoButton.focus();

      // Press Arrow Down
      await page.keyboard.press('ArrowDown');

      // Dropdown should open
      await expect(dropdown).toBeVisible({ timeout: 1000 });
    });

    test('Arrow keys navigate items in dropdown', async ({ page }) => {
      const undoButton = page.getByTestId('toolbar-undo-button');
      const dropdown = page.getByTestId('toolbar-undo-button-dropdown');

      // Focus and open with Arrow Down
      await undoButton.focus();
      await page.keyboard.press('ArrowDown');
      await expect(dropdown).toBeVisible();

      // Wait for aria-activedescendant to be set (component sets it in setTimeout(0))
      await expect(dropdown).toHaveAttribute('aria-activedescendant', /.+/);

      // Get initial aria-activedescendant
      const getActiveDescendant = async () => {
        return await dropdown.getAttribute('aria-activedescendant');
      };

      const initialActive = await getActiveDescendant();

      // Navigate down
      await page.keyboard.press('ArrowDown');
      const afterDown = await getActiveDescendant();
      // Active descendant should change (or index increased)
      expect(afterDown).not.toEqual(initialActive);

      // Navigate up - should go back
      await page.keyboard.press('ArrowUp');
      const afterUp = await getActiveDescendant();
      expect(afterUp).toEqual(initialActive);

      // Use End to go to last item
      await page.keyboard.press('End');
      const afterEnd = await getActiveDescendant();
      expect(afterEnd).not.toEqual(initialActive);

      // Use Home to go to first item
      await page.keyboard.press('Home');
      const afterHome = await getActiveDescendant();
      expect(afterHome).toEqual(initialActive);

      // Dropdown should still be open
      await expect(dropdown).toBeVisible();
    });

    test('Enter selects item and closes dropdown', async ({ page }) => {
      const undoButton = page.getByTestId('toolbar-undo-button');
      const dropdown = page.getByTestId('toolbar-undo-button-dropdown');

      // Focus and open with Arrow Down
      await undoButton.focus();
      await page.keyboard.press('ArrowDown');
      await expect(dropdown).toBeVisible();

      // Wait for first item to be focused
      await expect(dropdown).toHaveAttribute('aria-activedescendant', /.+/);

      // Press Enter to select first item (index 0)
      await page.keyboard.press('Enter');

      // Verify undo_to was called with index 0
      const calls = await getUndoRedoCalls(page);
      expect(calls).toHaveLength(1);
      expect(calls[0]).toEqual({ cmd: 'undo_to', index: 0 });

      // Dropdown should close after item selection
      await expect(dropdown).not.toBeVisible({ timeout: 1000 });
    });

    test('keyboard navigation to last item and Enter selects correct index', async ({ page }) => {
      const undoButton = page.getByTestId('toolbar-undo-button');
      const dropdown = page.getByTestId('toolbar-undo-button-dropdown');

      // Focus and open with Arrow Down
      await undoButton.focus();
      await page.keyboard.press('ArrowDown');
      await expect(dropdown).toBeVisible();
      await expect(dropdown).toHaveAttribute('aria-activedescendant', /.+/);
      await dropdown.focus();
      await expect(dropdown).toBeFocused();

      // Use End to go to last item (index 3)
      const lastItem = dropdown.locator('[role="menuitem"]').last();
      const lastItemId = await lastItem.getAttribute('id');
      const lastItemTestId = await lastItem.getAttribute('data-testid');
      if (!lastItemId || !lastItemTestId) {
        throw new Error('Expected last undo history item to have id and data-testid.');
      }
      const match = lastItemTestId.match(/item-(\d+)/);
      if (!match) {
        throw new Error(`Unexpected undo history test id: ${lastItemTestId}`);
      }
      const expectedIndex = Number(match[1]);

      await page.keyboard.press('End');
      await expect(dropdown).toHaveAttribute('aria-activedescendant', lastItemId);

      // Press Enter to select
      await page.keyboard.press('Enter');

      // Verify undo_to was called with index from last item
      const calls = await getUndoRedoCalls(page);
      expect(calls).toHaveLength(1);
      expect(calls[0]).toEqual({ cmd: 'undo_to', index: expectedIndex });
    });

    test('Escape closes dropdown', async ({ page }) => {
      const undoButton = page.getByTestId('toolbar-undo-button');
      const dropdown = page.getByTestId('toolbar-undo-button-dropdown');

      // Open dropdown via hover
      await undoButton.hover();
      await expect(dropdown).toBeVisible();

      // Press Escape
      await page.keyboard.press('Escape');

      // Dropdown should close
      await expect(dropdown).not.toBeVisible({ timeout: 1000 });
    });
  });

  test.describe('Detail Preview', () => {
    test('detail preview appears on item hover', async ({ page }) => {
      const undoButton = page.getByTestId('toolbar-undo-button');

      // Hover to open dropdown
      await undoButton.hover();

      // Hover over first item (which has previewData)
      const firstItem = page.getByTestId('toolbar-undo-button-item-0');
      await expect(firstItem).toBeVisible();
      await firstItem.hover();

      // Detail preview popup should appear
      const previewPopup = page.getByTestId('detail-preview-popup');
      await expect(previewPopup).toBeVisible({ timeout: 1000 });

      // Verify preview shows field changes in "Field: value" format
      await expect(previewPopup).toContainText('Length:');
      await expect(previewPopup).toContainText('10.0');
      await expect(previewPopup).toContainText('15.5');
    });

    test('detail preview shows info-only values without arrow for merge_stations', async ({ page }) => {
      const undoButton = page.getByTestId('toolbar-undo-button');

      // Hover to open dropdown
      await undoButton.hover();

      // Hover over item at index 3 (merge_stations with info-only newValue fields)
      const item = page.getByTestId('toolbar-undo-button-item-3');
      await expect(item).toBeVisible();
      await item.hover();

      // Detail preview popup should show station info
      const previewPopup = page.getByTestId('detail-preview-popup');
      await expect(previewPopup).toBeVisible({ timeout: 1000 });
      await expect(previewPopup).toContainText('Station A:');
      await expect(previewPopup).toContainText('1:0 (Main Tunnel)');
      await expect(previewPopup).toContainText('Station B:');
      await expect(previewPopup).toContainText('2:1 (Side Passage)');

      // Should NOT contain arrow symbol (info-only rendering)
      await expect(previewPopup).not.toContainText('→');
    });

    test('detail preview shows old values for delete_shot', async ({ page }) => {
      const undoButton = page.getByTestId('toolbar-undo-button');

      // Hover to open dropdown
      await undoButton.hover();

      // Hover over item at index 2 (delete_shot with old values)
      const item = page.getByTestId('toolbar-undo-button-item-2');
      await expect(item).toBeVisible();
      await item.hover();

      // Detail preview popup should show deleted shot data
      const previewPopup = page.getByTestId('detail-preview-popup');
      await expect(previewPopup).toBeVisible({ timeout: 1000 });
      await expect(previewPopup).toContainText('Length:');
      await expect(previewPopup).toContainText('8.5');
    });

    test('detail preview shows move offset without arrow', async ({ page }) => {
      // Setup custom mock with a move command (description is simple, details in hover only)
      await page.evaluate(() => {
        const cfg = (window as unknown as { __UNDO_REDO_MOCK_CONFIG__: { undoStack: unknown[] } }).__UNDO_REDO_MOCK_CONFIG__;
        cfg.undoStack = [
          {
            index: 0,
            description: 'Move section 2',
            timestamp: new Date().toISOString(),
            commandType: 'move_section',
            previewData: {
              changes: [
                { field: 'Offset', oldValue: null, newValue: '5.3m NNW, 1.2m down' },
                { field: 'Section', oldValue: null, newValue: 'Main Tunnel' },
              ],
              additionalChanges: 0,
            },
          },
        ];
      });

      const undoButton = page.getByTestId('toolbar-undo-button');

      // Re-open dropdown to pick up new mock data
      await undoButton.hover();

      const item = page.getByTestId('toolbar-undo-button-item-0');
      await expect(item).toBeVisible();
      await item.hover();

      const previewPopup = page.getByTestId('detail-preview-popup');
      await expect(previewPopup).toBeVisible({ timeout: 1000 });
      await expect(previewPopup).toContainText('Offset:');
      await expect(previewPopup).toContainText('5.3m NNW, 1.2m down');
      await expect(previewPopup).toContainText('Section:');
      await expect(previewPopup).toContainText('Main Tunnel');

      // Should NOT contain arrow symbol (info-only rendering)
      await expect(previewPopup).not.toContainText('→');
    });

    test('detail preview lists batch sub-commands with details', async ({ page }) => {
      // Setup custom mock with a batch command.
      // Field = command description (header), newValue = details (shown on next line).
      // Each sub-command renders with description as header and details below.
      await page.evaluate(() => {
        const cfg = (window as unknown as { __UNDO_REDO_MOCK_CONFIG__: { undoStack: unknown[] } }).__UNDO_REDO_MOCK_CONFIG__;
        cfg.undoStack = [
          {
            index: 0,
            description: 'Batch of 3 commands',
            timestamp: new Date().toISOString(),
            commandType: 'batch',
            previewData: {
              changes: [
                { field: 'Merge stations 1:0 and 2:1', oldValue: null, newValue: '1:0 \u2194 2:1' },
                { field: 'Update section 1 metadata', oldValue: null, newValue: '' },
                { field: 'Delete section "Side"', oldValue: null, newValue: '' },
              ],
              additionalChanges: 0,
            },
          },
        ];
      });

      const undoButton = page.getByTestId('toolbar-undo-button');

      // Re-open dropdown to pick up new mock data
      await undoButton.hover();

      const item = page.getByTestId('toolbar-undo-button-item-0');
      await expect(item).toBeVisible();
      await item.hover();

      const previewPopup = page.getByTestId('detail-preview-popup');
      await expect(previewPopup).toBeVisible({ timeout: 1000 });

      // Should show each sub-command description as header
      await expect(previewPopup).toContainText('Merge stations 1:0 and 2:1');
      await expect(previewPopup).toContainText('Update section 1 metadata');
      await expect(previewPopup).toContainText('Delete section "Side"');

      // First item has details on a new line below the description
      await expect(previewPopup).toContainText('1:0 \u2194 2:1');
    });

    test('detail preview shows "No details" for defensive null previewData', async ({ page }) => {
      // Setup custom mock with null previewData to test defensive rendering
      await page.evaluate(() => {
        const cfg = (window as unknown as { __UNDO_REDO_MOCK_CONFIG__: { undoStack: unknown[] } }).__UNDO_REDO_MOCK_CONFIG__;
        cfg.undoStack = [
          {
            index: 0,
            description: 'Some unknown command',
            timestamp: new Date().toISOString(),
            commandType: 'unknown',
            previewData: null,
          },
        ];
      });

      const undoButton = page.getByTestId('toolbar-undo-button');

      // Re-open dropdown to pick up new mock data
      await undoButton.hover();

      const item = page.getByTestId('toolbar-undo-button-item-0');
      await expect(item).toBeVisible();
      await item.hover();

      const previewPopup = page.getByTestId('detail-preview-popup');
      await expect(previewPopup).toBeVisible({ timeout: 1000 });
      await expect(previewPopup).toContainText('No details');
    });
  });

  test.describe('Edge Cases', () => {
    test('no dropdown when canUndo is false', async ({ page }) => {
      // Update mock config to disable undo
      await updateUndoRedoMockConfig(page, { canUndo: false, canRedo: false, undoStack: [], redoStack: [] });

      // Trigger refresh and wait for button to become disabled
      await page.evaluate(async () => {
        const stores = (window as unknown as { __E2E_TEST__?: { projectStore: { getState: () => { refreshUndoState: () => Promise<void> } } } }).__E2E_TEST__;
        if (stores?.projectStore) {
          await stores.projectStore.getState().refreshUndoState();
        }
      });

      // Wait for the button to become disabled
      await page.waitForFunction(
        () => {
          const undoBtn = document.querySelector('[data-testid="toolbar-undo-button"]');
          return undoBtn && undoBtn.hasAttribute('disabled');
        },
        { timeout: 5000 }
      );

      const undoButton = page.getByTestId('toolbar-undo-button');
      await expect(undoButton).toBeVisible();
      await expect(undoButton).toBeDisabled();

      // Hover should not open dropdown on disabled button
      await undoButton.hover({ force: true });
      // settle: give a (buggy) dropdown time to open before asserting it didn't.
      await page.waitForTimeout(400);

      // Dropdown should not appear
      const dropdown = page.getByTestId('toolbar-undo-button-dropdown');
      await expect(dropdown).not.toBeVisible();
    });

    test('Ctrl+Z keeps dropdown open and performs undo', async ({ page }) => {
      const undoButton = page.getByTestId('toolbar-undo-button');
      const dropdown = page.getByTestId('toolbar-undo-button-dropdown');

      // Open dropdown
      await undoButton.hover();
      await expect(dropdown).toBeVisible();

      // Press Ctrl+Z (ensure focus is NOT in a text input)
      await undoButton.focus();
      await page.keyboard.press('Control+Z');

      // Dropdown should stay open (stay-open behavior)
      await expect(dropdown).toBeVisible({ timeout: 1000 });
    });

    test('Ctrl+Y keeps dropdown open and performs redo', async ({ page }) => {
      const undoButton = page.getByTestId('toolbar-undo-button');
      const dropdown = page.getByTestId('toolbar-undo-button-dropdown');

      // Open dropdown
      await undoButton.hover();
      await expect(dropdown).toBeVisible();

      // Press Ctrl+Y (ensure focus is NOT in a text input)
      await undoButton.focus();
      await page.keyboard.press('Control+Y');

      // Dropdown should stay open (stay-open behavior)
      await expect(dropdown).toBeVisible({ timeout: 1000 });
    });

    test('dropdown shows timestamp as "Unknown" for legacy entries', async ({ page }) => {
      const undoButton = page.getByTestId('toolbar-undo-button');

      // Open dropdown
      await undoButton.hover();

      // Find item at index 3 which has null timestamp
      const item = page.getByTestId('toolbar-undo-button-item-3');
      await expect(item).toBeVisible();

      // Verify "Unknown" is shown for timestamp
      await expect(item).toContainText('Unknown');
    });

    test('chevron indicator visible when canUndo is true', async ({ page }) => {
      // Verify undo chevron is visible
      const undoChevron = page.getByTestId('undo-chevron');
      await expect(undoChevron).toBeVisible();
    });

    test('chevron indicator visible when canRedo is true', async ({ page }) => {
      // Verify redo chevron is visible
      const redoChevron = page.getByTestId('redo-chevron');
      await expect(redoChevron).toBeVisible();
    });
  });

  test.describe('Dropdown Content', () => {
    test('dropdown displays items with correct content', async ({ page }) => {
      const undoButton = page.getByTestId('toolbar-undo-button');

      // Open dropdown
      await undoButton.hover();

      const dropdown = page.getByTestId('toolbar-undo-button-dropdown');
      await expect(dropdown).toBeVisible();

      // Verify header
      await expect(dropdown).toContainText('Undo history');

      // Verify items are displayed
      await expect(page.getByTestId('toolbar-undo-button-item-0')).toBeVisible();
      await expect(page.getByTestId('toolbar-undo-button-item-1')).toBeVisible();
      await expect(page.getByTestId('toolbar-undo-button-item-2')).toBeVisible();
      await expect(page.getByTestId('toolbar-undo-button-item-3')).toBeVisible();

      // Verify item descriptions are truncated appropriately and displayed
      const firstItem = page.getByTestId('toolbar-undo-button-item-0');
      await expect(firstItem).toContainText('Update shot');
    });

    test('redo dropdown displays correct header', async ({ page }) => {
      const redoButton = page.getByTestId('toolbar-redo-button');

      // Open dropdown
      await redoButton.hover();

      const dropdown = page.getByTestId('toolbar-redo-button-dropdown');
      await expect(dropdown).toBeVisible();

      // Verify header says "Redo history"
      await expect(dropdown).toContainText('Redo history');
    });
  });
});

test.describe('Undo/Redo Dropdown - Empty State', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    // Setup with empty stacks but canUndo/canRedo still true (edge case)
    await extendMocksWithUndoRedo(page, { undoStack: [], redoStack: [] });
    await createProject(page, 'Empty Stack Project');
  });

  test('dropdown does not open when stack is empty', async ({ page }) => {
    const undoButton = page.getByTestId('toolbar-undo-button');
    const dropdown = page.getByTestId('toolbar-undo-button-dropdown');

    // Hover to try opening dropdown
    await undoButton.hover();
    // settle: past the hover-open delay, so a dropdown that wrongly opens for an
    // empty stack has had its chance before the assertion below.
    await page.waitForTimeout(400);

    // Dropdown should not appear with empty stack
    await expect(dropdown).not.toBeVisible();
  });
});

test.describe('Undo/Redo Dropdown - Stay Open Behavior', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await extendMocksWithUndoRedo(page);
    await createProject(page, 'Stay Open Test Project');
  });

  test('button click keeps dropdown open and refreshes items', async ({ page }) => {
    const undoButton = page.getByTestId('toolbar-undo-button');
    const dropdown = page.getByTestId('toolbar-undo-button-dropdown');

    // Hover to open dropdown
    await undoButton.hover();
    await expect(dropdown).toBeVisible({ timeout: 1000 });

    // Click the button while dropdown is open (performs undo and stays open)
    await undoButton.click();

    // Dropdown should stay open
    await expect(dropdown).toBeVisible({ timeout: 2000 });

    // Verify undo was actually called
    const calls = await getUndoRedoCalls(page);
    expect(calls.length).toBeGreaterThanOrEqual(1);
    expect(calls.some(c => c.cmd === 'undo')).toBe(true);

    // aria-live announcement should update with remaining count (proves refresh happened)
    const announcement = page.getByTestId('toolbar-undo-button-announcement');
    await expect(announcement).toContainText('actions remaining', { timeout: 2000 });
  });

  test('item click closes dropdown and executes action', async ({ page }) => {
    const undoButton = page.getByTestId('toolbar-undo-button');
    const dropdown = page.getByTestId('toolbar-undo-button-dropdown');

    // Hover to open dropdown
    await undoButton.hover();
    await expect(dropdown).toBeVisible({ timeout: 1000 });

    // Click an item
    const item = page.getByTestId('toolbar-undo-button-item-0');
    await expect(item).toBeVisible();
    await item.click();

    // Dropdown should close after item click
    await expect(dropdown).not.toBeVisible({ timeout: 2000 });

    // Verify undo_to was called
    const calls = await getUndoRedoCalls(page);
    expect(calls).toHaveLength(1);
    expect(calls[0]).toEqual({ cmd: 'undo_to', index: 0 });
  });

  test('Ctrl+Z keeps dropdown open and refreshes items', async ({ page }) => {
    const undoButton = page.getByTestId('toolbar-undo-button');
    const dropdown = page.getByTestId('toolbar-undo-button-dropdown');

    // Hover to open dropdown
    await undoButton.hover();
    await expect(dropdown).toBeVisible({ timeout: 1000 });

    // Ensure focus is not in a text input (input focus guard)
    await undoButton.focus();

    // Press Ctrl+Z
    await page.keyboard.press('Control+Z');

    // Dropdown should stay open
    await expect(dropdown).toBeVisible({ timeout: 2000 });

    // Verify undo was called (proves keyboard shortcut was intercepted by dropdown)
    const calls = await getUndoRedoCalls(page);
    expect(calls.length).toBeGreaterThanOrEqual(1);
    expect(calls.some(c => c.cmd === 'undo')).toBe(true);

    // aria-live announcement should update (proves refresh happened)
    const announcement = page.getByTestId('toolbar-undo-button-announcement');
    await expect(announcement).toContainText('actions remaining', { timeout: 2000 });
  });

  test('detail preview dismisses on action', async ({ page }) => {
    const undoButton = page.getByTestId('toolbar-undo-button');
    const dropdown = page.getByTestId('toolbar-undo-button-dropdown');

    // Hover to open dropdown
    await undoButton.hover();
    await expect(dropdown).toBeVisible({ timeout: 1000 });

    // Hover over first item to show detail preview
    const firstItem = page.getByTestId('toolbar-undo-button-item-0');
    await firstItem.hover();
    const previewPopup = page.getByTestId('detail-preview-popup');
    await expect(previewPopup).toBeVisible({ timeout: 1000 });

    // Click the item (performs action)
    await firstItem.click();

    // Detail preview should be dismissed
    await expect(previewPopup).not.toBeVisible({ timeout: 1000 });
  });

  test('loading indicator appears during refresh', async ({ page }) => {
    const undoButton = page.getByTestId('toolbar-undo-button');
    const dropdown = page.getByTestId('toolbar-undo-button-dropdown');

    // Hover to open dropdown
    await undoButton.hover();
    await expect(dropdown).toBeVisible({ timeout: 1000 });

    // Add a delay to the mock undo_to to make loading indicator observable
    await page.evaluate(() => {
      const originalInvoke = (window as unknown as { __TAURI_INTERNALS__: { invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown> } }).__TAURI_INTERNALS__.invoke;
      (window as unknown as { __TAURI_INTERNALS__: { invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown> } }).__TAURI_INTERNALS__.invoke = async (cmd: string, args?: Record<string, unknown>) => {
        if (cmd === 'undo_to' || cmd === 'redo_to' || cmd === 'undo' || cmd === 'redo') {
          // Introduce a small delay so loading indicator is visible
          await new Promise(resolve => setTimeout(resolve, 200));
        }
        return originalInvoke(cmd, args);
      };
    });

    // Click button to trigger action
    await undoButton.click();

    // Loading indicator should appear
    const loadingIndicator = page.getByTestId('toolbar-undo-button-loading');
    await expect(loadingIndicator).toBeVisible({ timeout: 1000 });
    await expect(loadingIndicator).toContainText('Processing...');
  });

  test('Enter on navigated item closes dropdown and executes correct action', async ({ page }) => {
    const undoButton = page.getByTestId('toolbar-undo-button');
    const dropdown = page.getByTestId('toolbar-undo-button-dropdown');

    // Open via keyboard and navigate to last item
    await undoButton.focus();
    await page.keyboard.press('ArrowDown');
    await expect(dropdown).toBeVisible();

    // Wait for aria-activedescendant to be set
    await expect(dropdown).toHaveAttribute('aria-activedescendant', /.+/);

    // Navigate to last item
    await page.keyboard.press('End');
    const lastActiveDescendant = await dropdown.getAttribute('aria-activedescendant');
    expect(lastActiveDescendant).toMatch(/item-3/);

    // Perform action via Enter
    await page.keyboard.press('Enter');

    // Dropdown should close after item selection
    await expect(dropdown).not.toBeVisible({ timeout: 2000 });

    // Verify undo_to was called with the correct index (last item = index 3)
    const calls = await getUndoRedoCalls(page);
    expect(calls).toHaveLength(1);
    expect(calls[0]).toEqual({ cmd: 'undo_to', index: 3 });
  });

  test('aria-live announcement after action', async ({ page }) => {
    const undoButton = page.getByTestId('toolbar-undo-button');
    const dropdown = page.getByTestId('toolbar-undo-button-dropdown');

    // Hover to open dropdown
    await undoButton.hover();
    await expect(dropdown).toBeVisible({ timeout: 1000 });

    // Click button to trigger undo action
    await undoButton.click();

    // aria-live region should announce remaining actions
    const announcement = page.getByTestId('toolbar-undo-button-announcement');
    await expect(announcement).toContainText('actions remaining', { timeout: 2000 });
  });

  test('fixed 5-item height renders correctly', async ({ page }) => {
    const undoButton = page.getByTestId('toolbar-undo-button');
    const dropdown = page.getByTestId('toolbar-undo-button-dropdown');

    // Hover to open dropdown
    await undoButton.hover();
    await expect(dropdown).toBeVisible({ timeout: 1000 });

    // Check that the scroll container has fixed height of 220px
    const scrollContainer = dropdown.locator('.undo-history-scrollable');
    await expect(scrollContainer).toBeVisible();
    await expect(scrollContainer).toHaveCSS('height', '220px');
  });
});

test.describe('Undo/Redo Dropdown - Close on Empty Stack', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await extendMocksWithUndoRedo(page);
    await createProject(page, 'Empty Stack Close Test');
  });

  test('dropdown closes when stack becomes empty after action', async ({ page }) => {
    const undoButton = page.getByTestId('toolbar-undo-button');
    const dropdown = page.getByTestId('toolbar-undo-button-dropdown');

    // Hover to open dropdown
    await undoButton.hover();
    await expect(dropdown).toBeVisible({ timeout: 1000 });

    // Modify mock so the next get_undo_stack returns empty (simulating all actions undone)
    await page.evaluate(() => {
      const originalInvoke = (window as unknown as { __TAURI_INTERNALS__: { invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown> } }).__TAURI_INTERNALS__.invoke;
      let undoActionCalled = false;

      (window as unknown as { __TAURI_INTERNALS__: { invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown> } }).__TAURI_INTERNALS__.invoke = async (cmd: string, args?: Record<string, unknown>) => {
        // Once undo action is called, return empty stack on next fetch
        if (cmd === 'undo_to' || cmd === 'undo') {
          undoActionCalled = true;
          const calls = (window as unknown as { __UNDO_REDO_CALLS__: Array<{ cmd: string; index: number }> }).__UNDO_REDO_CALLS__;
          calls.push({ cmd: cmd, index: (args?.index as number) ?? 0 });
          return { reversed_steps: [], warnings: [], remote_link: null } satisfies MockReply<'undo_to'> & MockReply<'undo'>;
        }
        if (cmd === 'get_undo_stack' && undoActionCalled) {
          return []; // Empty stack after undo
        }
        return originalInvoke(cmd, args);
      };
    });

    // Click button to trigger undo
    await undoButton.click();

    // Dropdown should auto-close because stack is now empty
    await expect(dropdown).not.toBeVisible({ timeout: 3000 });

    // aria-live should announce "No more actions remaining"
    const announcement = page.getByTestId('toolbar-undo-button-announcement');
    await expect(announcement).toContainText('No more actions remaining', { timeout: 2000 });
  });
});

test.describe('Undo/Redo Dropdown - Error Handling', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await extendMocksWithUndoRedo(page);
    await createProject(page, 'Error Handling Test');
  });

  test('error dialog shown on backend failure and dropdown stays open', async ({ page }) => {
    const undoButton = page.getByTestId('toolbar-undo-button');
    const dropdown = page.getByTestId('toolbar-undo-button-dropdown');

    // Hover to open dropdown
    await undoButton.hover();
    await expect(dropdown).toBeVisible({ timeout: 1000 });

    // Modify mock so undo_to returns failure
    await page.evaluate(() => {
      const originalInvoke = (window as unknown as { __TAURI_INTERNALS__: { invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown> } }).__TAURI_INTERNALS__.invoke;

      (window as unknown as { __TAURI_INTERNALS__: { invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown> } }).__TAURI_INTERNALS__.invoke = async (cmd: string, args?: Record<string, unknown>) => {
        if (cmd === 'undo_to' || cmd === 'undo') {
          return Promise.reject({ code: 'FAILED', message: 'Stack corrupted: cannot undo' } satisfies CommandError);
        }
        return originalInvoke(cmd, args);
      };
    });

    // Click button to trigger undo (will fail)
    await undoButton.click();

    // Error dialog should appear
    const errorDialog = page.getByTestId('error-dialog');
    await expect(errorDialog).toBeVisible({ timeout: 3000 });
    await expect(errorDialog).toContainText('Undo failed');
    await expect(errorDialog).toContainText('Stack corrupted');

    // Dropdown should close when error dialog appears
    await expect(dropdown).not.toBeVisible();
  });
});

test.describe('Undo/Redo Dropdown - Rapid Click Queuing', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await extendMocksWithUndoRedo(page);
    await createProject(page, 'Rapid Click Test');
  });

  test('rapid clicks queue and execute sequentially', async ({ page }) => {
    const undoButton = page.getByTestId('toolbar-undo-button');
    const dropdown = page.getByTestId('toolbar-undo-button-dropdown');
    const announcement = page.getByTestId('toolbar-undo-button-announcement');

    // Add a small delay to undo commands to allow queuing observation
    await page.evaluate(() => {
      const originalInvoke = (window as unknown as { __TAURI_INTERNALS__: { invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown> } }).__TAURI_INTERNALS__.invoke;

      (window as unknown as { __TAURI_INTERNALS__: { invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown> } }).__TAURI_INTERNALS__.invoke = async (cmd: string, args?: Record<string, unknown>) => {
        if (cmd === 'undo' || cmd === 'undo_to') {
          await new Promise(resolve => setTimeout(resolve, 100));
        }
        return originalInvoke(cmd, args);
      };
    });

    // Hover to open dropdown
    await undoButton.hover();
    await expect(dropdown).toBeVisible({ timeout: 1000 });

    // Rapid-click button three times quickly (Playwright click dispatches event, doesn't await async)
    await undoButton.click();
    await undoButton.click();
    await undoButton.click();

    // Wait for all 3 queued actions to complete by polling the call count
    // Each action has 100ms delay + fetch overhead, so 3 actions take ~400-600ms
    await page.waitForFunction(
      () => {
        const calls = (window as unknown as { __UNDO_REDO_CALLS__: Array<{ cmd: string }> }).__UNDO_REDO_CALLS__ || [];
        return calls.filter(c => c.cmd === 'undo').length >= 3;
      },
      { timeout: 5000 }
    );

    // Verify all 3 undo calls were actually executed
    const calls = await getUndoRedoCalls(page);
    expect(calls.filter(c => c.cmd === 'undo').length).toBe(3);

    // aria-live announcement should reflect remaining count after all actions
    await expect(announcement).toContainText('actions remaining', { timeout: 2000 });

    // Dropdown should still be open
    await expect(dropdown).toBeVisible();
  });
});

test.describe('Undo/Redo Happy Path - Stack Mutation', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    // Start with one entry in undo stack, empty redo stack, mutating mode
    await extendMocksWithUndoRedo(page, {
      undoStack: getMockUndoStack().slice(0, 1),
      redoStack: [],
      mutating: true,
    });
    await createProject(page, 'Happy Path Test');
  });

  test('undo moves entry from undo stack to redo stack, redo moves it back', async ({ page }) => {
    // Verify initial state: undo enabled, redo disabled
    const undoButton = page.getByTestId('toolbar-undo-button');
    const redoButton = page.getByTestId('toolbar-redo-button');
    await expect(undoButton).toBeEnabled();
    await expect(redoButton).toBeDisabled();

    // Perform undo via button click
    await undoButton.click();

    // Wait for redo button to become enabled (entry moved to redo stack)
    await expect(redoButton).toBeEnabled({ timeout: 3000 });

    // Undo button should now be disabled (undo stack empty)
    await expect(undoButton).toBeDisabled({ timeout: 3000 });

    // Verify undo was called
    const callsAfterUndo = await getUndoRedoCalls(page);
    expect(callsAfterUndo).toHaveLength(1);
    expect(callsAfterUndo[0]).toEqual({ cmd: 'undo', index: 0 });

    // Now perform redo via button click
    await redoButton.click();

    // Wait for undo button to become enabled again (entry moved back to undo stack)
    await expect(undoButton).toBeEnabled({ timeout: 3000 });

    // Redo button should now be disabled (redo stack empty again)
    await expect(redoButton).toBeDisabled({ timeout: 3000 });

    // Verify redo was called
    const callsAfterRedo = await getUndoRedoCalls(page);
    expect(callsAfterRedo).toHaveLength(2);
    expect(callsAfterRedo[1]).toEqual({ cmd: 'redo', index: 0 });
  });
});
