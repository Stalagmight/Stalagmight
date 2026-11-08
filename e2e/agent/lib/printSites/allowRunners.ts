/**
 * #576 W51: the print and write sites of the runners and entry points at the top of e2e/agent, the MCP server and client, and the OS-input layer, checked by
 * `printSites.test.ts` against `rules.ts`. Every entry says why its value may
 * reach its sink; see `rules.ts` for the classes and the reasons.
 *
 * Two reasons recur here:
 * - A runner prints `session ${started.state} (... ${started.buildAction})`:
 *   `state` is a `SessionState` and `buildMode` / `buildAction` are the
 *   literal unions of `StartResult` (sessionTypes.ts), set from literals and
 *   `agentBuildMode()` in session.ts / sessionMac.ts `startInner`.
 * - The MCP server's tool handlers (and the os/ layer, which only they call)
 *   throw errors that `run()` / `record()` in mcp/server.ts turn into the tool
 *   result the MCP client reads (`asMessage` -> `errorResult`) and into the
 *   session's actions.jsonl. No runner prints them: that channel is the
 *   agent's working view, not a shareable sink (see the W51 report).
 */
import { R, type SiteGroup } from './rules';

const STARTED_STATE = "the session's SessionState (sessionTypes.ts), set from literals in startInner";
const STARTED_BUILD = "StartResult's buildMode/buildAction literal union, from agentBuildMode() and literals in startInner";
const MCP_ONLY =
  'thrown inside an MCP tool handler: run()/record() in mcp/server.ts return it to the MCP client and actions.jsonl, no runner prints it';
const OS_MCP_ONLY =
  'an os/ error: os/ is imported only by mcp/server.ts, whose os_* tool handlers return it to the MCP client and actions.jsonl via run()/record()';
const PROGRESS_PRINT = "the line progressLogger hands its print callback: already shareableProgressLine's form (lib/shareableFailure.ts)";
const METRICS_JSON =
  "a scenario's metrics: keys each checked where ctx.metric/ctx.setMetric makes it (printSites scan.ts METRIC), values numbers";

