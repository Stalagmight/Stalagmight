/**
 * #576 W51: the print and write sites of the scenarios, the journeys and the shared e2e/lib helpers, checked by
 * `printSites.test.ts` against `rules.ts`. Every entry says why its value may
 * reach its sink; see `rules.ts` for the classes and the reasons.
 */
import { R, type SiteGroup } from './rules';

/**
 * The fuzz fixture (`lib/fuzzKnobs.ts` `fuzzFixture()`, `STALAGMIGHT_FUZZ_FIXTURE`).
 * It is the knob as the user wrote it, so an error or progress line prints it
 * folded (scenarioFailOutput / shareableProgressLine -> shareableFailureText ->
 * foldLocalPaths) and a metric key folds it in place (`foldLocalPaths(fixture)`).
 * The documented reproduce-line policy keeps it (#576 RV18-W23-4,
 * docs/agent-harness.md "What a run shares"). Whether a private fixture's name
 * should print at all is an open question in the W51 report.
 */
const FIXTURE =
  'the fixture as the user named it (default fractal_plant_iter3.stalx), folded by the runner (shareableFailureText -> ' +
  'foldLocalPaths) or by foldLocalPaths at a metric key; the documented reproduce-line policy keeps it (RV18-W23-4); ' +
  'see the W51 open question on private fixture names';

/** A trace path: `writeRunTrace` / `openReplay`'s path, printed in a progress or error line, folded by the runner. */
const TRACE_PATH =
  'a trace path: STALAGMIGHT_FUZZ_REPLAY / STALAGMIGHT_FUZZ_TRACE as given, or <session>/traces/<scenario>-seed<n>-' +
  '<stamp>.json (lib/fuzzTrace.ts defaultTracePath, writeTrace returns it); every line it is in prints through ' +
  'shareableProgressLine / scenarioFailOutput -> shareableFailureText -> foldLocalPaths, so the checkout prints as ' +
  '<repo> and the home directory as ~';

/** `iterations`: `runScenario` hands every scenario `Number(opts.iterations ?? scenario.defaultIterations)`. */
const ITERATIONS =
  'the iteration count: scenarios/index.ts runScenario passes Number(opts.iterations ?? scenario.defaultIterations), ' +
  'a number whatever the caller read it from';

/** A loop index or a count the scenario keeps itself. */
const LOOP_COUNT = 'a loop index or a counter the scenario increments itself (a number)';

/** `ctx.seed`: `makeContext` sets it to `Number(seed) || 1`. */
const SEED = 'the seed: scenarios/framework.ts makeContext sets ctx.seed to Number(seed) || 1, a number';

/** A metric's value read back: `ctx.metric` adds numbers, `ctx.setMetric` sets one. */
const METRIC_COUNT = 'a metric value: ctx.metric / ctx.setMetric only ever store numbers (scenarios/framework.ts)';

/** A skip reason from an in-page step script of exportFuzzActions.ts. */
const EXPORT_SKIP =
  "an ExportSkip word: exportFuzzActions.ts's in-page scripts return only the literals of `ExportSkip` " +
  "(exportFuzzActions.ts:327-334: absent, not-actionable, option-absent, no-change, ..., unconsumed) or null";

/** A screen id from the in-page screen digest. */
const SCREEN_ID =
  "a ScreenId: lib/importScreenDigest.ts's in-page classifier assigns only its own literals (importScreenDigest.ts " +
  ':327-363, idle ... unknown), never a store or DOM value';

/** A verdict from the undo fuzzers' in-page step script. */
const STEP_VERDICT =
  "a step verdict: undoRedoFuzzBridge.ts's in-page step script sets only 'recorded' | 'equal' | 'differ' | " +
  "'unexpected' (undoRedoFuzzBridge.ts:202, 367-407)";

/** A trace step's kind, validated by `readTrace`. */
const STEP_KIND =
  "a trace step's kind after readTrace (via openReplay): lib/fuzzTrace.ts validateStep refuses any kind not in " +
  'STEP_KINDS (fuzzTrace.ts:942), so it is one of that set';

