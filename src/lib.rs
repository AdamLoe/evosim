//! evosim — browser-deployed idle evolution sandbox (Rust → wasm).

#![allow(clippy::needless_range_loop)]

pub(crate) mod brain;
// pub(crate) → pub for grass, constants, rng: needed by the native criterion
// benchmark (benches/grass_scatter.rs). The types within were already `pub`;
// only the module visibility changes. No API surface expands beyond what was
// visible inside the crate. `world` and `profiler` stay pub(crate).
pub mod constants;
pub mod control_sab;
pub(crate) mod creature;
pub mod grass;
pub(crate) mod grid;
pub(crate) mod profiler;
pub mod rng;
pub(crate) mod world;

mod wasm_api;

pub use wasm_api::*;

// v2.0.4 S1: LOD config math tests (budget=4096, lod_bias, aspect-ratio fix).
#[cfg(test)]
#[path = "wasm_lod_config_tests.rs"]
mod wasm_lod_config_tests;

// v2.0.4 S2: grass_size construction slider — cell-count scaling + rebuild tests.
#[cfg(test)]
#[path = "grass_size_tests.rs"]
mod grass_size_tests;

// Threads: re-export `init_thread_pool` so wasm-bindgen emits an
// `initThreadPool(num_threads: number) => Promise<void>` JS export.
// JS must `await initThreadPool(navigator.hardwareConcurrency)` before
// the first WorldHandle construction; see web/src/main.ts.
#[cfg(feature = "threads")]
pub use wasm_bindgen_rayon::init_thread_pool;

use wasm_bindgen::prelude::*;

#[wasm_bindgen(start)]
pub fn _start() {
    // Always install the panic hook so release builds surface panics in the
    // browser console instead of silently aborting (see S2 audit item).
    std::panic::set_hook(Box::new(|info| {
        web_sys::console::error_1(&JsValue::from_str(&format!("{info}")));
    }));
}
