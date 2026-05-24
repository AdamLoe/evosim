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
    sourcemap: true,
  },
  // Ensure the .wasm artifact next to the JS glue is treated as an asset Vite
  // can fingerprint and emit.
  assetsInclude: ["**/*.wasm"],
});
