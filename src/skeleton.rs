//! Milestone A skeleton: a single bouncing ball driven from Rust.
//!
//! Replaced wholesale once Milestone B lands the real World type.

use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct BouncingBall {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    radius: f32,
    width: f32,
    height: f32,
}

#[wasm_bindgen]
impl BouncingBall {
    #[wasm_bindgen(constructor)]
    pub fn new(width: f32, height: f32) -> Self {
        Self {
            x: width * 0.5,
            y: height * 0.5,
            vx: 1.7,
            vy: 1.3,
            radius: 18.0,
            width,
            height,
        }
    }

    pub fn step(&mut self, dt: f32) {
        self.x += self.vx * dt;
        self.y += self.vy * dt;
        if self.x < self.radius {
            self.x = self.radius;
            self.vx = -self.vx;
        } else if self.x > self.width - self.radius {
            self.x = self.width - self.radius;
            self.vx = -self.vx;
        }
        if self.y < self.radius {
            self.y = self.radius;
            self.vy = -self.vy;
        } else if self.y > self.height - self.radius {
            self.y = self.height - self.radius;
            self.vy = -self.vy;
        }
    }

    #[wasm_bindgen(getter)]
    pub fn x(&self) -> f32 {
        self.x
    }
    #[wasm_bindgen(getter)]
    pub fn y(&self) -> f32 {
        self.y
    }
    #[wasm_bindgen(getter)]
    pub fn radius(&self) -> f32 {
        self.radius
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ball_bounces_within_bounds() {
        let mut b = BouncingBall::new(100.0, 100.0);
        for _ in 0..10_000 {
            b.step(1.0);
            assert!(b.x >= b.radius && b.x <= 100.0 - b.radius);
            assert!(b.y >= b.radius && b.y <= 100.0 - b.radius);
        }
    }
}
