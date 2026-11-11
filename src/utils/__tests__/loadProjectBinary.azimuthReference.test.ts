/**
 * #438 - the binary load path carries both halves of the north reference.
 *
 * The binary envelope is the FAST path: every real project open goes through it
 * and the JSON shape is the fallback. It used to stop at `surface_elevation`,
 * so a project whose sections carry a declination decoded to one with none -
 * nothing failed, the map was simply drawn (and exported) rotated by the
 * correction, while the JSON path showed it right.
 *
 * `buildResult` is the pure half of the decoder, so these drive it directly
 * rather than round-tripping msgpack through a mocked IPC.
 */
import { describe, it, expect } from 'vitest';

import { buildResult, type BinaryEnvelope } from '../loadProjectBinary';
import type { AzimuthReference } from '../../types/ipc/AzimuthReference';

const DECLARED: AzimuthReference = {
  north: 'magnetic',
  correction: 10.5,
  source: { igrf_auto: { epoch_year: 2024 } },
};
const PROJECT_DEFAULT: AzimuthReference = {
  north: 'magnetic',
  correction: -4.25,
  source: 'file_declared',
};

/** One project, two sections, no shots - the reference fields are the subject. */
function envelope(overrides: {
  projectDefault?: AzimuthReference;
  sectionReference?: AzimuthReference;
}): BinaryEnvelope {
  const section = (id: number, name: string, reference?: AzimuthReference) => ({
    id,
    name,
    surveyor: null,
    date: null,
    color: '#ff0000',
    direction: 'in' as const,
    quality: 1.0,
    shot_offset: 0,
    shot_count: 0,
    next_shot_id: 0,
    ...(reference ? { azimuth_reference: reference } : {}),
  });

  return {
    version: 6,
    loading_id: 1,
    is_remote: false,
    project_header: {
      name: 'declination',
      location: null,
      created_at: '2026-01-01T00:00:00Z',
      modified_at: '2026-01-01T00:00:00Z',
      file_path: null,
      next_section_id: 3,
      next_closure_id: 1,
      sections: [
        section(1, 'Declared', overrides.sectionReference),
        // Declares nothing: it INHERITS, which is what the project-level field
        // has to preserve.
        section(2, 'Inheriting'),
      ],
      station_merges: [],
      zero_datums: [],
      loop_closures: [],
      section_offsets: [],
      station_offsets: [],
      station_comments_map: [],
      imported_file_hashes: [],
      imported_section_hashes: [],
      validation: {} as never,
      surface_elevation: null,
      ...(overrides.projectDefault
        ? { default_azimuth_reference: overrides.projectDefault }
        : {}),
    },
    shot_count: 0,
    shots_floats: new Uint8Array(0),
    shots_uints: new Uint8Array(0),
    shots_flags: new Uint8Array(0),
    shot_comments: [],
    station_count: 0,
    stations_floats: new Uint8Array(0),
    stations_uints: new Uint8Array(0),
    stations_flags: new Uint8Array(0),
    station_gps: [],
    station_merged_with: [],
    station_closures_with: [],
    station_comments: [],
    station_names: [],
    stats: {} as never,
    warnings: [],
    display_settings: {} as never,
    frozen_shots: [],
  } as unknown as BinaryEnvelope;
}

describe('loadProjectBinary decodes #438 north references', () => {
  it('carries the project default and each section reference onto the Project', () => {
    const result = buildResult(
      envelope({ projectDefault: PROJECT_DEFAULT, sectionReference: DECLARED }),
      0,
    );

    expect(result.project.default_azimuth_reference).toEqual(PROJECT_DEFAULT);
    expect(result.project.sections[0]!.azimuth_reference).toEqual(DECLARED);
    // A section that declares nothing must not be handed a value here: `null`
    // is what makes it INHERIT the project default downstream.
    expect(result.project.sections[1]!.azimuth_reference).toBeNull();
  });

  it('mirrors both onto the SoA section metadata the renderer reads', () => {
    const result = buildResult(
      envelope({ projectDefault: PROJECT_DEFAULT, sectionReference: DECLARED }),
      0,
    );

    expect(result.surveyData.sections[0]!.azimuth_reference).toEqual(DECLARED);
    expect(result.surveyData.sections[1]!.azimuth_reference).toBeNull();
  });

  it('leaves the project default ABSENT when the envelope has none', () => {
    const result = buildResult(envelope({}), 0);

    // Absent, not null: the generated `Project` declares it optional, and a
    // project that never declared one is a different state from one that did.
    expect(result.project.default_azimuth_reference).toBeUndefined();
    expect(result.project.sections[0]!.azimuth_reference).toBeNull();
  });
});
