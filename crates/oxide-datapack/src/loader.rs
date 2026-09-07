//! Top-level datapack loading: walks the registries, runs reference
//! resolution, and exposes everything as one [`Datapack`].

use crate::biome::Biome;
use crate::density_function::DensityFunction;
use crate::dimension::{BiomeSource, Dimension, DimensionType};
use crate::error::Result;
use crate::multi_noise::MultiNoiseBiomeSourceParameterList;
use crate::noise_param::NormalNoiseParameters;
use crate::noise_settings::NoiseGeneratorSettings;
use crate::registry::{load_registry, load_registry_at, Registry};
use crate::resolve::{
    multi_noise_biome_ids, resolve_biome_refs, resolve_density_functions,
    resolve_noise_refs_in_density_functions, resolve_noise_settings, ResolutionReport,
};
use crate::structure::{ProcessorList, Structure, StructureSet, TemplatePool};
use crate::version::{load_data_version, load_pack_meta, DataVersion, PackMeta};
use std::path::Path;

#[derive(Debug)]
pub struct Datapack {
    pub data_version: DataVersion,
    pub pack_meta: Option<PackMeta>,

    pub density_functions: Registry<DensityFunction>,
    pub noise_params: Registry<NormalNoiseParameters>,
    pub noise_settings: Registry<NoiseGeneratorSettings>,
    pub biomes: Registry<Biome>,
    pub dimensions: Registry<Dimension>,
    pub dimension_types: Registry<DimensionType>,
    pub multi_noise_parameter_lists: Registry<MultiNoiseBiomeSourceParameterList>,
    pub configured_carvers: Registry<crate::carver::ConfiguredCarver>,
    pub structures: Registry<Structure>,
    pub structure_sets: Registry<StructureSet>,
    pub template_pools: Registry<TemplatePool>,
    pub processor_lists: Registry<ProcessorList>,
    pub placed_features: Registry<crate::placement::PlacedFeature>,
    pub configured_features: Registry<crate::feature::ConfiguredFeature>,
}

/// Load and fully validate a datapack-shaped directory tree rooted at
/// `pack_root` (i.e. `pack_root/data/<namespace>/...`).
pub fn load_datapack(pack_root: &Path) -> Result<Datapack> {
    load_datapack_stack(&[pack_root])
}

