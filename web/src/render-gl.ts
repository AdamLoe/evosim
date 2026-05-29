// WebGL2 renderer. Replaces the Canvas2D path with one instanced "disc"
// draw call covering all creature bodies, trait rings, and highlights, plus
// one fullscreen quad that samples an R8 grass-density texture and one
// LINES draw for the world-bounds frame.
//
// Why: at pop ≈ 2000+, Canvas2D's per-`arc` path overhead (5+ arcs per
// creature) was the dominant frame cost. Instancing collapses that to a
// single draw call. CPU-side frustum culling skips off-screen creatures
// before they touch the GPU; an LOD threshold omits rings when the
// on-screen body radius is < 6 px (they'd be invisible anyway).

import { type Camera, PX_PER_SIZE } from "./render";
import { getSettings } from "./settings";
import { span } from "./perf";
import { CREATURE_STRIDE } from "./sim-bridge";

// v1.6 Wave C: the renderer reads typed-array views over the live SAB slot
// directly (no postMessage payload, no copy). `creatures` is a Float32Array
// view over the stride-8 SoA in the snapshot slot; `grass` is a Uint8Array
// view over the same slot's density region. IDs are interleaved at byte offset
// +24 within each 32-byte creature stride (two raw u32s reinterpreted as f32
// by Rust via `f32::from_bits`) — the highlight pass extracts them inline by
// constructing a Uint32Array view over the creatures' underlying buffer.

// ─── Shader sources ──────────────────────────────────────────────────────

// Unified "disc" program: draws an annulus when v_inner_px > 0, otherwise a
// filled disc. Bodies, trait rings, and the highlight ring all share this.
const DISC_VS = `#version 300 es
precision highp float;
layout(location = 0) in vec2 a_corner;       // unit quad in [-0.5, 0.5]
layout(location = 1) in vec2 a_center;       // world-space center
layout(location = 2) in vec2 a_radii_px;     // (outer, inner) in screen px
layout(location = 3) in vec4 a_color;        // rgba float

uniform vec2 u_viewport;   // CSS px
uniform vec2 u_cam_pos;    // world-space camera center
uniform float u_zoom;      // px per world unit

out vec2 v_local;          // [-1, 1] inside the bounding quad
out vec2 v_radii_px;
out vec4 v_color;

void main() {
  v_radii_px = a_radii_px;
  v_color = a_color;
  v_local = a_corner * 2.0;
  vec2 center_px = (a_center - u_cam_pos) * u_zoom + u_viewport * 0.5;
  vec2 pos_px = center_px + a_corner * 2.0 * a_radii_px.x;
  vec2 ndc = (pos_px / u_viewport) * 2.0 - 1.0;
  ndc.y = -ndc.y;
  gl_Position = vec4(ndc, 0.0, 1.0);
}`;

const DISC_FS = `#version 300 es
precision highp float;
in vec2 v_local;
in vec2 v_radii_px;
in vec4 v_color;
out vec4 out_color;

uniform vec4 u_ring_color;
uniform float u_ring_inner;
uniform float u_ring_outer;

// v1.9.2: annulus branch (trait rings + highlight pass) keeps its original
// flat-fill behavior so UI elements stay crisp. Body branch (inner_px == 0)
// gets AA edge + radial shading + soft outline ring.
void main() {
  float r = length(v_local);
  if (r > 1.0) discard;
  if (v_radii_px.y > 0.0) {
    if (r * v_radii_px.x < v_radii_px.y) discard;
    out_color = v_color;
    return;
  }
  float aa = fwidth(r) * 1.5;
  float alpha = 1.0 - smoothstep(1.0 - aa, 1.0, r);
  float shade = mix(1.15, 0.85, r);
  float ring_t = smoothstep(u_ring_inner - aa, u_ring_inner + aa, r)
               * (1.0 - smoothstep(u_ring_outer - aa, u_ring_outer + aa, r));
  vec3 body = v_color.rgb * shade;
  vec3 col = mix(body, u_ring_color.rgb, ring_t * u_ring_color.a);
  out_color = vec4(col, v_color.a * alpha);
}`;

