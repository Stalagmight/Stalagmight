/**
 * GLSL ES 3.00 shader sources + shared attribute/uniform/flag contract for the
 * custom-render LABEL pass (#customRender labels, Phase 0).
 *
 * Sibling of `shaders.ts` (lines + circles); same target (WebGL2 / GLSL ES 3.00,
 * premultiplied-alpha output for MapLibre's `blendFunc(ONE, ONE_MINUS_SRC_ALPHA)`)
 * and the same anchor-relative-mercator + screen-px-offset primitive: one anchor
 * per label is projected by `u_matrix` (composed via `composeAnchorMatrix` from
 * `defaultProjectionData.mainMatrix`), then each glyph corner is displaced by a
 * per-vertex screen-space offset - exactly the `CIRCLE_VERTEX_SRC`
 * `offsetPx / u_viewport * 2.0 * base.w` idiom - so a whole word tracks the map
 * during pan for free (matrix reprojection), only the glyph quad varying.
 *
 * WHAT IS NEW vs the geometry shaders:
 *  - The FIRST texture sampler in this renderer: an R8 SDF glyph atlas
 *    (`glyphAtlas.ts`). `a_uv` carries ATLAS PIXEL coords (not normalized) so a
 *    grow (atlas height doubling) needs only the `u_atlas_size` uniform updated,
 *    never a per-glyph re-layout; the vertex shader normalizes.
 *  - Integer attributes `a_kind` / `a_flags` (bound with `vertexAttribIPointer`,
 *    UNSIGNED_BYTE) so the fragment can index per-kind uniform arrays and the
 *    vertex can test flag bits - the design's `(a_flags & HIDE) != 0` sketch.
 *  - Per-kind uniform arrays (one slot per {@link LABEL_KIND}) so zoom
 *    size / opacity / visibility / halo are CPU-recomputed each frame WITHOUT any
 *    buffer rebuild (§3 of the design: the load-bearing "no rebuild for zoom /
 *    toggle / theme" property).
 *  - A per-label float `a_reveal_zoom` (the min zoom at which the label is
 *    visible) evaluated against the live `u_zoom` each frame. This REPLACES both
 *    the old per-build boolean `COLLIDE_HIDE` flag AND the per-frame
 *    `mod(a_ordinal, N)` decimation: declutter + decimation are computed ONCE per
 *    build (`labelReveal.ts`) as a single reveal threshold. Because the shader
 *    test is `u_zoom >= a_reveal_zoom`, the visible set is MONOTONE in zoom by
 *    construction - zooming in only ever ADDS labels (never removes one), a band
 *    crossing never flashes the full set, and admission tracks live zoom with no
 *    rebuild. Zoom-in needs no rebuild at all (the built set is a superset).
 *
 * SDF ENCODING (must agree with `glyphAtlas.ts`): TinySDF with `cutoff = 0.25`
 * puts the glyph EDGE at byte `255 * (1 - cutoff) = 191`, i.e. normalized
 * `d = 0.75` - NOT `0.5`. This is exactly MapLibre's own glyph shader constant
 * (`buff = 192/256 = 0.75`); the companion design doc's "0.5 = edge" is a
 * transcription slip for the stated `cutoff: 0.25`, corrected to {@link SDF_EDGE}
 * here so the fragment math matches the bytes the atlas actually produces.
 * Distances grow toward the glyph interior (higher `d` = deeper inside); halos
 * extend OUTWARD (lower `d`), which is why `cutoff 0.25` (6 px outside vs 2 px
 * inside headroom at `radius 8`) is the right asymmetry for outward halos.
 */

/** Per-label KIND, indexing every per-kind uniform array. The first five mirror
 *  the stock symbol layers; SHOT_ERROR / CLOSURE_ERROR are the Errors labels (a
 *  shot's reading deltas, a loop closure's error). The numeric values are the
 *  shader's `a_kind` contract - keep in lock-step with the uniform-array order. */
export const LABEL_KIND = {
  SHOT_LABEL: 0,
  STATION_LABEL: 1,
  SECTION_NAME: 2,
  STATION_COMMENT: 3,
  SHOT_COMMENT: 4,
  SHOT_ERROR: 5,
  CLOSURE_ERROR: 6,
} as const;
export type LabelKind = (typeof LABEL_KIND)[keyof typeof LABEL_KIND];
/** Number of label kinds == length of every per-kind uniform array. */
export const LABEL_KIND_COUNT = 7;

