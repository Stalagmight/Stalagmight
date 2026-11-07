import type { CommandError } from '../../../src/types/ipc/CommandError';
import type { MockCommandFamily, MockReply } from './types';

/**
 * The Compass (.mak/.dat) import pair of the Playwright IPC mock (#576 NF-W3CI-2):
 * `parse_compass` serves a fully synthetic two-survey file (no Compass file is
 * read anywhere), and `execute_compass_import` lands one section per survey in
 * the core's open project and returns the receipt the dialog presents.
 *
 * [#576 E3] The landing is canned: what the file lands with the default options
 * (the excluded leg left out), whatever the dialog sent. Which legs land under which
 * options, and the receipt's counts, are the backend's (app import_command_tests
 * `the_compass_command_honours_include_excluded_and_records_the_file`, parsers
 * compass `preview_shot_counts_match_the_landed_count_in_all_four_toggle_states`).
 *
 * Runs IN THE PAGE (serialized with `Function.prototype.toString`): only `ctx`
 * and browser globals, so every helper lives inside the factory.
 */
export const compassImportCommands: MockCommandFamily = (ctx) => {
  type Leg = { length: number; azimuth: number; depthIn: number; depthOut: number; excluded: boolean };
  const SURVEYS: Array<{ name: string; date: string; team: string | null; declination: number; legs: Leg[] }> = [
    {
      name: 'A1',
      date: '2024-03-01',
      team: 'Diver One',
      declination: 0,
      legs: [
        { length: 10, azimuth: 0, depthIn: 1, depthOut: 2, excluded: false },
        { length: 8, azimuth: 45, depthIn: 2, depthOut: 3, excluded: false },
        { length: 6, azimuth: 90, depthIn: 3, depthOut: 3, excluded: true },
        { length: 5, azimuth: 135, depthIn: 3, depthOut: 4, excluded: false },
      ],
    },
    {
      name: 'A2',
      date: '2024-03-02',
      team: null,
      declination: 0,
      legs: [
        { length: 7, azimuth: 180, depthIn: 1, depthOut: 1, excluded: false },
        { length: 9, azimuth: 225, depthIn: 1, depthOut: 2, excluded: false },
        { length: 4, azimuth: 270, depthIn: 2, depthOut: 2, excluded: false },
      ],
    },
  ];
  const noInvalid = () => ({
    non_excluded_corrected: 0,
    non_excluded_uncorrected: 0,
    excluded_corrected: 0,
    excluded_uncorrected: 0,
  });
  const excludedIn = (legs: Leg[]) => legs.filter((leg) => leg.excluded).length;

  return (cmd, args) => {
    switch (cmd) {
      case 'parse_compass':
        return {
          cave_name: 'CaveA',
          survey_count: SURVEYS.length,
          shot_count: SURVEYS.reduce((sum, s) => sum + s.legs.length, 0),
          excluded_shot_count: SURVEYS.reduce((sum, s) => sum + excludedIn(s.legs), 0),
          invalid_shot_counts: noInvalid(),
          has_backsights: false,
          datum: null,
          utm_zone: null,
          has_georeference: false,
          declination_mode: 'file',
          length_unit: 'meters',
          surveys: SURVEYS.map((s) => ({
            name: s.name,
            date: s.date,
            team: s.team,
            shot_count: s.legs.length,
            excluded_shot_count: excludedIn(s.legs),
            invalid_shot_counts: noInvalid(),
            declination: s.declination,
          })),
          warnings: [],
        } satisfies MockReply<'parse_compass'>;

      case 'execute_compass_import': {
        const project = ctx.currentProject() as { sections: Array<Record<string, unknown>> } | null;
        if (!project) return Promise.reject({ code: 'FAILED', message: 'No project open' } satisfies CommandError);
        let nextId = project.sections.reduce((max, s) => Math.max(max, Number(s.id) || 0), 0);
        const sectionIds: number[] = [];
        let shotCount = 0;
        for (const survey of SURVEYS) {
          const legs = survey.legs.filter((leg) => !leg.excluded);
          if (legs.length === 0) continue;
          nextId += 1;
          const sectionId = nextId;
          project.sections.push({
            id: sectionId,
            name: survey.name,
            surveyor: survey.team ?? '',
            date: survey.date,
            color: '#3b82f6',
            direction: 'in',
            // The core's shot builder (uuid included - the real backend never
            // produces a uuid-less shot), at the same id stride as its sections.
            shots: legs.map((leg, index) =>
              ctx.buildShot(sectionId * 100000 + index, {
                azimuth_in: leg.azimuth,
                azimuth_out: leg.azimuth,
                length: leg.length,
                depth_in: leg.depthIn,
                depth_out: leg.depthOut,
              })
            ),
          });
          sectionIds.push(sectionId);
          shotCount += legs.length;
        }
        const path = (args?.path as string | undefined) ?? '';
        return {
          warnings: [],
          remote_link: ctx.remoteLink(),
          section_ids: sectionIds,
          import_receipt: {
            source_format: 'Compass',
            file_name: path.split('/').pop() ?? path,
            sections_found: SURVEYS.length,
            sections_imported: sectionIds.length,
            shot_count: shotCount,
            closures_requested: 0,
            closures_created: 0,
            unit_conversion: null,
            warnings: [],
            info: [],
            zero_datum_created: null,
          },
        } satisfies MockReply<'execute_compass_import'>;
      }

      default:
        return ctx.unhandled;
    }
  };
};
