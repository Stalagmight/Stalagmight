/**
 * stdout hygiene for the MCP stdio transport. MUST be imported before
 * webdriverio (or anything that might log to stdout): the MCP protocol owns
 * stdout, so any stray console.log there corrupts JSON-RPC framing. We silence
 * the wdio logger and route console.log/info/warn/debug to stderr.
 */
process.env.WDIO_LOG_LEVEL = process.env.WDIO_LOG_LEVEL ?? 'silent';

const methods = ['log', 'info', 'warn', 'debug'] as const;
for (const method of methods) {
  console[method] = (...args: unknown[]) => {
    process.stderr.write(`${args.map((a) => (typeof a === 'string' ? a : JSON.stringify(a))).join(' ')}\n`);
  };
}