/** A message an oracle error class carries: its own construction site is checked in lib/. */
const ORACLE_ERROR =
  "an oracle error's message: every `new ExportOracleError(...)` / `new ImportOracleError(...)` is in " +
  'lib/exportOracles.ts / lib/importFlowOracles.ts, an error site of its own that the Lib group checks; fail() ' +
  'keeps it as its first line';

/** A JSON value after `: ` on a headline: shareableHeadline withholds it. */
const JSON_AFTER_COLON =
  "a JSON array or object right after `: ` on the first line: shareableHeadline's `/^(.*?): ([[{].*)$/` shape " +
  '(lib/shareableFailure.ts APP_ERROR_HEADLINES) prints it as its length; the scanner does not model that shape';

/** A session directory the scenario made under the checkout. */
const SESSION_DIR =
  "a path under the session directory (<repo>/.agent-artifacts/sessions/<id>, lib/recorder.ts ARTIFACTS_ROOT) or " +
  "the run's own mkdtemp under os.tmpdir(); its line prints through scenarioFailOutput -> shareableFailureText -> " +
  'foldLocalPaths';

/** The in-page probe of `countUnanchoredComponents` (importDialogFuzz.ts), keyed by its whole expression. */
const UNANCHORED_PROBE =
  "ctx.eval<number>(` const core = window.__TAURI__ && window.__TAURI__.core; if (!core || typeof core.invoke !== 'function') return -1; const state = await core.invoke('get_project_state'); const project = (state && state.project) || null; if (!project || !Array.isArray(project.sections)) return -1; const parent = new Map(); const find = (x) => { while (parent.get(x) !== x) { parent.set(x, parent.get(parent.get(x))); x = parent.get(x); } return x; }; for (const section of project.sections) parent.set(section.id, section.id); const union = (a, b) => { if (!parent.has(a) || !parent.has(b)) return; const ra = find(a); const rb = find(b); if (ra !== rb) parent.set(ra, rb); }; for (const merge of (project.station_merges || [])) { union(merge.station_a && merge.station_a.section_id, merge.station_b && merge.station_b.section_id); } // \\`ZeroDatumWire.station_ref\\` - the field name matters, and getting it // wrong is silent: every component then reads as unanchored. const anchored = new Set(); for (const datum of (project.zero_datums || [])) { const ref = datum && datum.station_ref; const id = ref && ref.section_id; if (id !== undefined && parent.has(id)) anchored.add(find(id)); } if (anchored.size === 0 && (project.zero_datums || []).length > 0) return -2; const roots = new Set(); for (const section of project.sections) roots.add(find(section.id)); let count = 0; for (const root of roots) if (!anchored.has(root)) count += 1; return count; `)";

const ACTIVE_TOOL_PROBE =
  'ctx.eval<string>( ` const ui = window.__E2E_TEST__.uiStore.getState(); ui.setActiveTool(args[0]); return window.__E2E_TEST__.uiStore.getState().activeTool; `, [tool], )';

const EXPORT_TRACE =
  "writeRunTrace( ctx, recorder, { scenario: SCENARIO, iterations, fixture, stream: 'export-v1' }, failure, traceOut, )";
const IMPORT_TRACE =
  "writeRunTrace( ctx, recorder, { scenario: SCENARIO, iterations, fixture, // v3 = the five-format vocabulary: corpus sources, the armed // keyboard picker, and the auxiliary interjection stream. stream: 'import-v3', noProjectEpisodes: headlessEpisodes, }, failure, traceOut, )";
const UNDO_TRACE =
  'writeRunTrace( ctx, recorder, { scenario: undoRedoFuzz.name, iterations, fixture, stream: streamName() }, failure, explicitPath, )';