/**
 * `a_flags` bitfield (a UNSIGNED_BYTE integer attribute). Kept here because the
 * shader is the contract for their meaning. `labelTessellate.ts` sets them.
 *  - FIRST_IN_SECTION: station label that is first in its section -> always shown
 *    (overrides decimation), mirroring the stock `isFirstInSection` filter branch.
 *    CPU-ONLY now: the reveal pass (`labelReveal.ts`) reads it to floor the reveal
 *    zoom at the kind minzoom; the SHADER no longer tests it (decimation moved
 *    out of the shader into `a_reveal_zoom`). Still stamped for the CPU refresh
 *    paths + parity tests.
 *  - REFERENCE: shot label of a reference shot -> collapsed when reference shots
 *    are hidden.
 *  - ALL_REFERENCE: station label / all-reference section name -> collapsed with
 *    reference shots (mirrors `allShotsReference` / `allStationsReference`).
 *  - DIM: anchored / immovable label -> drawn at 0.25 opacity (move tools).
 *  - DRAG_HIDE: drag-excluded (a move tool is dragging this feature) -> collapsed.
 *    A flags-only upload flips it without disturbing the per-label reveal zoom
 *    (which the reveal-only upload path owns), so an active drag and a reveal
 *    rerun never clobber each other.
 *  - COMMENT_HIDE: the single comment picked up during `move_comment` -> collapsed
 *    so only the cursor-following tag shows (parity with the deleted stock
 *    hidden-comment filter). A flags-only bit like DRAG_HIDE; only ever set on the
 *    one shot / station comment being moved (C3).
 *
 * The old `COLLIDE_HIDE` bit is GONE: collision decluttering is no longer a
 * boolean baked per zoom (which flashed stale bits on band crossings and evicted
 * lower-zoom winners) but a per-label reveal zoom (`a_reveal_zoom`) evaluated per
 * frame - see the module header + `labelReveal.ts`.
 */
export const LABEL_FLAG = {
  FIRST_IN_SECTION: 1 << 0,
  REFERENCE: 1 << 1,
  ALL_REFERENCE: 1 << 2,
  DIM: 1 << 3,
  DRAG_HIDE: 1 << 4,
  COMMENT_HIDE: 1 << 5,
  /** Set on a Jump shot's own labels (#27). REFERENCE means the Reference kind
   *  specifically - the two split when the display toggles did, so a label can
   *  be culled with the exact kind whose geometry the line shader collapsed. */
  JUMP: 1 << 6,
  /** Station/section counterpart of ALL_REFERENCE: the station is touched by
   *  Jumps (#27). A station can be touched by BOTH virtual kinds, so unlike a
   *  shot it needs a pair of bits rather than one - it stays drawn while ANY
   *  kind touching it is drawn, which is not the same as "hide if my bit is
   *  hidden". Mirrors the circle shader's mask exactly. */
  ALL_JUMP: 1 << 7,
} as const;

/** Normalized SDF value at the glyph edge (= `1 - cutoff`, cutoff 0.25). See the
 *  module header: this is MapLibre's `192/256`, not `0.5`. */
export const SDF_EDGE = 0.75;

/** Vertex-attribute names, in the order the GL layer binds them. `a_kind` and
 *  `a_flags` are INTEGER attributes (bind with `vertexAttribIPointer`,
 *  UNSIGNED_BYTE); the rest are FLOAT except `a_color` (UNSIGNED_BYTE, normalized),
 *  matching the geometry layer's idiom. */
export const LABEL_ATTRIBS = {
  anchor: 'a_anchor', // vec2 FLOAT   anchor-relative mercator (shared per label)
  offsetEm: 'a_offset_em', // vec2 FLOAT   glyph-corner offset from anchor, in em
  uv: 'a_uv', // vec2 FLOAT   atlas PIXEL coords (normalized in shader)
  color: 'a_color', // vec4 UNSIGNED_BYTE (normalized) data-driven text color
  kind: 'a_kind', // uint UNSIGNED_BYTE (integer) LABEL_KIND
  revealZoom: 'a_reveal_zoom', // float FLOAT  min live zoom at which the label draws
  flags: 'a_flags', // uint UNSIGNED_BYTE (integer) LABEL_FLAG bitfield
} as const;

