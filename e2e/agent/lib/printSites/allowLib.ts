/**
 * #576 W51: the print and write sites of the harness library, e2e/agent/lib, checked by
 * `printSites.test.ts` against `rules.ts`. Every entry says why its value may
 * reach its sink; see `rules.ts` for the classes and the reasons.
 *
 * An `error` value is what a runner prints as the first line of a FAIL block or
 * a `fatal:` line (`shareableFailureText`, `shareableErrorText`): the scan
 * already withholds what follows a ` failed: ` there and what sits on a later
 * line, so the entries below are the values left on that first line.
 */
import { R, type SiteGroup } from './rules';

/** The runners keep only the `ipc <command>:` / `eval:` / `eval compile:` prefix of such a headline. */
const AFTER_APP_PREFIX =
  "the page's or the app's error after an `ipc <one-word command>: `, `eval: ` or `eval compile: ` prefix: " +
  "shareableHeadline (APP_ERROR_HEADLINES[0], shareableFailure.ts) prints it as its length";

/** `runDeterminismPair`'s divergence (fuzzDeterminismRun.ts), keyed by its whole expression. */
const DIVERGENCE =
  "formatDivergence( { scenario: meta.scenario, seed: meta.seed, iterations: meta.iterations, fixture: meta.fixture, knob: meta.mode === 'one-session' ? '1' : '2sessions', lastAction: lastActionBefore(a.probe.records, divergence.index), }, divergence, )";

/**
 * Its first line (fuzzDeterminism.ts:404-405) is the seed, two counts, a record
 * kind and the record's label. The probe makes every label from literals and
 * counters (fuzzDeterminismProbe.ts:101, 133-134, 140, 150, 162, 191-192, 201-203)
 * and what its callers pass: a literal with a loop counter (exportRoundtripFuzz.ts:2182,
 * 2209, 2238; undoPersistenceFuzz.ts:473; undoRedoFuzz.ts:810), a StepRequest kind
 * and a FUZZABLE_ACTIONS name runStep checked before the hook runs
 * (undoFuzzHarness.ts:246, 420; undoRedoFuzzBridge.ts:322), a `T<n>` / `#<n>` /
 * `boundary` tag (undoFuzzHarness.ts:144, 393; undoPersistenceFuzz.ts:427;
 * undoRedoFuzz.ts:1445, 1486), an `export*` store action its callers name in
 * literals (fuzzDeterminismProbe.ts:200 drops the rest), and the table names of the
 * `.stalx` the app's save just wrote to a fresh file (project_io.rs:781-791
 * clear_save_temp then Database::open_scratch: the app's own schema).
 */
const DIVERGENCE_REASON =
  'the divergence: line 1 the seed (Number() at both callers, scenarios/index.ts:78, scenario-run.ts:164), two ' +
  'counts, a DetRecordKind word (fuzzDeterminism.ts:214) and a record label made of harness words, counters, ' +
  "FUZZABLE_ACTIONS names and the app's own table names (each source cited where printSites/allowLib.ts defines " +
  'this reason); the path, the two values and the last action ' +
  "are later lines, withheld; the reproduce: line is the knob literal 1 or 2sessions (fuzzDeterminismRun.ts:105), " +
  'the fixture as the user named it, folded by the runner (shareableFailureText -> foldLocalPaths; the FIXTURE ' +
  'policy of allowScenarios.ts), the registry name and the two numbers';

