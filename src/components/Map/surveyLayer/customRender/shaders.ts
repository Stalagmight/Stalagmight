/**
 * GLSL ES 3.00 shader sources for the custom-render prototype (#customRender).
 *
 * Targets WebGL2 (MapLibre 5.16 runs a WebGL2 context) with premultiplied-alpha
 * output to match MapLibre's `gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA)`.
 *
 * The line vertex extrusion + fragment edge-feather (AA) math is adapted from
 * MapLibre GL JS `src/shaders/line.vertex.glsl` and `line.fragment.glsl`
 * (BSD-3-Clause, derived from Mapbox GL JS pre-fork). What was reused: the
 * screen-space normal extrusion by an `outset = halfwidth + antialiasing`, and
 * the fragment `alpha = clamp(min(dist + blur, outset - dist) / blur, 0, 1)`
 * feather. What differs: MapLibre extrudes in tile space using its packed
 * `a_pos_normal` / `a_data` encoding and `projectTile()`; here each vertex
 * carries both segment endpoints (anchor-relative mercator) and the normal is
 * computed from the projected screen-space direction, so the quads work with a
 * raw projection matrix. The perspective `v_gamma_scale` term is dropped
 * (prototype assumes near-nadir viewing); note this if AA looks soft under pitch.
 *
 * ROUND CAPS: the shot quad is extended by `outset` ALONG the segment past both
 * endpoints (in the vertex shader, so the vertex count is unchanged), and the
 * fragment measures distance to the line SEGMENT (a capsule SDF) rather than to
 * the infinite centerline. Inside the body that distance is `abs(perp)` (the old
 * butt-cap AA path, byte-for-byte), and past an endpoint it is the radial
 * distance to that endpoint, so the end rounds off with the same feather. This
 * removes the notches where consecutive shots meet at a shared station (stock's
 * depth-gradient sublayer likewise used `line-cap: round`). Per-endpoint depth
 * color is carried as two attributes and interpolated by the CLAMPED
 * along-fraction, so each rounded cap holds its endpoint color exactly.
 *
 * TRANSLUCENT DOUBLE-BLEND (measured, accepted): where two TRANSLUCENT shots
 * (reference at 0.5, dimmed at 0.25) share a station, their round caps overlap in
 * a ~outset-wide disc and premultiplied blending applies twice, so the joint
 * reads brighter/more-saturated than the shot body (a live probe on 4naharon
 * measured a reference joint at 0.75 effective coverage vs 0.5 mid-shot, matching
 * the prediction). This is only visible with reference shots shown AND stations
 * hidden AND zoomed in: the station disc (radius >= the line half-width) is drawn
 * over every junction and covers the overlap in normal use, and stock's
 * round-capped depth sublayer had the same behavior. Accepted as-is; if it ever
 * needs removing, zero `capSign` for translucent verts (a_reference / a_dim) to
 * give those shots butt caps.
 */

/**
 * Historical BAKED highlight multipliers (the stock feature-state size bumps the
 * deleted layers used): shot line hover 4/2 = 2x, selection 3/2 = 1.5x; station
 * radius hover 6/4 = 1.5x, selection 5/4 = 1.25x. Now the FALLBACK when the config
 * sizes are unavailable (base width/radius non-positive, or the emphasized size
 * unset) - see {@link highlightSizeMul}. The live uniforms (`u_hover_width_mul`
 * etc.) let independent `map.shot.hover_width` / `selection_width` +
 * `map.station.hover_radius` / `selection_radius` config drive the widths (C2).
 */
/** Per-kind line alpha (#27). Exported because the interact/hover overlay draws
 *  the SAME leg through MapLibre rather than this shader, and a divergence there
 *  makes a hovered reference tie pop brighter than its own base rendering. */
export const SHOT_ALPHA_DIMMED = 0.25;
export const SHOT_ALPHA_REFERENCE = 0.28;
export const SHOT_ALPHA_JUMP = 0.5;
export const SHOT_ALPHA_NORMAL = 1;