/** Uniform names (documentation + a single source the GL layer can reference). */
export const LABEL_UNIFORMS = {
  matrix: 'u_matrix',
  viewport: 'u_viewport',
  atlasSize: 'u_atlas_size',
  atlas: 'u_atlas',
  fontSize: 'u_font_size',
  sdfRadius: 'u_sdf_radius',
  zoom: 'u_zoom', // live map zoom, compared against a_reveal_zoom
  textSizePx: 'u_text_size_px', // float[LABEL_KIND_COUNT]
  opacity: 'u_opacity', // float[LABEL_KIND_COUNT]
  kindVisible: 'u_kind_visible', // float[LABEL_KIND_COUNT]
  haloColor: 'u_halo_color', // vec4[LABEL_KIND_COUNT]
  haloWidthPx: 'u_halo_width_px', // float[LABEL_KIND_COUNT]
  hideReference: 'u_hide_reference',
  hideJump: 'u_hide_jump',
} as const;

// The flag-bit literals are inlined into the GLSL (a `#define` block) so the
// shader and the TS `LABEL_FLAG` table share one set of numbers by construction.
const FLAG_DEFINES = `
#define FLAG_REFERENCE ${LABEL_FLAG.REFERENCE}u
#define FLAG_JUMP ${LABEL_FLAG.JUMP}u
#define FLAG_ALL_REFERENCE ${LABEL_FLAG.ALL_REFERENCE}u
#define FLAG_ALL_JUMP ${LABEL_FLAG.ALL_JUMP}u
#define FLAG_DIM ${LABEL_FLAG.DIM}u
#define FLAG_DRAG_HIDE ${LABEL_FLAG.DRAG_HIDE}u
#define FLAG_COMMENT_HIDE ${LABEL_FLAG.COMMENT_HIDE}u
#define KIND_COUNT ${LABEL_KIND_COUNT}`;

/**
 * Label vertex shader. Projects the shared label anchor, adds the per-glyph em
 * offset scaled by the per-kind rendered text size (em -> screen px), and
 * COLLAPSES the whole glyph quad to a clipped point (`vec4(2,2,2,1)`, the
 * geometry layer's degenerate-vertex idiom) whenever the label should not draw:
 * kind toggled off / below its minzoom (`u_kind_visible`), drag-excluded
 * (DRAG_HIDE), reference-hidden, or NOT YET REVEALED (`u_zoom < a_reveal_zoom`).
 * The reveal test folds in BOTH the declutter/collision result and the
 * decimation band, computed once per build (`labelReveal.ts`); because it is a
 * single monotone `u_zoom >= a_reveal_zoom` compare, zooming in only adds labels
 * and never removes one. All four verts of a glyph share
 * `a_anchor`/`a_kind`/`a_reveal_zoom`/`a_flags`, so both triangles degenerate
 * together - same guarantee the line/circle collapses rely on.
 */
