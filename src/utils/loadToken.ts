/**
 * [#576 W44-NF3] The name of one load, create or file import the frontend starts.
 *
 * The frontend hands it to the command that starts the load (`loadToken`), and
 * every Cancel made for that load names it (`cancel_loading`): the backend cancels
 * the load in progress only when it carries that name, so a Cancel that reaches the
 * backend after its load ended cannot cancel a newer one.
 *
 * Random, not a counter: the backend outlives a webview reload, and a counter that
 * restarts with the page could give a new load the name of one started before the
 * reload that is still running, which the new page's Cancel would then stop.
 * 128 random bits from `getRandomValues`, which (unlike `randomUUID`) needs no
 * secure context.
 */
export function newLoadToken(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  return Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
}
