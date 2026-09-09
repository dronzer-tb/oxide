//! Parsed `worldgen/template_pool` definitions — the candidate pieces a jigsaw connector may
//! pull from, and the fallback pool it falls back to when the depth budget runs out.
//!
//! Ported from 26.2's `StructureTemplatePool`. The one thing worth knowing: weights are not a
//! weighted *draw*. The constructor expands the list, appending each element `weight` times, and
//! selection then indexes or shuffles that flat list — so a weight-3 element occupies three
//! slots and can be tried three times in a row before the pool moves on.

use oxide_core::{RandomSource, ResourceLocation};
use serde::{Deserialize, Serialize};

use crate::rotation::shuffle;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplatePool {
    #[serde(default, with = "oxide_datapack::ident_serde::rl_opt")]
    pub fallback: Option<ResourceLocation>,
    pub elements: Vec<PoolElementEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolElementEntry {
    pub weight: i32,
    pub element: PoolElement,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Projection {
    /// The piece keeps the Y its connector implies.
    #[default]
    #[serde(rename = "rigid")]
    Rigid,
    /// The piece is dropped onto the terrain surface instead.
    #[serde(rename = "terrain_matching")]
    TerrainMatching,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "element_type")]
pub enum PoolElement {
    #[serde(rename = "minecraft:single_pool_element")]
    Single {
        #[serde(with = "oxide_datapack::ident_serde::rl")]
        location: ResourceLocation,
        #[serde(default)]
        projection: Projection,
        /// Either a processor-list id or an inline list; vanilla's codec accepts both and
        /// packs use both. Held raw because processors are not applied yet — parsing it into a
        /// `ResourceLocation` rejected every pool that inlines one, which is most of the
        /// village.
        #[serde(default)]
        processors: serde_json::Value,
    },
    #[serde(rename = "minecraft:legacy_single_pool_element")]
    LegacySingle {
        #[serde(with = "oxide_datapack::ident_serde::rl")]
        location: ResourceLocation,
        #[serde(default)]
        projection: Projection,
        /// Either a processor-list id or an inline list; vanilla's codec accepts both and
        /// packs use both. Held raw because processors are not applied yet — parsing it into a
        /// `ResourceLocation` rejected every pool that inlines one, which is most of the
        /// village.
        #[serde(default)]
        processors: serde_json::Value,
    },
    #[serde(rename = "minecraft:empty_pool_element")]
    Empty,
    #[serde(rename = "minecraft:list_pool_element")]
    List {
        elements: Vec<PoolElement>,
        #[serde(default)]
        projection: Projection,
    },
    #[serde(rename = "minecraft:feature_pool_element")]
    Feature {
        #[serde(with = "oxide_datapack::ident_serde::rl")]
        feature: ResourceLocation,
        #[serde(default)]
        projection: Projection,
    },
}

impl PoolElement {
    /// The template this element places, if it places exactly one.
    ///
    /// `list` (several templates stacked at one spot) and `feature` (a configured feature rather
    /// than a template) return `None`: they are parsed so a pack loads and its other elements
    /// still work, but placing them needs the feature pipeline, so a pool made only of them
    /// behaves as an empty pool rather than silently placing something wrong.
    pub fn template(&self) -> Option<&ResourceLocation> {
        match self {
            PoolElement::Single { location, .. } | PoolElement::LegacySingle { location, .. } => {
                Some(location)
            }
            _ => None,
        }
    }

    pub fn projection(&self) -> Projection {
        match self {
            PoolElement::Single { projection, .. }
            | PoolElement::LegacySingle { projection, .. }
            | PoolElement::List { projection, .. }
            | PoolElement::Feature { projection, .. } => *projection,
            PoolElement::Empty => Projection::Rigid,
        }
    }

    pub fn is_empty(&self) -> bool {
        matches!(self, PoolElement::Empty)
    }
}

impl TemplatePool {
    /// The weight-expanded candidate list vanilla's constructor builds.
    pub fn expanded(&self) -> Vec<&PoolElement> {
        let mut out = Vec::new();
        for entry in &self.elements {
            for _ in 0..entry.weight.max(0) {
                out.push(&entry.element);
            }
        }
        out
    }

    /// `getRandomTemplate`: one `nextInt(size)` into the expanded list. An empty pool yields the
    /// empty element, exactly as vanilla's `EmptyPoolElement.INSTANCE` does.
    pub fn random_template(&self, random: &mut impl RandomSource) -> Option<&PoolElement> {
        let expanded = self.expanded();
        if expanded.is_empty() {
            return None;
        }
        let index = random.next_int_bounded(expanded.len() as i32) as usize;
        Some(expanded[index])
    }

    /// `getShuffledTemplates`: `Util.shuffle` over a copy of the expanded list.
    pub fn shuffled_templates<'a>(&'a self, random: &mut impl RandomSource) -> Vec<&'a PoolElement> {
        let mut expanded = self.expanded();
        shuffle(&mut expanded, random);
        expanded
    }

    pub fn size(&self) -> usize {
        self.elements.iter().map(|e| e.weight.max(0) as usize).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_core::LegacyRandom;

    fn single(name: &str) -> PoolElement {
        PoolElement::Single {
            location: ResourceLocation::minecraft(name),
            projection: Projection::Rigid,
            processors: serde_json::Value::Null,
        }
    }

    fn pool() -> TemplatePool {
        TemplatePool {
            fallback: None,
            elements: vec![
                PoolElementEntry {
                    weight: 3,
                    element: single("house"),
                },
                PoolElementEntry {
                    weight: 1,
                    element: single("well"),
                },
            ],
        }
    }

    #[test]
    fn weights_expand_into_repeated_slots() {
        let pool = pool();
        let expanded = pool.expanded();
        assert_eq!(expanded.len(), 4);
        assert_eq!(pool.size(), 4);
        let houses = expanded
            .iter()
            .filter(|e| e.template().unwrap().path() == "house")
            .count();
        assert_eq!(houses, 3, "weight 3 must occupy three slots, not bias a draw");
    }

    #[test]
    fn a_shuffled_pool_keeps_every_slot() {
        let pool = pool();
        let mut random = LegacyRandom::new(7);
        let shuffled = pool.shuffled_templates(&mut random);
        assert_eq!(shuffled.len(), 4);
        assert_eq!(
            shuffled
                .iter()
                .filter(|e| e.template().unwrap().path() == "well")
                .count(),
            1
        );
    }
}