/** Gas-reach "beyond budget" dim factor (#395): a lighter dim than the
 *  anchored/immovable `SHOT_ALPHA_DIMMED`, so a beyond-range shot/station keeps
 *  its base color visibly readable while clearly receding vs. in-reach geometry.
 *  Encoded in the shared `a_dim` / `v_dim` channel at level 0.5 (see
 *  `GAS_DIM_LEVEL` in controller.ts); the shader maps > 0.75 to the full dim and
 *  > 0.25 to this one. */
export const GAS_DIM_ALPHA = 0.45;

/** Base line alpha for a shot of each kind, so the interact/hover overlay and
 *  the shader cannot drift apart. Does not include the dimmed case, which wins
 *  over the kind. */
export const SHOT_ALPHA_BY_KIND: Record<'Normal' | 'Jump' | 'Reference', number> = {
  Normal: SHOT_ALPHA_NORMAL,
  Jump: SHOT_ALPHA_JUMP,
  Reference: SHOT_ALPHA_REFERENCE,
};

/** Render a number as a GLSL float literal: always a decimal point, no trailing
 *  zero padding, so the generated source stays byte-identical to the constants
 *  it was hand-written with. */
export const glslFloat = (n: number): string => (Number.isInteger(n) ? `${n}.0` : String(n));

export const DEFAULT_SHOT_HOVER_WIDTH_MUL = 2;
export const DEFAULT_SHOT_SELECT_WIDTH_MUL = 1.5;
export const DEFAULT_STATION_HOVER_SIZE_MUL = 1.5;
export const DEFAULT_STATION_SELECT_SIZE_MUL = 1.25;

/**
 * Resolve a highlight size multiplier from configured ABSOLUTE px: the emphasized
 * size (hover / selection width or radius) over the base, so the shader keeps its
 * `size * mul` structure while honoring independent config values. Falls back to
 * the historical baked ratio when the base is non-positive or the emphasized px is
 * unset / non-positive (config missing), so behavior is unchanged without config.
 */
export function highlightSizeMul(
  basePx: number,
  emphasizedPx: number | undefined,
  fallback: number,
): number {
  if (!(basePx > 0) || emphasizedPx === undefined || !(emphasizedPx > 0)) return fallback;
  return emphasizedPx / basePx;
}

/** All px sizes are DEVICE pixels; `u_viewport` is `[drawingBufferWidth,
 *  drawingBufferHeight]`. `u_matrix` maps anchor-relative mercator -> clip; it
 *  is composed from MapLibre's `defaultProjectionData.mainMatrix` (the mercator
 *  [0,1] -> clip matrix), NOT `modelViewProjectionMatrix` (world-pixel space). */
