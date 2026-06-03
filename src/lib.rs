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
