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
import { CREATURE_STRIDE, MAX_POP_FOR_SIM } from "./sim-bridge";

// v1.13 Wave 2: position interpolation between snapshots was removed. The
// main RAF callback now seq-gates paints (FPS ≤ TPS) so painting the same
// snapshot twice — a precondition for lerping — is forbidden. The trail
// pass still draws between the previous and current id-keyed positions,
// but those are now updated once per painted frame from the SoA directly.

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

// Halo program: rim-light glow. A narrow Gaussian band centered just
// outside the body edge (r ≈ 0.55 in halo-quad coords) — peaks at the
// silhouette, falls off in both directions. Reads as a bioluminescent
// edge rather than a cloud around each creature, which is what the
// earlier `pow(1-r,n)` / centered-Gaussian designs converged toward at
// high pop (overlapping clouds → saturated blobs).
//
// Blend mode is `SRC_ALPHA, ONE` (not pure additive) so the alpha gates
// how much each rim adds to the framebuffer — dense clusters don't blow
// out. The pulse animation was dropped because at pop > 1000 it reads
// as flicker, not life.
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
  // v_local stays in [-1, 1] over the bounding quad — same convention as
  // DISC_VS. The FS discards outside r=1 (round inscribed circle).
  v_local = a_corner * 2.0;
  v_color = a_color;
  vec2 center_px = (a_center - u_cam_pos) * u_zoom + u_viewport * 0.5;
  // 4.0 = 2.0 (quad half-extent → full quad width) × 2.0 (halo scale).
  // Halo extends to 2× body radius; combined with Gaussian falloff this
  // gives a soft outer glow that fades to invisible well within the quad.
  vec2 pos_px = center_px + a_corner * 4.0 * a_radii_px.x;
  vec2 ndc = (pos_px / u_viewport) * 2.0 - 1.0;
  ndc.y = -ndc.y;
  gl_Position = vec4(ndc, 0.0, 1.0);
}`;

const HALO_FS = `#version 300 es
precision highp float;
in vec2 v_local;
in vec4 v_color;
out vec4 out_color;

uniform float u_strength;    // peak alpha, theme-driven via --creature-halo