// Halo program (v1.9.2 Wave 2.5): reuses the disc instance buffer but
// scales the quad to outer_px * 1.8 and renders with additive blending
// for a bloom-like glow. Body color × falloff alpha; clamped to keep
// dense-pop scenes from washing out.
const HALO_VS = `#version 300 es
precision highp float;
layout(location = 0) in vec2 a_corner;
layout(location = 1) in vec2 a_center;
layout(location = 2) in vec2 a_radii_px;
layout(location = 3) in vec4 a_color;

uniform vec2 u_viewport;
uniform vec2 u_cam_pos;
uniform float u_zoom;

out vec2 v_local;
out vec4 v_color;

void main() {
  v_local = a_corner * 2.0;
  v_color = a_color;
  vec2 center_px = (a_center - u_cam_pos) * u_zoom + u_viewport * 0.5;
  // 3.6 = 2.0 (quad half-extent → full) × 1.8 (halo scale).
  vec2 pos_px = center_px + a_corner * 3.6 * a_radii_px.x;
  vec2 ndc = (pos_px / u_viewport) * 2.0 - 1.0;
  ndc.y = -ndc.y;
  gl_Position = vec4(ndc, 0.0, 1.0);
}`;

const HALO_FS = `#version 300 es
precision highp float;
in vec2 v_local;
in vec4 v_color;
out vec4 out_color;

uniform float u_halo_alpha;

void main() {
  // v_local is in [-2, 2] because the quad was scaled by 1.8 in VS.
  // Re-normalize against the bigger quad so r in [0, 1] across the halo.
  float r = length(v_local) / 1.8;
  if (r > 1.0) discard;
  float falloff = pow(1.0 - clamp(r, 0.0, 1.0), 2.0);
  float a = min(falloff * u_halo_alpha, 0.3);
  out_color = vec4(v_color.rgb, a);
}`;

// Grass: one quad spanning the world, samples an R8 density texture
// (v1.5 S6 — was R32F) and fades green by density. Quantization to u8 happens
// Rust-side in the sim worker's `write_snapshot_to`; the shader treats `.r`
// as a normalized f32 in [0, 1] either way.
const GRASS_VS = `#version 300 es
precision highp float;
layout(location = 0) in vec2 a_corner;   // unit quad [0, 1]

uniform vec2 u_viewport;
uniform vec2 u_cam_pos;
uniform float u_zoom;
uniform float u_world_size;

out vec2 v_uv;

void main() {
  v_uv = a_corner;
  vec2 world_pos = a_corner * u_world_size;
  vec2 px_pos = (world_pos - u_cam_pos) * u_zoom + u_viewport * 0.5;
  vec2 ndc = (px_pos / u_viewport) * 2.0 - 1.0;
  ndc.y = -ndc.y;
  gl_Position = vec4(ndc, 0.0, 1.0);
}`;

const GRASS_FS = `#version 300 es
precision highp float;
in vec2 v_uv;
uniform sampler2D u_grass;
uniform float u_opacity;
uniform float u_time;
uniform vec3 u_grass_tint;
out vec4 out_color;

// v1.9.2: base tint is themed (default soft green for Charcoal). A
// low-amplitude procedural wave modulates the apparent density so the
// field looks alive when zoomed in; the wave is multiplicative against
// the sampled density so empty cells stay empty (no painting outside
// grass). The earlier d<=0 discard early-out is preserved
// for that same reason and for fragment-shading cost at high zoom.
void main() {
  float d = clamp(texture(u_grass, v_uv).r, 0.0, 1.0);
  if (d <= 0.0) discard;
  float wave = 0.08 * sin(v_uv.x * 18.0 + u_time * 0.6)
             + 0.06 * sin(v_uv.y * 22.0 - u_time * 0.4);
  float d2 = clamp(d + wave * d, 0.0, 1.0);
  out_color = vec4(u_grass_tint, d2 * u_opacity);
}`;

// World-bounds frame: 4 lines (one per edge).
const FRAME_VS = `#version 300 es
precision highp float;
layout(location = 0) in vec2 a_world;
uniform vec2 u_viewport;
uniform vec2 u_cam_pos;
uniform float u_zoom;
void main() {
  vec2 px_pos = (a_world - u_cam_pos) * u_zoom + u_viewport * 0.5;
  vec2 ndc = (px_pos / u_viewport) * 2.0 - 1.0;
  ndc.y = -ndc.y;
  gl_Position = vec4(ndc, 0.0, 1.0);
}`;

const FRAME_FS = `#version 300 es
precision highp float;
out vec4 out_color;
void main() {
  out_color = vec4(180.0/255.0, 200.0/255.0, 230.0/255.0, 0.25);
}`;

// ─── Renderer state ──────────────────────────────────────────────────────

const FLOATS_PER_INSTANCE = 8; // cx, cy, outer_px, inner_px, r, g, b, a
const INSTANCE_STRIDE_BYTES = FLOATS_PER_INSTANCE * 4;