export const LABEL_VERTEX_SRC = `#version 300 es
precision highp float;
${FLAG_DEFINES}
uniform mat4 u_matrix;
uniform vec2 u_viewport;
uniform vec2 u_atlas_size;
uniform float u_zoom;
uniform float u_text_size_px[KIND_COUNT];
uniform float u_opacity[KIND_COUNT];
uniform float u_kind_visible[KIND_COUNT];
uniform float u_hide_reference;
uniform float u_hide_jump;
in vec2 a_anchor;
in vec2 a_offset_em;
in vec2 a_uv;
in vec4 a_color;
in uint a_kind;
in float a_reveal_zoom;
in uint a_flags;
out vec2 v_uv;
out vec4 v_color;
out float v_opacity;
out float v_size_px;
flat out int v_kind;
void collapse() {
  gl_Position = vec4(2.0, 2.0, 2.0, 1.0);
  v_uv = vec2(0.0);
  v_color = vec4(0.0);
  v_opacity = 0.0;
  v_size_px = 1.0;
  v_kind = 0;
}
void main() {
  int kind = int(a_kind);
  // Per-kind on/off (setShow* toggles + minzoom gate baked into the uniform).
  if (u_kind_visible[kind] < 0.5) { collapse(); return; }
  // Drag-excluded: a move tool is dragging this feature.
  if ((a_flags & FLAG_DRAG_HIDE) != 0u) { collapse(); return; }
  // The one comment picked up during move_comment: hidden so only the
  // cursor-following tag shows (C3).
  if ((a_flags & FLAG_COMMENT_HIDE) != 0u) { collapse(); return; }
  // A virtual shot's labels collapse with the geometry of THEIR OWN kind, so
  // hiding one kind cannot orphan the other's text over an empty line (#27).
  if (u_hide_reference > 0.5 && (a_flags & FLAG_REFERENCE) != 0u) { collapse(); return; }
  if (u_hide_jump > 0.5 && (a_flags & FLAG_JUMP) != 0u) { collapse(); return; }
  // A station (or section name) whose every leg is virtual disappears when every
  // kind that touches IT is hidden - the same per-kind rule the circle shader
  // applies, so a label can never outlive its own station marker. The two bits
  // are independent: a station touched by both kinds carries both.
  bool allJump = (a_flags & FLAG_ALL_JUMP) != 0u;
  bool allRef = (a_flags & FLAG_ALL_REFERENCE) != 0u;
  bool keptByJump = allJump && u_hide_jump < 0.5;
  bool keptByRef = allRef && u_hide_reference < 0.5;
  if ((allJump || allRef) && !keptByJump && !keptByRef) {
    collapse();
    return;
  }
  // Reveal-zoom declutter + decimation, baked once per build. Monotone in zoom
  // by construction: the visible set { u_zoom >= a_reveal_zoom } only grows as
  // the camera zooms in, so no label vanishes and a band crossing never flashes.
  if (u_zoom < a_reveal_zoom) { collapse(); return; }
  vec4 anchorClip = u_matrix * vec4(a_anchor, 0.0, 1.0);
  // em -> screen px via the per-kind rendered size (1 em == u_text_size_px[kind]).
  vec2 offsetPx = a_offset_em * u_text_size_px[kind];
  gl_Position = vec4(
    anchorClip.xy + offsetPx / u_viewport * 2.0 * anchorClip.w,
    anchorClip.z,
    anchorClip.w
  );
  v_uv = a_uv / u_atlas_size; // atlas px -> normalized (grow-safe)
  v_color = a_color;
  // Anchored / immovable labels dim to 0.25; folds into the per-kind opacity.
  v_opacity = ((a_flags & FLAG_DIM) != 0u ? 0.25 : 1.0) * u_opacity[kind];
  v_size_px = u_text_size_px[kind];
  v_kind = kind;
}`;

/**
 * Label fragment shader. Samples the R8 SDF atlas and resolves a size-adaptive
 * text edge + outward halo (§1 of the design). `d` (0..1) is the atlas red
 * channel, {@link SDF_EDGE} (0.75) is the glyph edge; deeper inside = higher `d`.
 * The AA half-band `gamma` and the halo edge are both expressed in SDF units via
 * `sdfPerScreenPx = u_font_size / (u_sdf_radius * v_size_px)` - the raster-px ->
 * screen-px -> SDF-unit chain - so a halo width given in SCREEN px lands at the
 * right SDF threshold at any rendered size. Output is premultiplied to match the
 * geometry layer's `fragColor = vec4(col.rgb * a, a)` convention.
 */
export const LABEL_FRAGMENT_SRC = `#version 300 es
precision highp float;
${FLAG_DEFINES}
#define SDF_EDGE ${SDF_EDGE}
uniform sampler2D u_atlas;
uniform float u_font_size;
uniform float u_sdf_radius;
uniform vec4 u_halo_color[KIND_COUNT];
uniform float u_halo_width_px[KIND_COUNT];
in vec2 v_uv;
in vec4 v_color;
in float v_opacity;
in float v_size_px;
flat in int v_kind;
out vec4 fragColor;
void main() {
  float d = texture(u_atlas, v_uv).r;
  // One screen px == this many SDF units at the current rendered size.
  float sdfPerScreenPx = u_font_size / (u_sdf_radius * max(v_size_px, 1.0));
  float gamma = 0.6 * sdfPerScreenPx; // ~0.6 px antialias half-band
  float textAlpha = smoothstep(SDF_EDGE - gamma, SDF_EDGE + gamma, d);
  // Halo extends OUTWARD (toward lower d) by its screen-px width.
  float haloWidth = u_halo_width_px[v_kind];
  float haloEdge = SDF_EDGE - haloWidth * sdfPerScreenPx;
  float haloAlpha = smoothstep(haloEdge - gamma, haloEdge + gamma, d);
  vec4 haloColor = u_halo_color[v_kind];
  vec3 rgb = mix(haloColor.rgb, v_color.rgb, textAlpha);
  // Blend the source alphas across the halo->text transition, then fold in the
  // per-kind (dimmed) opacity and the halo coverage.
  float srcA = mix(haloColor.a, v_color.a, textAlpha);
  float a = haloAlpha * srcA * v_opacity;
  fragColor = vec4(rgb * a, a);
}`;
