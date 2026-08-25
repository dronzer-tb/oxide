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
    let data_version = load_data_version(pack_root)?;
    let pack_meta = load_pack_meta(pack_root)?;

    let density_functions =
        load_registry(pack_root, "density_function", "worldgen/density_function")?;
    let noise_params = load_registry(pack_root, "noise", "worldgen/noise")?;
    let noise_settings = load_registry(pack_root, "noise_settings", "worldgen/noise_settings")?;
    let biomes = load_registry(pack_root, "biome", "worldgen/biome")?;
    let dimension_types =
        load_registry_at(pack_root, Path::new("dimension_type"), "dimension_type")?;
    let multi_noise_parameter_lists = load_registry(
        pack_root,
        "multi_noise_biome_source_parameter_list",
        "worldgen/multi_noise_biome_source_parameter_list",
    )?;
    let configured_carvers =
        load_registry(pack_root, "configured_carver", "worldgen/configured_carver")?;
    let structures = load_registry(pack_root, "structure", "worldgen/structure")?;
    let structure_sets = load_registry(pack_root, "structure_set", "worldgen/structure_set")?;
    let template_pools = load_registry(pack_root, "template_pool", "worldgen/template_pool")?;
    let processor_lists = load_registry(pack_root, "processor_list", "worldgen/processor_list")?;
    let placed_features = load_registry(pack_root, "placed_feature", "worldgen/placed_feature")?;
    let configured_features = load_registry(
        pack_root,
        "configured_feature",
        "worldgen/configured_feature",
    )?;

    // `dimension/*.json` lives directly under `data/<ns>/dimension/`, not
    // under `worldgen/`.
    let dimensions: Registry<Dimension> =
        load_registry_at(pack_root, Path::new("dimension"), "dimension")?;

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
