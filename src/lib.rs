//! evosim — browser-deployed idle evolution sandbox (Rust → wasm).

#![allow(clippy::needless_range_loop)]

pub mod brain;
pub mod carrion;
pub mod constants;
pub mod creature;
pub mod events;
pub mod genome;
pub mod grid;
pub mod rng;
pub mod species;
pub mod sun;
pub mod vision;
pub mod world;

mod wasm_api;

pub use wasm_api::*;

use wasm_bindgen::prelude::*;

#[wasm_bindgen(start)]
pub fn _start() {
    #[cfg(debug_assertions)]
    std::panic::set_hook(Box::new(|info| {
        web_sys::console::error_1(&JsValue::from_str(&format!("{info}")));
    }));
}
