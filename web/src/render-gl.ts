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
import { CREATURE_STRIDE, MAX_POP_FOR_SIM, GRASS_LOD_BUDGET_AXIS, type WindowMetadata } from "./sim-bridge";

// v2.0.3 Stream 2c: grass cell size in world units (matches Rust GRASS_CELL_SIZE = 5.0).
// Used to derive the UV transform from the clipmap window metadata.
const GRASS_CELL_SIZE = 5.0;

// v1.13 Wave 2: position interpolation between snapshots was removed. The
// main RAF callback now seq-gates paints (FPS ≤ TPS) so painting the same
// snapshot twice — a precondition for lerping — is forbidden. The trail
// pass still draws between the previous and current id-keyed positions,
// but those are now updated once per painted frame from the SoA directly.

// v1.6 Wave C: the renderer reads typed-array views over the live SAB slot
// directly (no postMessage payload, no copy). `creatures` is a Float32Array
// view over the stride-8 SoA in the snapshot slot; `grass` is a Uint8Array
// view over the same slot's density region.
//
// v2.0 Wave 2a/2b creature lane order (8 f32-lanes / 32 B per record):
//   [0] x (f32)  [1] y (f32)  [2] radius (f32, body_size-derived)
//   [3] color_u32 (RGBA8 LE, A=255)  [4] id_lo (u32)  [5] id_hi (u32)
//   [6] packed_u32 (flash_tag bits 0..2, flash_ticks bits 3..6,
//       species_id bits 7..22 — 0 single-pool)  [7] pad
// color_u32 / id halves / packed_u32 are raw u32 bits; the loops build a
// Uint32Array view over the creatures' backing buffer to read them directly.

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
// v2.0 Wave 1c: a NEGATIVE inner_px is the "flat point" sentinel — a
// sub-pixel creature at survey zoom draws as a solid 1px-ish dot in its
// display color with no disc shading or AA, so faction territories stay
// visible when zoomed all the way out.
void main() {
  float r = length(v_local);
  if (r > 1.0) discard;
  if (v_radii_px.y < 0.0) {
    out_color = v_color;
    return;
  }
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

// Biome layer (v2.0 Wave 1b): a fullscreen world-quad drawn UNDER the grass.
// Samples an R8 biome-id texture (one u8 `Biome` tag per grass cell: 0=Plains,
// 1=Water, 2=Desert) with NEAREST filtering so cells stay flat-colored, and
// maps each id to a flat biome color. The grass density layer brightens on top.
//
// v2.0.3 Stream 2d: the biome texture is now a windowed clipmap (same window as
// the grass channel). u_uv_offset + u_uv_scale map the world-space quad UV into
// the window sub-region of the budget² texture — IDENTICAL formula to GRASS_VS.
// This keeps biome tint locked to the grass window at every LOD level.
const BIOME_VS = `#version 300 es
precision highp float;
layout(location = 0) in vec2 a_corner;   // unit quad [0, 1]

uniform vec2 u_viewport;
uniform vec2 u_cam_pos;
uniform float u_zoom;
uniform float u_world_size;
// v2.0.3 Stream 2d: clipmap UV transform — same derivation as GRASS_VS.
uniform vec2 u_uv_scale;
uniform vec2 u_uv_offset;

out vec2 v_uv;

void main() {
  v_uv = a_corner * u_uv_scale + u_uv_offset;
  vec2 world_pos = a_corner * u_world_size;
  vec2 px_pos = (world_pos - u_cam_pos) * u_zoom + u_viewport * 0.5;
  vec2 ndc = (px_pos / u_viewport) * 2.0 - 1.0;
  ndc.y = -ndc.y;
  gl_Position = vec4(ndc, 0.0, 1.0);
}`;

const BIOME_FS = `#version 300 es
precision highp float;
in vec2 v_uv;
uniform sampler2D u_biome;   // R8 biome id, sampled NEAREST
uniform vec3 u_plains;
uniform vec3 u_water;
uniform vec3 u_desert;
out vec4 out_color;

void main() {
  // R8 unorm: id stored as (biome/255). Recover the integer tag.
  float id = floor(texture(u_biome, v_uv).r * 255.0 + 0.5);
  vec3 col = u_plains;
  if (id > 1.5) col = u_desert;       // 2 = Desert
  else if (id > 0.5) col = u_water;   // 1 = Water
  out_color = vec4(col, 1.0);
}`;

// Grass: one quad spanning the world, samples an R8 density texture and
// brightens green by density. Quantization to u8 (0..255) happens Rust-side
// in `quantize_grass_into`; R8 unorm sampling returns `.r` already normalized
// to [0, 1], so the shader needs no extra scaling.
// v2.0.3 Stream 2c: u_uv_offset + u_uv_scale map the world-space quad UV into
// the clipmap window sub-region of the budget² texture.
// Derivation: budget_uv = a_corner * u_uv_scale + u_uv_offset where
//   u_uv_scale  = world_size / (GRASS_CELL_SIZE * 2^mip_level * BUDGET_AXIS)
//   u_uv_offset = -vec2(win_origin_x, win_origin_y) / BUDGET_AXIS
// u_lod_blend is reserved for future trilinear blending (always 0.0 now).
const GRASS_VS = `#version 300 es
precision highp float;
layout(location = 0) in vec2 a_corner;   // unit quad [0, 1]

uniform vec2 u_viewport;
uniform vec2 u_cam_pos;
uniform float u_zoom;
uniform float u_world_size;
// v2.0.3 Stream 2c: clipmap UV transform uniforms.
uniform vec2 u_uv_scale;
uniform vec2 u_uv_offset;

out vec2 v_uv;

void main() {
  // UV into the budget² texture: maps world fraction → window sub-region.
  v_uv = a_corner * u_uv_scale + u_uv_offset;
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
// v2.0.3 Stream 2c: reserved for future trilinear LOD blending (always 0.0 now).
uniform float u_lod_blend;
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
  // u_lod_blend reserved: always 0.0 in this wave (nearest-mip only).
  out_color = vec4(u_grass_tint, d2 * u_opacity * (1.0 - u_lod_blend * 0.0));
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

// v2.0 Wave 2b: action ring-flash. The Rust packs a transient FlashTag (0..5)
// + a ticks-remaining countdown into the creature snapshot's packed_u32 lane.
// When flash_ticks > 0 we draw a transient outer ring in the tag's color, on
// top of the body. This is distinct from (and coexists with) the inspector
// selection ring (the yellow highlight pass below). The tag→RGB legend (these
// RGBs are owned here, the visual-identity flags):
//   1 Born=teal, 2 Grazed=green, 3 Attacked=yellow, 4 CreatedChild=blue,
//   5 Killed=red. Index 0 (None) is never drawn. Matches FlashTag in
//   src/creature.rs.
const FLASH_TICKS_MAX = 5; // src/creature.rs FLASH_TICKS
const FLASH_COLORS: ReadonlyArray<readonly [number, number, number]> = [
  [0, 0, 0], // 0 None — unused
  [0.18, 0.85, 0.78], // 1 Born — teal
  [0.30, 0.85, 0.30], // 2 Grazed — green
  [0.95, 0.85, 0.20], // 3 Attacked — yellow
  [0.30, 0.55, 1.0], // 4 CreatedChild — blue
  [0.95, 0.25, 0.25], // 5 Killed — red
];

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
  biomeProgram: WebGLProgram;
  biomeU: {
    viewport: WebGLUniformLocation;
    camPos: WebGLUniformLocation;
    zoom: WebGLUniformLocation;
    worldSize: WebGLUniformLocation;
    plains: WebGLUniformLocation;
    water: WebGLUniformLocation;
    desert: WebGLUniformLocation;
    // v2.0.3 Stream 2d: clipmap UV transform (mirrors grassU).
    uvScale: WebGLUniformLocation;
    uvOffset: WebGLUniformLocation;
  };
  biomeVao: WebGLVertexArrayObject;
  biomeTex: WebGLTexture;
  // v2.0.3 Stream 2d: biome texture is now budget²-sized (same as grass).
  // biomeTexDim is kept for initial texImage2D allocation tracking only.
  biomeTexDim: number;
  grassProgram: WebGLProgram;
  grassU: {
    viewport: WebGLUniformLocation;
    camPos: WebGLUniformLocation;
    zoom: WebGLUniformLocation;
    worldSize: WebGLUniformLocation;
    opacity: WebGLUniformLocation;
    time: WebGLUniformLocation;
    tint: WebGLUniformLocation;
    // v2.0.3 Stream 2c: clipmap UV transform + LOD blend reserve.
    uvScale: WebGLUniformLocation;
    uvOffset: WebGLUniformLocation;
    lodBlend: WebGLUniformLocation;
  };
  grassVao: WebGLVertexArrayObject;
  grassTex: WebGLTexture;
  // v2.0.3 Stream 2c: texture is now fixed GRASS_LOD_BUDGET_AXIS² — no per-frame resize.
  // grassTexDim removed; budget texture allocated once in initRenderer.
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
  // v2.0 Wave 2b: separate scratch for the action ring-flash annulus pass so
  // it survives the body draw (the highlight pass reuses instanceScratch).
  flashScratch: Float32Array;
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
  // Ring stop uniforms (u_ring_inner / u_ring_outer) are now set per-frame
  // in renderWorldImpl based on zoom level (v2.0.1 LOD). Initialize to the
  // near-zoom defaults so an early draw before the first renderWorldImpl call
  // does not leave them uninitialised.
  gl.useProgram(discProgram);
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

  // ─── Biome program (v2.0 Wave 1b) ───
  // Fullscreen world-quad drawn under the grass. Reuses the same [0,1] quad as
  // grass; samples an R8 biome-id texture with NEAREST so cells stay flat.
  const biomeProgram = link(
    gl,
    compile(gl, gl.VERTEX_SHADER, BIOME_VS),
    compile(gl, gl.FRAGMENT_SHADER, BIOME_FS),
  );
  const biomeVao = mustGet(gl.createVertexArray(), "createVertexArray");
  gl.bindVertexArray(biomeVao);
  const biomeQuadBuf = mustGet(gl.createBuffer(), "createBuffer");
  gl.bindBuffer(gl.ARRAY_BUFFER, biomeQuadBuf);
  gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([
    0, 0,
    1, 0,
    0, 1,
    1, 1,
  ]), gl.STATIC_DRAW);
  gl.enableVertexAttribArray(0);
  gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 0, 0);
  gl.bindVertexArray(null);
  const biomeU = {
    viewport: mustGet(gl.getUniformLocation(biomeProgram, "u_viewport"), "u_viewport"),
    camPos: mustGet(gl.getUniformLocation(biomeProgram, "u_cam_pos"), "u_cam_pos"),
    zoom: mustGet(gl.getUniformLocation(biomeProgram, "u_zoom"), "u_zoom"),
    worldSize: mustGet(gl.getUniformLocation(biomeProgram, "u_world_size"), "u_world_size"),
    plains: mustGet(gl.getUniformLocation(biomeProgram, "u_plains"), "u_plains"),
    water: mustGet(gl.getUniformLocation(biomeProgram, "u_water"), "u_water"),
    desert: mustGet(gl.getUniformLocation(biomeProgram, "u_desert"), "u_desert"),
    // v2.0.3 Stream 2d: clipmap UV transform (mirrors grassU).
    uvScale: mustGet(gl.getUniformLocation(biomeProgram, "u_uv_scale"), "u_uv_scale"),
    uvOffset: mustGet(gl.getUniformLocation(biomeProgram, "u_uv_offset"), "u_uv_offset"),
  };
  // v2.0.3 Stream 2d: biome id texture (R8). NEAREST so each cell is flat color.
  // Allocated at GRASS_LOD_BUDGET_AXIS² (same budget as grass) once in initRenderer.
  // Per-frame we texSubImage2D only the win_w × win_h window bytes at (0,0).
  const biomeTex = mustGet(gl.createTexture(), "createTexture");
  gl.bindTexture(gl.TEXTURE_2D, biomeTex);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
  // Allocate fixed budget² storage (null data; filled per frame).
  gl.texImage2D(
    gl.TEXTURE_2D, 0, gl.R8,
    GRASS_LOD_BUDGET_AXIS, GRASS_LOD_BUDGET_AXIS,
    0, gl.RED, gl.UNSIGNED_BYTE, null,
  );
  // Initialize UV transform to identity (full-field default; overwritten per frame).
  gl.useProgram(biomeProgram);
  gl.uniform2f(biomeU.uvScale, 1.0, 1.0);
  gl.uniform2f(biomeU.uvOffset, 0.0, 0.0);

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
    // v2.0.3 Stream 2c: clipmap UV transform + LOD blend.
    uvScale: mustGet(gl.getUniformLocation(grassProgram, "u_uv_scale"), "u_uv_scale"),
    uvOffset: mustGet(gl.getUniformLocation(grassProgram, "u_uv_offset"), "u_uv_offset"),
    lodBlend: mustGet(gl.getUniformLocation(grassProgram, "u_lod_blend"), "u_lod_blend"),
  };

  // v2.0 Wave 1a: grass texture is R8 (u8 density per cell, quantized
  // Rust-side in `quantize_grass_into`). R8 is a normalized unorm format.
  // R8 requires `UNPACK_ALIGNMENT = 1` since each row is `dim` bytes (not a
  // multiple of 4 in general); set it once here (it's global GL state, but
  // the only single-byte textures we upload are grass + biome).
  // v2.0.3 Stream 2c: allocate the budget² texture once at GRASS_LOD_BUDGET_AXIS
  // size (2048×2048 = ~4 MB R8). Each frame we texSubImage2D only the win_w×win_h
  // window bytes into the (0,0) corner. NEAREST filtering per Q2 decision
  // (nearest-mip first; trilinear enabled later by changing one texParameteri).
  gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
  const grassTex = mustGet(gl.createTexture(), "createTexture");
  gl.bindTexture(gl.TEXTURE_2D, grassTex);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
  // Allocate the fixed budget² texture storage with null data (filled per frame).
  gl.texImage2D(
    gl.TEXTURE_2D, 0, gl.R8,
    GRASS_LOD_BUDGET_AXIS, GRASS_LOD_BUDGET_AXIS,
    0, gl.RED, gl.UNSIGNED_BYTE, null,
  );
  // Initialize UV transform to identity (full-field default; overwritten per frame).
  gl.useProgram(grassProgram);
  gl.uniform2f(grassU.uvScale, 1.0, 1.0);
  gl.uniform2f(grassU.uvOffset, 0.0, 0.0);
  gl.uniform1f(grassU.lodBlend, 0.0);

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
    biomeProgram, biomeU, biomeVao, biomeTex, biomeTexDim: 0,
    grassProgram, grassU, grassVao, grassTex,
    frameProgram, frameU, frameVao, frameBuf,
    instanceScratch: new Float32Array(4096 * FLOATS_PER_INSTANCE),
    flashScratch: new Float32Array(1024 * FLOATS_PER_INSTANCE),
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

// Browsers normalise any CSS color (hex / named / rgba) to "rgb(r, g, b)"
// or "rgba(r, g, b, a)" via getComputedStyle. We exploit that to read
// theme-defined hex tokens without writing a hex parser.
const colorProbe: HTMLDivElement = (() => {
  const d = document.createElement("div");
  d.style.cssText = "position:absolute;width:0;height:0;visibility:hidden;pointer-events:none;";
  document.documentElement.appendChild(d);
  return d;
})();
function readCssColorRgb(varName: string): [number, number, number] {
  colorProbe.style.color = `var(${varName})`;
  const computed = getComputedStyle(colorProbe).color;
  const m = computed.match(/rgba?\(([^)]+)\)/);
  if (!m) return [0, 0, 0];
  const parts = m[1].split(",").map((p) => parseFloat(p.trim()));
  return [
    (parts[0] ?? 0) / 255,
    (parts[1] ?? 0) / 255,
    (parts[2] ?? 0) / 255,
  ];
}

let bgCanvasCacheKey = "";
let bgCanvasCacheVal: [number, number, number] = [0.02, 0.03, 0.04];
function readBgCanvas(): [number, number, number] {
  const raw = readCssVar("--bg-canvas");
  if (raw === bgCanvasCacheKey) return bgCanvasCacheVal;
  bgCanvasCacheKey = raw;
  bgCanvasCacheVal = raw ? readCssColorRgb("--bg-canvas") : [0.02, 0.03, 0.04];
  return bgCanvasCacheVal;
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
  grass: Uint8Array,
  // v2.0.3 Stream 2d: biome is now the per-slot windowed biome bytes from the
  // snapshot (mode-downsampled, same win_w×win_h as grass). Null before first
  // snapshot. The old static biome Uint8Array (from biome_buf) is no longer
  // passed; the slot carries the windowed data alongside grass.
  biomeWin: Uint8Array | null,
  pop: number,
  world_size: number,
  grass_dim: number,
  wrap_world: boolean,
  highlightMap: Map<number, number>,
  // v2.0.3 Stream 2c: clipmap window metadata (mip level, origin, dims).
  // Null before the first snapshot is consumed; falls back to full-field defaults.
  windowMeta: WindowMetadata | null,
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
      gl, cam, viewW, viewH, creatures, grass, biomeWin, pop,
      world_size, grass_dim, wrap_world, highlightMap, windowMeta,
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
  grass: Uint8Array,
  biomeWin: Uint8Array | null,
  pop: number,
  world_size: number,
  grass_dim: number,
  wrap_world: boolean,
  highlightMap: Map<number, number>,
  windowMeta: WindowMetadata | null,
): void {
  const stride = CREATURE_STRIDE;
  if (!state) state = initRenderer(gl);
  const s = state;

  // Match physical-pixel viewport to the canvas drawing buffer (DPR-aware).
  gl.viewport(0, 0, gl.drawingBufferWidth, gl.drawingBufferHeight);
  const [br, bg, bb] = readBgCanvas();
  gl.clearColor(br, bg, bb, 1.0);
  gl.clear(gl.COLOR_BUFFER_BIT);

  // v2.0.1: Compute toroidal tile offsets EARLY so both the grass/biome
  // and creature passes can share the same frustum-derived offset sets.
  // Matches the creature pass logic exactly (see xOffsets/yOffsets below).
  const W_early = world_size;
  const tileHalfW = cam.zoom > 0 ? (viewW / cam.zoom) * 0.5 : 0;
  const tileHalfH = cam.zoom > 0 ? (viewH / cam.zoom) * 0.5 : 0;
  const tileMinX = cam.cx - tileHalfW;
  const tileMaxX = cam.cx + tileHalfW;
  const tileMinY = cam.cy - tileHalfH;
  const tileMaxY = cam.cy + tileHalfH;
  const tileXOffsets: number[] = [0];
  const tileYOffsets: number[] = [0];
  if (wrap_world && W_early > 0) {
    if (tileMinX < 0) tileXOffsets.push(-W_early);
    if (tileMaxX > W_early) tileXOffsets.push(W_early);
    if (tileMinY < 0) tileYOffsets.push(-W_early);
    if (tileMaxY > W_early) tileYOffsets.push(W_early);
  }

  // ─── Biome layer (v2.0 Wave 1b, v2.0.3 Stream 2d) ───
  // Drawn first, UNDER the grass. An R8 biome-id texture mapped to flat
  // per-biome colors. The grass density layer brightens green on top.
  //
  // v2.0.3 Stream 2d: the biome texture is now fed from the per-slot windowed
  // biome channel (mode-downsampled, same win_w × win_h as the grass window).
  // Uploaded every frame via texSubImage2D at (0,0) into the budget²-sized
  // biomeTex (allocated once in initRenderer). The UV transform reuses the
  // same u_uv_scale/u_uv_offset derived from the grass window metadata.
  //
  // At default scale: mip=0, origin=(0,0), win=1920×1920 — byte-identical
  // to the old full-field path. Biome tint is visually unchanged at default scale.
  //
  // v2.0.1: Toroidal tiling — extra draws for each non-zero offset pair.
  if (biomeWin && biomeWin.length > 0) {
    // Derive win_w/win_h from windowMeta (same as grass path).
    const winW    = windowMeta ? windowMeta.winW    : grass_dim;
    const winH    = windowMeta ? windowMeta.winH    : grass_dim;
    const mipLevel   = windowMeta ? windowMeta.mipLevel   : 0;
    const winOriginX = windowMeta ? windowMeta.winOriginX : 0;
    const winOriginY = windowMeta ? windowMeta.winOriginY : 0;

    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, s.biomeTex);
    // Upload the windowed biome bytes into the (0,0) corner of the budget² texture.
    // Fix minor #6: pass an explicit subarray of winW*winH bytes so the upload
    // is correct if UNPACK_ROW_LENGTH is ever set (currently 0/unset, but
    // this makes the call robust against that latent footgun).
    gl.texSubImage2D(
      gl.TEXTURE_2D, 0,
      0, 0, winW, winH,
      gl.RED, gl.UNSIGNED_BYTE,
      biomeWin.subarray(0, winW * winH),
    );

    // UV transform — identical formula to the grass path (GRASS_CELL_SIZE × 2^mip × BUDGET).
    const cellSizeL = GRASS_CELL_SIZE * Math.pow(2, mipLevel);
    const uvScaleVal = world_size / (cellSizeL * GRASS_LOD_BUDGET_AXIS);
    const uvOffsetX = -winOriginX / GRASS_LOD_BUDGET_AXIS;
    const uvOffsetY = -winOriginY / GRASS_LOD_BUDGET_AXIS;

    gl.useProgram(s.biomeProgram);
    gl.uniform2f(s.biomeU.viewport, viewW, viewH);
    gl.uniform1f(s.biomeU.zoom, cam.zoom);
    gl.uniform1f(s.biomeU.worldSize, world_size);
    // Visual-identity defaults (Wave 1c): Plains / Water / Desert flat colors.
    gl.uniform3f(s.biomeU.plains, 74 / 255, 106 / 255, 58 / 255);
    gl.uniform3f(s.biomeU.water, 40 / 255, 92 / 255, 168 / 255);
    gl.uniform3f(s.biomeU.desert, 196 / 255, 178 / 255, 128 / 255);
    // v2.0.3 Stream 2d: clipmap UV transform (same as grass).
    gl.uniform2f(s.biomeU.uvScale, uvScaleVal, uvScaleVal);
    gl.uniform2f(s.biomeU.uvOffset, uvOffsetX, uvOffsetY);
    gl.bindVertexArray(s.biomeVao);
    for (let xi = 0; xi < tileXOffsets.length; xi++) {
      for (let yi = 0; yi < tileYOffsets.length; yi++) {
        gl.uniform2f(s.biomeU.camPos,
          cam.cx - tileXOffsets[xi],
          cam.cy - tileYOffsets[yi],
        );
        gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
      }
    }
  }

  // ─── Grass ───
  // Skipping the upload + draw entirely when the user toggles grass off is
  // a real perf win at high cell counts (1920² cells = ~3.7 MB R8 upload
  // per frame + a fullscreen-discard fragment pass).
  // v1.7: bracketed by `frame.render_world.grass` so the parent
  // `frame.render_world` row's overhead column reads correctly.
  // v2.0.1: Toroidal tiling — same extra-draw loop as biome above.
  // v2.0.3 Stream 2c: texSubImage2D only the win_w × win_h window bytes into
  // the fixed GRASS_LOD_BUDGET_AXIS² budget texture at (0,0). The UV transform
  // (u_uv_scale, u_uv_offset) maps the world-space quad into the window region.
  if (getSettings().showGrass) {
    const grassSpan = span("frame.render_world.grass");
    try {
      gl.activeTexture(gl.TEXTURE0);
      gl.bindTexture(gl.TEXTURE_2D, s.grassTex);

      // v2.0.3 Stream 2c: derive win_w/win_h and mip_level from windowMeta.
      // Fall back to full-field defaults (mip=0, origin=(0,0), win=grass_dim)
      // when metadata is not yet available (before the first snapshot).
      const mipLevel   = windowMeta ? windowMeta.mipLevel   : 0;
      const winOriginX = windowMeta ? windowMeta.winOriginX : 0;
      const winOriginY = windowMeta ? windowMeta.winOriginY : 0;
      const winW       = windowMeta ? windowMeta.winW       : grass_dim;
      const winH       = windowMeta ? windowMeta.winH       : grass_dim;

      // Upload only the window bytes into the budget texture sub-region (0,0).
      // grass[] carries exactly winW × winH bytes (grassRegionBytes = min(dim,BUDGET)²;
      // at default scale that equals grass_dim² — same as before).
      // Fix minor #6: explicit subarray guard — robust if UNPACK_ROW_LENGTH is
      // ever set (currently 0/unset; this is a latent footgun defence).
      gl.texSubImage2D(
        gl.TEXTURE_2D, 0,
        0, 0, winW, winH,
        gl.RED, gl.UNSIGNED_BYTE,
        grass.subarray(0, winW * winH),
      );

      // UV transform:
      //   u_uv_scale  = world_size / (GRASS_CELL_SIZE * 2^mipLevel * BUDGET_AXIS)
      //   u_uv_offset = -vec2(winOriginX, winOriginY) / BUDGET_AXIS
      // This maps a_corner ∈ [0,1] (world fraction) to the window sub-region
      // of the budget texture. At default scale (mip=0, origin=(0,0), win=1920):
      //   scale = 9600 / (5 * 1 * 2048) = 0.9375  (= 1920/2048 = winW/BUDGET)
      //   offset = (0,0)
      // giving v_uv ∈ [0, 0.9375] — exactly the 1920 data pixels in 2048 texture.
      const cellSizeL = GRASS_CELL_SIZE * Math.pow(2, mipLevel);
      const uvScaleVal = world_size / (cellSizeL * GRASS_LOD_BUDGET_AXIS);
      const uvOffsetX = -winOriginX / GRASS_LOD_BUDGET_AXIS;
      const uvOffsetY = -winOriginY / GRASS_LOD_BUDGET_AXIS;

      gl.useProgram(s.grassProgram);
      gl.uniform2f(s.grassU.viewport, viewW, viewH);
      gl.uniform1f(s.grassU.zoom, cam.zoom);
      gl.uniform1f(s.grassU.worldSize, world_size);
      gl.uniform1f(s.grassU.opacity, getSettings().grassOpacity);
      gl.uniform1f(s.grassU.time, performance.now() / 1000);
      const tint = readGrassTint();
      gl.uniform3f(s.grassU.tint, tint[0], tint[1], tint[2]);
      gl.uniform2f(s.grassU.uvScale, uvScaleVal, uvScaleVal);
      gl.uniform2f(s.grassU.uvOffset, uvOffsetX, uvOffsetY);
      gl.uniform1f(s.grassU.lodBlend, 0.0);
      gl.bindVertexArray(s.grassVao);
      for (let xi = 0; xi < tileXOffsets.length; xi++) {
        for (let yi = 0; yi < tileYOffsets.length; yi++) {
          gl.uniform2f(s.grassU.camPos,
            cam.cx - tileXOffsets[xi],
            cam.cy - tileYOffsets[yi],
          );
          gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
        }
      }
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
    // The wrap-aware ghost-copy grow below widens this further when needed.
    const neededFloats = pop * FLOATS_PER_INSTANCE;
    if (s.instanceScratch.length < neededFloats) {
      s.instanceScratch = new Float32Array(neededFloats * 2);
    }

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
        // v2.0 Wave 2b: id halves moved to lanes 4/5 (were 6/7).
        const idLo = idView[base + 4];
        const idHi = idView[base + 5];
        const cid = idHi * 4294967296 + idLo;
        currById.set(cid, { x: creatures[base], y: creatures[base + 1] });
      }
    }

    // Trail scratch: parallel to bodies, one entry per creature that has
    // both prev and curr and is actually moving.
    const trailScratch = s.trailScratch;
    let trailOff = 0;
    const enableTrails = pop <= 12000;

    // v2.0 Wave 2b: action ring-flash scratch (annulus instances). Filled in
    // the body loop for creatures with flash_ticks > 0 and drawn after the
    // bodies. Reuses the same wrap-aware ghost positions as the body.
    let flashScratch = s.flashScratch;
    let flashOff = 0;

    // v2.0 Wave 1c: wrap-aware draw. On a toroidal world a creature near x=0
    // must also render at x=world_size+δ when the camera straddles the x=0 (or
    // x=W) seam, and likewise for y. Build the small set of per-axis world
    // offsets {-W, 0, +W} the visible frustum actually needs; the common case
    // (not wrapping, or frustum well inside the world) is a single {0} offset
    // so the inner loop stays a no-op pass. Grow the body scratch to cover the
    // worst case (pop × ghost-copy count).
    const W = world_size;
    const xOffsets: number[] = [0];
    const yOffsets: number[] = [0];
    if (wrap_world && W > 0) {
      if (minX < 0) xOffsets.push(-W);
      if (maxX > W) xOffsets.push(W);
      if (minY < 0) yOffsets.push(-W);
      if (maxY > W) yOffsets.push(W);
    }
    const ghostCopies = xOffsets.length * yOffsets.length;
    if (ghostCopies > 1) {
      const worstFloats = pop * ghostCopies * FLOATS_PER_INSTANCE;
      if (s.instanceScratch.length < worstFloats) {
        s.instanceScratch = new Float32Array(worstFloats * 2);
      }
    }
    const bodyScratch = s.instanceScratch;

    // v2.0 Wave 1c: at survey zoom a body's on-screen radius drops below 1px.
    // Rather than an AA-shaded disc (which fades to near-invisible), draw a
    // flat ~1px point in the display color so faction territories stay legible.
    const POINT_THRESHOLD_PX = 1.0;
    // v2.0.1 LOD thresholds (zoom-adaptive ring + flash):
    //   < MID_THRESHOLD_PX  → small shaded circle, no flash, no visible ring
    //   MID..NEAR           → slim ring (0.985..1.0), no flash
    //   ≥ NEAR_THRESHOLD_PX → full ring (0.96..1.0), flash allowed
    const MID_THRESHOLD_PX = 4.0;
    const NEAR_THRESHOLD_PX = 12.0;

    let off = 0;
    for (let i = 0; i < pop; i++) {
      const base = i * stride;
      const cx_raw = creatures[base];
      const cy_raw = creatures[base + 1];
      const radiusWorld = creatures[base + 2];
      // Look up prev by id for the trail pass; the body itself draws at
      // the raw SoA position.
      // v2.0 Wave 2b: id halves moved to lanes 4/5 (were 6/7).
      const idLo = idView[base + 4];
      const idHi = idView[base + 5];
      const cid = idHi * 4294967296 + idLo;
      const prev = prevById.get(cid);
      const rawRadiusPx = radiusWorld * PX_PER_SIZE * cam.zoom;
      const isPoint = rawRadiusPx < POINT_THRESHOLD_PX;
      const radiusPx = isPoint ? 1 : rawRadiusPx;
      // inner_px sentinel: -1 ⇒ flat point (no shading), 0 ⇒ shaded disc.
      const innerPx = isPoint ? -1 : 0;
      // v2.0 Wave 2b: display color is a single packed RGBA8 u32 (lane 3),
      // little-endian (R = bits 0..8, G = 8..16, B = 16..24, A = 24..32 = 255).
      // This replaces the old color_r/g/b f32 lanes. The Rust HSV mapping
      // already keeps creatures legible, so the old 125/255 floor is dropped.
      const packedColor = idView[base + 3];
      const cr = (packedColor & 0xff) / 255;
      const cg = ((packedColor >>> 8) & 0xff) / 255;
      const cb = ((packedColor >>> 16) & 0xff) / 255;
      // v2.0 Wave 2b: packed render flags (lane 6). flash_tag = bits 0..2,
      // flash_ticks = bits 3..6, species_id = bits 7..22 (ignored single-pool).
      const packedFlags = idView[base + 6];
      const flashTag = packedFlags & 0x7;
      const flashTicks = (packedFlags >>> 3) & 0xf;
      const margin = radiusWorld + 2;

      // Trail displacement (computed once; skip if it crosses the seam on a
      // torus — a wrapped step would otherwise streak across the whole world).
      let trailDx = 0, trailDy = 0, drawTrail = false;
      if (enableTrails && prev !== undefined) {
        trailDx = cx_raw - prev.x;
        trailDy = cy_raw - prev.y;
        const seam = wrap_world && W > 0 && (Math.abs(trailDx) > W * 0.5 || Math.abs(trailDy) > W * 0.5);
        drawTrail = !seam && (trailDx * trailDx + trailDy * trailDy > 0.04);
      }

      // Emit one (and, near a wrapped seam, up to a few ghost) copies.
      for (let xi = 0; xi < xOffsets.length; xi++) {
        const ox = xOffsets[xi];
        const x = cx_raw + ox;
        if (x < minX - margin || x > maxX + margin) continue;
        for (let yi = 0; yi < yOffsets.length; yi++) {
          const oy = yOffsets[yi];
          const y = cy_raw + oy;
          if (y < minY - margin || y > maxY + margin) continue;
          bodyScratch[off    ] = x;
          bodyScratch[off + 1] = y;
          bodyScratch[off + 2] = radiusPx;
          bodyScratch[off + 3] = innerPx;
          bodyScratch[off + 4] = cr;
          bodyScratch[off + 5] = cg;
          bodyScratch[off + 6] = cb;
          bodyScratch[off + 7] = 1; // alpha
          off += FLOATS_PER_INSTANCE;

          // v2.0 Wave 2b: action ring-flash. Draw a transient outer annulus in
          // the tag's color when flash_ticks > 0. Fades with the countdown.
          // Coexists with the inspector selection ring (separate pass below).
          // v2.0.1: skip flash at far/mid zoom — below MID_THRESHOLD_PX the
          // body is too small for the annulus to be legible.
          if (rawRadiusPx >= MID_THRESHOLD_PX && flashTicks > 0 && flashTag > 0 && flashTag < FLASH_COLORS.length) {
            if (flashOff + FLOATS_PER_INSTANCE > flashScratch.length) {
              const grown = new Float32Array(flashScratch.length * 2);
              grown.set(flashScratch);
              flashScratch = grown;
              s.flashScratch = grown;
            }
            const fc = FLASH_COLORS[flashTag];
            const fade = flashTicks / FLASH_TICKS_MAX;
            // Annulus just outside the body; inner_px > 0 selects the flat
            // annulus branch in DISC_FS.
            const outer = radiusPx + 5;
            const inner = radiusPx + 2;
            flashScratch[flashOff] = x;
            flashScratch[flashOff + 1] = y;
            flashScratch[flashOff + 2] = outer;
            flashScratch[flashOff + 3] = inner;
            flashScratch[flashOff + 4] = fc[0];
            flashScratch[flashOff + 5] = fc[1];
            flashScratch[flashOff + 6] = fc[2];
            flashScratch[flashOff + 7] = fade;
            flashOff += FLOATS_PER_INSTANCE;
          }

          // Trail: start = prev (one paint behind), end = current body, both
          // shifted by the same wrap offset so the segment stays attached to
          // this ghost copy. FS fades alpha from 0 at the back to a at the
          // front (comet tail). Thickness scales with body radius (min 2 px).
          if (drawTrail && prev !== undefined && trailOff + TRAIL_FLOATS_PER_INSTANCE <= trailScratch.length) {
            trailScratch[trailOff    ] = prev.x + ox;
            trailScratch[trailOff + 1] = prev.y + oy;
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
    }
    const bodyCount = (off / FLOATS_PER_INSTANCE) | 0;
    const trailCount = (trailOff / TRAIL_FLOATS_PER_INSTANCE) | 0;
    if (bodyCount > 0) {
      gl.bindBuffer(gl.ARRAY_BUFFER, s.discInstanceBuf);
      gl.bufferData(gl.ARRAY_BUFFER, bodyScratch.subarray(0, off), gl.DYNAMIC_DRAW);

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
      // v2.0.1: zoom-adaptive body ring. Use a representative screen radius
      // (median creature at current zoom) to pick a LOD tier. The ring uniforms
      // are vec1 globals that affect all instances in this draw call, so we
      // choose the tier based on the overall zoom level rather than per-creature.
      // At far zoom (typical radius < MID_THRESHOLD_PX) suppress the ring
      // entirely (inner==outer=1.0); at mid zoom use a slim 1.5%-wide ring;
      // at near zoom restore the full 4%-wide ring (0.96..1.0).
      // A typical creature has radiusWorld≈1; rawRadiusPx = radiusWorld*PX_PER_SIZE*zoom.
      const typicalRadiusPx = 1.0 * PX_PER_SIZE * cam.zoom;
      if (typicalRadiusPx >= NEAR_THRESHOLD_PX) {
        // Near zoom: full ring visible
        gl.uniform1f(s.discU.ringInner, 0.96);
        gl.uniform1f(s.discU.ringOuter, 1.0);
      } else if (typicalRadiusPx >= MID_THRESHOLD_PX) {
        // Mid zoom: slim 1.5% ring — subtle shading cue but not visually heavy
        gl.uniform1f(s.discU.ringInner, 0.985);
        gl.uniform1f(s.discU.ringOuter, 1.0);
      } else {
        // Far zoom: suppress ring (inner == outer, smoothstep collapses to 0)
        gl.uniform1f(s.discU.ringInner, 1.0);
        gl.uniform1f(s.discU.ringOuter, 1.0);
      }
      gl.bindVertexArray(s.discVao);
      gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, bodyCount);

      // ─── Action ring-flash pass (v2.0 Wave 2b) ───
      // Transient outer annulus drawn on top of the bodies in the flash tag's
      // color. The annulus branch in DISC_FS (inner_px > 0) flat-fills, so the
      // per-creature color rides in the instance a_color; the disc-level
      // u_ring_color is left at the body value (unused for annulus instances).
      const flashCount = (flashOff / FLOATS_PER_INSTANCE) | 0;
      if (flashCount > 0) {
        gl.bindBuffer(gl.ARRAY_BUFFER, s.discInstanceBuf);
        gl.bufferData(
          gl.ARRAY_BUFFER,
          flashScratch.subarray(0, flashOff),
          gl.DYNAMIC_DRAW,
        );
        gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, flashCount);
      }
    }
  }

  // ─── Highlight rings (rare path) ───
  // Inspector selection + transient highlights produce ≤ 1–2 rings/frame.
  // v2.0 Wave 2b: id is at byte offset +16 (id_lo) / +20 (id_hi) of each
  // 32-byte stride (lanes 4/5). Build a Uint32Array view over the same backing
  // SAB to read the raw u32 halves without conversion cost.
  if (highlightMap.size > 0 && pop > 0) {
    const nowMs = performance.now();
    const idView = new Uint32Array(
      creatures.buffer,
      creatures.byteOffset,
      pop * stride * (creatures.BYTES_PER_ELEMENT / 4),
    );
    // v2.0 Wave 1c: same wrap offsets as the body pass so a highlighted
    // creature near the seam also rings at its wrapped position.
    const W = world_size;
    const halfW = (viewW / cam.zoom) * 0.5;
    const halfH = (viewH / cam.zoom) * 0.5;
    const hMinX = cam.cx - halfW, hMaxX = cam.cx + halfW;
    const hMinY = cam.cy - halfH, hMaxY = cam.cy + halfH;
    const hxOff: number[] = [0];
    const hyOff: number[] = [0];
    if (wrap_world && W > 0) {
      if (hMinX < 0) hxOff.push(-W);
      if (hMaxX > W) hxOff.push(W);
      if (hMinY < 0) hyOff.push(-W);
      if (hMaxY > W) hyOff.push(W);
    }
    const scratch = s.instanceScratch;
    let off = 0;
    for (let k = 0; k < pop; k++) {
      const base = k * stride;
      // u32-pair → number via the f64 ladder. The max u64 a v1 session ever
      // mints stays under 2^53 (DECISIONS E.21), so the f64 round-trip is
      // lossless and `Map<number, …>` lookups stay correct.
      // v2.0 Wave 2b: id halves moved to lanes 4/5 (were 6/7).
      const idLo = idView[base + 4];
      const idHi = idView[base + 5];
      const cid = idHi * 4294967296 + idLo;
      const exp = highlightMap.get(cid);
      if (exp === undefined || nowMs >= exp) continue;
      const x0 = creatures[base];
      const y0 = creatures[base + 1];
      const radiusWorld = creatures[base + 2];
      const radiusPx = Math.max(1, radiusWorld * PX_PER_SIZE * cam.zoom);
      const alpha = exp >= PERMANENT_EXP - 1
        ? 1.0
        : Math.min(1.0, (exp - nowMs) / HIGHLIGHT_FADE_MS);
      for (let xi = 0; xi < hxOff.length; xi++) {
        const x = x0 + hxOff[xi];
        if (x < hMinX - radiusWorld - 4 || x > hMaxX + radiusWorld + 4) continue;
        for (let yi = 0; yi < hyOff.length; yi++) {
          const y = y0 + hyOff[yi];
          if (y < hMinY - radiusWorld - 4 || y > hMaxY + radiusWorld + 4) continue;
          if (off + FLOATS_PER_INSTANCE > scratch.length) break;
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
      }
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
