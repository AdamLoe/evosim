//! Event log. Ring buffer of last 200 entries shown in UI; full history is
//! written to save file per v6 §I.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    pub tick: u32,
    pub kind: EventKind,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum EventKind {
    Speciation {
        new_species_id: u32,
        parent_species_id: u32,
        new_species_name: String,
        creature_id: u64,
    },
    Extinction {
        species_id: u32,
        species_name: String,
    },
    PopulationMilestone {
        population: u32,
    },
    FirstToMove {
        creature_id: u64,
    },
    FirstToEat {
        creature_id: u64,
    },
    WorldEnded {
        ticks_lived: u32,
        peak_population: u32,
        peak_species: u32,
    },
}

#[derive(Clone, Default)]
pub struct EventLog {
    /// Full history (saved to disk).
    pub all: Vec<Event>,
    /// UI ring buffer of recent N (default 200).
    pub recent: std::collections::VecDeque<Event>,
    pub ring_cap: usize,
}

impl EventLog {
    pub fn new() -> Self {
        Self {
            all: Vec::new(),
            recent: std::collections::VecDeque::with_capacity(200),
            ring_cap: 200,
        }
    }

    pub fn push(&mut self, ev: Event) {
        if self.recent.len() == self.ring_cap {
            self.recent.pop_front();
        }
        self.recent.push_back(ev.clone());
        self.all.push(ev);
    }
}