export const LINE_VERTEX_SRC = `#version 300 es
precision highp float;
uniform mat4 u_matrix;
uniform vec2 u_viewport;
uniform float u_half_width;
uniform float u_aa;
uniform float u_hide_jump;
uniform float u_hide_reference;
uniform float u_hover_width_mul;
uniform float u_select_width_mul;
in vec2 a_pos_a;
in vec2 a_pos_b;
in float a_endpoint;
in float a_side;
in vec4 a_color_from;
in vec4 a_color_to;
// #27: 0 = permanent, 1 = jump, 2 = reference (the ShotKind discriminant, not
// a bool). Both virtual kinds test as > 0.5, so "is virtual" reads the same.
in float a_reference;
in float a_dim;
in float a_hide;
in float a_highlight;
out vec4 v_color_from;
out vec4 v_color_to;
out float v_alpha_mul;
out float v_perp;
out float v_along;
out float v_len;
out float v_width_mul;
void main() {
  // Drag-hidden shots (a moving feature excluded during a move drag): collapse
  // the whole quad to a single clipped point. All 4 verts of a segment share
  // a_hide, so both triangles degenerate together. Mirrors the stock bulk
  // layers' setFilter exclusion without omitting the shot from the buffers.
  if (a_hide > 0.5) {
    gl_Position = vec4(2.0, 2.0, 2.0, 1.0);
    v_color_from = vec4(0.0);
    v_color_to = vec4(0.0);
    v_alpha_mul = 1.0;
    v_perp = 0.0;
    v_along = 0.0;
    v_len = 1.0;
    v_width_mul = 1.0;
    return;
  }
  // Jumps and references hide independently (#27). Collapse the whole quad to a
  // single clipped point so it produces no fragments. All 4 vertices of a
  // segment share a_reference, so both triangles degenerate together.
  bool isJump = a_reference > 0.5 && a_reference < 1.5;
  bool isRef = a_reference > 1.5;
  if ((isJump && u_hide_jump > 0.5) || (isRef && u_hide_reference > 0.5)) {
    gl_Position = vec4(2.0, 2.0, 2.0, 1.0);
    v_color_from = vec4(0.0);
    v_color_to = vec4(0.0);
    v_alpha_mul = 1.0;
    v_perp = 0.0;
    v_along = 0.0;
    v_len = 1.0;
    v_width_mul = 1.0;
    return;
  }
  vec4 pa = u_matrix * vec4(a_pos_a, 0.0, 1.0);
  vec4 pb = u_matrix * vec4(a_pos_b, 0.0, 1.0);
  vec2 sa = pa.xy / pa.w;
  vec2 sb = pb.xy / pb.w;
  // Screen-pixel A->B vector (NDC delta * 0.5 * viewport) and its length; dir is
  // the unit segment direction, normal the perpendicular. Guard a zero-length
  // (coincident-endpoint) segment so normalize() never yields NaN.
  vec2 segPx = (sb - sa) * u_viewport * 0.5;
  float segLen = length(segPx);
  vec2 dir = (segLen > 0.0001) ? (segPx / segLen) : vec2(1.0, 0.0);
  vec2 normal = vec2(-dir.y, dir.x);
  vec4 base = (a_endpoint < 0.5) ? pa : pb;
  // Line emphasis level carried in a_highlight: 2 = hover, 1 = selection,
  // 0 = none. Widen highlighted shots by the CONFIGURED width ratios (default
  // hover_width/width = 4/2 = 2x, selection_width/width = 3/2 = 1.5x), passed as
  // uniforms so map.shot.hover_width / selection_width config drive them (C2).
  // Segment-hover (Move Segment), single-line hover, and panel-row hover all
  // resolve to level 2; line selection to level 1. When a line is both
  // hovered and selected the controller merges to level 2 (hover wins), matching
  // the stock paint case order (hover before selected). The fragment recomputes
  // its outset from the same multiplier so the AA feather scales with the line.
  float widthMul = (a_highlight > 1.5) ? u_hover_width_mul : ((a_highlight > 0.5) ? u_select_width_mul : 1.0);
  float outset = u_half_width * widthMul + u_aa;
  // Round caps: extrude by outset ALONG the segment past each endpoint (A verts
  // backward, B verts forward) in addition to the perpendicular width offset, so
  // the fragment's capsule SDF has room to round the ends. capSign folds the
  // along-extension into the same clip offset; vertex count is unchanged.
  float capSign = (a_endpoint < 0.5) ? -1.0 : 1.0;
  vec2 offsetPx = normal * a_side * outset + dir * capSign * outset;
  vec2 offsetClip = offsetPx / u_viewport * 2.0 * base.w;
  gl_Position = vec4(base.xy + offsetClip, base.z, base.w);
  v_color_from = a_color_from;
  v_color_to = a_color_to;
  // Dimmed shots (anchored / immovable during move tools) draw at 0.25 and this
  // WINS over the virtual cases, matching the stock line-opacity case order
  // (immovable 0.25, anchored 0.25, virtual, else 1). A jump keeps the 0.5 it
  // has always had so existing surveys look unchanged; a reference is fainter
  // still, because it is not a leg anyone swims (#27). The gas-dim (0.45) level
  // must never BRIGHTEN a fainter per-kind base (a reference at 0.28), so it
  // clamps against the per-kind alpha via min() rather than replacing it.
  v_alpha_mul = (a_dim > 0.75) ? ${glslFloat(SHOT_ALPHA_DIMMED)} : ((a_dim > 0.25) ? min(${glslFloat(GAS_DIM_ALPHA)}, (isRef ? ${glslFloat(SHOT_ALPHA_REFERENCE)} : (isJump ? ${glslFloat(SHOT_ALPHA_JUMP)} : ${glslFloat(SHOT_ALPHA_NORMAL)}))) : (isRef ? ${glslFloat(SHOT_ALPHA_REFERENCE)} : (isJump ? ${glslFloat(SHOT_ALPHA_JUMP)} : ${glslFloat(SHOT_ALPHA_NORMAL)})));
  // Signed perpendicular px, along-axis px (from endpoint A; the cap extension
  // shifts each base vert by capSign*outset), and segment length px. The fragment
  // rebuilds the capsule distance and the clamped color fraction from these.
  v_perp = a_side * outset;
  v_along = ((a_endpoint < 0.5) ? 0.0 : segLen) + capSign * outset;
  v_len = segLen;
  v_width_mul = widthMul;
}`;

