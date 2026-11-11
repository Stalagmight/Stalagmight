/**
 * Minimal fake WebGL2 context for the custom-layer GL path (#339). NOT a test
 * suite - a helper the suites in this directory import.
 *
 * It implements exactly the calls SurveyCustomLayer's onAdd / render / onRemove
 * make, counts them, and records the arguments the location-caching tests care
 * about. `FakeGl` and `FakeWebGLBuffer` are meant to be installed as the
 * `WebGL2RenderingContext` / `WebGLBuffer` globals with `vi.stubGlobal`, so the
 * layer's `instanceof` guards accept them in the node test environment.
 */

/** Stands in for a `WebGLBuffer` (the layer's freeGpu() instanceof-checks it). */
export class FakeWebGLBuffer {}

export class FakeWebGLShader {
  constructor(readonly type: number) {}
}

export class FakeWebGLProgram {
  constructor(readonly id: number) {}
}

/** One recorded `vertexAttribPointer` call (the layout triple is what the
 *  attribute binders must preserve). */
export interface AttribPointerCall {
  loc: number;
  size: number;
  type: number;
  normalized: boolean;
}

/** One recorded `vertexAttribIPointer` call (integer attributes take no
 *  `normalized` flag - that is the whole point of the separate entry point). */
export interface AttribIPointerCall {
  loc: number;
  size: number;
  type: number;
}

export class FakeGl {
  // Real GL enum values, so a swapped constant cannot pass unnoticed.
  readonly VERTEX_SHADER = 0x8b31;
  readonly FRAGMENT_SHADER = 0x8b30;
  readonly COMPILE_STATUS = 0x8b81;
  readonly LINK_STATUS = 0x8b82;
  readonly ARRAY_BUFFER = 0x8892;
  readonly ELEMENT_ARRAY_BUFFER = 0x8893;
  readonly COPY_WRITE_BUFFER = 0x8f37;
  readonly STATIC_DRAW = 0x88e4;
  readonly FLOAT = 0x1406;
  readonly UNSIGNED_BYTE = 0x1401;
  readonly UNSIGNED_INT = 0x1405;
  readonly TRIANGLES = 0x0004;
  readonly BLEND = 0x0be2;
  readonly DEPTH_TEST = 0x0b71;
  readonly CULL_FACE = 0x0b44;
  readonly ONE = 1;
  readonly ONE_MINUS_SRC_ALPHA = 0x0303;
  readonly NO_ERROR = 0;

  drawingBufferWidth = 800;
  drawingBufferHeight = 600;

  /** Scripted failures: flip before linking to exercise the bail paths. */
  compileOk = true;
  linkOk = true;

  /** Per-method call counts, read via {@link count}. */
  readonly calls: Record<string, number> = {};
  /** Every name passed to getUniformLocation / getAttribLocation, in order. */
  readonly uniformQueries: string[] = [];
  readonly attribQueries: string[] = [];
  readonly attribPointers: AttribPointerCall[] = [];
  readonly attribIPointers: AttribIPointerCall[] = [];
  readonly bufferBinds: { target: number; buffer: unknown }[] = [];

  private readonly uniformLocations = new Map<string, object>();
  private readonly attribLocations = new Map<string, number>();
  private nextProgramId = 1;
  private nextAttribLoc = 0;

  count(name: string): number {
    return this.calls[name] ?? 0;
  }

  /** The location object handed out for `name`, or undefined if never queried.
   *  Throws when `name` was queried on more than one program and no `progId`
   *  disambiguates - locations are per-program, and a caller that ignores that
   *  would hide exactly the cross-program mix-up the keying exists to expose. */
  uniformLocation(name: string, progId?: number): object | undefined {
    return this.lookup(this.uniformLocations, name, progId);
  }

  /** The location index handed out for `name`, or undefined if never queried. */
  attribLocation(name: string, progId?: number): number | undefined {
    return this.lookup(this.attribLocations, name, progId);
  }

  private lookup<T>(map: Map<string, T>, name: string, progId?: number): T | undefined {
    if (progId !== undefined) return map.get(`${progId}:${name}`);
    const hits = [...map.entries()].filter(([k]) => k.endsWith(`:${name}`));
    if (hits.length > 1) {
      throw new Error(`location for '${name}' is ambiguous across programs; pass progId`);
    }
    return hits[0]?.[1];
  }

  private bump(name: string): void {
    this.calls[name] = this.count(name) + 1;
  }

  createShader(type: number): FakeWebGLShader {
    this.bump('createShader');
    return new FakeWebGLShader(type);
  }

  shaderSource(_sh: FakeWebGLShader, _src: string): void {
    this.bump('shaderSource');
  }

  compileShader(_sh: FakeWebGLShader): void {
    this.bump('compileShader');
  }

