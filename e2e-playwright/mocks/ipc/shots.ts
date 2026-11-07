/**
 * The one shot builder of the Playwright IPC mock (#576 NF-FIX3B-2).
 *
 * `create_section`, `import_sections` and `create_shot` each used to spell out the shot
 * record inline, and the copies drifted: `create_shot` (#576 NF-W3UI-1) and then
 * `import_sections` built shots without a `uuid`, while the real backend mints one for
 * every shot it creates and uuid anchors (plan pins, the distance origin) resolve through
 * it. One builder means one place to keep in step with `Shot`.
 *
 * Runs IN THE PAGE: `setupTauriMocks` serializes it with `Function.prototype.toString`
 * into its init script and hands it to the mock core, the same way it ships the command
 * families - so it may use only its arguments, never an import or a module binding.
 */
export const buildMockShot = (id: number, data: Record<string, unknown>) => ({
  id,
  // A deterministic, stable uuid per shot id, as the backend keys anchors off `Shot.uuid`.
  uuid: `mock-shot-uuid-${id}`,
  azimuth_in: (data.azimuth_in as number) ?? 0,
  azimuth_out: (data.azimuth_out as number) ?? 0,
  length: (data.length as number) ?? 0,
  depth_in: (data.depth_in as number) ?? 0,
  depth_out: (data.depth_out as number) ?? 0,
  pitch_in: (data.pitch_in as number | null | undefined) ?? null,
  pitch_out: (data.pitch_out as number | null | undefined) ?? null,
  left: (data.left as number | null | undefined) ?? null,
  right: (data.right as number | null | undefined) ?? null,
  up: (data.up as number | null | undefined) ?? null,
  down: (data.down as number | null | undefined) ?? null,
  comment: (data.comment as string | null | undefined) ?? null,
  kind: (data.kind as string | undefined) ?? 'Normal',
  marker_index: (data.marker_index as number | null | undefined) ?? null,
  temperature: (data.temperature as number | null | undefined) ?? null,
});
