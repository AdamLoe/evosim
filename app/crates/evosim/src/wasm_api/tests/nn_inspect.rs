//! v2.1 P1 (Stream C): unit tests for `creature_nn_inspect_json`.
//!
//! Asserts that:
//! 1. The `inputs` array length equals the active `NnInputLayout::width()`.
//! 2. Each input `value` is finite.
//! 3. The `outputs.chosen` field decodes to a valid action string.
//! 4. The function returns `None` for an out-of-range index.
//!
//! Attached via the `mod nn_inspect;` declaration in `wasm_api/mod.rs`.

use super::super::*;

/// Helper: construct a minimal WorldHandle with default settings (walled world,
/// single-pool, grass_multisight=true so GrassBandsFar is active).
fn make_handle() -> WorldHandle {
    WorldHandle::new_with_founder_count(
        "nn-inspect-test",
        0,      // grass_initial_seed_count
        100.0,  // energy_max
        4,      // founder_count
        false,  // full_grass_on_init
        "",     // nn_topology_json (legacy default)
        1200.0, // world_size
        false,  // wrap_world (walled: WallProximity active)
        42,     // world_seed
        false,  // species_mode (single-pool: CreatureSectors = 8)
        0.0,    // crossover_mode
        1,      // starting_species_count
        1,      // starting_species_member_count
        0.0,    // starting_species_member_variance
        5.0,    // grass_cell_size (default)
        true,   // grass_multisight (GrassBandsFar active)
        0,      // grass_clump_count
        0,      // grass_clump_size
        1.0,    // init_graze_boost
        1.0,    // init_split_boost
    )
    .expect("WorldHandle construction must succeed in test")
}

/// The JSON `inputs` array length must equal the active NnInputLayout's real
/// (unpadded) slot count.
///
/// Default config (walled, single-pool, grass_multisight=true) — v2.1 P2:
/// `SelfMemory(8) + WallProximity(4) + CreatureSectors(8) + GrassSectors(8)`
/// `+ GrassBandsFar(8) + CurrGrass(1) + Bias(1)`
/// `= 38 real slots → padded layout width = 40.`
///
/// `inputs` has one entry per active slot (38, excluding the 2 SIMD pad lanes).
/// We verify against the known real count (38) and the padded layout width (40).
#[test]
fn nn_inspect_input_count_matches_layout_width() {
    let handle = make_handle();

    // Verify index 0 returns Some.
    let json_str = handle
        .creature_nn_inspect_json(0)
        .expect("creature 0 must be present");

    let parsed: serde_json::Value =
        serde_json::from_str(&json_str).expect("creature_nn_inspect_json must produce valid JSON");

    let inputs = parsed["inputs"]
        .as_array()
        .expect("inputs must be a JSON array");

    // Padded layout width for walled + single-pool + grass_multisight=true is 40 (v2.1 P2).
    assert_eq!(
        handle.inner.nn_input_layout.width(),
        40,
        "walled + single-pool + grass_multisight layout width must be 40 (v2.1 P2)"
    );

    // The `inputs` array has one entry per ACTIVE slot (no pad entries).
    // For this config: 38 active slots (= 40 padded - 2 SIMD pad lanes).
    // We verify the count by checking against the known real count directly,
    // and confirm it is strictly less than the padded width (pad slots omitted).
    let n = inputs.len();
    assert!(
        n < handle.inner.nn_input_layout.width(),
        "inputs count ({n}) must be less than padded layout width ({}) — pad slots omitted",
        handle.inner.nn_input_layout.width()
    );
    assert_eq!(
        n, 38,
        "active slot count must be 38 for walled + single-pool + multisight config, got {n}"
    );
}

#[test]
fn nn_inspect_omits_curr_biome_type() {
    let handle = make_handle();
    let json_str = handle
        .creature_nn_inspect_json(0)
        .expect("creature 0 must be present");
    let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    let inputs = parsed["inputs"].as_array().unwrap();
    assert!(
        inputs.iter().all(|entry| entry["group"] != "CurrBiomeType"),
        "inspector NN inputs must not expose CurrBiomeType"
    );
}