export const LINE_FRAGMENT_SRC = `#version 300 es
precision highp float;
uniform float u_half_width;
uniform float u_aa;
in vec4 v_color_from;
in vec4 v_color_to;
in float v_alpha_mul;
in float v_perp;
in float v_along;
in float v_len;
in float v_width_mul;
out vec4 fragColor;
void main() {
  float outset = u_half_width * v_width_mul + u_aa;
  // Capsule SDF: distance to the SEGMENT [0, v_len] along-axis. In the body
  // (0..v_len) alongClamp == v_along so dist == abs(v_perp), byte-for-byte the
  // old butt-cap AA path; past an endpoint it is the radial distance to that
  // endpoint, rounding the cap with the same feather.
  float alongClamp = clamp(v_along, 0.0, v_len);
  float dist = length(vec2(v_along - alongClamp, v_perp));
  // MapLibre line AA: feather the last u_aa px of the outset.
  float alpha = clamp(min(dist + u_aa, outset - dist) / u_aa, 0.0, 1.0);
  // Per-endpoint depth color, interpolated by the CLAMPED along-fraction so each
  // rounded cap holds the endpoint color exactly (stock round-cap-gradient
  // parity); flat / severity / overlay modes have from == to (a no-op mix).
  float t = (v_len > 0.0001) ? clamp(v_along / v_len, 0.0, 1.0) : 0.0;
  vec4 col = mix(v_color_from, v_color_to, t);
  float a = col.a * v_alpha_mul * alpha;
  fragColor = vec4(col.rgb * a, a);
}`;

