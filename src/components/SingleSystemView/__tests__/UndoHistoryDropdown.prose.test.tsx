/**
 * RS-PROSE: a history row names its stations, shots and lines as prose pills.
 * The backend words the label at record time and keeps its parts beside it
 * (`UndoHistoryItem.descriptionParts`, persisted without slots); the row draws
 * them, while its tooltip and a screen reader read the plain label, word for
 * word. A row without parts (an earlier history format, a label that names
 * nothing) reads exactly as before. Injection: render `item.description`
 * again and no pill is drawn.
 */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';

vi.mock('../../../utils/ipc', () => ({ invokeIpc: vi.fn(), getErrorMessage: String }));

import '../../../i18n';
import UndoHistoryDropdown, { HISTORY_TOGGLE_ATTR } from '../UndoHistoryDropdown';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { accessibleText } from '../../../test/accessibleText';
import { partsToPlain } from '../../../utils/prose';
import type { ProsePart } from '../../../types/ipc/ProsePart';
import type { UndoHistoryItem } from '../../../types/ipc.generated';

const MERGE_PARTS: ProsePart[] = [
  { kind: 'text', text: 'Merge ' },
  { kind: 'station', text: 'Side / 0', line_id: 2, context: 'Side', name: '0', station_id: 7 },
  { kind: 'text', text: ' into ' },
  { kind: 'station', text: 'Main / 12', line_id: 1, context: 'Main', name: '12', station_id: 11 },
  { kind: 'text', text: ' (keeps 12)' },
];
const MERGE = partsToPlain(MERGE_PARTS);

beforeEach(() => {
  useUIStore.setState({ syntacticLevel: 'full', resolvedTheme: 'dark' });
});

afterEach(() => {
  cleanup();
});

async function open(items: UndoHistoryItem[]) {
  useProjectStore.setState({ getUndoStack: vi.fn(async () => items), canUndo: true });
  render(
    <UndoHistoryDropdown mode="undo" disabled={false} onSingleAction={() => {}} testId="undo" ariaLabel="Undo">
      <span data-testid="chevron" {...{ [HISTORY_TOGGLE_ATTR]: '' }}>v</span>
    </UndoHistoryDropdown>,
  );
  await act(async () => {
    fireEvent.click(screen.getByTestId('chevron'));
  });
}

const item = (index: number, description: string, descriptionParts?: ProsePart[]): UndoHistoryItem => ({
  index,
  description,
  ...(descriptionParts ? { descriptionParts } : {}),
  timestamp: null,
  commandType: 'merge_stations',
  previewData: { changes: [], additionalChanges: 0 },
});

it('draws the names a label says as pills and reads the plain label', async () => {
  expect(MERGE).toBe('Merge Side / 0 into Main / 12 (keeps 12)');
  await open([item(0, MERGE, MERGE_PARTS), item(1, 'Reverse line "Side"')]);
  const row = await screen.findByTestId('undo-description-0');
  const pills = row.querySelectorAll('.sh-ppill');
  expect([...pills].map((p) => p.getAttribute('data-prose-kind'))).toEqual(['station', 'station']);
  expect(pills[0]!.querySelector('[aria-hidden="true"]')!.textContent).toBe('Side: 0#7');
  expect(accessibleText(row)).toBe(MERGE);
  // The tooltip stays plain.
  expect(row.getAttribute('title')).toBe(MERGE);
  // A row without parts reads as it always did.
  const plain = screen.getByTestId('undo-description-1');
  expect(plain.querySelector('.sh-ppill')).toBeNull();
  expect(plain.textContent).toBe('Reverse line "Side"');
});

it('is the plain label below Full', async () => {
  useUIStore.setState({ syntacticLevel: 'off' });
  await open([item(0, MERGE, MERGE_PARTS)]);
  const row = await screen.findByTestId('undo-description-0');
  expect(row.querySelector('.sh-ppill')).toBeNull();
  expect(row.textContent).toBe(MERGE);
});
