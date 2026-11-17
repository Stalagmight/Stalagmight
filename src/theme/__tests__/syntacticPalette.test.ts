import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import {
  SYNTACTIC_ROLES,
  SYNTACTIC_PRESETS,
  TOOL_ROLE_MAP,
  resolveSyntacticColors,
  syntacticVarName,
} from '../syntacticPalette';

const ROLE_IDS = SYNTACTIC_ROLES.map((r) => r.id);

describe('SYNTACTIC_PRESETS', () => {
  it('every preset defines a valid #rrggbb hex for every role', () => {
    for (const [preset, palette] of Object.entries(SYNTACTIC_PRESETS)) {
      for (const role of ROLE_IDS) {
        const color = palette[role];
        expect(color, `${preset}.${role}`).toMatch(/^#[0-9a-fA-F]{6}$/);
      }
    }
  });

  it('default preset matches the spec object colors', () => {
    expect(SYNTACTIC_PRESETS.default!.segment).toBe('#ff8000');
    expect(SYNTACTIC_PRESETS.default!.line).toBe('#a335ee');
    expect(SYNTACTIC_PRESETS.default!.shot).toBe('#0070dd');
  });
});

describe('TOOL_ROLE_MAP', () => {
  it('maps every tool to a known role', () => {
    for (const role of Object.values(TOOL_ROLE_MAP)) {
      expect(ROLE_IDS).toContain(role);
    }
  });

  it('move-group tools reuse their entity color role', () => {
    expect(TOOL_ROLE_MAP.move_station).toBe('station');
    expect(TOOL_ROLE_MAP.move_shot).toBe('shot');
    expect(TOOL_ROLE_MAP.move_segment).toBe('segment');
  });
});

describe('resolveSyntacticColors', () => {
  it('returns base preset colors when there are no overrides', () => {
    const resolved = resolveSyntacticColors('default', {});
    expect(resolved).toEqual(SYNTACTIC_PRESETS.default);
  });

  it('applies a valid override for the active preset only', () => {
    const resolved = resolveSyntacticColors('default', {
      default: { station: '#123456' },
      deuteranopia: { station: '#abcdef' },
    });
    expect(resolved.station).toBe('#123456');
    // Untouched roles keep their base value.
    expect(resolved.shot).toBe(SYNTACTIC_PRESETS.default!.shot);
  });

  it('ignores malformed override values (falls back to base)', () => {
    const resolved = resolveSyntacticColors('default', {
      default: { station: 'not-a-color', shot: '#00ff00' },
    });
    expect(resolved.station).toBe(SYNTACTIC_PRESETS.default!.station);
    expect(resolved.shot).toBe('#00ff00');
  });

  it('falls back to the default preset for an unknown preset value', () => {
    // The generated type allows "unknown"; resolver must not throw.
    const resolved = resolveSyntacticColors('unknown', undefined);
    expect(resolved).toEqual(SYNTACTIC_PRESETS.default);
  });
});

describe('syntacticVarName', () => {
  it('builds the --sh-<role> custom property name', () => {
    expect(syntacticVarName('station')).toBe('--sh-station');
    expect(syntacticVarName('delete')).toBe('--sh-delete');
  });
});

describe('index.css --sh-* defaults', () => {
  // The :root defaults in index.css avoid a flash before useSyntacticHighlighting
  // paints. They must mirror the default preset (the source of truth) exactly -
  // this guards against the two copies silently drifting apart.
  it('mirror the default preset for every role', () => {
    const cssPath = fileURLToPath(new URL('../../index.css', import.meta.url));
    const css = readFileSync(cssPath, 'utf8');
    for (const role of ROLE_IDS) {
      const varName = syntacticVarName(role);
      // Capture the whole value up to the semicolon so a malformed or shorthand
      // value is caught too (exact match against the #rrggbb source of truth).
      const match = new RegExp(`${varName}:\\s*([^;]+);`).exec(css);
      expect(match, `${varName} missing from index.css :root`).not.toBeNull();
      expect(match![1]!.trim().toLowerCase(), varName).toBe(
        SYNTACTIC_PRESETS.default![role]!.toLowerCase(),
      );
    }
  });
});