/// Load and merge multiple datapacks in stack order (earlier packs are base, later packs override).
/// Resolves all cross-datapack references against the unified registry graph.
pub fn load_datapack_stack(pack_roots: &[&Path]) -> Result<Datapack> {
    if pack_roots.is_empty() {
        return Err(crate::error::DatapackError::NotADatapack(
            std::path::PathBuf::from(""),
        ));
    }

    let base_root = pack_roots[0];
    let data_version = load_data_version(base_root)?;
    let pack_meta = load_pack_meta(base_root)?;

    let mut density_functions =
        load_registry(base_root, "density_function", "worldgen/density_function")?;
    let mut noise_params = load_registry(base_root, "noise", "worldgen/noise")?;
    let mut noise_settings =
        load_registry(base_root, "noise_settings", "worldgen/noise_settings")?;
    let mut biomes = load_registry(base_root, "biome", "worldgen/biome")?;
    let mut dimension_types =
        load_registry_at(base_root, Path::new("dimension_type"), "dimension_type")?;
    let mut multi_noise_parameter_lists = load_registry(
        base_root,
        "multi_noise_biome_source_parameter_list",
        "worldgen/multi_noise_biome_source_parameter_list",
    )?;
    let mut configured_carvers =
        load_registry(base_root, "configured_carver", "worldgen/configured_carver")?;
    let mut structures = load_registry(base_root, "structure", "worldgen/structure")?;
    let mut structure_sets =
        load_registry(base_root, "structure_set", "worldgen/structure_set")?;
    let mut template_pools =
        load_registry(base_root, "template_pool", "worldgen/template_pool")?;
    let mut processor_lists =
        load_registry(base_root, "processor_list", "worldgen/processor_list")?;
    let mut placed_features =
        load_registry(base_root, "placed_feature", "worldgen/placed_feature")?;
    let mut configured_features = load_registry(
        base_root,
        "configured_feature",
        "worldgen/configured_feature",
    )?;
    let mut dimensions: Registry<Dimension> =
        load_registry_at(base_root, Path::new("dimension"), "dimension")?;

    for &overlay_root in &pack_roots[1..] {
        if overlay_root.join("data").is_dir() {
            if let Ok(df) =
                load_registry(overlay_root, "density_function", "worldgen/density_function")
            {
                density_functions.merge(df);
            }
            if let Ok(np) = load_registry(overlay_root, "noise", "worldgen/noise") {
                noise_params.merge(np);
            }
            if let Ok(ns) =
                load_registry(overlay_root, "noise_settings", "worldgen/noise_settings")
            {
                noise_settings.merge(ns);
            }
            if let Ok(b) = load_registry(overlay_root, "biome", "worldgen/biome") {
                biomes.merge(b);
            }
            if let Ok(dt) =
                load_registry_at(overlay_root, Path::new("dimension_type"), "dimension_type")
            {
                dimension_types.merge(dt);
            }
            if let Ok(mn) = load_registry(
                overlay_root,
                "multi_noise_biome_source_parameter_list",
                "worldgen/multi_noise_biome_source_parameter_list",
            ) {
                multi_noise_parameter_lists.merge(mn);
            }
            if let Ok(cc) =
                load_registry(overlay_root, "configured_carver", "worldgen/configured_carver")
            {
                configured_carvers.merge(cc);
            }
            if let Ok(s) = load_registry(overlay_root, "structure", "worldgen/structure") {
                structures.merge(s);
            }
            if let Ok(ss) =
                load_registry(overlay_root, "structure_set", "worldgen/structure_set")
            {
                structure_sets.merge(ss);
            }
            if let Ok(tp) =
                load_registry(overlay_root, "template_pool", "worldgen/template_pool")
            {
                template_pools.merge(tp);
            }
            if let Ok(pl) =
                load_registry(overlay_root, "processor_list", "worldgen/processor_list")
            {
                processor_lists.merge(pl);
            }
            if let Ok(pf) =
                load_registry(overlay_root, "placed_feature", "worldgen/placed_feature")
            {
                placed_features.merge(pf);
            }
            if let Ok(cf) = load_registry(
                overlay_root,
                "configured_feature",
                "worldgen/configured_feature",
            ) {
                configured_features.merge(cf);
            }
            if let Ok(dim) =
                load_registry_at(overlay_root, Path::new("dimension"), "dimension")
            {
                dimensions.merge(dim);
            }
        }
    }

    let mut report = ResolutionReport::default();

    resolve_density_functions(&density_functions, &mut report);
    resolve_noise_refs_in_density_functions(&density_functions, &noise_params, &mut report);

    for (id, settings) in noise_settings.iter() {
        resolve_noise_settings(id, settings, &density_functions, &noise_params, &mut report);
    }

    for (id, list) in multi_noise_parameter_lists.iter() {
        resolve_biome_refs(
            "worldgen/multi_noise_biome_source_parameter_list",
            id,
            multi_noise_biome_ids(list),
            &biomes,
            &mut report,
        );
    }

    for (id, dim) in dimensions.iter() {
        if let Some(source) = &dim.generator.biome_source {
            let ids: Vec<_> = match source {
                BiomeSource::Fixed { biome } => vec![biome.clone()],
                BiomeSource::Checkerboard { biomes: bs, .. } => bs.clone(),
                BiomeSource::MultiNoise(mn) => multi_noise_biome_ids(mn),
                BiomeSource::TheEnd {} | BiomeSource::Unknown => Vec::new(),
            };
            resolve_biome_refs("dimension", id, ids, &biomes, &mut report);
        }
    }

    report.into_result()?;

    Ok(Datapack {
        data_version,
        pack_meta,
        density_functions,
        noise_params,
        noise_settings,
        biomes,
        dimensions,
        dimension_types,
        multi_noise_parameter_lists,
        configured_carvers,
        structures,
        structure_sets,
        template_pools,
        processor_lists,
        placed_features,
        configured_features,
    })
}
