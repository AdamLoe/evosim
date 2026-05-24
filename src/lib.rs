//! evosim — browser-deployed idle evolution sandbox.
//!
//! Milestone A: walking skeleton. Exposes a `BouncingBall` demo for the web
//! shell to drive while later milestones flesh out the real simulation.

use wasm_bindgen::prelude::*;

mod skeleton;

#[wasm_bindgen(start)]
pub fn _start() {
    // Better panic messages in the browser console for dev builds.
    #[cfg(debug_assertions)]
    console_error_panic_hook_lite();
}

#[cfg(debug_assertions)]
fn console_error_panic_hook_lite() {
    std::panic::set_hook(Box::new(|info| {
        web_sys::console::error_1(&JsValue::from_str(&format!("{info}")));
    }));
}

pub use skeleton::BouncingBall;
