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

/**
 * v1.6 Wave B: the renderer consumes a snapshot received via postMessage from
 * the sim worker rather than calling into wasm directly. Carries the same
 * fields previously exposed by `WorldHandle` getters that the renderer used.
 *
 * `creatures` is the stride-8 SoA from `creatures_buffer`; `ids` is the
 * index-aligned `creature_ids_buffer`; `grass` is the u8-quantized density.
 */
export interface SimSnapshot {
  creatures: Float32Array;
  ids: Float64Array;
  grass: Uint8Array;
  pop: number;
  world_size: number;
  grass_dim: number;
}

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

void main() {
  float r_norm = length(v_local);
  if (r_norm > 1.0) discard;
  if (v_radii_px.y > 0.0 && r_norm * v_radii_px.x < v_radii_px.y) discard;
  out_color = v_color;
}`;

// Grass: one quad spanning the world, samples an R8 density texture
// (v1.5 S6 — was R32F) and fades green by density. Quantization to u8 happens
// Rust-side via `grass_buffer_u8`; the shader treats `.r` as a normalized
// f32 in [0, 1] either way.
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
out vec4 out_color;

// Fade from white at density=1.0 to the dark background as density → 0.
// Alpha tracks density so under-blend leaves the (near-black) clear color
// showing through where grass is thin; at full density it's solid white.
// u_opacity scales the whole alpha range so the user can dial grass back
// without affecting density semantics.
void main() {
  float d = clamp(texture(u_grass, v_uv).r, 0.0, 1.0);
  if (d <= 0.0) discard;
  vec3 base = vec3(1.0, 1.0, 1.0);
  out_color = vec4(base, d * u_opacity);
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
  };
  discVao: WebGLVertexArrayObject;
  discInstanceBuf: WebGLBuffer;
  grassProgram: WebGLProgram;
  grassU: {
    viewport: WebGLUniformLocation;
    camPos: WebGLUniformLocation;
    zoom: WebGLUniformLocation;
    worldSize: WebGLUniformLocation;
    opacity: WebGLUniformLocation;
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
  };

  const grassTex = mustGet(gl.createTexture(), "createTexture");
  gl.bindTexture(gl.TEXTURE_2D, grassTex);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
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
    grassProgram, grassU, grassVao, grassTex, grassTexDim: 0,
    frameProgram, frameU, frameVao, frameBuf,
    instanceScratch: new Float32Array(4096 * FLOATS_PER_INSTANCE),
  };
}

// Reusable 8-float buffer for the world-bounds frame line endpoints.
const FRAME_VERTS = new Float32Array(8);

export function renderWorld(
  gl: WebGL2RenderingContext,
  cam: Camera,
  viewW: number,
  viewH: number,
  snapshot: SimSnapshot,
  highlightMap: Map<number, number>,
  nowMs: number,
): void {
  // v1.6 Wave B: stride is fixed at 8 post-sim-worker. The Wave A1 tolerant
  // guard (`6 || 8`) is retired here — Stage 1 onward, the snapshot SoA always
  // ships the stride-8 layout. Mismatch indicates Rust/TS const drift.
  const stride = CREATURE_STRIDE;
  if (stride !== 8) {
    throw new Error(
      `creature_stride mismatch: expected 8, got ${stride} ` +
      `(rebuild wasm — Rust/TS const drift)`,
    );
  }

  if (!state) state = initRenderer(gl);
  const s = state;

  // Match physical-pixel viewport to the canvas drawing buffer (DPR-aware).
  gl.viewport(0, 0, gl.drawingBufferWidth, gl.drawingBufferHeight);
  gl.clearColor(0x04 / 255, 0x07 / 255, 0x0b / 255, 1.0);
  gl.clear(gl.COLOR_BUFFER_BIT);

  // ─── Grass ───
  // Skipping the upload + draw entirely when the user toggles grass off is
  // a real perf win at high cell counts (230_400 cells = 230 KB R8 upload
  // per frame + a fullscreen-discard fragment pass).
  if (getSettings().showGrass) {
    const dim = snapshot.grass_dim;
    // The grass typed-array is now a direct postMessage'd Uint8Array (no
    // wasm-bindgen quantize+copy here — that runs in the sim worker). The
    // span name is retained so the existing perf-tree comparison still works.
    const readSpan = span("grass.upload.read");
    const density = snapshot.grass;
    readSpan.close();
    const gpuSpan = span("grass.upload.gpu");
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, s.grassTex);
    if (dim !== s.grassTexDim) {
      gl.texImage2D(gl.TEXTURE_2D, 0, gl.R8, dim, dim, 0, gl.RED, gl.UNSIGNED_BYTE, density);
      s.grassTexDim = dim;
    } else {
      gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, 0, dim, dim, gl.RED, gl.UNSIGNED_BYTE, density);
    }
    gpuSpan.close();
    gl.useProgram(s.grassProgram);
    gl.uniform2f(s.grassU.viewport, viewW, viewH);
    gl.uniform2f(s.grassU.camPos, cam.cx, cam.cy);
    gl.uniform1f(s.grassU.zoom, cam.zoom);
    gl.uniform1f(s.grassU.worldSize, snapshot.world_size);
    gl.uniform1f(s.grassU.opacity, getSettings().grassOpacity);
    gl.bindVertexArray(s.grassVao);
    gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
  }

  // ─── World-bounds frame ───
  {
    const W = snapshot.world_size;
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

  // ─── Creatures (bodies) ───
  // JS-side frustum cull + GPU instance pack (replaces Rust-side
  // pack_render_buffer which ran inside the world). With the sim now in a
  // worker, there's no extra wasm-bindgen boundary to skip — we just iterate
  // the postMessage'd Float32Array directly.
  const creatures = snapshot.creatures;
  const pop = snapshot.pop;
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
      gl.useProgram(s.discProgram);
      gl.uniform2f(s.discU.viewport, viewW, viewH);
      gl.uniform2f(s.discU.camPos, cam.cx, cam.cy);
      gl.uniform1f(s.discU.zoom, cam.zoom);
      gl.bindVertexArray(s.discVao);
      gl.bindBuffer(gl.ARRAY_BUFFER, s.discInstanceBuf);
      gl.bufferData(gl.ARRAY_BUFFER, scratch.subarray(0, off), gl.DYNAMIC_DRAW);
      gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, bodyCount);
    }
  }

  // ─── Highlight rings (rare path) ───
  // Inspector selection + transient highlights produce ≤ 1–2 rings/frame.
  if (highlightMap.size > 0) {
    const ids = snapshot.ids;
    // Highlight pack lives in its own scratch slice — reuse instanceScratch
    // but write after the body pack (or alone if no bodies). Simplest: reuse
    // a small fixed scratch since highlights are ≤ ~8 per frame.
    const scratch = s.instanceScratch;
    let off = 0;
    for (let k = 0; k < ids.length; k++) {
      const cid = ids[k];
      const exp = highlightMap.get(cid);
      if (exp === undefined || nowMs >= exp) continue;
      const base = k * stride;
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
}