export const CIRCLE_VERTEX_SRC = `#version 300 es
precision highp float;
uniform mat4 u_matrix;
uniform vec2 u_viewport;
uniform float u_radius;
uniform float u_stroke_width;
uniform float u_aa;
uniform float u_hide_jump;
uniform float u_hide_reference;
uniform float u_hover_size_mul;
uniform float u_select_size_mul;
in vec2 a_center;
in vec2 a_corner;
in vec4 a_fill;
in vec4 a_stroke;
in float a_all_reference;
in float a_size_mul;
in float a_dim;
in float a_hide;
in float a_highlight;
out vec4 v_fill;
out vec4 v_stroke;
out vec2 v_offset;
out float v_size_mul;
out float v_dim;
void main() {
  // Drag-hidden stations (moving during a move drag): collapse the quad to a
  // single clipped point, without omitting the station from the buffers.
  if (a_hide > 0.5) {
    gl_Position = vec4(2.0, 2.0, 2.0, 1.0);
    v_fill = vec4(0.0);
    v_stroke = vec4(0.0);
    v_offset = vec2(0.0);
    v_size_mul = 1.0;
    v_dim = 0.0;
    return;
  }
  // A station touched only by virtual shots disappears when every kind that
  // touches it is hidden (#27). a_all_reference is a mask, not a bool:
  // 0 = a Normal shot touches it, 1 = only Jumps, 2 = only References,
  // 3 = both. Testing "any virtual kind is visible" instead left an orphan
  // marker on, say, a reference-only station while References were hidden and
  // Jumps were not - its own legs were all gone.
  bool onlyVirtual = a_all_reference > 0.5;
  bool touchedByJump = a_all_reference > 0.5 && a_all_reference < 1.5
                    || a_all_reference > 2.5;
  bool touchedByRef = a_all_reference > 1.5;
  bool keptByJump = touchedByJump && u_hide_jump < 0.5;
  bool keptByRef = touchedByRef && u_hide_reference < 0.5;
  if (onlyVirtual && !keptByJump && !keptByRef) {
    gl_Position = vec4(2.0, 2.0, 2.0, 1.0);
    v_fill = vec4(0.0);
    v_stroke = vec4(0.0);
    v_offset = vec2(0.0);
    v_size_mul = 1.0;
    v_dim = 0.0;
    return;
  }
  vec4 c = u_matrix * vec4(a_center, 0.0, 1.0);
  // Line emphasis level carried in a_highlight: 2 = hover, 1 = selection,
  // 0 = none. Enlarge highlighted stations to the CONFIGURED radius ratios
  // (default hover_radius/radius = 6/4 = 1.5x, selection_radius/radius = 5/4 =
  // 1.25x), passed as uniforms so map.station.hover_radius / selection_radius
  // config drive them (C2). Highlight takes PRECEDENCE over zero-datum sizing
  // (a_size_mul = 2): a hovered/selected zero-datum station uses the highlight
  // size directly, matching the stock precedence (hover 6px / selection 5px, and
  // the 2x = 8px only when unhighlighted) - NOT a_size_mul * highlight, which
  // would balloon the entrance station to 12/10px. The fragment recomputes
  // disc/stroke radii from v_size_mul, so no separate fragment change is needed.
  float sizeMul = (a_highlight > 1.5) ? u_hover_size_mul
                : (a_highlight > 0.5) ? u_select_size_mul
                : a_size_mul;
  // Zero-datum stations render 2x (a_size_mul) when unhighlighted, so the quad
  // extent must grow to fit; the fragment recomputes radii with the same mul.
  float ext = (u_radius + u_stroke_width) * sizeMul + u_aa;
  vec2 offsetPx = a_corner * ext;
  vec2 offsetClip = offsetPx / u_viewport * 2.0 * c.w;
  gl_Position = vec4(c.xy + offsetClip, c.z, c.w);
  v_fill = a_fill;
  v_stroke = a_stroke;
  v_offset = offsetPx;
  v_size_mul = sizeMul;
  v_dim = a_dim;
}`;

export const CIRCLE_FRAGMENT_SRC = `#version 300 es
precision highp float;
uniform float u_radius;
uniform float u_stroke_width;
uniform float u_aa;
in vec4 v_fill;
in vec4 v_stroke;
in vec2 v_offset;
in float v_size_mul;
in float v_dim;
out vec4 fragColor;
void main() {
  float rad = u_radius * v_size_mul;
  float sw = u_stroke_width * v_size_mul;
  float d = length(v_offset);
  float outer = rad + sw;
  float outerAlpha = clamp((outer - d) / u_aa + 0.5, 0.0, 1.0);
  float fillT = clamp((rad - d) / u_aa + 0.5, 0.0, 1.0);
  vec4 col = mix(v_stroke, v_fill, fillT);
  // Dimmed stations (anchored / immovable) draw at 0.25 (stock circle-opacity +
  // circle-stroke-opacity anchored case); applies uniformly to fill + stroke.
  float dimF = (v_dim > 0.75) ? 0.25 : ((v_dim > 0.25) ? ${glslFloat(GAS_DIM_ALPHA)} : 1.0);
  float a = col.a * outerAlpha * dimF;
  fragColor = vec4(col.rgb * a, a);
}`;
