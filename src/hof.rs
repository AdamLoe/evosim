//! Hall-of-Fame snapshots for the eulogy card (F.28). Collected during the run;
//! consumed by F.28. All fields are serde-able so F.26 can persist them.
//! v6 §L definitions: biggest, weirdest (v5 §11.1), last_survivor, first_mover.
//!
//! D3: Genome field removed. Size is now the constant FOUNDER_SIZE.

use serde::{Deserialize, Serialize};

/// Snapshot of a notable creature, captured at a specific moment for the F.28 eulogy.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HallOfFame {
    pub creature_id: u64,
    pub species_name: String,
    pub captured_tick: u32,
    pub captured_age: u32,
}