  getShaderParameter(_sh: FakeWebGLShader, _pname: number): boolean {
    this.bump('getShaderParameter');
    return this.compileOk;
  }

  getShaderInfoLog(_sh: FakeWebGLShader): string {
    this.bump('getShaderInfoLog');
    return 'fake shader log';
  }

  deleteShader(_sh: FakeWebGLShader): void {
    this.bump('deleteShader');
  }

  createProgram(): FakeWebGLProgram {
    this.bump('createProgram');
    return new FakeWebGLProgram(this.nextProgramId++);
  }

  attachShader(_prog: FakeWebGLProgram, _sh: FakeWebGLShader): void {
    this.bump('attachShader');
  }

  linkProgram(_prog: FakeWebGLProgram): void {
    this.bump('linkProgram');
  }

  getProgramParameter(_prog: FakeWebGLProgram, _pname: number): boolean {
    this.bump('getProgramParameter');
    return this.linkOk;
  }

  getProgramInfoLog(_prog: FakeWebGLProgram): string {
    this.bump('getProgramInfoLog');
    return 'fake program log';
  }

  deleteProgram(_prog: FakeWebGLProgram): void {
    this.bump('deleteProgram');
  }

  useProgram(_prog: FakeWebGLProgram | null): void {
    this.bump('useProgram');
  }

  /** A distinct object per (program, name), stable across repeat queries. Keyed
   *  by the program too - as in real GL - so a draw path that reads a location
   *  from the WRONG bundle gets a different object, not a lookalike. */
  getUniformLocation(prog: FakeWebGLProgram, name: string): object {
    this.bump('getUniformLocation');
    this.uniformQueries.push(name);
    const key = `${prog.id}:${name}`;
    let loc = this.uniformLocations.get(key);
    if (!loc) {
      loc = { name };
      this.uniformLocations.set(key, loc);
    }
    return loc;
  }

  /** A distinct index per (program, name), stable across repeat queries. */
  getAttribLocation(prog: FakeWebGLProgram, name: string): number {
    this.bump('getAttribLocation');
    this.attribQueries.push(name);
    const key = `${prog.id}:${name}`;
    let loc = this.attribLocations.get(key);
    if (loc === undefined) {
      loc = this.nextAttribLoc++;
      this.attribLocations.set(key, loc);
    }
    return loc;
  }

  createBuffer(): FakeWebGLBuffer {
    this.bump('createBuffer');
    return new FakeWebGLBuffer();
  }

  bindBuffer(target: number, buffer: unknown): void {
    this.bump('bindBuffer');
    this.bufferBinds.push({ target, buffer });
  }

  bufferData(_target: number, _data: unknown, _usage: number): void {
    this.bump('bufferData');
  }

  /** Every `bufferSubData` call: the target it wrote through and the bytes. */
  readonly subDataCalls: { target: number; offset: number; data: unknown }[] = [];
  bufferSubData(target: number, offset: number, data: unknown): void {
    this.bump('bufferSubData');
    this.subDataCalls.push({ target, offset, data });
  }

  deleteBuffer(_buf: unknown): void {
    this.bump('deleteBuffer');
  }

  enableVertexAttribArray(_loc: number): void {
    this.bump('enableVertexAttribArray');
  }

  vertexAttribPointer(
    loc: number,
    size: number,
    type: number,
    normalized: boolean,
    _stride: number,
    _offset: number,
  ): void {
    this.bump('vertexAttribPointer');
    this.attribPointers.push({ loc, size, type, normalized });
  }

  vertexAttribIPointer(loc: number, size: number, type: number, _stride: number, _offset: number): void {
    this.bump('vertexAttribIPointer');
    this.attribIPointers.push({ loc, size, type });
  }

  uniformMatrix4fv(_loc: unknown, _transpose: boolean, _value: unknown): void {
    this.bump('uniformMatrix4fv');
  }

  uniform2f(_loc: unknown, _x: number, _y: number): void {
    this.bump('uniform2f');
  }

  uniform1f(_loc: unknown, _x: number): void {
    this.bump('uniform1f');
  }

  drawElements(_mode: number, _count: number, _type: number, _offset: number): void {
    this.bump('drawElements');
  }

  enable(_cap: number): void {
    this.bump('enable');
  }

  disable(_cap: number): void {
    this.bump('disable');
  }

  blendFunc(_sfactor: number, _dfactor: number): void {
    this.bump('blendFunc');
  }

  getError(): number {
    this.bump('getError');
    return this.NO_ERROR;
  }
}

/** The cast every suite needs: the layer's API is typed against the real
 *  context, and FakeGl implements only the slice it exercises. */
export const asGl = (fake: FakeGl): WebGL2RenderingContext =>
  fake as unknown as WebGL2RenderingContext;
