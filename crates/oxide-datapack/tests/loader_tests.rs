//! Integration tests over the fixture packs in `tests/fixtures/`.

use oxide_datapack::density_function::{DensityFunction, DensityFunctionObject, SplineValue};
use oxide_datapack::noise_settings::NoiseGeneratorSettings;
use oxide_datapack::{load_datapack, DatapackError};
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn density_function_tree_parses_number_string_object_and_nested_spline() {
    let bytes = std::fs::read(fixture("density_function_tree.json")).unwrap();
    let df: DensityFunction = serde_json::from_slice(&bytes).unwrap();

    let DensityFunction::Object(obj) = df else {
        panic!("expected an object-form density function at the root");
    };
    let DensityFunctionObject::Add {
        argument1,
        argument2,
    } = *obj
    else {
        panic!("expected minecraft:add at the root");
    };

    // Bare number form.
    assert!(matches!(argument1, DensityFunction::Constant(v) if v == 5.5));

    // Object form wrapping a spline.
    let DensityFunction::Object(obj2) = argument2 else {
        panic!("expected an object-form second argument");
    };
    let DensityFunctionObject::Spline { spline } = *obj2 else {
        panic!("expected minecraft:spline");
    };

    // Bare string form (a named reference) as the spline's coordinate.
    assert!(
        matches!(spline.coordinate, DensityFunction::Reference(id) if id.to_string() == "testmod:erosion")
    );

    assert_eq!(spline.points.len(), 2);
    assert!(matches!(spline.points[0].value, SplineValue::Constant(v) if v == 0.5));
    // Nested spline value.
    match &spline.points[1].value {
        SplineValue::Spline(inner) => {
            assert_eq!(inner.points.len(), 1);
            assert!(matches!(inner.coordinate, DensityFunction::Constant(v) if v == 1.0));
        }
        SplineValue::Constant(_) => panic!("expected a nested spline, got a constant"),
    }
}

#[test]
fn minimal_noise_settings_parses() {
    let bytes = std::fs::read(fixture("noise_settings_minimal.json")).unwrap();
    let settings: NoiseGeneratorSettings = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(settings.sea_level, 63);
    assert_eq!(settings.noise.min_y, -64);
    assert_eq!(settings.noise.height, 384);
    // Defaults not present in the fixture.
    assert!(settings.aquifers_enabled);
    assert!(settings.ore_veins_enabled);
    assert!(!settings.disable_mob_generation);
    assert!(settings.spawn_target.is_empty());
}

#[test]
fn valid_pack_loads_and_resolves() {
    let pack = load_datapack(&fixture("valid_pack")).expect("valid pack should load");

    assert_eq!(pack.data_version.data_version, 4501);
    assert_eq!(pack.data_version.version_name, "26.2 test fixture");
    assert_eq!(pack.density_functions.len(), 2);
    assert_eq!(pack.noise_params.len(), 1);
    assert_eq!(pack.noise_settings.len(), 1);
    assert_eq!(pack.biomes.len(), 1);
    assert_eq!(pack.dimensions.len(), 1);
}

#[test]
fn dangling_density_function_reference_is_reported() {
    let err = load_datapack(&fixture("dangling_pack"))
        .expect_err("dangling reference should fail to load");
    match err {
        DatapackError::DanglingReference { kind, target, .. } => {
            assert_eq!(kind, "density_function");
            assert_eq!(target, "testmod:missing");
        }
        other => panic!("expected DanglingReference, got {other:?}"),
    }
}

#[test]
fn reference_cycle_is_detected() {
    let err = load_datapack(&fixture("cycle_pack")).expect_err("a->b->a cycle should fail to load");
    match err {
        DatapackError::Cycle { cycle, .. } => {
            assert!(cycle.contains("testmod:a"));
            assert!(cycle.contains("testmod:b"));
        }
        other => panic!("expected Cycle, got {other:?}"),
    }
}

/// A diamond — the same named density function referenced from two different
/// places via cache-boundary nodes — is not a cycle and must load cleanly.
/// See `resolve.rs` module docs for why this needs recursion-stack-based DFS
/// rather than a naive "seen before" set.
#[test]
fn diamond_shaped_shared_reference_is_not_a_cycle() {
    let pack =
        load_datapack(&fixture("diamond_pack")).expect("diamond-shaped sharing is not a cycle");
    assert_eq!(pack.density_functions.len(), 4);
}

#[test]
fn missing_data_version_fails_loudly() {
    // No version.json and no pack.mcmeta at all: must error, never guess.
    let dir = std::env::temp_dir().join(format!("oxide-datapack-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("data/testmod/worldgen/density_function")).unwrap();

    let err = load_datapack(&dir).expect_err("no version metadata should fail to load");
    assert!(matches!(err, DatapackError::MissingDataVersion { .. }));

    let _ = std::fs::remove_dir_all(&dir);
}