/// All input `value` fields must be finite (no NaN or infinity from the forward pass).
#[test]
fn nn_inspect_input_values_all_finite() {
    let handle = make_handle();
    let json_str = handle
        .creature_nn_inspect_json(0)
        .expect("creature 0 must be present");
    let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    let inputs = parsed["inputs"].as_array().unwrap();
    for (k, entry) in inputs.iter().enumerate() {
        let v = entry["value"]
            .as_f64()
            .expect("each input value must be a number");
        assert!(
            v.is_finite(),
            "input[{k}] group={} label={} value={v} must be finite",
            entry["group"],
            entry["label"]
        );
    }
}

/// The `outputs.chosen` field must be one of the valid action strings.
#[test]
fn nn_inspect_outputs_chosen_is_valid_action() {
    let handle = make_handle();
    let json_str = handle
        .creature_nn_inspect_json(0)
        .expect("creature 0 must be present");
    let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    let chosen = parsed["outputs"]["chosen"]
        .as_str()
        .expect("outputs.chosen must be a string");
    assert!(
        matches!(chosen, "Graze" | "Attack" | "Split" | "Mate"),
        "outputs.chosen must be Graze/Attack/Split/Mate, got {chosen:?}"
    );
}

/// `outputs` must have `vx`, `vy`, `logits` (array of 3), and `chosen`.
#[test]
fn nn_inspect_outputs_shape() {
    let handle = make_handle();
    let json_str = handle
        .creature_nn_inspect_json(0)
        .expect("creature 0 must be present");
    let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    let outputs = &parsed["outputs"];
    assert!(outputs["vx"].is_number(), "outputs.vx must be a number");
    assert!(outputs["vy"].is_number(), "outputs.vy must be a number");
    let logits = outputs["logits"]
        .as_array()
        .expect("outputs.logits must be an array");
    assert_eq!(
        logits.len(),
        3,
        "outputs.logits must have exactly 3 elements"
    );
    for (k, v) in logits.iter().enumerate() {
        assert!(v.is_number(), "outputs.logits[{k}] must be a number");
    }
    assert!(
        outputs["chosen"].is_string(),
        "outputs.chosen must be a string"
    );
}

/// Input entries have the correct field names (group, label, value).
#[test]
fn nn_inspect_input_entry_shape() {
    let handle = make_handle();
    let json_str = handle
        .creature_nn_inspect_json(0)
        .expect("creature 0 must be present");
    let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    let inputs = parsed["inputs"].as_array().unwrap();
    assert!(!inputs.is_empty(), "inputs must be non-empty");
    for (k, entry) in inputs.iter().enumerate() {
        assert!(
            entry["group"].is_string(),
            "input[{k}].group must be a string"
        );
        assert!(
            entry["label"].is_string(),
            "input[{k}].label must be a string"
        );
        assert!(
            entry["value"].is_number(),
            "input[{k}].value must be a number"
        );
    }
}

/// Out-of-range index returns None.
#[test]
fn nn_inspect_out_of_range_returns_none() {
    let handle = make_handle();
    let result = handle.creature_nn_inspect_json(9999);
    assert!(result.is_none(), "out-of-range idx must return None");
}

/// The first SelfMemory slot (hunger) must be in [0, 1].
/// The Bias slot (last active entry) must be exactly 1.0.
#[test]
fn nn_inspect_known_slot_values() {
    let handle = make_handle();
    let json_str = handle
        .creature_nn_inspect_json(0)
        .expect("creature 0 must be present");
    let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    let inputs = parsed["inputs"].as_array().unwrap();

    // Find hunger (first SelfMemory slot).
    let hunger_entry = inputs.iter().find(|e| e["label"] == "hunger");
    if let Some(e) = hunger_entry {
        let v = e["value"].as_f64().unwrap();
        assert!(
            (0.0..=1.0).contains(&v),
            "hunger must be in [0, 1], got {v}"
        );
    }

    // Find bias slot — must be 1.0.
    let bias_entry = inputs.iter().find(|e| e["group"] == "Bias");
    if let Some(e) = bias_entry {
        let v = e["value"].as_f64().unwrap();
        assert!((v - 1.0).abs() < 1e-5, "Bias slot must be 1.0, got {v}");
    }
}