export const GROUP: SiteGroup = {
  covers: ['e2e/agent/scenarios/', 'e2e/agent/journeys/', 'e2e/lib/'],
  programs: {},
  allow: {
    // ---- journeys -----------------------------------------------------------
    'e2e/agent/journeys/framework.ts': {
      'expect: label':
        'a check label: every ctx.expect call passes a string literal (shareableRunnerOutput.test.ts "every journey ' +
        'progress value ..." checks it)',
      'expect: detail':
        'withheld at runtime: it ends the line after ` - failed: ` (framework.ts:73), which shareableHeadline prints as ' +
        'its length; it sits in a nested template the scanner does not flatten',
      'checkpoint: name':
        'a checkpoint name: every ctx.checkpoint call passes a string literal (shareableRunnerOutput.test.ts checks it)',
      'path.join(shotsDir, `${name}.png`)':
        'the checkpoint screenshot path <session>/journeys/<journey>/<literal name>.png (framework.ts:65, 83): the ' +
        'progress line prints through shareableProgressLine -> foldLocalPaths',
    },
    'e2e/agent/journeys/newProjectFromDmp.ts': {
      'digest.screen': SCREEN_ID,
      "visited.join(' -> ')": `the screens the journey walked, each pushed from digest.screen (newProjectFromDmp.ts:163): ${SCREEN_ID}`,
      'action.label':
        "a move label `click <test id>` built from primaryAction's literal test ids (newProjectFromDmp.ts:75-121)",
      'outcome.reason':
        "a SkipReason word: importDialogBridge.ts's in-page step script sets only the literals of SkipReason " +
        '(importDialogBridge.ts:311-317, 545-690) or null',
    },
    // ---- scenarios ----------------------------------------------------------
    'e2e/agent/scenarios/common.ts': {
      'loadFixtureWithoutUndoHistory: fixture': `${FIXTURE}; every caller passes fuzzFixture(), DEFAULT_FIXTURE, or a trace's reload fixture that exportRoundtripFuzz first requires to equal the run's fixture (exportRoundtripFuzz.ts reload check, lib/fuzzHarness.ts openReplay)`,
    },
    'e2e/agent/scenarios/dialogCycling.ts': {
      'DIALOGS[i % DIALOGS.length]': "a dialog id from the scenario's own DIALOGS table (dialogCycling.ts:11)",
      'run: iterations': ITERATIONS,
      'ctx.eval<number>(\'return document.querySelectorAll("*").length;\')':
        "a DOM node count: the harness's own in-page script returns `.length`",
    },
    'e2e/agent/scenarios/exportFuzzActions.ts': {
      "ALL_FORMATS.join(',')": "the scenario's own format table (exportFuzzActions.ts ALL_FORMATS)",
    },
    'e2e/agent/scenarios/exportRoundtripFuzz.ts': {
      'fail: message': R.FORWARD,
      'err.message': ORACLE_ERROR,
      'assertIntegrity: after':
        'an integrity-check label: every caller passes a literal or `round-trip import pass ${pass}` with a loop ' +
        'number (exportRoundtripFuzz.ts assertIntegrity callers)',
      'afterLoad.appMode':
        "uiStore.appMode, typed AppMode = 'survey' | 'plan' | 'export' (src/store/uiStore.ts:56), read by " +
        "readModeSnapshot (exportFuzzActions.ts:461-486, 'survey' when null)",
      'JSON.stringify(options)': JSON_AFTER_COLON,
      'JSON.stringify(leftover.slice(0, 8))': JSON_AFTER_COLON,
      "enabledFormats().join(',')":
        'enabledFormats keeps only entries of ALL_FORMATS and refuses any other (exportFuzzActions.ts:61-80)',
      'readImageOptions: name': 'an OracleName: runOracle dispatches on ORACLE_NAMES members only (oracleFromStep checks a trace\'s)',
      'readExtension: name': 'an OracleName: runOracle dispatches on ORACLE_NAMES members only (oracleFromStep checks a trace\'s)',
      "ORACLE_NAMES.join(', ')": "the scenario's own oracle table (exportRoundtripFuzz.ts ORACLE_NAMES)",
      'run: replayPath': TRACE_PATH,
      'dumpTrace: replayPath': TRACE_PATH,
      [EXPORT_TRACE]: TRACE_PATH,
      'replay.fixture': `${FIXTURE}; openReplay refuses a trace whose fixture is not the run's (lib/fuzzHarness.ts openReplay)`,
      'run: episode': LOOP_COUNT,
      'run: moves': LOOP_COUNT,
      'runRoundtrip: pass': LOOP_COUNT,
      // Metric keys.
      'typed.reason': EXPORT_SKIP,
      'chosen.reason': EXPORT_SKIP,
      'dpi.reason': EXPORT_SKIP,
      'threshold.reason': EXPORT_SKIP,
      'toggle.reason': EXPORT_SKIP,
      'outcome.reason': EXPORT_SKIP,
      'color.reason': EXPORT_SKIP,
      'answered.reason': EXPORT_SKIP,
      'applyFormControls: testid': 'a test id from the literal `checkboxes` table (exportRoundtripFuzz.ts:975-982)',
      'performExport: skipped':
        "runStoreExport's skip word: 'no-dive-route' or 'preview-has-no-store-route' (exportRoundtripFuzz.ts:994-1028)",
      'draw.format':
        'an ExportFormat: drawExport draws from ALL_FORMATS, and drawFromStep refuses a trace format isExportFormat ' +
        'rejects (#576 W51)',
      'draw.route':
        "'modal' | 'panel' | 'store': drawExport's table, and drawFromStep refuses any other trace route",
      'draw.shape': "a PathShape: drawExport's table, and drawFromStep refuses a trace shape not in PATH_SHAPE_NAMES",
      'runDifferential: format': "the loop over the literal ['svg', 'png', 'pdf'] (runDifferential)",
      'runByteIdentity: extension':
        "'tml' | 'tmlu': readExtension refuses anything else on a replay, the generator draws one of the two",
      'verdict.via': "checkConvergence's 'raw' | 'canonical' (lib/exportOracles.ts:956-968)",
      'verdict.settledAt': "checkConvergence's settledAt, a loop index + 1 (lib/exportOracles.ts:966-968)",
      'step.oracle':
        'an OracleName: the metric follows runOracle(oracleFromStep(step)), which throws for a name not in ORACLE_NAMES',
      'step.kind': STEP_KIND,
      'digest.missingUuids': 'a count from readProjectDigest (reviewed in shareableRunnerOutput.test.ts HARNESS_VALUES)',
      'digest.duplicateUuids': 'a count from readProjectDigest (reviewed in shareableRunnerOutput.test.ts HARNESS_VALUES)',
    },
    'e2e/agent/scenarios/fractalStress.ts': {
      'checkBudget: phase':
        "a phase label: every caller passes a literal or `<literal> (<index>/<count>)` with numbers (fractalStress.ts " +
        '72, 84, 88, 131, 134, 137, 162)',
      'run: iterations': ITERATIONS,
      'sections.reduce((n, s) => n + s.shots.length, 0)': 'a sum of array lengths (the generated survey\'s shot count)',
      'run: index': LOOP_COUNT,
      'section.name':
        "a section name the harness generated: e2e/lib/fractal.ts executeLSystem names them `S0`, `S<n>` " +
        '(fractal.ts:112, 158)',
      'sections[index]?.name': 'the generated name `S<n>` the store was expected to hold (e2e/lib/fractal.ts:112, 158)',
      'path.join(saveDir, `fractal-iter${iterations}.stalx`)':
        "the scenario's own temp file <os.tmpdir()>/fractal-stress-XXXXXX/fractal-iter<N>.stalx (fractalStress.ts:144-145); " +
        'the error prints through scenarioFailOutput -> shareableFailureText -> foldLocalPaths',
      'verifyAgainstBackend: expectedSections': 'a count: the caller passes sections.length (fractalStress.ts:136)',
      "missing.slice(0, 3).join('; ')":
        'merge keys `<int>-<int>-<int>-<int>` (e2e/lib/fractal.ts normalizeMergeKey:249-260) of integer ids and indices',
      "extra.slice(0, 3).join('; ')":
        "merge keys `<int>-<int>-<int>-<int>` (normalizeMergeKey) of get_project_state's integer station_merges ids",
    },
    'e2e/agent/scenarios/importDialogBridge.ts': {
      'move.kind': "a Move's kind, 'ui' | 'input' (literals in the generators and in importDialogTrace replayedMove)",
    },
    'e2e/agent/scenarios/importDialogFuzz.ts': {
      "enabledFamilies().join(',')":
        'enabledFamilies keeps only SURVEY_FORMAT_FAMILIES members (importDialogGenerators.ts:410-419)',
      "formats.join(',')": 'enabledFormats(): FORMATS_IN_FAMILY values of the enabled families (importDialogGenerators.ts:422-426)',
      "enabledFormats().join(',')": 'FORMATS_IN_FAMILY values of the enabled families (importDialogGenerators.ts:422-426)',
      'describeCorpus(inventory)':
        'counts per corpus format, skip counts and crc20 ids, never a path (lib/fixtureCorpus.ts describeCorpus; ' +
        'reviewed in shareableRunnerOutput.test.ts HARNESS_VALUES)',
      'run: CORPUS_DIR_ENV': R.ENV_NAME,
      'run: written':
        'a path the scenario wrote: the self-check file <session>/fuzz-files/selfcheck/selfcheck-<format>-<i>.<format> ' +
        '(importDialogFuzz.ts self-check); the line prints through scenarioFailOutput -> shareableFailureText -> ' +
        'foldLocalPaths',
      [IMPORT_TRACE]: TRACE_PATH,
      'claim.min': "a section/survey count selfCheckClaim derives from the generated spec (lib/surveyFileSpecs.ts:1668-1730)",
      'claim.max': "a section/survey count or null from selfCheckClaim (lib/surveyFileSpecs.ts:1668-1730)",
      'claim.what': "selfCheckClaim's literal 'section' | 'survey' (lib/surveyFileSpecs.ts:1668-1730)",
      'claim.command': "selfCheckClaim's literal IPC command name (import_dmp, parse_tml, parse_compass, parse_walls, ...)",
      'claim.read(parsed)':
        "a number: readArrayLength / readCount return an array length or a finite numeric field, else -1 " +
        '(lib/surveyFileSpecs.ts:1658-1666)',
      'noProjectEpisodeProbability()': R.KNOB,
      'fuzzFixture()': FIXTURE,
      'runEpisodes: episode': LOOP_COUNT,
      'runEpisodes: dir': SESSION_DIR,
      'path.resolve(sessionDir)': SESSION_DIR,
      'JSON.stringify(opening.messages ?? [])': JSON_AFTER_COLON,
      'digest.screen': SCREEN_ID,
      'before.screen': SCREEN_ID,
      'escapeFrom: ESCAPE_BUDGET': 'the number 6 (importDialogModel.ts:507 ESCAPE_BUDGET)',
      'err.message': ORACLE_ERROR,
      "verdict.gates.join(' + ')":
        "exportChordVerdict's gates (lib/importFlowOracles.ts:420-422): the literals 'placementMode active' and " +
        "'dataTableFullScreen', and `activeDialog=\"<shareableTestId>\"`",
      'move.testid':
        'a test id from a fixed set: orphanMove is move.testid only when ORPHAN_ACCEPTS / ORPHAN_DECLINES has it ' +
        '(importDialogFuzz.ts:671-672), and wizardCommit requires INCOMPLETE_GUARDED_MOVES to have it (:645)',
      'move.value':
        "overlapMode, printed only in the fail() detail (a later line, withheld) and as a metric key behind " +
        'OVERLAP_MODES.includes(overlapMode) (importDialogModel.ts:356; the scanner does not see the check)',
      'report.headline':
        "describeRunError's headline (lib/fuzzHarness.ts): its own STALL line, or the first line of an error thrown " +
        'elsewhere - an error site of its own, or a page/IPC error in an APP_ERROR_HEADLINES shape (`eval:`, ' +
        '`ipc <cmd>:`, `javascript error:`, `... failed:`), whose text shareableHeadline prints as its length',
      // Metric keys.
      'source.shape': "FileSource.shape, only ever 'suggestive' (importDialogGenerators.ts:77, 486)",
      'copied.reason': "a MaterializeSkip: 'bytes' | 'files' | 'depth' | 'io' (lib/fixtureCorpus.ts:441)",
      'execute: skip':
        "execute's skip word: a SkipReason / DialogOutcome reason literal, 'corpus-missing', 'file-missing', " +
        "'close-refused', 'map-unavailable', 'undo-refused' / 'redo-refused', 'save-refused' (importDialogFuzz.ts execute)",
      'work.reason':
        "a NoWorkReason: 'not-eligible' | 'not-performed' | 'cancel' | 'unconsumed' | 'gate-refused' " +
        '(lib/importFlowOracles.ts:185-188, s4WorkStarted)',
      'drawInterjection(interjectRng)':
        "drawInterjection's literal kind: escape | route | undo | redo | save (importDialogGenerators.ts drawInterjection)",
      'drawAuxInterjection(auxRng)':
        "drawAuxInterjection's literal kind: export | mode | close | openKey (importDialogGenerators.ts:1992-1998)",
      'generator.name': "a generator's literal name from importDialogGenerators.ts's table (`start:picker`, ...)",
      'outcome.skip': "execute's skip word (see `execute: skip`), or the literal 'unknown'",
      'step.kind': STEP_KIND,
      'digest.ctx': "uiStore.importContext as the digest reads it: 'new-project' | 'existing-project' (importScreenDigest.ts:151)",
      [UNANCHORED_PROBE]:
        "a number: countUnanchoredComponents' in-page script returns a component count, -1 or -2 (importDialogFuzz.ts)",
      'guarded: what()':
        "guarded(fn) returns fn(): at its one print, the f1Pushed metric key (importDialogFuzz.ts runEpisodes), fn is " +
        'checkPushed, which returns a number (lib/importFlowOracles.ts:676-681)',
      "ctx.metrics['file:shape:suggestive']": METRIC_COUNT,
    },
    'e2e/agent/scenarios/importDialogTrace.ts': {
      'replayedMove: scenario': "the caller's scenario name: importDialogFuzz passes its SCENARIO constant",
      "[...REPLAYABLE_UI_ACTIONS].join(', ')": "the module's own table (importDialogTrace.ts:125, ['closeProject'])",
    },
    'e2e/agent/scenarios/importRepeatedly.ts': {
      'run: i': LOOP_COUNT,
      'run: iterations': ITERATIONS,
      'undoDepth(ctx)': "an array length: common.ts undoDepth returns getUndoStack's length, or 0",
    },
    'e2e/agent/scenarios/index.ts': {
      'runScenario: name':
        'a scenario name: the prints at index.ts:96 and :100 follow the REGISTRY check (the unknown name prints as ' +
        'valueShape)',
      "listScenarios().join(', ')": "the registry's scenario names (index.ts:41-44)",
    },
    'e2e/agent/scenarios/mapStorm.ts': {
      'run: i': LOOP_COUNT,
      'run: iterations': ITERATIONS,
      'run: maxJumpMs': 'a duration: Math.max of Date.now() differences (mapStorm.ts)',
    },
    'e2e/agent/scenarios/rapidProjectCycle.ts': {
      'run: i': LOOP_COUNT,
      'run: iterations': ITERATIONS,
    },
    'e2e/agent/scenarios/saveUnderChurn.ts': {
      'run: i': LOOP_COUNT,
      'run: iterations': ITERATIONS,
    },
    'e2e/agent/scenarios/seedLargeProject.ts': {
      'counts.sections': "an array length the harness's in-page script returns (seedLargeProject.ts:19-24)",
      'counts.stations': "an array length the harness's in-page script returns (seedLargeProject.ts:19-24)",
      'counts.view': "uiStore.view, typed ViewType = 'global' | 'single' (src/store/uiStore.ts:36)",
    },
    'e2e/agent/scenarios/toolSwitchSpam.ts': {
      'TOOLS[i % TOOLS.length]': "a tool id from the scenario's own TOOLS table (toolSwitchSpam.ts:10-21)",
      [ACTIVE_TOOL_PROBE]: 'uiStore.activeTool, typed ToolType, a union of tool-id literals (src/store/uiStore.ts:202, 485)',
      'run: iterations': ITERATIONS,
    },
    'e2e/agent/scenarios/undoPersistenceFuzz.ts': {
      "Object.entries(UNREACHABLE_VARIANTS) .map(([name, why]) => `${name} (${why})`) .join('; ')":
        "the literal UNREACHABLE_VARIANTS table (undoPersistenceCoverage.ts:126-133)",
      'fuzzFixture()': FIXTURE,
      'ctx.seed': SEED,
      'run: iterations': ITERATIONS,
      'run: totalEntries': LOOP_COUNT,
      "[...seen.entries()].map(([v, c]) => `${v}=${c}`).sort().join(' ')":
        'UndoVariant names from VARIANT_BY_COMMAND_TYPE values (undoPersistenceCoverage.ts:80) with their counts',
      'run: variant': 'an UndoVariant: a VARIANT_BY_COMMAND_TYPE value (undoPersistenceFuzz.ts runIteration :894-904)',
      'runIteration: variant': 'an UndoVariant from PLANNED_VARIANTS (undoPersistenceCoverage.ts:860)',
      'runIteration: iteration': LOOP_COUNT,
      'runIteration: top': LOOP_COUNT,
      'runIteration: cursor': LOOP_COUNT,
      'runIteration: depth': LOOP_COUNT,
      'Object.values(stripped.cleared).reduce((sum, n) => sum + n, 0)':
        'a sum of the row counts stripUndoHistory deleted (lib/fixtures.ts stripUndoHistory, Number(before.count))',
      'readUndoPersistLimit(ctx)':
        "a number: the config's undo_persist_limit when typeof number, else 20 (lib/undoFuzzHarness.ts:58-64)",
      'applyMutation: stepName':
        'a COVERAGE_STEPS name, or `replay:<action>` after assertReplayableMutation checked the action against ' +
        'REPLAYABLE_MUTATIONS (undoPersistenceFuzz.ts:691, 830-833; lib/undoFuzzHarness.ts:82-89)',
      'res.verdict': STEP_VERDICT,
      'plan.name': "a COVERAGE_STEPS entry's literal name (undoPersistenceCoverage.ts:399)",
      'plan.variant': "a COVERAGE_STEPS entry's literal UndoVariant (undoPersistenceCoverage.ts:399)",
      'walk: direction': "walk's 'undo' | 'redo' parameter (callers pass the literals)",
      'walk: cursor': LOOP_COUNT,
      'JSON.stringify([...new Set(unmapped)])': JSON_AFTER_COLON,
      'JSON.stringify(missingGuaranteed)': `${JSON_AFTER_COLON}; it is GUARANTEED_VARIANTS names anyway`,
    },
    'e2e/agent/scenarios/undoRedoChurn.ts': {
      'run: i': LOOP_COUNT,
      'run: created': LOOP_COUNT,
    },
    'e2e/agent/scenarios/undoRedoFuzz.ts': {
      'JSON.stringify(importContext.value)':
        "uiStore.importContext, typed 'new-project' | 'existing-project' | null (src/store/uiStore.ts:817)",
      'fuzzFixture()': FIXTURE,
      'Object.values(stripped.cleared).reduce((sum, n) => sum + n, 0)':
        'a sum of the row counts stripUndoHistory deleted (lib/fixtures.ts stripUndoHistory, Number(before.count))',
      'init.undoLen': "get_undo_stack's length (undoRedoFuzzBridge.ts:265)",
      'init.redoLen': "get_redo_stack's length (undoRedoFuzzBridge.ts:266)",
      'JSON.stringify(stripped.cleared)':
        'UNDO_HISTORY_TABLES names with row counts (lib/fixtures.ts stripUndoHistory: keys from the frozen table list)',
      'JSON.stringify(stripped.missing)': 'UNDO_HISTORY_TABLES names (lib/fixtures.ts stripUndoHistory)',
      'path.resolve(filesRoot)': SESSION_DIR,
      'resolveFileArgs: action': 'a FILE_ARG_ACTIONS member: resolveFileArgs returns before the throw for any other action',
      'applyTraceStep: tag': '`T<index>` from lib/undoFuzzHarness.ts replayTrace (:144)',
      'walk: direction': "walk's 'undo' | 'redo' parameter",
      'walk: jumpSteps': 'a number: Math.min(traceStep.steps, reach) or drawJump (undoRedoFuzz.ts:1433, 1463)',
      'walk: cursor': LOOP_COUNT,
      'res.verdict': STEP_VERDICT,
      'action.action':
        'a FUZZABLE_ACTIONS name: the step dispatch (undoRedoFuzzBridge.ts runStep) throws for any other before ' +
        'these lines run',
      'runFuzz: top': LOOP_COUNT,
      'runFuzz: cursor': LOOP_COUNT,
      [UNDO_TRACE]: TRACE_PATH,
      'runFuzz: replayPath': TRACE_PATH,
      'streamName()': "streamName's literal 'v1' | 'v2' | 'v3' (undoRedoFuzz.ts:646-649)",
      'swarm.name': "swarmProfileFor's literal profile name (undoRedoFuzzActions.ts:577-596, 608-609, 622-628)",
      'ctx.seed': SEED,
      'ctx.metrics.mutations': METRIC_COUNT,
      'ctx.metrics.undos': METRIC_COUNT,
      'ctx.metrics.redos': METRIC_COUNT,
      'ctx.metrics.rejected': METRIC_COUNT,
      'ctx.metrics.noops': METRIC_COUNT,
      'runFuzz: deepest': 'Math.max of the cursor (undoRedoFuzz.ts:1002)',
      'runFuzz: maxBytes': "Math.max of the in-page snapshot's byte length (undoRedoFuzz.ts:1001)",
    },
    // ---- e2e/lib ------------------------------------------------------------
    'e2e/lib/env.ts': {
      'readBundleIdentifier: confPath':
        "the checkout's src-tauri/tauri.conf.json (env.ts:173 tauriConfPath, the one production caller " +
        'bundleIdentifier at :183); a runner prints the error through shareableErrorText, which folds it to <repo>',
      'readBundleIdentifier: err':
        "fs / JSON.parse's error about that tracked file: its errno and path, or a snippet of the public file",
      'JSON.stringify(identifier)': "the `identifier` field of the tracked src-tauri/tauri.conf.json",
    },
  },
  fileSinks: {
    'e2e/agent/scenarios/exportRoundtripFuzz.ts': {
      'performExport -> plnxSentinel': {
        class: 'shareable',
        what: "the X2 probe's .plnx sentinel beside the open project's session copy",
        reason:
          "literal content only ('export-fuzz dive-plan sentinel'); the file is the app's input for the dive-plan " +
          'refusal probe and nothing prints it',
      },
    },
    'e2e/lib/env.ts': {
      'clearRecentProjectsIn -> configPath': {
        class: 'private',
        what: "the app's own config.toml",
        reason:
          "the app's config file with recent_projects blanked (env.ts:277-285): the user's own settings, rewritten in " +
          'place for the app and never printed',
      },
    },
  },
  passthrough: {},
  wrappers: {
    // Each fuzzer's `fail(headline, detail?)` throws an Error whose FIRST line
    // is `headline` (exportRoundtripFuzz: `${message}\n...`; the other three
    // through lib/fuzzHarness.ts failureText + throwFailure, called only inside
    // `fail`), so its first argument is an error message of its own.
    'e2e/agent/scenarios/exportRoundtripFuzz.ts': { fail: 'error' },
    'e2e/agent/scenarios/importDialogFuzz.ts': { fail: 'error' },
    'e2e/agent/scenarios/undoPersistenceFuzz.ts': { fail: 'error' },
    'e2e/agent/scenarios/undoRedoFuzz.ts': { fail: 'error' },
    // The journey context's `expect` / `checkpoint` lines go through the
    // ScenarioContext it wraps (`base`), whose `log` is the runner's progress
    // logger (scenarios/framework.ts makeContext `log`), so `base.log` is
    // `ctx.log` under another name.
    'e2e/agent/journeys/framework.ts': { 'base.log': 'progress' },
  },
  builders: {},
  fileHelpers: {},
};