export const GROUP: SiteGroup = {
  covers: ['e2e/agent/lib/'],
  programs: {},
  allow: {
    'e2e/agent/lib/build.ts': {
      // passLinesFolded(child.stdout/stderr, write): every text it hands `write` is foldLocalPaths(...) of a line.
      'write: text':
        "a line of `pnpm tauri build`'s output, folded by passLinesFolded (foldedLines.ts:31,37) before it reaches write",
      'buildApp: mode': "the AgentBuildMode, 'debug' or 'release' (e2e/lib/env.ts agentBuildMode)",
      // build.ts:833: execSync(`git checkout -- ${SCHEMA_PATHSPEC}`, { stdio: 'ignore' }) is the only call in the try.
      'err.message':
        "Node's `Command failed: git checkout -- src-tauri/gen/schemas` (stdio ignored, so no stderr is appended) or a spawn errno message",
      'buildApp: err':
        "the error of `git checkout -- src-tauri/gen/schemas` run with stdio ignored: Node's `Command failed: <that command>`",
      '(err as NodeJS.ErrnoException).code': 'the errno code of the failed stamp write (EACCES, EISDIR, ...), or the word io',
    },
    'e2e/agent/lib/controls.ts': {
      'requireSelector: action': "a literal tool name at every call: 'hover', 'select_option', 'element_state' (controls.ts:126,211,233)",
      'outcome.interaction':
        "'placement' or 'maplibre': the harness's own page script picks one of the two literals (controls.ts:368)",
    },
    'e2e/agent/lib/displayPool.ts': {
      'parseDisplayPool: raw':
        'only commas and whitespace: it is printed only when every comma-separated entry of it trims to nothing',
    },
    'e2e/agent/lib/exportOracles.ts': {
      'exportBaseName: shape': "'openProject' or 'plnxSibling': the switch case the throw sits in (exportOracles.ts:176-177)",
      "broken.map((a) => a.format).join(' and ')":
        "'png' and/or 'pdf': the arms runDifferential builds from its literal ['svg', 'png', 'pdf'] (exportRoundtripFuzz.ts)",
      "'ABC'[index]": 'one letter of the literal ABC, the index of the export (0 to 2)',
      'args.extension':
        "'tml' or 'tmlu': drawn by ctx.pick(['tml', 'tmlu']), or on replay refused otherwise by readExtension (exportRoundtripFuzz.ts)",
      'firstDifference(c1, c2)': R.COUNT,
      "lines.join('\\n')":
        "checkShotMultiset's message: line 1 is the literal `X6 the shot set changed across an export + re-import`, every pushed line is a later line",
      'args.during.appMode':
        "printed only when APP_MODES (AppMode's words, src/store/uiStore.ts) has it, checked on the line before; else its length",
      'args.during.geometryMode':
        "printed only when GEOMETRY_MODES (GeometryMode's words, src/store/uiStore.ts) has it, checked just before; else its length",
    },
    'e2e/agent/lib/fixtureCorpus.ts': {
      'entry.id': "a corpus entry's publishable id: 5 hex characters of a CRC (crc32.ts shortId), never its path",
      '(err as NodeJS.ErrnoException).code': "the errno code of the failed stat, or the word io - never the fs message, which names the path",
    },
    'e2e/agent/lib/fixtures.ts': {
      'path.resolve(databasePath)': R.FOLDED,
    },
    'e2e/agent/lib/foldedLines.ts': {
      'finish: partial': R.FOLDED,
      'decoder.end()': R.FOLDED,
      "lines.map((line) => `${foldLocalPaths(line)}\\n`).join('')":
        "the whole lines of the child's output that arrived, each folded by foldLocalPaths inside the map (the docs' rule, #576 RV19b-P8, W47-NF3)",
    },
    'e2e/agent/lib/fuzzDeterminismProbe.ts': {
      'this.states': "the probe's own state-record counter, a number: 0 (fuzzDeterminismProbe.ts:74), += 1 per state() (:112)",
    },
    'e2e/agent/lib/fuzzDeterminismRun.ts': {
      [DIVERGENCE]: DIVERGENCE_REASON,
      'meta.seed':
        'the seed, a number: both callers pass Number(...) (scenarios/index.ts:78 into :89-91, scenario-run.ts:164)',
    },
    'e2e/agent/lib/fuzzHarness.ts': {
      'replayTracePath()': R.FOLDED,
      'openReplay: fixture':
        "the run's fixture, fuzzKnobs.fuzzFixture() at all four callers (an e2e/fixtures/ name the run already opened); the scenarios' reproduce: lines print it too",
      'throwFailure: message':
        "throwFailure's parameter, forwarded whole: the scenario's failure message, failureText's headline first - its callers in e2e/agent/scenarios/ are where that headline is built (see the W51 report)",
      'throwFailure: dumpTrace({ message })':
        "the trace path writeRunTrace wrote, on a later `  trace: ` line, which a runner keeps only as `trace: <one path>.json`, folded by shareableFailureText's foldLocalPaths",
    },
    'e2e/agent/lib/fuzzMatrixArgs.ts': {
      'bad: message': R.FORWARD,
      'parseSeeds: raw': 'digits, dots and minus signs only: it matched /^(-?\\d+)\\.\\.(-?\\d+)$/ on the line before',
      'positiveInt: flag': "a literal flag name at every call: 'workers', 'iterations' (fuzzMatrixArgs.ts:205,229)",
      'booleanFlag: flag': "the literal flag name 'release' at its one call (fuzzMatrixArgs.ts:230)",
      'raw.slice(0, eq)': 'the axis key, printed only once RESERVED_AXIS_KEYS has it (checked on the line before): an env name from that table',
      'RESERVED_BY[key]': 'the owner text of a reserved axis key, from the RESERVED_BY table in this file',
      "[...RESERVED_AXIS_KEYS].join(', ')": R.ENV_NAME,
      'token.slice(2, eq)': "the flag name, printed only once `known` has it (checked on the line before): one of the parser's own flag names",
      'token.slice(2)': "the flag name, printed only once `known` has it (checked on the line before): one of the parser's own flag names",
      "path.dirname(fixtureSourcePath('probe.stalx'))": R.FOLDED,
    },
    'e2e/agent/lib/fuzzOracleCore.ts': {
      'part.name': "a literal function name from the parts table in oracleInjectionSource (fuzzOracleCore.ts:276-279)",
      'part.source.slice(0, 80)':
        "the first 80 characters of a harness function's own source (Function.prototype.toString of code in this file): public tree code",
    },
    'e2e/agent/lib/fuzzTrace.ts': {
      'writeTrace: filePath': R.FOLDED,
      'writeTrace: index': R.COUNT,
      'corpusNameLeakAt(trace.steps)': 'the index of the first offending step, a number (the message prints it plus one)',
      'corpusNameLeakAt(parsed.steps)': 'the index of the first offending step, a number (the message prints it plus one)',
      'validateStep: filePath': R.FOLDED,
      'validateStep: index': R.COUNT,
      'readTrace: filePath': R.FOLDED,
      // The trace FILE (a shareable sink): see corpusTracePrivacy.test.ts for the corpus half.
      'JSON.stringify({ ...trace, failure }, null, 2)':
        'the trace: its header (scenario, seed, iterations, fixture, stream, gitRev, the recorded knobs), steps whose corpus files are ids (refused otherwise above), and the failure in its shareable form',
      // validateStep builds `where` as `trace ${foldLocalPaths(filePath)} step ${index + 1}` (fuzzTrace.ts:935), and
      // passes it, or that plus ` (<a FILE_ARG_ACTIONS name>)` (:949), to these four.
      'requireText: where':
        "validateStep's `where` (`trace <the folded trace path> step <n>`, fuzzTrace.ts:935), or that plus ` (<a FILE_ARG_ACTIONS action>)` via requireSessionRelative",
      'requireSessionRelative: where':
        "validateStep's `where`, or that plus ` (<action>)` for an action FILE_ARG_ACTIONS has (fuzzTrace.ts:935,949)",
      'requireCorpusId: where': "validateStep's `where`: `trace <the folded trace path> step <n>` (fuzzTrace.ts:935)",
      'requireCorpusHash: where': "validateStep's `where`: `trace <the folded trace path> step <n>` (fuzzTrace.ts:935)",
      // Every call passes a literal: 'testid', 'label', 'oracle', 'format', 'fixture', 'rel', 'args[0]', `rel[${at}]`, ...
      'requireText: field':
        "a literal field name at every call ('testid', 'label', 'oracle', 'format', 'fixture', ...), or the field requireSessionRelative/requireCorpusId/requireCorpusHash were given",
      'requireSessionRelative: field': "a literal field name at every call ('rel', 'args[0]'), or `rel[<index>]` (fuzzTrace.ts:987)",
      'requireCorpusId: field': "the literal field name 'corpusId' at its one call (fuzzTrace.ts:1019)",
      'requireCorpusHash: field': "the literal field name 'corpusHash' at its one call (fuzzTrace.ts:1020)",
    },
    'e2e/agent/lib/importFlowOracles.ts': {
      'fail: headline': R.FORWARD,
      'claim.shown': "a count: digest.receipt.shown, listItems' number of <li> in the receipt's warning list (importScreenDigest.ts)",
      // importDialogFuzz.ts, the only caller: const what = `flow ${flow + 1} of episode ${episode + 1}`.
      'checkResidue: what': "`flow <n> of episode <m>`, two counts (importDialogFuzz.ts, its only caller)",
      'checkTempFiles: what': "`flow <n> of episode <m>`, two counts (importDialogFuzz.ts, its only caller)",
      'checkPushed: what': "`flow <n> of episode <m>`, two counts (importDialogFuzz.ts, its only caller)",
      'checkRoundtripState: what': "`undoing flow <n> of episode <m>` or `redoing ...`, two counts (importDialogFuzz.ts)",
      'checkRoundtripState: label': "the literal 'pre-flow' or 'post-flow' at its two calls (importDialogFuzz.ts)",
    },
    'e2e/agent/lib/importScreenDigest.ts': {
      'source.slice(0, 80)':
        "the first 80 characters of the harness's own computeImportScreenDigest source (Function.prototype.toString): public tree code",
    },
    'e2e/agent/lib/junctionDepthOracle.ts': {
      "lines.join('\\n')":
        "checkJunctionDepths' message: line 1 is the literal `X6c the depths, tapes or plan changed beyond the junction rule (...)` " +
        '(junctionDepthOracle.ts:326-327), the counts and each failure are later lines, withheld, and the last is its one ' +
        "caller's `reproduce: pnpm agent:scenario <SCENARIO> --seed <ctx.seed> --iterations <n>` (exportRoundtripFuzz.ts:890, 1490)",
    },
    'e2e/agent/lib/mapControls.ts': {
      'request.featureType': "'shot' or 'station': validateRequest refuses any other before the click (mapControls.ts:44)",
      'request.sectionId': 'an integer: validateRequest refuses any other before the click (mapControls.ts:47)',
      'request.shotId': "an integer when the feature type is 'shot', the only branch that prints it (validateRequest, mapControls.ts:48)",
      'request.stationIndex':
        "an integer when the feature type is 'station', the only branch that prints it (validateRequest, mapControls.ts:51)",
    },
    'e2e/agent/lib/page.ts': {
      'invokeInApp: command': "the IPC command's name, printed only when it is one word (/^\\w+$/ on the same line), as a Tauri command is",
      'raw.__agentError': AFTER_APP_PREFIX,
      'evalInApp: err': `the eval body's compile error, ${AFTER_APP_PREFIX}`,
    },
    'e2e/agent/lib/oracles.ts': {
      'JSON.stringify(shareableFinding(finding), null, 2)':
        "the finding's shareable projection (shareableFinding, scripts/lib/shareable-finding.mjs) as JSON",
    },
    'e2e/agent/lib/procs.ts': {
      'recordPid: pid': "a process id, a number: the spawned driver's ChildProcess.pid (session.ts and sessionMac.ts, the only callers)",
      'findFreePort: base': 'a port number (session.ts parses STALAGMIGHT_AGENT_BASE_PORT with Number.parseInt, or adds 1000 to one)',
      'findFreePort: span': 'the number of ports to try, a number (10 by default)',
    },
    'e2e/agent/lib/recorder.ts': {
      'allocateSessionDir: root': R.FOLDED,
      'deps.pid': "a process id, the number AllocateDeps.pid holds (a test's), written to the session's owner marker",
      'process.pid': "this process's id, a number, written to the session's owner marker",
    },
    'e2e/agent/lib/replayHelpers.ts': {
      'driverScript: timeoutMs':
        "the replay's timeout in ms: 60000, or the number generate-replay.mjs emits from the recorded call (the MCP driver_script tool takes z.number(), mcp/server.ts)",
    },
    'e2e/agent/lib/scenarioRunArgs.ts': {
      'integerFlag: flag': "the literal '--seed' at its one call (scenarioRunArgs.ts:68)",
      'positiveIntFlag: flag': "the literal '--iterations' at its one call (scenarioRunArgs.ts:69)",
      'rest[i]': "the flag, printed (`flag <flag> needs a value`) only once it is '--seed' or '--iterations', checked on the line before",
    },
    'e2e/agent/lib/seedWindow.ts': {
      'envInt: name': 'SEED_FIRST_ENV or SEED_COUNT_ENV, the literal env names at its two calls (seedWindow.ts:29, 31)',
      'envInt: min': 'the literal 0 or 1 at its two calls (seedWindow.ts:29, 31)',
    },
    'e2e/agent/lib/sessionCore.ts': {
      'process.versions.node': "the running Node's version string (digits and dots), a fact about the toolchain, not the app or the user",
      'this.state': "a SessionState word ('idle', 'building', 'ready', 'busy', 'crashed', ...; e2e/agent/sessionTypes.ts)",
    },
    'e2e/agent/lib/sessionShared.ts': {
      'attachBrowser: driverName': "'tauri-driver' (the default, session.ts) or the literal 'tauri-wd' (sessionMac.ts), its two callers",
      'attachBrowser: port': 'the WebDriver port, a number findFreePort returned',
      'process.versions.node': "the running Node's version string (digits and dots), a fact about the toolchain, not the app or the user",
    },
    'e2e/agent/lib/stores.ts': {
      '(result as { __agentError: string }).__agentError':
        "`store <STORE_KEY[store]> unavailable`: the harness's own page script sets __agentError only there (stores.ts:45), the key from STORE_KEY",
    },
    'e2e/agent/lib/undoFuzzHarness.ts': {
      'step.kind':
        "a trace step's kind, one of fuzzTrace's STEP_KINDS (readTrace refuses any other); at historyReach one of the four history kinds (assertHistoryStep runs first)",
      'assertHistoryStep: scenario': 'a registry name: undoRedoFuzz.name or undoPersistenceFuzz.name at its two calls',
      'entry.id':
        'a KnownAsymmetry id from SAVE_HEALING_MUTATIONS or RELOAD_RESOLVED_GEOMETRY (projectSnapshot.ts): classifyDiffs returns only entries of the list it is given',
    },
  },
  fileSinks: {
    'e2e/agent/lib/build.ts': {
      'writeBuildStamp -> stampPath': {
        class: 'private',
        what: 'the E2E build stamp',
        reason: 'JSON beside the binary under CARGO_TARGET_DIR, read back by checkBuildFreshness; it records the absolute checkout path (repoRoot)',
      },
    },
    'e2e/agent/lib/fixtureCorpus.ts': {
      'materializeCorpusEntry -> path.join(destDir, ...segments)': {
        class: 'private',
        what: 'the corpus copies',
        reason: "a private corpus entry's files, copied into the session for the app to open",
      },
    },
    'e2e/agent/lib/fixtures.ts': {
      'materializeFixture -> destination': {
        class: 'private',
        what: 'the fixture copies',
        reason: 'a tracked fixture copied into the session, which the app then opens, migrates and saves into',
      },
    },
    'e2e/agent/lib/fuzzTrace.ts': {
      'writeTrace -> filePath': {
        class: 'shareable',
        what: 'a fuzz trace',
        reason: 'attached to issues (docs/agent-harness.md); its failure is the shareable form, its corpus steps ids',
      },
    },
    'e2e/agent/lib/oracles.ts': {
      "captureFinding -> path.join(dir, 'finding.json')": {
        class: 'shareable',
        what: 'findings/<n>/finding.json',
        reason: "the finding's shareable projection (shareableFinding), which docs/agent-harness.md says is attached",
      },
      "captureFinding -> path.join(dir, 'screenshot.png')": {
        class: 'private',
        what: '`findings/<n>/screenshot.png`',
        reason: "the app's window as it was when the finding fired",
      },
      'captureFinding -> path.join(dir, path.basename(marker))': {
        class: 'private',
        what: 'the crash-marker copies',
        reason: "the app's crash marker (its panic message), corpus-folded",
      },
    },
    'e2e/agent/lib/procs.ts': {
      'recordPid -> pidFile': {
        class: 'shareable',
        what: "the session's pids file",
        reason: 'one process id per line, read back by reapPidsFromFile',
      },
    },
    'e2e/agent/lib/recorder.ts': {
      'allocateSessionDir -> path.join(dir, OWNER_FILE)': {
        class: 'shareable',
        what: "the session's owner marker",
        reason: 'the pid that claimed the session directory, read back by reapPreviousSessions',
      },
      'append -> this.jsonlPath': {
        class: 'private',
        what: "session's `actions.jsonl`",
        reason: "every tool call with its params and result, every finding and failure in full; corpus-folded only",
      },
      'writeScreenshot -> path.join(this.screenshotsDir, file)': {
        class: 'private',
        what: '`screenshots/`',
        reason: "the app's window",
      },
      'writeState -> path.join(this.stateDir, file)': {
        class: 'private',
        what: 'the `state/` snapshots',
        reason: "the app's store state, corpus-folded only",
      },
    },
    'e2e/agent/lib/surveyFileSpecs.ts': {
      'materializeSurveyFile -> target': {
        class: 'private',
        what: 'the generated survey files',
        reason: 'synthetic survey files a scenario feeds the importers; a spec can come back from a trace',
      },
    },
  },
  passthrough: {},
  wrappers: {
    // `buildApp`'s and `writeBuildStamp`'s log callback: session.ts and sessionMac.ts hand it a
    // folding process.stderr.write.
    'e2e/agent/lib/build.ts': { onLog: 'stream' },
    // `passLinesFolded`'s callback: build.ts and the MCP smoke hand it process.stderr.write.
    'e2e/agent/lib/foldedLines.ts': { write: 'stream' },
    'e2e/agent/lib/fuzzMatrixArgs.ts': { bad: 'error' },
    // The headline is the error's first line; the detail goes on later lines, which the runners withhold.
    'e2e/agent/lib/importFlowOracles.ts': { fail: { kind: 'error', args: [0] } },
    // `progressLogger`'s print: the runner's stdout.
    'e2e/agent/lib/shareableFailure.ts': { print: 'stream' },
  },
  builders: {},
  fileHelpers: {
    'e2e/agent/lib/fuzzMatrixArgs.ts': {
      typed: 'a value the user typed, as its length (fuzzMatrixArgs.ts typed)',
    },
  },
};
