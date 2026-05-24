//! evosim — browser-deployed idle evolution sandbox (Rust → wasm).

#![allow(clippy::needless_range_loop)]

pub mod brain;
pub mod carrion;
pub mod constants;
pub mod creature;
pub mod events;
pub mod genome;
pub mod grid;
pub mod hof;
pub mod profiler;
pub mod rng;
pub mod save;
pub mod snapshot_hash;
pub mod species;
pub mod sun;
pub mod vision;
pub mod world;

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
    #[cfg(debug_assertions)]
    std::panic::set_hook(Box::new(|info| {
        web_sys::console::error_1(&JsValue::from_str(&format!("{info}")));
    }));
}