void main() {
  // v_local ∈ [-1, 1] over the halo quad; body occupies r ∈ [0, 0.5]
  // since the halo quad is scaled 2× body radius.
  float r = length(v_local);
  if (r > 1.0) discard;

  // Narrow Gaussian band centered at r = 0.55 (just outside body edge).
  // exp(-d²·90) hits ~0.37 at d=0.105, ~0.01 at d=0.22 — a thin glowing
  // band hugging the silhouette.
  float d = r - 0.55;
  float rim = exp(-d * d * 90.0);

  // Mask the body interior so the glow stays outside the silhouette.
  // Smoothstep over (0.48, 0.53) tracks the body's AA edge.
  float outside = smoothstep(0.48, 0.53, r);

  // Fade toward the halo-quad boundary so the discard at r=1 never
  // shows up as a hard ring.
  float outerFade = 1.0 - smoothstep(0.80, 1.0, r);

  float a = rim * outside * outerFade * u_strength;

  // Tint the glow slightly toward cool blue-white so it reads as
  // bioluminescence rather than just "creature color, but transparent."
  vec3 glow = mix(v_color.rgb, vec3(0.55, 0.75, 1.0), 0.25);
  out_color = vec4(glow, a);
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

// Trail program (v1.9.2 Wave 3): a thin quad-as-line drawn between two
// endpoints per creature, fading alpha toward the trailing edge. One
// instance per creature with both prev and curr positions; pre-allocated
// to the same MAX_POP_FOR_SIM ceiling as bodies.
const TRAIL_VS = `#version 300 es
precision highp float;
layout(location = 0) in vec2 a_corner;        // unit quad in [0, 1]
layout(location = 1) in vec2 a_start;         // world-space trail start
layout(location = 2) in vec2 a_end;           // world-space trail end (= body)
layout(location = 3) in vec4 a_color;         // rgb + per-instance alpha
layout(location = 4) in float a_thickness;    // screen px

uniform vec2 u_viewport;
uniform vec2 u_cam_pos;
uniform float u_zoom;

out float v_t;
out vec4 v_color;

void main() {
  vec2 start_px = (a_start - u_cam_pos) * u_zoom + u_viewport * 0.5;
  vec2 end_px   = (a_end   - u_cam_pos) * u_zoom + u_viewport * 0.5;
  vec2 axis = end_px - start_px;
  float len = max(length(axis), 0.0001);
  vec2 dir = axis / len;
  vec2 perp = vec2(-dir.y, dir.x);
  // a_corner.x ∈ [0, 1] selects along the trail (start → end);
  // a_corner.y ∈ [0, 1] selects across the trail width.
  vec2 pos_px = start_px + dir * (a_corner.x * len)
              + perp * ((a_corner.y - 0.5) * a_thickness);
  v_t = a_corner.x;
  v_color = a_color;
  vec2 ndc = (pos_px / u_viewport) * 2.0 - 1.0;
  ndc.y = -ndc.y;
  gl_Position = vec4(ndc, 0.0, 1.0);
}`;

const TRAIL_FS = `#version 300 es
precision highp float;
in float v_t;     // 0 at trail start (back), 1 at body (front)
in vec4 v_color;
out vec4 out_color;

void main() {
  // Fade alpha toward the trailing edge so the body looks like it's
  // dragging a comet tail.
  out_color = vec4(v_color.rgb, v_color.a * v_t);
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

// v1.9.2 Wave 3: trail instance is [startX, startY, endX, endY, r, g, b, a, thickness].
const TRAIL_FLOATS_PER_INSTANCE = 9;

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
    strength: WebGLUniformLocation;
  };
  haloVao: WebGLVertexArrayObject;
  trailProgram: WebGLProgram;
  trailU: {
    viewport: WebGLUniformLocation;
    camPos: WebGLUniformLocation;
    zoom: WebGLUniformLocation;
  };
  trailVao: WebGLVertexArrayObject;
  trailInstanceBuf: WebGLBuffer;
  // Per-trail instance: [startX, startY, endX, endY, r, g, b, a, thickness] (9 f32).
  trailScratch: Float32Array;
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
  // Ring sits at the actual disc edge (0.96..1.0) and is half the thickness
  // of the v1.9.2 initial values (was 0.84..0.92 — 8% inset and 8% thick).
  gl.uniform1f(discU.ringInner, 0.96);
  gl.uniform1f(discU.ringOuter, 1.0);

  // ─── Halo program ───
  // Soft Gaussian glow with per-creature pulse. Reuses the disc instance
  // buffer; the VS scales the bounding quad 2× the body radius.
  const haloProgram = link(
    gl,
    compile(gl, gl.VERTEX_SHADER, HALO_VS),
    compile(gl, gl.FRAGMENT_SHADER, HALO_FS),
  );
  const haloVao = mustGet(gl.createVertexArray(), "createVertexArray");
  gl.bindVertexArray(haloVao);
  const haloQuadBuf = mustGet(gl.createBuffer(), "createBuffer");
  gl.bindBuffer(gl.ARRAY_BUFFER, haloQuadBuf);
  gl.bufferData(gl.ARRAY_BUFFER, unitQuad, gl.STATIC_DRAW);
  gl.enableVertexAttribArray(0);
  gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 0, 0);
  // Bind the SAME instance buffer as disc — halo reads the body instance
  // stream verbatim and just scales the quad 2× in the vertex shader.
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
    strength: mustGet(gl.getUniformLocation(haloProgram, "u_strength"), "u_strength"),
  };

  // ─── Trail program (Wave 3) ───
  const trailProgram = link(
    gl,
    compile(gl, gl.VERTEX_SHADER, TRAIL_VS),
    compile(gl, gl.FRAGMENT_SHADER, TRAIL_FS),
  );

  const trailVao = mustGet(gl.createVertexArray(), "createVertexArray");
  gl.bindVertexArray(trailVao);
  // Unit quad covering the trail rectangle (x ∈ [0, 1] along the trail,
  // y ∈ [0, 1] across its width — recentered to ±0.5 in the VS).
  const trailQuadBuf = mustGet(gl.createBuffer(), "createBuffer");
  gl.bindBuffer(gl.ARRAY_BUFFER, trailQuadBuf);
  gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([
    0, 0,
    1, 0,
    0, 1,
    1, 1,
  ]), gl.STATIC_DRAW);
  gl.enableVertexAttribArray(0);
  gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 0, 0);

  const trailInstanceBuf = mustGet(gl.createBuffer(), "createBuffer");
  gl.bindBuffer(gl.ARRAY_BUFFER, trailInstanceBuf);
  const trailStrideBytes = TRAIL_FLOATS_PER_INSTANCE * 4;
  gl.enableVertexAttribArray(1);
  gl.vertexAttribPointer(1, 2, gl.FLOAT, false, trailStrideBytes, 0);
  gl.vertexAttribDivisor(1, 1);
  gl.enableVertexAttribArray(2);
  gl.vertexAttribPointer(2, 2, gl.FLOAT, false, trailStrideBytes, 8);
  gl.vertexAttribDivisor(2, 1);
  gl.enableVertexAttribArray(3);
  gl.vertexAttribPointer(3, 4, gl.FLOAT, false, trailStrideBytes, 16);
  gl.vertexAttribDivisor(3, 1);
  gl.enableVertexAttribArray(4);
  gl.vertexAttribPointer(4, 1, gl.FLOAT, false, trailStrideBytes, 32);
  gl.vertexAttribDivisor(4, 1);
  gl.bindVertexArray(null);

  const trailU = {
    viewport: mustGet(gl.getUniformLocation(trailProgram, "u_viewport"), "u_viewport"),
    camPos: mustGet(gl.getUniformLocation(trailProgram, "u_cam_pos"), "u_cam_pos"),
    zoom: mustGet(gl.getUniformLocation(trailProgram, "u_zoom"), "u_zoom"),
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

  // v1.11 (D): grass texture is R32F (raw f32 density per cell, no quantize).
  // Linear filtering on f32 textures requires OES_texture_float_linear; mips
  // would additionally require EXT_color_buffer_float for renderability, so
  // we skip mipmapping and use plain LINEAR. NEAREST is the fallback if the
  // float-linear extension is missing.
  const floatLinearOk = gl.getExtension("OES_texture_float_linear") !== null;
  const grassTex = mustGet(gl.createTexture(), "createTexture");
  gl.bindTexture(gl.TEXTURE_2D, grassTex);
  if (floatLinearOk) {
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
  } else {
    console.warn("[render] OES_texture_float_linear unavailable; grass will use nearest filter");
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
  }
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
    trailProgram, trailU, trailVao, trailInstanceBuf,
    trailScratch: new Float32Array(MAX_POP_FOR_SIM * TRAIL_FLOATS_PER_INSTANCE),
    grassProgram, grassU, grassVao, grassTex, grassTexDim: 0,
    frameProgram, frameU, frameVao, frameBuf,
    instanceScratch: new Float32Array(4096 * FLOATS_PER_INSTANCE),
  };
}

// ─── Per-paint trail state (v1.13 Wave 2) ────────────────────────────────
// Two id-keyed Maps hold the previous and current snapshot positions. The
// trail pass draws a fading segment from prev to curr; on every painted
// frame we swap the maps and refill `currById` from the SoA. The renderer
// no longer interpolates body positions between snapshots — that required
// painting duplicates, which the main-thread seq-gate now forbids.

interface XY { x: number; y: number; }
let prevById: Map<number, XY> = new Map();
let currById: Map<number, XY> = new Map();

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
let grassTintCacheVal: [number, number, number] = [0.08, 0.32, 0.08];
function readGrassTint(): [number, number, number] {
  const raw = readCssVar("--grass-tint");
  if (raw === grassTintCacheKey) return grassTintCacheVal;
  grassTintCacheKey = raw;
  grassTintCacheVal = raw ? parseRgbVec3(raw) : [0.08, 0.32, 0.08];
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

// --creature-halo is consumed as an rgba where only the alpha matters —
// the halo paints in the creature's own color, not the token color.
let haloStrengthCacheKey = "";
let haloStrengthCacheVal = 0.18;
function readHaloStrength(): number {
  const raw = readCssVar("--creature-halo");
  if (raw === haloStrengthCacheKey) return haloStrengthCacheVal;
  haloStrengthCacheKey = raw;
  haloStrengthCacheVal = raw ? parseRgba(raw)[3] : 0.18;
  return haloStrengthCacheVal;
}

export function renderWorld(
  gl: WebGL2RenderingContext,
  cam: Camera,
  viewW: number,
  viewH: number,
  creatures: Float32Array,
  grass: Float32Array,
  pop: number,
  world_size: number,
  grass_dim: number,
  highlightMap: Map<number, number>,
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
    renderWorldImpl(
      gl, cam, viewW, viewH, creatures, grass, pop, world_size, grass_dim,
      highlightMap,
    );
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
  grass: Float32Array,
  pop: number,
  world_size: number,
  grass_dim: number,
  highlightMap: Map<number, number>,
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
      // v1.11 (D): grass is now f32 per cell, uploaded as R32F. WebGL2
      // supports R32F natively. No more (d * 255) as u8 quantize on the
      // sim-worker side — the texture sees the exact density value.
      if (dim !== s.grassTexDim) {
        gl.texImage2D(gl.TEXTURE_2D, 0, gl.R32F, dim, dim, 0, gl.RED, gl.FLOAT, grass);
        s.grassTexDim = dim;
      } else {
        gl.texSubImage2D(gl.TEXTURE_2D, 0, 0, 0, dim, dim, gl.RED, gl.FLOAT, grass);
      }
      gl.useProgram(s.grassProgram);
      gl.uniform2f(s.grassU.viewport, viewW, viewH);
      gl.uniform2f(s.grassU.camPos, cam.cx, cam.cy);
      gl.uniform1f(s.grassU.zoom, cam.zoom);
      gl.uniform1f(s.grassU.worldSize, world_size);
      gl.uniform1f(s.grassU.opacity, getSettings().grassOpacity);
      gl.uniform1f(s.grassU.time, performance.now() / 1000);
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

    // v1.13 Wave 2: the main RAF callback seq-gates paints, so each call
    // here corresponds to a new snapshot. Rotate the id-keyed position
    // maps (pointer swap, no allocation) so `prevById` holds the
    // previous-paint positions for the trail pass; bodies render at the
    // raw SoA position (no interpolation).
    const idView = new Uint32Array(
      creatures.buffer,
      creatures.byteOffset,
      pop * stride * (creatures.BYTES_PER_ELEMENT / 4),
    );
    {
      const tmp = prevById;
      prevById = currById;
      currById = tmp;
      currById.clear();
      for (let i = 0; i < pop; i++) {
        const base = i * stride;
        const idLo = idView[base + 6];
        const idHi = idView[base + 7];
        const cid = idHi * 4294967296 + idLo;
        currById.set(cid, { x: creatures[base], y: creatures[base + 1] });
      }
    }

    // Trail scratch: parallel to bodies, one entry per creature that has
    // both prev and curr and is actually moving.
    const trailScratch = s.trailScratch;
    let trailOff = 0;
    const enableTrails = pop <= 12000;

    let off = 0;
    for (let i = 0; i < pop; i++) {
      const base = i * stride;
      const cx_raw = creatures[base];
      const cy_raw = creatures[base + 1];
      const radiusWorld = creatures[base + 2];
      // Look up prev by id for the trail pass; the body itself draws at
      // the raw SoA position.
      const idLo = idView[base + 6];
      const idHi = idView[base + 7];
      const cid = idHi * 4294967296 + idLo;
      const prev = prevById.get(cid);
      const x = cx_raw;
      const y = cy_raw;
      const margin = radiusWorld + 2;
      if (x < minX - margin || x > maxX + margin ||
          y < minY - margin || y > maxY + margin) continue;
      const radiusPx = Math.max(1, radiusWorld * PX_PER_SIZE * cam.zoom);
      let cr = creatures[base + 3];
      let cg = creatures[base + 4];
      let cb = creatures[base + 5];
      // Brightness floor: creatures whose action-EMA leaves them
      // near-black are invisible against the canvas. Lift the max
      // channel to ~125/255 while preserving hue; true zero (no
      // history at all) becomes mid-gray.
      const COLOR_FLOOR = 125 / 255;
      const maxC = Math.max(cr, cg, cb);
      if (maxC < COLOR_FLOOR) {
        if (maxC < 1e-4) {
          cr = cg = cb = COLOR_FLOOR;
        } else {
          const scale = COLOR_FLOOR / maxC;
          cr *= scale; cg *= scale; cb *= scale;
        }
      }
      scratch[off    ] = x;
      scratch[off + 1] = y;
      scratch[off + 2] = radiusPx;
      scratch[off + 3] = 0; // inner_px (filled disc)
      scratch[off + 4] = cr;
      scratch[off + 5] = cg;
      scratch[off + 6] = cb;
      scratch[off + 7] = 1; // alpha
      off += FLOATS_PER_INSTANCE;

      // Trail: only when we have a prev and the creature actually moved.
      // Start = prev (one paint behind), end = current body position.
      // Length = the inter-paint displacement; the FS fades alpha from 0
      // at the back to v_color.a at the front, producing a comet tail.
      // Thickness scales with body radius so big creatures get visible
      // tails; min 2 px so small ones aren't lost at low zoom.
      if (enableTrails && prev !== undefined) {
        const dx = cx_raw - prev.x;
        const dy = cy_raw - prev.y;
        if (dx * dx + dy * dy > 0.04) {
          trailScratch[trailOff    ] = prev.x;
          trailScratch[trailOff + 1] = prev.y;
          trailScratch[trailOff + 2] = x;
          trailScratch[trailOff + 3] = y;
          trailScratch[trailOff + 4] = cr;
          trailScratch[trailOff + 5] = cg;
          trailScratch[trailOff + 6] = cb;
          trailScratch[trailOff + 7] = 0.6;
          trailScratch[trailOff + 8] = Math.max(2, radiusPx * 0.6);
          trailOff += TRAIL_FLOATS_PER_INSTANCE;
        }
      }
    }
    const bodyCount = (off / FLOATS_PER_INSTANCE) | 0;
    const trailCount = (trailOff / TRAIL_FLOATS_PER_INSTANCE) | 0;
    if (bodyCount > 0) {
      gl.bindBuffer(gl.ARRAY_BUFFER, s.discInstanceBuf);
      gl.bufferData(gl.ARRAY_BUFFER, scratch.subarray(0, off), gl.DYNAMIC_DRAW);

      // ─── Halo pass ───
      // Soft Gaussian glow with per-creature breathing pulse. Drawn under
      // bodies (and under trails) with additive blending. Skipped at high
      // pop to avoid overdraw cost + visual mush.
      const haloStrength = readHaloStrength();
      if (pop <= 8000 && haloStrength > 0.0) {
        gl.useProgram(s.haloProgram);
        gl.uniform2f(s.haloU.viewport, viewW, viewH);
        gl.uniform2f(s.haloU.camPos, cam.cx, cam.cy);
        gl.uniform1f(s.haloU.zoom, cam.zoom);
        gl.uniform1f(s.haloU.strength, haloStrength);
        gl.blendFunc(gl.SRC_ALPHA, gl.ONE);
        gl.bindVertexArray(s.haloVao);
        gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, bodyCount);
        gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA);
      }

      // ─── Trail pass (Wave 3) ───
      // Drawn under the body so the body always paints over its own trail's
      // leading edge. Alpha blend (the global default), with per-fragment
      // alpha fade toward the trailing tip.
      if (trailCount > 0) {
        gl.bindBuffer(gl.ARRAY_BUFFER, s.trailInstanceBuf);
        gl.bufferData(
          gl.ARRAY_BUFFER,
          trailScratch.subarray(0, trailOff),
          gl.DYNAMIC_DRAW,
        );
        gl.useProgram(s.trailProgram);
        gl.uniform2f(s.trailU.viewport, viewW, viewH);
        gl.uniform2f(s.trailU.camPos, cam.cx, cam.cy);
        gl.uniform1f(s.trailU.zoom, cam.zoom);
        gl.bindVertexArray(s.trailVao);
        gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, trailCount);
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
    const nowMs = performance.now();
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