// Permanent-highlight sentinel; matches HIGHLIGHT_PERMANENT in highlight.ts.
const PERMANENT_EXP = Number.MAX_SAFE_INTEGER;
const HIGHLIGHT_FADE_MS = 1500;

interface GLState {
  gl: WebGL2RenderingContext;
  discProgram: WebGLProgram;
  discU: {
    viewport: WebGLUniformLocation;
    camPos: WebGLUniformLocation;
    zoom: WebGLUniformLocation;
    ringColor: WebGLUniformLocation;
    ringInner: WebGLUniformLocation;
    ringOuter: WebGLUniformLocation;
  };
  discVao: WebGLVertexArrayObject;
  discInstanceBuf: WebGLBuffer;
  haloProgram: WebGLProgram;
  haloU: {
    viewport: WebGLUniformLocation;
    camPos: WebGLUniformLocation;
    zoom: WebGLUniformLocation;
    haloAlpha: WebGLUniformLocation;
  };
  haloVao: WebGLVertexArrayObject;
  grassProgram: WebGLProgram;
  grassU: {
    viewport: WebGLUniformLocation;
    camPos: WebGLUniformLocation;
    zoom: WebGLUniformLocation;
    worldSize: WebGLUniformLocation;
    opacity: WebGLUniformLocation;
    time: WebGLUniformLocation;
    tint: WebGLUniformLocation;
  };
  grassVao: WebGLVertexArrayObject;
  grassTex: WebGLTexture;
  grassTexDim: number;
  frameProgram: WebGLProgram;
  frameU: {
    viewport: WebGLUniformLocation;
    camPos: WebGLUniformLocation;
    zoom: WebGLUniformLocation;
  };
  frameVao: WebGLVertexArrayObject;
  frameBuf: WebGLBuffer;
  // Scratch buffer for packing instance data; grown on demand.
  instanceScratch: Float32Array;
}

let state: GLState | null = null;

function compile(gl: WebGL2RenderingContext, type: number, src: string): WebGLShader {
  const sh = gl.createShader(type);
  if (!sh) throw new Error("createShader returned null");
  gl.shaderSource(sh, src);
  gl.compileShader(sh);
  if (!gl.getShaderParameter(sh, gl.COMPILE_STATUS)) {
    const log = gl.getShaderInfoLog(sh);
    gl.deleteShader(sh);
    throw new Error(`shader compile failed: ${log}\n${src}`);
  }
  return sh;
}

function link(gl: WebGL2RenderingContext, vs: WebGLShader, fs: WebGLShader): WebGLProgram {
  const prog = gl.createProgram();
  if (!prog) throw new Error("createProgram returned null");
  gl.attachShader(prog, vs);
  gl.attachShader(prog, fs);
  gl.linkProgram(prog);
  if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) {
    const log = gl.getProgramInfoLog(prog);
    gl.deleteProgram(prog);
    throw new Error(`program link failed: ${log}`);
  }
  return prog;
}

function mustGet<T>(v: T | null, label: string): T {
  if (v === null) throw new Error(`${label} returned null`);
  return v;
}

