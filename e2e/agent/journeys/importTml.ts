/**
 * import-tml: the release golden flow for Ariane TML import (#391/#411 class).
 * Parses the fixture first and derives EVERY expectation from the parse result,
 * so the journey is fixture-content-agnostic: whatever the file says, the
 * imported project must agree with it, surface a receipt, and cost exactly one
 * undo entry.
 */
import type { Journey } from './framework';
import { loadFixtureWithoutUndoHistory, disableAutoSave, undoDepth } from '../scenarios/common';

const TML_FIXTURE = 'test_simple.tml';

interface ParsedShot {
  shot_type: string;
  excluded: boolean;
}

interface ParsedTml {
  sections: { shots: ParsedShot[] }[];
}

/**
 * Mirror of the import-side filter (`parsers/tml/mod.rs`, convert step): CLOSURE
 * shots never import, excluded shots import only with `includeExcluded`, and a
 * section left with zero shots after filtering is dropped entirely. Expected
 * counts must be derived through this filter or a fixture containing any of
 * those features reds the journey on CORRECT app behavior.
 */
function importableCounts(parsed: ParsedTml): { sections: number; shots: number } {
  let sections = 0;
  let shots = 0;
  for (const section of parsed.sections) {
    const kept = section.shots.filter((s) => s.shot_type !== 'Closure' && !s.excluded).length;
    if (kept === 0) continue;
    sections += 1;
    shots += kept;
  }
  return { sections, shots };
}

interface StoreCounts {
  sections: number;
  shots: number;
}

export const importTml: Journey = {
  name: 'import-tml',
  description: 'Parse a .tml, import it, and hold the project to what the parse promised.',
  async run(ctx) {
    await loadFixtureWithoutUndoHistory(ctx);
    await disableAutoSave(ctx);
    const tml = ctx.fixturePath(TML_FIXTURE);

    const parsedRes = (await ctx.callAction('project', 'parseTml', [tml])) as {
      ok: boolean;
      result?: ParsedTml;
      error?: string;
    };
    if (!parsedRes.ok || !parsedRes.result) {
      throw new Error(`parseTml failed: ${parsedRes.error ?? 'unknown'}`);
    }
    const parsed = parsedRes.result;
    const { sections: expectedSections, shots: expectedShots } = importableCounts(parsed);
    ctx.expect('parse found importable survey data', expectedSections > 0 && expectedShots > 0,
      `importable sections=${expectedSections} shots=${expectedShots}`);

    const before = await counts(ctx);
    const depthBefore = await undoDepth(ctx);

    const hashRes = (await ctx.callAction('project', 'calculateFileHash', [tml])) as {
      ok: boolean;
      result?: string;
    };
    const importRes = (await ctx.callAction('project', 'executeTmlImport', [
      tml,
      hashRes.ok ? hashRes.result : null,
      false,
    ])) as {
      ok: boolean;
      result?: {
        success?: boolean;
        error?: string | null;
        section_ids?: number[];
        import_receipt?: unknown;
      };
    };
    await ctx.checkpoint('after-import');

    ctx.expect('import reported success', importRes.ok && importRes.result?.success === true,
      `error=${importRes.result?.error ?? 'none'}`);
    ctx.expect(
      'one section id returned per parsed section',
      (importRes.result?.section_ids?.length ?? 0) === expectedSections,
      `section_ids=${importRes.result?.section_ids?.length ?? 'absent'} expected=${expectedSections}`,
    );
    ctx.expect('an import receipt was returned', importRes.result?.import_receipt !== undefined,
      'import_receipt absent from the import outcome');

    const after = await counts(ctx);
    ctx.expect('project gained exactly the parsed sections',
      after.sections === before.sections + expectedSections,
      `before=${before.sections} after=${after.sections} expected+=${expectedSections}`);
    ctx.expect('project gained exactly the parsed shots',
      after.shots === before.shots + expectedShots,
      `before=${before.shots} after=${after.shots} expected+=${expectedShots}`);

    const depthAfter = await undoDepth(ctx);
    ctx.expect('import cost exactly one undo entry', depthAfter === depthBefore + 1,
      `depth ${depthBefore} -> ${depthAfter}`);

    const undoRes = (await ctx.callAction('project', 'undo', [])) as { ok: boolean };
    const reverted = await counts(ctx);
    ctx.expect('undo removed the import completely',
      undoRes.ok && reverted.sections === before.sections && reverted.shots === before.shots,
      `after undo: sections=${reverted.sections}/${before.sections} shots=${reverted.shots}/${before.shots}`);
  },
};

async function counts(ctx: Parameters<Journey['run']>[0]): Promise<StoreCounts> {
  return ctx.eval<StoreCounts>(`
    const p = window.__E2E_TEST__.projectStore.getState().project;
    if (!p) return { sections: 0, shots: 0 };
    const sections = (p.sections || []).length;
    const shots = (p.sections || []).reduce((n, s) => n + ((s.shots || []).length), 0);
    return { sections, shots };
  `);
}
