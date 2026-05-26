//! Corpse pool. One entry per dead creature, lasts up to 100 ticks or
//! until fully drained (v5 §3.5 step 10, §8).

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Carrion {
    pub id: u64,
    pub x: f32,
    pub y: f32,
    pub pool: f32,
    pub age: u32,
}