function initRenderer(gl: WebGL2RenderingContext): GLState {
  // ─── Disc program ───
  const discProgram = link(
    gl,
    compile(gl, gl.VERTEX_SHADER, DISC_VS),
    compile(gl, gl.FRAGMENT_SHADER, DISC_FS),
  );

  const unitQuad = new Float32Array([
    -0.5, -0.5,
    0.5, -0.5,
    -0.5, 0.5,
    0.5, 0.5,
  ]);

  const discVao = mustGet(gl.createVertexArray(), "createVertexArray");
  gl.bindVertexArray(discVao);
  const quadBuf = mustGet(gl.createBuffer(), "createBuffer");
  gl.bindBuffer(gl.ARRAY_BUFFER, quadBuf);
  gl.bufferData(gl.ARRAY_BUFFER, unitQuad, gl.STATIC_DRAW);
  gl.enableVertexAttribArray(0);
  gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 0, 0);

  const discInstanceBuf = mustGet(gl.createBuffer(), "createBuffer");
  gl.bindBuffer(gl.ARRAY_BUFFER, discInstanceBuf);
  gl.enableVertexAttribArray(1);
  gl.vertexAttribPointer(1, 2, gl.FLOAT, false, INSTANCE_STRIDE_BYTES, 0);
  gl.vertexAttribDivisor(1, 1);
  gl.enableVertexAttribArray(2);
  gl.vertexAttribPointer(2, 2, gl.FLOAT, false, INSTANCE_STRIDE_BYTES, 8);
  gl.vertexAttribDivisor(2, 1);
  gl.enableVertexAttribArray(3);
  gl.vertexAttribPointer(3, 4, gl.FLOAT, false, INSTANCE_STRIDE_BYTES, 16);
  gl.vertexAttribDivisor(3, 1);
  gl.bindVertexArray(null);

  const discU = {
    viewport: mustGet(gl.getUniformLocation(discProgram, "u_viewport"), "u_viewport"),
    camPos: mustGet(gl.getUniformLocation(discProgram, "u_cam_pos"), "u_cam_pos"),
    zoom: mustGet(gl.getUniformLocation(discProgram, "u_zoom"), "u_zoom"),
    ringColor: mustGet(gl.getUniformLocation(discProgram, "u_ring_color"), "u_ring_color"),
    ringInner: mustGet(gl.getUniformLocation(discProgram, "u_ring_inner"), "u_ring_inner"),
    ringOuter: mustGet(gl.getUniformLocation(discProgram, "u_ring_outer"), "u_ring_outer"),
  };
  // Ring stops are constants — set once at link time. Thickness isn't a
  // theme concern; per-theme styling lives in u_ring_color's alpha.
  gl.useProgram(discProgram);
  gl.uniform1f(discU.ringInner, 0.84);
  gl.uniform1f(discU.ringOuter, 0.92);

  // ─── Halo program (Wave 2.5) ───
  const haloProgram = link(
    gl,
    compile(gl, gl.VERTEX_SHADER, HALO_VS),
    compile(gl, gl.FRAGMENT_SHADER, HALO_FS),
  );

  // Halo reuses the same instance attribute layout as disc, but bound to
  // its own VAO so the static unit-quad attribute index can stay (0).
  const haloVao = mustGet(gl.createVertexArray(), "createVertexArray");
  gl.bindVertexArray(haloVao);
  const haloQuadBuf = mustGet(gl.createBuffer(), "createBuffer");
  gl.bindBuffer(gl.ARRAY_BUFFER, haloQuadBuf);
  gl.bufferData(gl.ARRAY_BUFFER, unitQuad, gl.STATIC_DRAW);
  gl.enableVertexAttribArray(0);
  gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 0, 0);
  // Bind the SAME instance buffer as disc — halo reads the body instance
  // stream verbatim and just scales the quad 1.8× in the vertex shader.
  gl.bindBuffer(gl.ARRAY_BUFFER, discInstanceBuf);
  gl.enableVertexAttribArray(1);
  gl.vertexAttribPointer(1, 2, gl.FLOAT, false, INSTANCE_STRIDE_BYTES, 0);
  gl.vertexAttribDivisor(1, 1);
  gl.enableVertexAttribArray(2);
  gl.vertexAttribPointer(2, 2, gl.FLOAT, false, INSTANCE_STRIDE_BYTES, 8);
  gl.vertexAttribDivisor(2, 1);
  gl.enableVertexAttribArray(3);
  gl.vertexAttribPointer(3, 4, gl.FLOAT, false, INSTANCE_STRIDE_BYTES, 16);
  gl.vertexAttribDivisor(3, 1);
  gl.bindVertexArray(null);

  const haloU = {
    viewport: mustGet(gl.getUniformLocation(haloProgram, "u_viewport"), "u_viewport"),
    camPos: mustGet(gl.getUniformLocation(haloProgram, "u_cam_pos"), "u_cam_pos"),
    zoom: mustGet(gl.getUniformLocation(haloProgram, "u_zoom"), "u_zoom"),
    haloAlpha: mustGet(gl.getUniformLocation(haloProgram, "u_halo_alpha"), "u_halo_alpha"),
  };

  // ─── Grass program ───
  const grassProgram = link(
    gl,
    compile(gl, gl.VERTEX_SHADER, GRASS_VS),
    compile(gl, gl.FRAGMENT_SHADER, GRASS_FS),
  );

  const grassQuad = new Float32Array([
    0, 0,
    1, 0,
    0, 1,
    1, 1,
  ]);

  const grassVao = mustGet(gl.createVertexArray(), "createVertexArray");
  gl.bindVertexArray(grassVao);
  const grassQuadBuf = mustGet(gl.createBuffer(), "createBuffer");
  gl.bindBuffer(gl.ARRAY_BUFFER, grassQuadBuf);
  gl.bufferData(gl.ARRAY_BUFFER, grassQuad, gl.STATIC_DRAW);
  gl.enableVertexAttribArray(0);
  gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 0, 0);
  gl.bindVertexArray(null);

  const grassU = {
    viewport: mustGet(gl.getUniformLocation(grassProgram, "u_viewport"), "u_viewport"),
    camPos: mustGet(gl.getUniformLocation(grassProgram, "u_cam_pos"), "u_cam_pos"),
    zoom: mustGet(gl.getUniformLocation(grassProgram, "u_zoom"), "u_zoom"),
    worldSize: mustGet(gl.getUniformLocation(grassProgram, "u_world_size"), "u_world_size"),
    opacity: mustGet(gl.getUniformLocation(grassProgram, "u_opacity"), "u_opacity"),
    time: mustGet(gl.getUniformLocation(grassProgram, "u_time"), "u_time"),
    tint: mustGet(gl.getUniformLocation(grassProgram, "u_grass_tint"), "u_grass_tint"),
  };

  const grassTex = mustGet(gl.createTexture(), "createTexture");
  gl.bindTexture(gl.TEXTURE_2D, grassTex);
  // v1.9.2: bilinear + mipmaps so grass reads as a landscape, not a pixel
  // grid. `gl.generateMipmap` is called after every per-frame sub-image
  // upload below; the cost (~0.5–1 ms at 960×960) is acceptable for the
  // visual win. If perf regresses, drop back to plain `LINEAR`.
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR_MIPMAP_LINEAR);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);

  // ─── Frame program ───
  const frameProgram = link(
    gl,
    compile(gl, gl.VERTEX_SHADER, FRAME_VS),
    compile(gl, gl.FRAGMENT_SHADER, FRAME_FS),
  );

  const frameU = {
    viewport: mustGet(gl.getUniformLocation(frameProgram, "u_viewport"), "u_viewport"),
    camPos: mustGet(gl.getUniformLocation(frameProgram, "u_cam_pos"), "u_cam_pos"),
    zoom: mustGet(gl.getUniformLocation(frameProgram, "u_zoom"), "u_zoom"),
  };

  const frameVao = mustGet(gl.createVertexArray(), "createVertexArray");
  gl.bindVertexArray(frameVao);
  const frameBuf = mustGet(gl.createBuffer(), "createBuffer");
  gl.bindBuffer(gl.ARRAY_BUFFER, frameBuf);
  gl.bufferData(gl.ARRAY_BUFFER, 8 * 4, gl.DYNAMIC_DRAW); // 8 floats placeholder
  gl.enableVertexAttribArray(0);
  gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 0, 0);
  gl.bindVertexArray(null);

  gl.enable(gl.BLEND);
  gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA);

  return {
    gl,
    discProgram, discU, discVao, discInstanceBuf,
    haloProgram, haloU, haloVao,
    grassProgram, grassU, grassVao, grassTex, grassTexDim: 0,
    frameProgram, frameU, frameVao, frameBuf,
    instanceScratch: new Float32Array(4096 * FLOATS_PER_INSTANCE),
  };
}

