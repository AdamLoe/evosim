---
status: review
owner: user
last_updated: 2026-06-02
okay_to_delete: false
long_lived: false
owning_docs:
  - docs/architecture/simulation-core.md
  - docs/architecture/shared-memory-and-protocol.md
  - docs/architecture/app-shell.md
  - docs/architecture/render-pipeline.md
  - docs/decisions/sim.md
  - docs/decisions/render.md
---

# Close-out: grass perf (v2.0.2 scatter + v2.0.3 LOD pyramid)

**Lead handoff.** The orchestrated build-first effort is **code-complete and fully
gated green on both feature sets.** This doc is the one-page review surface: what
landed, the result, the gate evidence, the things **you need to ratify/decide**, and
what was deliberately deferred. The blow-by-blow decision log lives in
[`grass-perf-hub.md`](grass-perf-hub.md); the design specs in
[`v2.0.2-grass-scatter.md`](v2.0.2-grass-scatter.md) +
[`v2.0.3-grass-lod-pyramid.md`](v2.0.3-grass-lod-pyramid.md); the code review +
perf numbers in [`grass-stage3-review.md`](grass-stage3-review.md).

Nothing is committed — it's all an uncommitted working-tree diff on `feat/v2.0.0`,
waiting on your review.

## TL;DR

- **Grass propagation is now a stochastic u8 scatter kernel** (replaces the separable
  blur, which is retained behind a `GrassPropagation` selector). **Measured ~34× faster**
  than blur, **linear O(N)** scaling, and it **does not balloon past 2048-cell worlds**.
- **A u8 box-filter LOD pyramid + windowed snapshot + clipmap renderer** removes the
  1B-scale output walls (upload bandwidth, texture size). Grass **and** biome now render
  through a moving clipmap window; default-scale rendering is byte-identical to before.
- **One load-bearing property changed and needs your ratification:** the live (always-
  threaded) sim is now **non-reproducible run-to-run** under the same seed. See below.
- The **50–100× stretch target was not met** (34× at realistic density), so the **blur
  was kept**, not deleted.

## Gate evidence (all green)

`cargo fmt --check` clean · `cargo clippy` clean **both feature sets** · `cargo test
--lib` **213 passed** (default) **and** `--features threads` 213 passed (×3 + the 2
determinism tests 15× consecutive, zero flakes) · `pnpm typecheck` + `pnpm build` clean ·
threaded `wasm-pack` build OK (initThreadPool=2, shared:true) · **Playwright e2e 11/11**.
Each Stage-1/2 wave was also browser-confirmed live (boot + tick + grass/biome render at
default scale and zoomed-out LOD).

## ⚠ Ratify / decide (these are yours)

1. **Run-to-run reproducibility is gone under threads (the big one).** The scatter kernel
   does lossy cross-tile atomic RMW (concurrent collisions clobber). That makes the grass
   density thread-order-dependent, and creatures **sense** it, so the whole sim diverges
   run-to-run on the same seed. The old blur was deterministic; scatter trades that for the
   perf. This was diagnosed as **inherent to the accepted "intentional fuzz" decision**, not
   a bug (single-threaded is still exactly deterministic; SimRng is untouched; no non-grass
   race). The 2 determinism tests now assert exact equality **single-threaded only** and
   liveness-bounded under threads. **Decision needed:** ratify this (idle sandbox, fuzz is
   fine) **or** ask for a deterministic-scatter redesign. Options if you want determinism
   back: (A) tile-local spread only — kills cross-tile spreading; (B) per-tile inbox merge —
   sequential, costs the perf; (C) CAS add-with-cap — likely the cheapest, additive spread
   commutes so it may restore determinism with modest contention cost. **My recommendation:
   ratify the fuzz for now; revisit (C) only if seed-reproducibility becomes a feature.**
2. **Blur deletion — held.** Perf hit 34×, under the 50–100× gate target (the target only
   holds at sparse early-game fill). Blur stays behind the selector as a reference/fallback.
   Delete it after a tuning pass or when you're satisfied 34× is the ship bar.
3. **Feel-tune the 6 grass sliders.** They're live and default to the old constants, but the
   scatter **energy economy is hotter** (plains super-critical → faster population growth; a
   balance test had to be pinned to Blur). The aesthetic/balance tuning is a taste call —
   tune in the browser; defaults are a starting point, not final.

## Known limitations & deferred (documented, not blocking)

- **Toroidal wrap-seam (MAJOR, narrow):** a clipmap window straddling the world wrap on a
  **toroidal** world with **grass_dim > 2048 at non-default zoom** shows the wrong
  grass/biome at the seam. Documented TODO in `grass.rs::viewport_window` +
  `wasm_api.rs` (origin clamp) + the render ghost-copy UV. Default/walled worlds and
  default scale are unaffected. Fix sketch is in the code TODO + the review doc.
- **Perf optimizations left on the table:** the pyramid `refresh` is a full recompute each
  tick (active-set-keyed dirty-subtree walk is a TODO); the biome window is mode-downsampled
  from the static biome grid every tick (a precomputed static biome mip would cut it). Both
  are O(N) and cheap today; revisit at extreme scale.
- **Minor cleanups:** the dead `biome_buf` (~3.7 MB static biome region, no longer consumed
  by the renderer); `viewport_w/h` reserved-but-unused (square-window approximation); the
  criterion bench widened a few modules `pub(crate)→pub` + added `[profile.bench]
  panic=unwind` (benign "panic ignored for bench" warning) — keep-the-bench-with-tighter-
  visibility vs remove.
- **2e mip-sensing dropped:** scale-invariant NN sensing was not implemented (it needs its
  own behavioral A/B and isn't required for the grass-perf goals). Sensing stays the 1/d²
  proximity scan. → v2 backlog.

## Test rigor note

Re-authoring the u8-domain assertions (which Stage 1 had widened by one quantum) surfaced
real quantization facts the loose bands had masked: **desert saturates at byte 76** (not 77),
**plains at byte 254** (not 255), and graze carries a **systematic 256/255 bite-count bias**.
These are now pinned byte-exact, plus a scatter battery (disc roundness, decay-floor,
persistence/no-collapse, biome-cap clamp).

## Where things live / how to ship

- **Recon maps:** `grass-perf-recon.md` (Stage 1), `grass-perf-recon-stage2.md` (Stage 2).
- **Review + perf:** `grass-stage3-review.md`.
- **Durable facts migrated** into `architecture/{simulation-core,shared-memory-and-protocol,
  app-shell,render-pipeline}.md` + `decisions/{sim,render}.md` (the `owning_docs`). The two
  design sub-plans (`v2.0.2-grass-scatter.md`, `v2.0.3-grass-lod-pyramid.md`) are already
  `okay_to_delete` (content migrated); the recon + review docs are scratch. This close-out +
  the hub stay as the review surface until you ratify the items above and commit.
- **To ship:** the diff is uncommitted on `feat/v2.0.0`. Suggest a `cargo fmt`-clean commit
  (already formatted) once you've reviewed; re-run the threaded gate before merge. The
  `grass-lod-smoke.spec.ts` e2e is a clean permanent boot+window smoke (the per-stream
  throwaways were removed).

## See also

- [`grass-perf-hub.md`](grass-perf-hub.md) — the full orchestration log + every decision +
  the carried-forward debt (each item marked fixed/confirmed/deferred).
