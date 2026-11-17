//! The features that place structure templates: `fossil` and `template`.
//!
//! This file is the seam to [`Feature`](super::Feature): one enum, [`TemplatedFeature`],
//! compiled by [`TemplatedFeature::compile`] and placed by [`TemplatedFeature::place`]. The
//! template machinery under it — `structure_template.rs` (`StructureTemplate.placeInWorld`),
//! `processor.rs` (structure processors) and `shape.rs` (the neighbour-shape pass) — is written
//! against [`FeatureContext`] and the block tags only, so jigsaw placement can reuse the
//! processors.
//!
//! Templates are read from the datapack tree when a feature that names them is compiled, once
//! per id ([`TemplateLibrary`]), which is a few dozen small files rather than every template
//! the pack carries. An id with no file is kept as missing: placing it is placing vanilla's
//! empty template (`StructureTemplateManager.getOrCreate` hands back a fresh one), which writes
//! nothing, and is tallied as `template:<id>` each time it is picked.

mod fossil;
mod processor;
mod shape;
mod structure_template;
mod template_feature;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use anyhow::{bail, Context, Result};
use oxide_core::rotation::Rotation;
use oxide_core::ResourceLocation;
use oxide_datapack::feature::{ConfiguredFeature, Holder};
use oxide_datapack::template_feature::ProcessorList;
use oxide_datapack::{BlockFacts, Datapack};

use super::context::FeatureContext;

pub use fossil::FossilFeature;
pub use processor::{Processor, Rule};
pub use structure_template::{
    mirror_block_state, place_in_world, randomizable_container_id, BlockInfo, BlockNbt,
    BoundingBox, PlaceSettings, Template, TemplateNbt,
};
pub use template_feature::TemplateFeature;

/// A compiled `fossil` or `template` feature.
#[derive(Debug)]
pub enum TemplatedFeature {
    Fossil(FossilFeature),
    Template(TemplateFeature),
}

/// A template id and what the datapack tree holds for it.
#[derive(Debug, Clone)]
pub struct TemplateRef {
    pub id: ResourceLocation,
    template: Option<Arc<Template>>,
}

static EMPTY_TEMPLATE: LazyLock<Template> = LazyLock::new(Template::default);

impl TemplateRef {
    /// The template to place: the loaded one, or (tallied) vanilla's empty one.
    fn resolve(&self, cx: &mut FeatureContext<'_, '_, '_>) -> &Template {
        match &self.template {
            Some(template) => template,
            None => {
                cx.unsupported.record("template", &self.id.to_string());
                &EMPTY_TEMPLATE
            }
        }
    }
}

/// Reads templates and processor lists out of a loaded datapack for compilation, memoising
/// templates by id.
pub struct TemplateLibrary<'d> {
    datapack: &'d Datapack,
    loaded: HashMap<ResourceLocation, Option<Arc<Template>>>,
}

impl<'d> TemplateLibrary<'d> {
    pub fn new(datapack: &'d Datapack) -> Self {
        Self {
            datapack,
            loaded: HashMap::new(),
        }
    }

    fn template(&mut self, id: &ResourceLocation) -> Result<TemplateRef> {
        if let Some(found) = self.loaded.get(id) {
            return Ok(TemplateRef {
                id: id.clone(),
                template: found.clone(),
            });
        }
        let raw = self.datapack.structure_template(id)?;
        let template =
            raw.map(|raw| Arc::new(Template::from_raw(&raw, &self.datapack.block_facts)));
        self.loaded.insert(id.clone(), template.clone());
        Ok(TemplateRef {
            id: id.clone(),
            template,
        })
    }