export const GROUP: SiteGroup = {
  covers: ['e2e/agent/*', 'e2e/agent/mcp/', 'e2e/agent/os/'],
  programs: {
    'e2e/agent/os/helper/src/cli.rs': {
      class: 'private',
      reason: "the Tier 2 OS helper's argument parsing: its only output is the one JSON line runHelper captures for the MCP client",
    },
    'e2e/agent/os/helper/src/input.rs': {
      class: 'private',
      reason: 'the Tier 2 OS helper posting CGEvents: prints nothing; its result is the JSON line runHelper captures for the MCP client',
    },
    'e2e/agent/os/helper/src/json.rs': {
      class: 'private',
      reason: "the Tier 2 OS helper's JSON encoder: writes into a String that main.rs prints as the line runHelper captures",
    },
    'e2e/agent/os/helper/src/keyspec.rs': {
      class: 'private',
      reason: "the Tier 2 OS helper's key-spec parser: prints nothing; its errors ride the JSON line runHelper captures",
    },
    'e2e/agent/os/helper/src/main.rs': {
      class: 'private',
      reason: "the Tier 2 OS helper's entry: println! of one JSON line (or --help) to stdout, which runHelper captures (execFile) and never passes on",
    },
    'e2e/agent/os/helper/src/perms.rs': {
      class: 'private',
      reason: "the Tier 2 OS helper's TCC preflight: prints nothing; its result is the JSON line runHelper captures for the MCP client",
    },
    'e2e/agent/os/helper/src/window.rs': {
      class: 'private',
      reason: "the Tier 2 OS helper's window lookup: prints nothing; its result is the JSON line runHelper captures for the MCP client",
    },
  },
  allow: {
    'e2e/agent/createSession.ts': {
      'createSession: process.platform': "Node's process.platform, one of Node's fixed platform names (the override prints by its shape)",
    },
    'e2e/agent/fuzz-matrix.ts': {
      // The cell counter and the cell's coordinates.
      'runOne: label': 'the cell counter `[i/n]` main builds from two counts and passes to runOne/printDone',
      'printDone: label': 'the cell counter `[i/n]` main builds from two counts and passes to runOne/printDone',
      'runOne: seed': 'a seed: Options.seeds, integers parseSeeds checked against /^-?\\d+$/ (lib/fuzzMatrixArgs.ts)',
      'printDone: result.seed': 'a seed: Options.seeds, integers parseSeeds checked against /^-?\\d+$/ (lib/fuzzMatrixArgs.ts)',
      'printReport: failure.seed': 'a seed: Options.seeds, integers parseSeeds checked against /^-?\\d+$/ (lib/fuzzMatrixArgs.ts)',
      'runOne: slot': R.COUNT,
      'runOne: BASE_PORT': 'the constant base port 4544 (lib/fuzzMatrixArgs.ts BASE_PORT)',
      'axisLabel: axis.key': 'an --axis key, which parseAxis only accepts as ^STALAGMIGHT_[A-Z0-9_]{1,64}$ outside RESERVED_AXIS_KEYS',
      'axisLabel: axis.value': 'an --axis value from the CLI, folded (a knob can take a path); see the W51 open question on knob values',
      'printDone: secs(result.durationMs)': 'secs(): a duration the runner measured, as seconds with toFixed(1)',
      'printDone: result.headline':
        "runOne's headline: `spawn failed:` + the spawn error through foldLocalPaths (runOne, where the headline is made), a timeout count, failHeadline (the child's own FAIL headline, printed by scenarioFailOutput), or exit code and signal name",
      "line: cells.map((cell, i) => (i === cells.length - 1 ? cell : cell.padEnd(widths[i]!))).join(' ')":
        "printReport's table: literal headers and the validated axis key; rows of the fixture and the axis value, each through foldLocalPaths where the row is made, String(seed), a verdict word, secs() and runOne's headline (see result.headline)",
      'printReport: failure.reproduce': R.FOLDED,
      'printReport: opts.scenario': 'the --scenario name, which parseArgs only accepts as /^[a-z][a-z0-9-]{1,63}$/ (lib/fuzzMatrixArgs.ts)',
      'main: opts.scenario': 'the --scenario name, which parseArgs only accepts as /^[a-z][a-z0-9-]{1,63}$/ (lib/fuzzMatrixArgs.ts)',
      "printReport: failure.detail.split('\\n').map((l) => ` | ${l}`).join('\\n')":
        "failureDetail's output (HELPERS), stored by runOne, each line prefixed with `    | `",
      "printReport: maxed.join(', ')": 'metric names filtered by MAX_MERGED_METRICS.has: names from the runner\'s own fixed set',
      'printReport: key':
        "a key parseMetrics read off the JSON of a cell's `PASS:` line, which only scenario-run.ts prints (JSON.stringify(metrics.metrics)): " +
        METRICS_JSON,
      'printReport: merged[key]': 'a merged metric: a sum or a max of the finite numbers parseMetrics kept',
      "printReport: matrixSummary(results, wallMs).join('\\n')": "matrixSummary's lines (HELPERS): counts and durations, joined",
      'main: note': 'an Options note: `note: --workers N clamped to M`, two integers (lib/fuzzMatrixArgs.ts parseArgs)',
      "main: dropped.join(', ')": "names from INHERITED_ENV_BLOCKLIST (lib/fuzzMatrixArgs.ts) that are set: the runner's own env-name table",
      'main: opts.axis.find((entry) => entry !== null)?.key':
        'an --axis key, which parseAxis only accepts as ^STALAGMIGHT_[A-Z0-9_]{1,64}$ outside RESERVED_AXIS_KEYS',
      'main: opts.iterations': R.KNOB,
      'main: agentBuildMode()': "agentBuildMode(): 'debug' or 'release' (e2e/lib/env.ts)",
      "main: freshness.reasons.join('; ')": R.FOLDED,
      'parseOrBail: err.message':
        "a MatrixArgError's message, printed by bail through shareableFailureText; its text is written at the bad() sites of lib/fuzzMatrixArgs.ts",
      "<module>: path.join(repoRoot, 'node_modules', '.bin', 'tsx')": R.FOLDED,
    },
    'e2e/agent/fuzz-shrink.ts': {
      "main: refused.map(envKey).join(', ')":
        'env keys through envKey: a knob name of the ^STALAGMIGHT_[A-Z0-9_]{1,64}$ shape, anything else (the trace is a file) by its shape',
      "main: cleared.map(envKey).join(', ')":
        'env keys through envKey: a knob name of the ^STALAGMIGHT_[A-Z0-9_]{1,64}$ shape, anything else by its shape',
      "main: withheld.map(envKey).join(', ')":
        "the trace's envWithheld keys through envKey: a knob name of the ^STALAGMIGHT_[A-Z0-9_]{1,64}$ shape, anything else (the trace is a file) by its shape (#576 W58 Q9)",
      'main: started.state': STARTED_STATE,
      'main: started.buildMode': STARTED_BUILD,
      'main: started.buildAction': STARTED_BUILD,
      'main: args.trace': R.FOLDED,
      'main: trace.scenario': 'a registry name: main throws unless getScenario(trace.scenario) found it, just above the print',
      "main: Object.entries(trace.env).map(([key, value]) => `${envKey(key)}=${shareableKnobValue(key, value)}`).join(' ')":
        "the trace's knobs as KEY=value: the key through envKey (knob shape, else valueShape), the value through shareableKnobValue (traceKnobs.ts: a registered knob's value of its shape, the fixture by shareableFixtureName, else valueShape; #576 W58 Q9)",
      'main: CORPUS_DIR_ENV': "CORPUS_DIR_ENV, the constant 'STALAGMIGHT_FUZZ_CORPUS_DIR' (lib/fixtureCorpus.ts)",
      'main: replayTimesMs[0]': R.COUNT,
      'onProgress: info.stats.tests': R.COUNT,
      'onProgress: info.candidateSize': R.COUNT,
      'onProgress: info.bestSize': R.COUNT,
      'onProgress: info.verdict': "ddmin's ShrinkVerdict: 'preserved' | 'passed' | 'divergent' (lib/fuzzShrink.ts)",
      'main: result.stats.tests': R.COUNT,
      'main: result.stats.cacheHits': R.COUNT,
      'main: result.stats.preserved': R.COUNT,
      'main: result.stats.passed': R.COUNT,
      'main: result.stats.divergent': R.COUNT,
      'main: args.out': R.FOLDED,
      "main: args.trace.replace(/\\.json$/, '')": R.FOLDED,
      'main: describeSteps(minimal)':
        "the minimal trace's steps (lib/fuzzTrace.ts describeSteps): one describeStep line per step, a builder whose every value the guard checks (allowLib.ts): step kinds, counts, each label through shareableStepLabel (#576 W58 Q10), a test id through shareableTestId, a path folded, a trigger, export format or oracle name only when lib/traceStepFields.ts lists it, a corpus id in its 5-hex form, a reload's fixture through shareableFixtureName (#576 W58 NF1, NF2); folded",
    },
    'e2e/agent/ipc-payload-probe.ts': {
      'main: started.state': STARTED_STATE,
      'main: started.buildMode': STARTED_BUILD,
      'main: started.buildAction': STARTED_BUILD,
      'main: probeCase.label': "a label of the probe's own literal `cases` table",
      'main: suspect.label': "a label of the probe's own literal SUSPECTS table",
      'main: max': 'the --max length: parseArgs refuses anything but a positive integer',
      'main: good': R.COUNT,
      'main: bad': R.COUNT,
    },
    'e2e/agent/journeys-run.ts': {
      'writeLedger: path.join(runsDir, `${stamp}-${record.gitSha}.jsonl`)': R.FOLDED,
      "main: listJourneys().join(', ')": 'journey names from the registry (journeys/index.ts listJourneys)',
      'main: started.state': STARTED_STATE,
      'main: started.buildAction': STARTED_BUILD,
      'main: name': 'a registry name: main exits unless getJourney(name) found every name it runs',
      'main > progressLogger: line': PROGRESS_PRINT,
      'writeLedger: record.verdict': "the run verdict, 'pass' or 'fail' from a literal conditional in main",
      'main: record.verdict': "the run verdict, 'pass' or 'fail' from a literal conditional in main",
      // The committed UAT ledger (docs/uat/).
      'writeLedger: JSON.stringify(ledgerRecord(record))': "ledgerRecord's projection (HELPERS) of the run record, serialised",
      'writeLedger: record.date.slice(0, 10)': "the date part of the record's new Date().toISOString(), made in main",
      'writeLedger: record.gitSha': "git rev-parse --short HEAD of the checkout (DEV_TOOL output, a hex sha) or the literal 'unknown'",
      'writeLedger: record.platform': "`${process.platform}-${process.arch}`: Node's own platform and architecture names",
      "writeLedger: failed.join(', ')": "journey names (runJourney's journey.name, from the registry) of the failed journeys",
      'writeLedger: path.basename(runFile)': 'the run file name main makes: a timestamp and the git sha, `.jsonl`',
    },
    'e2e/agent/mcp/bridge.ts': {
      'main > passLinesFolded: text': "a line of the MCP server's stderr as passLinesFolded hands it: the checkout and home folded (as agent:mcp:smoke prints it)",
      'main: process.versions.node': "Node's own version string (process.versions.node)",
      "main: path.join(bridgeDir, 'requests.jsonl')": R.FOLDED,
      'main: offset': 'a byte count: statSync(reqPath).size',
      'main: process.pid': "the bridge's own process id, an integer",
    },
    'e2e/agent/mcp/hygiene.ts': {
      "<module>: args.map((a) => (typeof a === 'string' ? a : JSON.stringify(a))).join(' ')":
        "the console.log/info/warn/debug override's arguments, moved to stderr: each such call in the server is a stream site of its own, checked there",
    },
    'e2e/agent/mcp/server.ts': {
      '<module> > define > run > session.exclusive: args.condition': 'wait_for\'s condition: a zod z.enum of the tool schema, and ' + MCP_ONLY,
      '<module> > define > run: args.timeoutMs': "driver_script's timeoutMs: a zod z.number of the tool schema, and " + MCP_ONLY,
    },
    'e2e/agent/mcp/smoke-client.ts': {
      'main > passLinesFolded: text': "a line of the MCP server's stderr as passLinesFolded hands it: the checkout and home folded (docs: RV19b-P8)",
      'check: label': R.FORWARD,
    },
    'e2e/agent/os/helper.ts': {
      'write: text': 'a line of cargo build output as passLinesFolded hands it: the checkout and home folded (as lib/build.ts passes the app build)',
      "runHelper: args.join(' ')": OS_MCP_ONLY,
      "runHelper: [failure?.message, stderr.trim(), trimmed && `stdout: ${trimmed}`] .filter(Boolean) .join(' | ')": OS_MCP_ONLY,
      'writeHelperStamp: JSON.stringify(stamp, null, 2)': 'the helper build stamp: schema version 1, two mtimes in ms and an ISO timestamp',
    },
    'e2e/agent/os/input.ts': {
      'activateApp > new Promise > execFile: pid': 'swallowed by ensureAppFrontmost (its catch), and ' + OS_MCP_ONLY,
      'activateApp > new Promise > execFile: errOut': 'swallowed by ensureAppFrontmost (its catch), and ' + OS_MCP_ONLY,
      'activateApp > new Promise > execFile: error.message': 'swallowed by ensureAppFrontmost (its catch), and ' + OS_MCP_ONLY,
      "requireOsAccess: permissionHelp('accessibility')": "permissionHelp's literal instructions (os/permissions.ts), and " + OS_MCP_ONLY,
      "requireOsAccess: permissionHelp('screenRecording')": "permissionHelp's literal instructions (os/permissions.ts), and " + OS_MCP_ONLY,
      'requireFrontmostForInput: what': "the literal 'click' or `keys <spec>`, and " + OS_MCP_ONLY,
      'requireAppPid: session.state': 'a SessionState, and ' + OS_MCP_ONLY,
      'requireAppPid: session.appPid?.()': 'a pid (an integer from the session pidfile), and ' + OS_MCP_ONLY,
      'windowForPid: pid': 'a pid requireAppPid checked as a positive integer, and ' + OS_MCP_ONLY,
      'osKeys: spec': "the agent's key spec (the os_keys tool's argument), and " + OS_MCP_ONLY,
    },
    'e2e/agent/os/screenshot.ts': {
      'screencaptureWindow > new Promise > execFile: windowId': "the helper's window id, a number, and " + OS_MCP_ONLY,
      'osScreenshot: outPath': 'the PNG path os_screenshot builds under the session dir, and ' + OS_MCP_ONLY,
    },
    'e2e/agent/scenario-run.ts': {
      'main: path.resolve(args.trace)': R.FOLDED,
      'main: args.trace': R.FOLDED,
      'main: runAt.seed': 'scenarioRunOptions: --seed through integerFlag, or the trace seed it refuses unless Number.isInteger',
      'main: runAt.iterations':
        'scenarioRunOptions: --iterations through positiveIntFlag, or the trace count it refuses unless a positive integer',
      "main: restored.map(([key, value]) => `${envKey(key)}=${shareableKnobValue(key, value)}`).join(' ')":
        "the knobs restored from the trace as KEY=value: the key through envKey (knob shape, else valueShape), the value through shareableKnobValue (traceKnobs.ts: a registered knob's value of its shape, the fixture by shareableFixtureName, else valueShape; #576 W58 Q9)",
      "main: cleared.map(envKey).join(', ')":
        'env keys through envKey: a knob name of the ^STALAGMIGHT_[A-Z0-9_]{1,64}$ shape, anything else by its shape',
      "main: withheld.map(envKey).join(', ')":
        "the trace's envWithheld keys through envKey: a knob name of the ^STALAGMIGHT_[A-Z0-9_]{1,64}$ shape, anything else (the trace is a file) by its shape (#576 W58 Q9)",
      "main: refused.map(envKey).join(', ')":
        'env keys through envKey: a knob name of the ^STALAGMIGHT_[A-Z0-9_]{1,64}$ shape, anything else (the trace is a file) by its shape',
      "main: listScenarios().join(', ')": 'scenario names from the registry (scenarios/index.ts listScenarios)',
      'main: started.state': STARTED_STATE,
      'main: started.buildMode': STARTED_BUILD,
      'main: started.buildAction': STARTED_BUILD,
      'main > progressLogger: line': PROGRESS_PRINT,
      'main: args.name': 'a registry name: main throws unless getScenario(args.name) found it, before the session starts',
      'main: metrics.seed': "runScenario's seed: the integer it ran at (opts.seed ?? 1, from scenarioRunOptions)",
      'main: metrics.iterations': "runScenario's iteration count: scenarioRunOptions' integer or the scenario's defaultIterations",
      'main: metrics.durationMs': R.COUNT,
      'main: JSON.stringify(metrics.metrics)': METRICS_JSON,
      // STALAGMIGHT_FUZZ_DETERMINISM=2sessions (main's two-session block).
      "main: [...DETERMINISM_SCENARIOS].join(', ')":
        'registry names: DETERMINISM_SCENARIOS is the .name of three registered scenarios (scenarios/index.ts:52-56)',
      'print: line': `${PROGRESS_PRINT}: print is handed only to progressLogger (scenario-run.ts:164, 188)`,
      'main > runDeterminismPair: replay':
        "'A' or 'B': runDeterminismPair calls runReplay only with its literal ['A', 'B'] (lib/fuzzDeterminismRun.ts:66, 72)",
      'main: pairFailOutput(args.name, err, replayLogs)':
        "pairFailOutput's FAIL block (lib/shareableFailure.ts): `FAIL: <args.name> threw:`, a registry name getScenario " +
        'found (:130-134), then shareableFailureText (HELPERS) of the message, as scenarioFailOutput prints one ' +
        '(pinned in shareableRunnerOutput.test.ts)',
    },
    'e2e/agent/scenarios-smoke.ts': {
      'main: started.state': STARTED_STATE,
      'main: started.buildAction': STARTED_BUILD,
      'main: name': 'a registry name: the loop runs over listScenarios()',
      'main > progressLogger: m': PROGRESS_PRINT,
      'main: metrics.durationMs': R.COUNT,
      'main: JSON.stringify(metrics.metrics)': METRICS_JSON,
    },
    'e2e/agent/session-http-probe.ts': {
      "main: dropped.join(', ')": "names from dropProxyEnv's literal list of proxy variables that were set",
      'main: process.versions.node': "Node's own version string (process.versions.node)",
      'main: process.versions.undici': "Node's bundled undici version string (process.versions.undici)",
      'main: browser.sessionId': "the session id webdriverio parsed from this probe's own loopback stub's reply (STUB_SESSION_ID)",
      // seen.push(`${req.method} ${req.url}`): each request webdriverio made to the stub.
      'main > http.createServer > req.on: req.method': "the HTTP method of a request webdriverio made to this probe's own loopback stub",
      'main > http.createServer > req.on: req.url': "the path of a request webdriverio made to this probe's own loopback stub (its WebDriver endpoints, /session...)",
    },
    'e2e/agent/session.ts': {
      "startInner: freshness.reasons.join('; ')": R.FOLDED,
      'startInner > buildApp: line': R.FOLDED,
      "startInner: pool.join(', ')": 'Xvfb displays parseDisplayPool accepted, each /^:\\d+$/ (lib/displayPool.ts)',
      'startInner: this.driverPort': 'a TCP port findFreePort picked, a number',
    },
    'e2e/agent/sessionMac.ts': {
      "startInner: freshness.reasons.join('; ')": R.FOLDED,
      'startInner > buildApp: line': R.FOLDED,
      'startInner: this.driverPort': 'a TCP port findFreePort picked, a number',
    },
    'e2e/agent/smoke.ts': {
      'log: msg': R.FORWARD,
      'check: name': R.FORWARD,
      'check: detail': R.FORWARD,
      "main: session.recorder.writeScreenshot(session.recorder.nextSeq(), 'smoke', shot)":
        "Recorder.writeScreenshot's session-relative path, screenshots/<seq>-smoke.png (lib/recorder.ts)",
      'main: before.latestSeq': "the log ring's latest sequence number (get_recent_logs_since's u64 latest_seq, lib/logs.ts)",
      'main: session.exclusive(() => session.browser.execute(() => document.readyState), )':
        "the page's document.readyState: one of the DOM's loading/interactive/complete",
      'main: parsed': R.COUNT,
      'main: failures': R.COUNT,
    },
  },
  fileSinks: {
    'e2e/agent/journeys-run.ts': {
      'writeLedger -> runFile': {
        class: 'shareable',
        what: 'the UAT run record committed under docs/uat/runs/',
        reason: "ledgerRecord's projection of the run (journeys/ledger.ts, #576 W39-NF1); the verbatim record is uat/run.jsonl",
      },
      'writeLedger -> ledgerFile': {
        class: 'shareable',
        what: 'the UAT ledger line appended to docs/uat/ledger.md',
        reason: 'a date, the git sha, the platform, the verdict, a count, failed journey names and the run file name',
      },
      "main -> path.join(artifactsDir, 'run.jsonl')": {
        class: 'private',
        what: 'uat/run.jsonl',
        reason: "the verbatim run record (each check's detail, each abort reason) in the session directory",
      },
    },
    'e2e/agent/mcp/bridge.ts': {
      'main -> reqPath': {
        class: 'private',
        what: "the MCP bridge's `requests.jsonl` and `responses.jsonl`",
        reason: 'creates the requests file (appends nothing); the requests in it are the client-written tool calls',
      },
      'respond -> resPath': {
        class: 'private',
        what: "the MCP bridge's `requests.jsonl` and `responses.jsonl`",
        reason: "each MCP tool result in full: store values, eval results, the app's error text",
      },
      "main -> path.join(bridgeDir, 'ready')": {
        class: 'shareable',
        what: "the bridge's ready marker",
        reason: 'holds only String(process.pid)',
      },
    },
    'e2e/agent/os/helper.ts': {
      'writeHelperStamp -> stampPath': {
        class: 'shareable',
        what: 'the helper build stamp beside the binary in the cargo target dir',
        reason: 'numbers and an ISO timestamp only (HelperBuildStamp)',
      },
    },
    'e2e/agent/session.ts': {
      "startInner -> path.join(this.recorder.dir, 'tauri-driver.log')": {
        class: 'private',
        what: 'tauri-driver.log',
        reason: "tauri-driver's and WebKitWebDriver's raw output in the session directory",
      },
      'startInner > this.driver.stdout.on -> <stream driverLog>': {
        class: 'private',
        what: 'tauri-driver.log',
        reason: "the driver's stdout and stderr chunks, written to its log as they arrive",
      },
      'startInner > this.driver.stderr.on -> <stream driverLog>': {
        class: 'private',
        what: 'tauri-driver.log',
        reason: "the driver's stdout and stderr chunks, written to its log as they arrive",
      },
      'stop -> <stream this.driverLog>': {
        class: 'private',
        what: 'tauri-driver.log',
        reason: "closes the driver's log (`end()` with nothing more to write)",
      },
    },
    'e2e/agent/sessionMac.ts': {
      "startInner -> path.join(this.recorder.dir, 'tauri-wd.log')": {
        class: 'private',
        what: 'tauri-wd.log',
        reason: "tauri-wd's raw output and its spawn error in the session directory (macOS)",
      },
      'startInner > this.driver.stdout.on -> <stream driverLog>': {
        class: 'private',
        what: 'tauri-wd.log',
        reason: "tauri-wd's stdout and stderr chunks and its spawn error, written to its log as they arrive",
      },
      'startInner > this.driver.stderr.on -> <stream driverLog>': {
        class: 'private',
        what: 'tauri-wd.log',
        reason: "tauri-wd's stdout and stderr chunks and its spawn error, written to its log as they arrive",
      },
      'startInner > this.driver.on -> <stream driverLog>': {
        class: 'private',
        what: 'tauri-wd.log',
        reason: "tauri-wd's stdout and stderr chunks and its spawn error, written to its log as they arrive",
      },
      'stop -> <stream this.driverLog>': {
        class: 'private',
        what: 'tauri-wd.log',
        reason: "closes tauri-wd's log (`end()` with nothing more to write)",
      },
    },
  },
  passthrough: {},
  wrappers: {
    'e2e/agent/fuzz-matrix.ts': { bail: 'error' },
    'e2e/agent/mcp/smoke-client.ts': { check: { kind: 'stream', args: [0] } },
    'e2e/agent/smoke.ts': { log: 'stream', check: { kind: 'stream', args: [0, 2] } },
  },
  builders: {
    'e2e/agent/fuzz-matrix.ts': ['axisLabel'],
  },
  fileHelpers: {
    'e2e/agent/fuzz-matrix.ts': {
      axisLabel: 'an --axis cell as `KEY=value` through foldLocalPaths (a builder here, so its return is checked too)',
    },
    // Validators the scan cannot see through: each prints its argument only when
    // it passes the check in the same expression, and valueShape (HELPERS) otherwise.
    'e2e/agent/fuzz-shrink.ts': {
      shownFlag: "a CLI token: itself only when it is one of the runner's literal FLAGS, else cliValueShape",
      gitRev: 'a trace revision: its first 12 characters only when /^(?:[0-9a-f]{7,40}|unknown)$/ holds, else valueShape',
    },
    'e2e/agent/ipc-payload-probe.ts': {
      shownFlag: "a CLI token: itself only when it is one of the probe's literal FLAGS, else cliValueShape",
      shownChar: "--char: the quoted default 'x', a label of the literal SUSPECTS table when the text is one, else valueShape",
      // #576 RV20-13: the probe reads and prints a call's error through lib/ipcPayloadProbe.ts (ipcPayloadProbe.test.ts).
      probeVerdict: "'arrived', 'truncated' or 'other': a ProbeVerdict word, from the class of the error",
      shownProbeError:
        "a call's error as a label of the literal PROBE_ERROR_CLASSES (or `not a known rejection`) and its length, never its text",
    },
    'e2e/agent/scenario-run.ts': {
      scenarioName:
        "a scenario name from the CLI or a trace: itself only when it has the registry's shape /^[a-z][a-z0-9-]{1,63}$/ (as fuzz-matrix validates --scenario), else valueShape",
      gitRev: 'a trace revision: its first 12 characters only when /^(?:[0-9a-f]{7,40}|unknown)$/ holds, else valueShape',
    },
    'e2e/agent/smoke.ts': {
      smokeStartedLine:
        'the StartResult (lib/smokeStartLine.ts): state/build words, ports, a commit hash, the display as `:N`, `aqua` or `<display>` (#576 W58 Q14), and its paths folded',
    },
    'e2e/agent/mcp/smoke-client.ts': {
      shownDetail:
        "a check's detail: a Shown made by own/shareable/finding/toolError (a count, tool names, shareableFinding, a crash headline), else its length (#576 W41-NF1, mcpSmokeOutput.test.ts)",
    },
  },
};
