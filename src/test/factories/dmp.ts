/**
 * Builders for the parsed mNemo `.dmp` data the backend sends to the import
 * dialogs (#576 E15), typed against the generated IPC types. Each returns a
 * complete object: every required field at a neutral default, overridden by
 * `overrides` (a shallow merge); `comment` is absent unless given. Keys come out
 * in the generated type's order. Synthetic data only.
 */
import type { DmpFile } from '../../types/ipc/DmpFile';
import type { DmpSection } from '../../types/ipc/DmpSection';
import type { DmpShot } from '../../types/ipc/DmpShot';

/** A Standard 5 m shot at heading 10, level, every other reading 0, at 00:00:00. */
export function makeDmpShot(overrides: Partial<DmpShot> = {}): DmpShot {
  return {
    shot_type: 'Standard',
    heading_in: 10,
    heading_out: 10,
    length: 5,
    depth_in: 0,
    depth_out: 0,
    pitch_in: 0,
    pitch_out: 0,
    left: 0,
    right: 0,
    up: 0,
    down: 0,
    temperature: 0,
    hour: 0,
    minute: 0,
    second: 0,
    marker_index: 0,
    ...overrides,
  };
}

/** `count` separate `makeDmpShot(overrides)` shots. */
export function makeDmpShots(count: number, overrides: Partial<DmpShot> = {}): DmpShot[] {
  return Array.from({ length: count }, () => makeDmpShot(overrides));
}

/** A version-1 inbound section "Anchor" dated 2026-01-01, with no shots unless given. */
export function makeDmpSection(overrides: Partial<DmpSection> = {}): DmpSection {
  return {
    version: 1,
    name: 'Anchor',
    direction: 'in',
    date: '2026-01-01',
    shots: [],
    content_hash: 'hash-anchor',
    ...overrides,
  };
}

/** A version-1 file with no sections and no parse warnings unless given. */
export function makeDmpFile(overrides: Partial<DmpFile> = {}): DmpFile {
  return {
    version: 1,
    sections: [],
    warnings: [],
    ...overrides,
  };
}
