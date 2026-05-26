//! evosim — browser-deployed idle evolution sandbox (Rust → wasm).

#![allow(clippy::needless_range_loop)]

pub(crate) mod brain;
pub(crate) mod carrion;
pub(crate) mod constants;
pub(crate) mod creature;
pub(crate) mod events;
pub(crate) mod genome;
pub(crate) mod grass;
pub(crate) mod grid;
pub(crate) mod hof;
pub(crate) mod profiler;
pub(crate) mod rng;
pub mod snapshot_hash; // used by tests/acceptance.rs
pub(crate) mod species;
pub(crate) mod torus;
pub(crate) mod vision;
pub mod world; // used by tests/acceptance.rs

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
