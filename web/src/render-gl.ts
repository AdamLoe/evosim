// WebGL2 renderer. Replaces the Canvas2D path with one instanced "disc"
// draw call covering all creature bodies, trait rings, and highlights, plus
// one fullscreen quad that samples an R32F grass-density texture and one
// LINES draw for the world-bounds frame.
//
// Why: at pop ≈ 2000+, Canvas2D's per-`arc` path overhead (5+ arcs per
// creature) was the dominant frame cost. Instancing collapses that to a
// single draw call. CPU-side frustum culling skips off-screen creatures
// before they touch the GPU; an LOD threshold omits rings when the
// on-screen body radius is < 6 px (they'd be invisible anyway).

import type { WorldHandle } from "../wasm/evosim";
import { type Camera, PX_PER_SIZE } from "./render";

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

// Grass: one quad spanning the world, samples an R32F density texture and
// emits uniform green wherever density > 0. (M2 dropped per-cell alpha
// modulation; cells are binary on/off.)
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
out vec4 out_color;

void main() {
  float d = texture(u_grass, v_uv).r;
  if (d <= 0.0) discard;
  out_color = vec4(60.0/255.0, 180.0/255.0, 60.0/255.0, 0.55);
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

// Ring drawing order (innermost → outermost) and colors. Match the v6 §B
// spec previously encoded in RING_COLORS in render.ts.
const RING_EYE = [1.0, 1.0, 1.0, 0.85] as const;
const RING_MOVE = [240 / 255, 220 / 255, 80 / 255, 0.85] as const;
const RING_MOUTH = [255 / 255, 80 / 255, 80 / 255, 0.85] as const;
const RING_ARMOR = [200 / 255, 200 / 255, 210 / 255, 0.85] as const;
const RING_STEP = 1.5;
// LOD threshold: below this on-screen body radius (px), skip trait rings.
const RING_LOD_RADIUS_PX = 6;
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
  world: WorldHandle,
  stride: number,
  ids: Float64Array,
  highlightMap: Map<number, number>,
  nowMs: number,
): void {
  // Mandatory stride guard: mismatch corrupts the instance pack loop.
  if (stride !== 10) {
    throw new Error(
      `creature_stride mismatch: expected 10, got ${stride} ` +
      `(Rust wasm_api.rs::creature_stride and web/src/render-gl.ts must agree)`,
    );
  }

  if (!state) state = initRenderer(gl);
  const s = state;

  // Match physical-pixel viewport to the canvas drawing buffer (DPR-aware).
  gl.viewport(0, 0, gl.drawingBufferWidth, gl.drawingBufferHeight);
  gl.clearColor(0x04 / 255, 0x07 / 255, 0x0b / 255, 1.0);
  gl.clear(gl.COLOR_BUFFER_BIT);

  // ─── Grass ───
  {
    const dim = world.grass_dim;
    const density = world.grass_buffer() as unknown as Float32Array;
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, s.grassTex);
    if (dim !== s.grassTexDim) {
      gl.texImage2D(gl.TEXTURE_2D, 0, gl.R32F, dim, dim, 0, gl.RED, gl.FLOAT, density);
      s.grassTexDim = dim;
    } else {
      gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, 0, dim, dim, gl.RED, gl.FLOAT, density);
    }
    gl.useProgram(s.grassProgram);
    gl.uniform2f(s.grassU.viewport, viewW, viewH);
    gl.uniform2f(s.grassU.camPos, cam.cx, cam.cy);
    gl.uniform1f(s.grassU.zoom, cam.zoom);
    gl.uniform1f(s.grassU.worldSize, world.world_size);
    gl.bindVertexArray(s.grassVao);
    gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
  }

  // ─── World-bounds frame ───
  {
    const W = world.world_size;
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

  // ─── Creatures (bodies + rings + highlights) ───
  const data = world.creatures_buffer() as unknown as Float32Array;
  const creatureCount = (data.length / stride) | 0;
  if (creatureCount === 0) return;

  // Worst case: 1 body + 4 rings + 1 highlight = 6 instances per creature.
  const maxFloats = creatureCount * 6 * FLOATS_PER_INSTANCE;
  if (s.instanceScratch.length < maxFloats) {
    s.instanceScratch = new Float32Array(Math.max(maxFloats, s.instanceScratch.length * 2));
  }
  const buf = s.instanceScratch;
  let off = 0;

  // Frustum cull bounds in world units (with margin for ring/highlight overhang).
  const halfW = (viewW / cam.zoom) * 0.5;
  const halfH = (viewH / cam.zoom) * 0.5;
  const margin = 8;
  const minX = cam.cx - halfW - margin;
  const maxX = cam.cx + halfW + margin;
  const minY = cam.cy - halfH - margin;
  const maxY = cam.cy + halfH + margin;
  const hasHighlights = highlightMap.size > 0;

  for (let i = 0; i < data.length; i += stride) {
    const x = data[i];
    const y = data[i + 1];
    if (x < minX || x > maxX || y < minY || y > maxY) continue;
    const radiusWorld = data[i + 2];
    const r = data[i + 3];
    const g = data[i + 4];
    const b = data[i + 5];
    const flagEye   = data[i + 6] > 0.5;
    const flagMove  = data[i + 7] > 0.5;
    const flagMouth = data[i + 8] > 0.5;
    const flagArmor = data[i + 9] > 0.5;

    const radiusPx = Math.max(1, radiusWorld * PX_PER_SIZE * cam.zoom);

    // Body
    buf[off    ] = x;
    buf[off + 1] = y;
    buf[off + 2] = radiusPx;
    buf[off + 3] = 0;
    buf[off + 4] = r;
    buf[off + 5] = g;
    buf[off + 6] = b;
    buf[off + 7] = 1.0;
    off += FLOATS_PER_INSTANCE;

    // Trait rings (LOD: skip when on-screen body is tiny).
    if (radiusPx >= RING_LOD_RADIUS_PX) {
      let ringR = radiusPx - 1;
      // Inline ring packer; manual unrolling avoids a hot-path closure.
      if (flagEye && ringR >= 1) {
        buf[off    ] = x; buf[off + 1] = y;
        buf[off + 2] = ringR + 0.5; buf[off + 3] = ringR - 0.5;
        buf[off + 4] = RING_EYE[0]; buf[off + 5] = RING_EYE[1];
        buf[off + 6] = RING_EYE[2]; buf[off + 7] = RING_EYE[3];
        off += FLOATS_PER_INSTANCE;
        ringR -= RING_STEP;
      }
      if (flagMove && ringR >= 1) {
        buf[off    ] = x; buf[off + 1] = y;
        buf[off + 2] = ringR + 0.5; buf[off + 3] = ringR - 0.5;
        buf[off + 4] = RING_MOVE[0]; buf[off + 5] = RING_MOVE[1];
        buf[off + 6] = RING_MOVE[2]; buf[off + 7] = RING_MOVE[3];
        off += FLOATS_PER_INSTANCE;
        ringR -= RING_STEP;
      }
      if (flagMouth && ringR >= 1) {
        buf[off    ] = x; buf[off + 1] = y;
        buf[off + 2] = ringR + 0.5; buf[off + 3] = ringR - 0.5;
        buf[off + 4] = RING_MOUTH[0]; buf[off + 5] = RING_MOUTH[1];
        buf[off + 6] = RING_MOUTH[2]; buf[off + 7] = RING_MOUTH[3];
        off += FLOATS_PER_INSTANCE;
        ringR -= RING_STEP;
      }
      if (flagArmor && ringR >= 1) {
        buf[off    ] = x; buf[off + 1] = y;
        buf[off + 2] = ringR + 0.5; buf[off + 3] = ringR - 0.5;
        buf[off + 4] = RING_ARMOR[0]; buf[off + 5] = RING_ARMOR[1];
        buf[off + 6] = RING_ARMOR[2]; buf[off + 7] = RING_ARMOR[3];
        off += FLOATS_PER_INSTANCE;
      }
    }

    // Highlight ring (always drawn regardless of LOD).
    if (hasHighlights) {
      const idx = (i / stride) | 0;
      const cid = ids[idx];
      const exp = highlightMap.get(cid);
      if (exp !== undefined && nowMs < exp) {
        const alpha = exp >= PERMANENT_EXP - 1
          ? 1.0
          : Math.min(1.0, (exp - nowMs) / HIGHLIGHT_FADE_MS);
        buf[off    ] = x;
        buf[off + 1] = y;
        buf[off + 2] = radiusPx + 4;
        buf[off + 3] = radiusPx + 2;
        buf[off + 4] = 255 / 255;
        buf[off + 5] = 200 / 255;
        buf[off + 6] = 50 / 255;
        buf[off + 7] = alpha;
        off += FLOATS_PER_INSTANCE;
      }
    }
  }

  const instanceCount = (off / FLOATS_PER_INSTANCE) | 0;
  if (instanceCount === 0) return;

  gl.useProgram(s.discProgram);
  gl.uniform2f(s.discU.viewport, viewW, viewH);
  gl.uniform2f(s.discU.camPos, cam.cx, cam.cy);
  gl.uniform1f(s.discU.zoom, cam.zoom);
  gl.bindVertexArray(s.discVao);
  gl.bindBuffer(gl.ARRAY_BUFFER, s.discInstanceBuf);
  // Orphan + upload via bufferData to avoid stalling on the previous frame's draws.
  gl.bufferData(gl.ARRAY_BUFFER, buf.subarray(0, off), gl.DYNAMIC_DRAW);
  gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, instanceCount);
}