// Reusable 8-float buffer for the world-bounds frame line endpoints.
const FRAME_VERTS = new Float32Array(8);

// ─── Theme-token readers (v1.9.2) ────────────────────────────────────────
// CSS-var values come back as strings; parse to vec3/vec4 once per frame.
// `getComputedStyle` is on the order of ~50 µs per call — cheap enough at
// 60 fps but we still cache the previous value to avoid re-parsing when
// nothing changed.

function parseRgba(s: string): [number, number, number, number] {
  const m = s.match(/rgba?\(([^)]+)\)/);
  if (!m) return [0, 0, 0, 1];
  const parts = m[1].split(",").map((p) => parseFloat(p.trim()));
  return [
    (parts[0] ?? 0) / 255,
    (parts[1] ?? 0) / 255,
    (parts[2] ?? 0) / 255,
    parts[3] ?? 1,
  ];
}

function parseRgbVec3(s: string): [number, number, number] {
  // Accept "r, g, b" floats in [0,1] (used by --grass-tint) OR a CSS color.
  const trimmed = s.trim();
  if (trimmed.startsWith("rgb")) {
    const rgba = parseRgba(trimmed);
    return [rgba[0], rgba[1], rgba[2]];
  }
  const parts = trimmed.split(",").map((p) => parseFloat(p.trim()));
  return [parts[0] ?? 0, parts[1] ?? 0, parts[2] ?? 0];
}

