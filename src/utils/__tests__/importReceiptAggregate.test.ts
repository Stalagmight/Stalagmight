/**
 * #814: `line_problems` is optional on the wire. Merging the receipts of a
 * multi-batch import keeps every batch's entries, and leaves the field absent
 * when no batch had any.
 */
import { describe, it, expect } from 'vitest';
import type { ImportReceipt } from '../../types/ipc/ImportReceipt';
import { aggregateImportReceipts } from '../importReceipt';

const receipt = (overrides: Partial<ImportReceipt> = {}): ImportReceipt => ({
  source_format: 'MNemo DMP',
  file_name: 'cave.dmp',
  lines_found: 2,
  lines_imported: 1,
  shot_count: 4,
  closures_requested: 0,
  closures_created: 0,
  unit_conversion: null,
  warnings: [],
  info: [],
  zero_datum_created: null,
  ...overrides,
});

describe('aggregateImportReceipts line_problems (#814)', () => {
  it('concatenates every batch\'s line problems in order', () => {
    const merged = aggregateImportReceipts([
      receipt({ line_problems: ['a'] }),
      receipt(),
      receipt({ line_problems: ['b', 'c'] }),
    ]);
    expect(merged?.line_problems).toEqual(['a', 'b', 'c']);
  });

  it('leaves the field absent when no batch had any', () => {
    const merged = aggregateImportReceipts([receipt(), receipt()]);
    expect(merged).not.toHaveProperty('line_problems');
  });
});
