import { defineConfig } from "vite";

// COOP/COEP headers so SharedArrayBuffer is available (required for
// wasm-bindgen-rayon if/when threads feature is enabled). Cloudflare Pages
// gets the same headers via /web/public/_headers.
const crossOriginIsolationHeaders = {
  "Cross-Origin-Opener-Policy": "same-origin",
  "Cross-Origin-Embedder-Policy": "require-corp",
};

export default defineConfig({
  root: ".",
  server: {
    host: true,
    port: 47821,
    strictPort: true,
    headers: crossOriginIsolationHeaders,
  },
  preview: {
    host: true,
    port: 47821,
    strictPort: true,
    headers: crossOriginIsolationHeaders,
  },
  build: {
    target: "es2022",
    outDir: "dist",
    emptyOutDir: true,
    // "hidden" in CI: emits the map file but does not reference it from the JS
    // bundle (safe for sentry-style upload; not served to end users).
    sourcemap: process.env.CI ? "hidden" : true,
  },
  // wasm-bindgen-rayon spawns Web Workers via new URL('./workerHelpers.js',
  // import.meta.url). Vite must bundle those workers as ES modules (not the
  // default IIFE) so that code-splitting works. Without this, `pnpm build`
  // errors with "UMD and IIFE output formats are not supported for
  // code-splitting builds". See docs/plans/perf-4-threads.md §2d.
  worker: {
    format: "es",
  },
  // Ensure the .wasm artifact next to the JS glue is treated as an asset Vite
  // can fingerprint and emit.
  assetsInclude: ["**/*.wasm"],
});