function readCssVar(name: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

let grassTintCacheKey = "";
let grassTintCacheVal: [number, number, number] = [0.55, 0.85, 0.45];
function readGrassTint(): [number, number, number] {
  const raw = readCssVar("--grass-tint");
  if (raw === grassTintCacheKey) return grassTintCacheVal;
  grassTintCacheKey = raw;
  grassTintCacheVal = raw ? parseRgbVec3(raw) : [0.55, 0.85, 0.45];
  return grassTintCacheVal;
}

let ringColorCacheKey = "";
let ringColorCacheVal: [number, number, number, number] = [0, 0, 0, 0.6];
function readRingColor(): [number, number, number, number] {
  const raw = readCssVar("--creature-ring");
  if (raw === ringColorCacheKey) return ringColorCacheVal;
  ringColorCacheKey = raw;
  ringColorCacheVal = raw ? parseRgba(raw) : [0, 0, 0, 0.6];
  return ringColorCacheVal;
}

let haloColorCacheKey = "";
let haloColorCacheVal: [number, number, number, number] = [0, 0, 0, 0.25];
function readHaloColor(): [number, number, number, number] {
  const raw = readCssVar("--creature-halo");
  if (raw === haloColorCacheKey) return haloColorCacheVal;
  haloColorCacheKey = raw;
  haloColorCacheVal = raw ? parseRgba(raw) : [0, 0, 0, 0.25];
  return haloColorCacheVal;
}

export function renderWorld(
  gl: WebGL2RenderingContext,
  cam: Camera,
  viewW: number,
  viewH: number,
  creatures: Float32Array,
  grass: Uint8Array,
  pop: number,
  world_size: number,
  grass_dim: number,
  highlightMap: Map<number, number>,
  nowMs: number,
): void {
  // v1.6 Wave C: stride is fixed at 8 post-SAB cutover; mismatch indicates
  // Rust/TS const drift on `sim-bridge.ts`'s `CREATURE_STRIDE`.
  const stride = CREATURE_STRIDE;
  if (stride !== 8) {
    throw new Error(
      `creature_stride mismatch: expected 8, got ${stride} ` +
      `(rebuild wasm — Rust/TS const drift)`,
    );
  }

  // v1.7: outer `frame.render_world` bracket. Parent of `.grass` and
  // `.creatures` children; the parent timer is a real measurement so any
  // setup time (viewport / clear) shows up as parent-minus-children overhead
  // instead of disappearing into a rollup.
  const renderWorldSpan = span("frame.render_world");
  try {
    renderWorldImpl(gl, cam, viewW, viewH, creatures, grass, pop, world_size, grass_dim, highlightMap, nowMs);
  } finally {
    renderWorldSpan.close();
  }
}

function renderWorldImpl(
  gl: WebGL2RenderingContext,
  cam: Camera,
  viewW: number,
  viewH: number,
  creatures: Float32Array,
  grass: Uint8Array,
  pop: number,
  world_size: number,
  grass_dim: number,
  highlightMap: Map<number, number>,
  nowMs: number,
): void {
  const stride = CREATURE_STRIDE;
  if (!state) state = initRenderer(gl);
  const s = state;

  // Match physical-pixel viewport to the canvas drawing buffer (DPR-aware).
  gl.viewport(0, 0, gl.drawingBufferWidth, gl.drawingBufferHeight);
  gl.clearColor(0x04 / 255, 0x07 / 255, 0x0b / 255, 1.0);
  gl.clear(gl.COLOR_BUFFER_BIT);

  // ─── Grass ───
  // Skipping the upload + draw entirely when the user toggles grass off is
  // a real perf win at high cell counts (921_600 cells = 921 KB R8 upload
  // per frame + a fullscreen-discard fragment pass).
  // v1.7: bracketed by `frame.render_world.grass` so the parent
  // `frame.render_world` row's overhead column reads correctly.
  if (getSettings().showGrass) {
    const grassSpan = span("frame.render_world.grass");
    try {
      const dim = grass_dim;
      gl.activeTexture(gl.TEXTURE0);
      gl.bindTexture(gl.TEXTURE_2D, s.grassTex);
      if (dim !== s.grassTexDim) {
        gl.texImage2D(gl.TEXTURE_2D, 0, gl.R8, dim, dim, 0, gl.RED, gl.UNSIGNED_BYTE, grass);
        s.grassTexDim = dim;
      } else {
        gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, 0, dim, dim, gl.RED, gl.UNSIGNED_BYTE, grass);
      }
      // v1.9.2: refresh the mip chain after every upload so the bilinear
      // min-filter has current data. Without this, mip levels 1+ retain
      // the previous frame's density and produce flicker on motion.
      gl.generateMipmap(gl.TEXTURE_2D);
      gl.useProgram(s.grassProgram);
      gl.uniform2f(s.grassU.viewport, viewW, viewH);
      gl.uniform2f(s.grassU.camPos, cam.cx, cam.cy);
      gl.uniform1f(s.grassU.zoom, cam.zoom);
      gl.uniform1f(s.grassU.worldSize, world_size);
      gl.uniform1f(s.grassU.opacity, getSettings().grassOpacity);
      gl.uniform1f(s.grassU.time, nowMs / 1000);
      const tint = readGrassTint();
      gl.uniform3f(s.grassU.tint, tint[0], tint[1], tint[2]);
      gl.bindVertexArray(s.grassVao);
      gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
    } finally {
      grassSpan.close();
    }
  }

  // ─── World-bounds frame ───
  {
    const W = world_size;
    FRAME_VERTS[0] = 0; FRAME_VERTS[1] = 0;
    FRAME_VERTS[2] = W; FRAME_VERTS[3] = 0;
    FRAME_VERTS[4] = W; FRAME_VERTS[5] = W;
    FRAME_VERTS[6] = 0; FRAME_VERTS[7] = W;
    gl.useProgram(s.frameProgram);
    gl.uniform2f(s.frameU.viewport, viewW, viewH);
    gl.uniform2f(s.frameU.camPos, cam.cx, cam.cy);
    gl.uniform1f(s.frameU.zoom, cam.zoom);
    gl.bindVertexArray(s.frameVao);
    gl.bindBuffer(gl.ARRAY_BUFFER, s.frameBuf);
    gl.bufferSubData(gl.ARRAY_BUFFER, 0, FRAME_VERTS);
    gl.drawArrays(gl.LINE_LOOP, 0, 4);
  }

  // ─── Creatures (bodies + highlight rings) ───
  // v1.7: combined under `frame.render_world.creatures`. Includes the JS-side
  // frustum cull, instance pack, body draw, and the rare highlight-ring pass.
  const creaturesSpan = span("frame.render_world.creatures");
  try {
  // JS-side frustum cull + GPU instance pack (replaces the Rust-side
  // `pack_render_buffer` which Wave C deleted). Reads creature SoA fields
  // directly from the SAB-backed Float32Array view — no wasm-bindgen
  // boundary, just a contiguous typed-array load.
  if (pop > 0 && cam.zoom > 0) {
    const halfW = (viewW / cam.zoom) * 0.5;
    const halfH = (viewH / cam.zoom) * 0.5;
    // body_radius is read per-creature from slot [i+2]; margin uses the
    // typical max so we cull liberally and still draw straddlers.
    const minX = cam.cx - halfW;
    const maxX = cam.cx + halfW;
    const minY = cam.cy - halfH;
    const maxY = cam.cy + halfH;
    // Grow the scratch buffer if needed (pop can exceed the initial reserve).
    const neededFloats = pop * FLOATS_PER_INSTANCE;
    if (s.instanceScratch.length < neededFloats) {
      s.instanceScratch = new Float32Array(neededFloats * 2);
    }
    const scratch = s.instanceScratch;
    let off = 0;
    for (let i = 0; i < pop; i++) {
      const base = i * stride;
      const x = creatures[base];
      const y = creatures[base + 1];
      const radiusWorld = creatures[base + 2];
      const margin = radiusWorld + 2;
      if (x < minX - margin || x > maxX + margin ||
          y < minY - margin || y > maxY + margin) continue;
      const radiusPx = Math.max(1, radiusWorld * PX_PER_SIZE * cam.zoom);
      scratch[off    ] = x;
      scratch[off + 1] = y;
      scratch[off + 2] = radiusPx;
      scratch[off + 3] = 0; // inner_px (filled disc)
      scratch[off + 4] = creatures[base + 3];
      scratch[off + 5] = creatures[base + 4];
      scratch[off + 6] = creatures[base + 5];
      scratch[off + 7] = 1; // alpha
      off += FLOATS_PER_INSTANCE;
    }
    const bodyCount = (off / FLOATS_PER_INSTANCE) | 0;
    if (bodyCount > 0) {
      // Upload instance data once; both halo and body draws consume the
      // same buffer (halo via its own VAO that re-binds the same buffer).
      gl.bindBuffer(gl.ARRAY_BUFFER, s.discInstanceBuf);
      gl.bufferData(gl.ARRAY_BUFFER, scratch.subarray(0, off), gl.DYNAMIC_DRAW);

      // ─── Halo pass (Wave 2.5) ───
      // Additive blend behind the body draw so bodies paint over their
      // own halo on the leading edge. Skipped at high pop to keep dense
      // crowds from washing the canvas and to save fragment cost.
      const haloColor = readHaloColor();
      if (pop <= 8000 && haloColor[3] > 0.0) {
        gl.useProgram(s.haloProgram);
        gl.uniform2f(s.haloU.viewport, viewW, viewH);
        gl.uniform2f(s.haloU.camPos, cam.cx, cam.cy);
        gl.uniform1f(s.haloU.zoom, cam.zoom);
        gl.uniform1f(s.haloU.haloAlpha, haloColor[3]);
        gl.blendFunc(gl.ONE, gl.ONE);
        gl.bindVertexArray(s.haloVao);
        gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, bodyCount);
        gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA);
      }

      // ─── Body draw ───
      gl.useProgram(s.discProgram);
      gl.uniform2f(s.discU.viewport, viewW, viewH);
      gl.uniform2f(s.discU.camPos, cam.cx, cam.cy);
      gl.uniform1f(s.discU.zoom, cam.zoom);
      const ringColor = readRingColor();
      gl.uniform4f(
        s.discU.ringColor,
        ringColor[0], ringColor[1], ringColor[2], ringColor[3],
      );
      gl.bindVertexArray(s.discVao);
      gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, bodyCount);
    }
  }

  // ─── Highlight rings (rare path) ───
  // Inspector selection + transient highlights produce ≤ 1–2 rings/frame.
  // Wave C: id is interleaved into the creature SoA at byte offset +24
  // (id_lo) / +28 (id_hi) of each 32-byte stride — Rust stores the two u32
  // halves of the stable u64 id via `f32::from_bits`. Build a Uint32Array
  // view over the same backing SAB to read them without conversion cost.
  if (highlightMap.size > 0 && pop > 0) {
    const idView = new Uint32Array(
      creatures.buffer,
      creatures.byteOffset,
      pop * stride * (creatures.BYTES_PER_ELEMENT / 4),
    );
    const scratch = s.instanceScratch;
    let off = 0;
    for (let k = 0; k < pop; k++) {
      const base = k * stride;
      // u32-pair → number via the f64 ladder. The max u64 a v1 session ever
      // mints stays under 2^53 (DECISIONS E.21), so the f64 round-trip is
      // lossless and `Map<number, …>` lookups stay correct.
      const idLo = idView[base + 6];
      const idHi = idView[base + 7];
      const cid = idHi * 4294967296 + idLo;
      const exp = highlightMap.get(cid);
      if (exp === undefined || nowMs >= exp) continue;
      const x = creatures[base];
      const y = creatures[base + 1];
      const radiusWorld = creatures[base + 2];
      const radiusPx = Math.max(1, radiusWorld * PX_PER_SIZE * cam.zoom);
      const alpha = exp >= PERMANENT_EXP - 1
        ? 1.0
        : Math.min(1.0, (exp - nowMs) / HIGHLIGHT_FADE_MS);
      scratch[off    ] = x;
      scratch[off + 1] = y;
      scratch[off + 2] = radiusPx + 4;
      scratch[off + 3] = radiusPx + 2;
      scratch[off + 4] = 255 / 255;
      scratch[off + 5] = 200 / 255;
      scratch[off + 6] = 50 / 255;
      scratch[off + 7] = alpha;
      off += FLOATS_PER_INSTANCE;
    }
    const hCount = (off / FLOATS_PER_INSTANCE) | 0;
    if (hCount > 0) {
      gl.useProgram(s.discProgram);
      gl.uniform2f(s.discU.viewport, viewW, viewH);
      gl.uniform2f(s.discU.camPos, cam.cx, cam.cy);
      gl.uniform1f(s.discU.zoom, cam.zoom);
      gl.bindVertexArray(s.discVao);
      gl.bindBuffer(gl.ARRAY_BUFFER, s.discInstanceBuf);
      gl.bufferData(gl.ARRAY_BUFFER, scratch.subarray(0, off), gl.DYNAMIC_DRAW);
      gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, hCount);
    }
  }
  } finally {
    creaturesSpan.close();
  }
}
