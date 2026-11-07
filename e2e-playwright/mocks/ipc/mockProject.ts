// The Node-side project and section builders `../tauri-ipc.ts` exports, split out of it
// unchanged (#576 AFK-D3).

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

export { createMockProject, createMockSection };
export type { MockProject, MockSection, MockShot };