    /// The processors of a processor-list holder, or `Err(type)` for the first one not
    /// implemented.
    fn processors(
        &self,
        holder: &Holder<ProcessorList>,
        facts: &BlockFacts,
    ) -> Result<std::result::Result<Vec<Processor>, String>> {
        let list = match holder {
            Holder::Inline(list) => (**list).clone(),
            Holder::Reference(id) => {
                let value = self
                    .datapack
                    .processor_lists
                    .get(id)
                    .with_context(|| format!("unknown processor list {id}"))?;
                serde_json::from_value(value.clone())
                    .with_context(|| format!("processor list {id}"))?
            }
        };
        let mut processors = Vec::with_capacity(list.processors().len());
        for value in list.processors() {
            match Processor::compile(value, facts)? {
                Ok(processor) => processors.push(processor),
                Err(kind) => return Ok(Err(kind)),
            }
        }
        Ok(Ok(processors))
    }
}

impl TemplatedFeature {
    /// Compiles `ir` when it is `fossil` or `template`; `Ok(None)` for every other type, and
    /// `Ok(Some(Err(detail)))` when it uses a processor this module does not implement.
    pub fn compile(
        ir: &ConfiguredFeature,
        facts: &BlockFacts,
        library: &mut TemplateLibrary<'_>,
    ) -> Result<Option<std::result::Result<Self, String>>> {
        Ok(Some(match ir {
            ConfiguredFeature::Fossil(config) => {
                if config.fossil_structures.is_empty() {
                    bail!("fossil structure lists need at least one entry");
                }
                if config.fossil_structures.len() != config.overlay_structures.len() {
                    bail!("fossil structure lists must be equal lengths");
                }
                let pairs = config
                    .fossil_structures
                    .iter()
                    .zip(&config.overlay_structures)
                    .map(|(base, overlay)| {
                        Ok((library.template(base)?, library.template(overlay)?))
                    })
                    .collect::<Result<Vec<_>>>()?;
                let fossil_processors =
                    match library.processors(&config.fossil_processors, facts)? {
                        Ok(processors) => processors,
                        Err(kind) => {
                            return Ok(Some(Err(format!("minecraft:fossil (processor {kind})"))))
                        }
                    };
                let overlay_processors =
                    match library.processors(&config.overlay_processors, facts)? {
                        Ok(processors) => processors,
                        Err(kind) => {
                            return Ok(Some(Err(format!("minecraft:fossil (processor {kind})"))))
                        }
                    };
                Ok(Self::Fossil(FossilFeature {
                    pairs,
                    fossil_processors,
                    overlay_processors,
                    max_empty_corners: config.max_empty_corners_allowed,
                }))
            }
            ConfiguredFeature::Template(config) => {
                let mut entries = Vec::with_capacity(config.templates.len());
                let mut total_weight = 0i32;
                for entry in &config.templates {
                    let rotations = match &entry.data.rotations {
                        Some(names) => names
                            .iter()
                            .map(|name| rotation_from_name(name))
                            .collect::<Result<Vec<_>>>()?,
                        None => Rotation::VALUES.to_vec(),
                    };
                    if rotations.is_empty() {
                        bail!("template {} allows no rotation", entry.data.id);
                    }
                    if entry.weight < 0 {
                        bail!("template {} has a negative weight", entry.data.id);
                    }
                    total_weight = total_weight
                        .checked_add(entry.weight)
                        .context("template weights overflow")?;
                    entries.push((library.template(&entry.data.id)?, rotations, entry.weight));
                }
                if total_weight <= 0 {
                    bail!("a template feature needs a positive total weight");
                }
                Ok(Self::Template(TemplateFeature {
                    entries,
                    total_weight,
                }))
            }
            _ => return Ok(None),
        }))
    }

    /// Places the feature at `cx.origin`, returning whether vanilla would report it placed.
    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        match self {
            Self::Fossil(feature) => feature.place(cx),
            Self::Template(feature) => feature.place(cx),
        }
    }
}

/// `Rotation`'s serialised names.
fn rotation_from_name(name: &str) -> Result<Rotation> {
    Ok(match name {
        "none" => Rotation::None,
        "clockwise_90" => Rotation::Clockwise90,
        "180" => Rotation::Clockwise180,
        "counterclockwise_90" => Rotation::Counterclockwise90,
        other => bail!("unknown rotation {other:?}"),
    })
}
