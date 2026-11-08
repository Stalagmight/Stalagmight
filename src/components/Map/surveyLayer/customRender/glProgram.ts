/**
 * Shader program + resolved location cache shared by the custom survey layers
 * (#339).
 *
 * Compiling and linking is unchanged from what each layer did inline; what is
 * added is that every uniform / attribute location is resolved ONCE, right
 * after the link, so the per-frame draw path reads a record instead of calling
 * `gl.getUniformLocation` / `gl.getAttribLocation` for every uniform and every
 * attribute of every program on every frame.
 */

/** Resolved uniform locations, keyed by the name tuple passed at link time.
 *  A `null` entry means the linker optimized that uniform out; WebGL treats a
 *  null uniform location as a silent no-op, which is the exact tolerance the
 *  old per-frame lookups had - preserved deliberately, no warning. */
export type UniformMap<N extends readonly string[]> = Record<N[number], WebGLUniformLocation | null>;

/** Resolved attribute locations; `-1` for a name the linker dropped, which the
 *  binders below skip exactly as the per-frame lookups did. */
export type AttribMap<N extends readonly string[]> = Record<N[number], number>;

/**
 * A linked program plus its location cache.
 *
 * Locations are fixed for the life of a program, so they are resolved once at
 * link time and `render()` never calls `getUniformLocation` /
 * `getAttribLocation` (#339). Built in `onAdd()` and freed in `onRemove()`.
 * MapLibre does not re-add a custom layer after a context restore or a worker
 * restart; `styleRestore.ts` does, running onRemove then onAdd, so the cache
 * rebuilds together with the program it describes.
 *
 * Deliberately NOT stored on the buffer GPU-state objects: `uploadBuffers()` /
 * `freeGpu()` recreate those on every data change, a shorter lifecycle than the
 * program's, which would drop the cache on every edit.
 */
export interface ProgramBundle<U extends readonly string[], A extends readonly string[]> {
  readonly program: WebGLProgram;
  readonly uniforms: UniformMap<U>;
  readonly attribs: AttribMap<A>;
}

function compile(
  gl: WebGL2RenderingContext,
  type: number,
  src: string,
  tag: string,
): WebGLShader | null {
  const sh = gl.createShader(type);
  if (!sh) return null;
  gl.shaderSource(sh, src);
  gl.compileShader(sh);
  if (!gl.getShaderParameter(sh, gl.COMPILE_STATUS)) {
    console.warn(`${tag} shader compile failed:`, gl.getShaderInfoLog(sh));
    gl.deleteShader(sh);
    return null;
  }
  return sh;
}

function link(
  gl: WebGL2RenderingContext,
  vsSrc: string,
  fsSrc: string,
  tag: string,
): WebGLProgram | null {
  const vs = compile(gl, gl.VERTEX_SHADER, vsSrc, tag);
  const fs = compile(gl, gl.FRAGMENT_SHADER, fsSrc, tag);
  if (!vs || !fs) return null;
  const prog = gl.createProgram();
  if (!prog) return null;
  gl.attachShader(prog, vs);
  gl.attachShader(prog, fs);
  gl.linkProgram(prog);
  gl.deleteShader(vs);
  gl.deleteShader(fs);
  if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) {
    console.warn(`${tag} program link failed:`, gl.getProgramInfoLog(prog));
    gl.deleteProgram(prog);
    return null;
  }
  return prog;
}

/** Compile + link the pair, then resolve every named location once. `tag` is
 *  the console prefix for the compile / link failure warnings. Returns null on
 *  any failure (the caller disables itself), having already logged the GL log. */
export function linkProgramBundle<U extends readonly string[], A extends readonly string[]>(
  gl: WebGL2RenderingContext,
  vsSrc: string,
  fsSrc: string,
  uniformNames: U,
  attribNames: A,
  tag: string,
): ProgramBundle<U, A> | null {
  const program = link(gl, vsSrc, fsSrc, tag);
  if (!program) return null;
  const uniforms: Record<string, WebGLUniformLocation | null> = {};
  for (const name of uniformNames) uniforms[name] = gl.getUniformLocation(program, name);
  const attribs: Record<string, number> = {};
  for (const name of attribNames) attribs[name] = gl.getAttribLocation(program, name);
  return {
    program,
    uniforms: uniforms as UniformMap<U>,
    attribs: attribs as AttribMap<A>,
  };
}

/** Free a bundle's program. Safe on null (a layer that never linked). */
export function deleteProgramBundle(
  gl: WebGL2RenderingContext,
  bundle: ProgramBundle<readonly string[], readonly string[]> | null,
): void {
  if (!bundle) return;
  gl.deleteProgram(bundle.program);
}

/** Bind `buf` as a 2-float attribute at a cached location. */
export function bindVec2(gl: WebGL2RenderingContext, loc: number, buf: WebGLBuffer): void {
  if (loc < 0) return;
  gl.bindBuffer(gl.ARRAY_BUFFER, buf);
  gl.enableVertexAttribArray(loc);
  gl.vertexAttribPointer(loc, 2, gl.FLOAT, false, 0, 0);
}

/** Bind `buf` as a single-float attribute at a cached location. */
export function bindFloat(gl: WebGL2RenderingContext, loc: number, buf: WebGLBuffer): void {
  if (loc < 0) return;
  gl.bindBuffer(gl.ARRAY_BUFFER, buf);
  gl.enableVertexAttribArray(loc);
  gl.vertexAttribPointer(loc, 1, gl.FLOAT, false, 0, 0);
}

/** Bind `buf` as a normalized RGBA8 attribute at a cached location. */
export function bindColor(gl: WebGL2RenderingContext, loc: number, buf: WebGLBuffer): void {
  if (loc < 0) return;
  gl.bindBuffer(gl.ARRAY_BUFFER, buf);
  gl.enableVertexAttribArray(loc);
  gl.vertexAttribPointer(loc, 4, gl.UNSIGNED_BYTE, true, 0, 0);
}

/** Bind a UNSIGNED_BYTE INTEGER attribute (`in uint`) with `vertexAttribIPointer`. */
export function bindIntU8(gl: WebGL2RenderingContext, loc: number, buf: WebGLBuffer): void {
  if (loc < 0) return;
  gl.bindBuffer(gl.ARRAY_BUFFER, buf);
  gl.enableVertexAttribArray(loc);
  gl.vertexAttribIPointer(loc, 1, gl.UNSIGNED_BYTE, 0, 0);
}
