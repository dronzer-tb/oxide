//! What each block state *is*: air, solid, replaceable, a fluid, counted by which heightmaps.
//!
//! None of this is in the worldgen JSON. The game hardcodes it per block, so it is exported from
//! the real 26.2 jar by `scripts/BlockFactsExporter.java` into `<pack root>/block_facts.tsv`
//! and read here. Decoration is the consumer: a feature that asks "can a tree replace this" or
//! "where is the surface of this column" needs the game's own answer, not a guess by block name.

use crate::error::{DatapackError, Result};
use oxide_core::{BlockState, ResourceLocation};
use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;

/// One fact about a block state. The letters are the exporter's file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum BlockFact {
    Air = 1 << 0,
    Solid = 1 << 1,
    BlocksMotion = 1 << 2,
    Liquid = 1 << 3,
    Replaceable = 1 << 4,
    SolidRender = 1 << 5,
    SturdyTop = 1 << 6,
    SturdyBottom = 1 << 7,
    NoCollision = 1 << 8,
    FullCollision = 1 << 9,
    Water = 1 << 10,
    Lava = 1 << 11,
    FluidSource = 1 << 12,
    /// Counted by `WORLD_SURFACE` and `WORLD_SURFACE_WG`.
    HeightWorldSurface = 1 << 13,
    /// Counted by `OCEAN_FLOOR` and `OCEAN_FLOOR_WG`.
    HeightOceanFloor = 1 << 14,
    HeightMotionBlocking = 1 << 15,
    HeightMotionBlockingNoLeaves = 1 << 16,
    /// `Block.hasDynamicShape`: excluded from a structure template's full-block list.
    DynamicShape = 1 << 17,
    /// `BlockState.hasBlockEntity`.
    BlockEntity = 1 << 18,
    /// `updateShape` never changes this block's state (it is inherited, or overridden only to
    /// schedule ticks), so a neighbour changing cannot reshape it.
    ShapeInert = 1 << 19,
}

impl BlockFact {
    fn from_letter(letter: char) -> Option<Self> {
        Some(match letter {
            'a' => Self::Air,
            's' => Self::Solid,
            'm' => Self::BlocksMotion,
            'l' => Self::Liquid,
            'r' => Self::Replaceable,
            'o' => Self::SolidRender,
            'u' => Self::SturdyTop,
            'd' => Self::SturdyBottom,
            'c' => Self::NoCollision,
            'f' => Self::FullCollision,
            'W' => Self::Water,
            'L' => Self::Lava,
            'S' => Self::FluidSource,
            '1' => Self::HeightWorldSurface,
            '2' => Self::HeightOceanFloor,
            '3' => Self::HeightMotionBlocking,
            '4' => Self::HeightMotionBlockingNoLeaves,
            'y' => Self::DynamicShape,
            'e' => Self::BlockEntity,
            'k' => Self::ShapeInert,
            _ => return None,
        })
    }
}

/// The exported facts of every block state, plus each block's default state.
#[derive(Debug, Default, Clone)]
pub struct BlockFacts {
    flags: HashMap<BlockState, u32>,
    defaults: HashMap<ResourceLocation, BlockState>,
}

impl BlockFacts {
    /// Reads `<pack_root>/block_facts.tsv`. A tree without the file yields an empty table
    /// (see [`BlockFacts::is_empty`]): terrain-only generation does not need it, and a caller
    /// that does need it must check rather than decorate against no facts.
    pub fn load(pack_root: &Path) -> Result<Self> {
        let path = pack_root.join("block_facts.tsv");
        if !path.is_file() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path).map_err(|source| DatapackError::Io {
            path: path.clone(),
            source,
        })?;
        Self::parse(&text, &path)
    }

    fn parse(text: &str, path: &Path) -> Result<Self> {
        let mut facts = Self::default();
        for line in text.lines().filter(|line| !line.is_empty()) {
            let (state_text, letters) = line.split_once('\t').unwrap_or((line, ""));
            let state = parse_state(state_text).ok_or_else(|| {
                DatapackError::InvalidResourceLocation(state_text.to_string(), path.to_path_buf())
            })?;
            let mut bits = 0;
            for letter in letters.chars() {
                if letter == 'D' {
                    facts.defaults.insert(state.name.clone(), state.clone());
                } else if let Some(fact) = BlockFact::from_letter(letter) {
                    bits |= fact as u32;
                }
            }
            facts.flags.insert(state, bits);
        }
        Ok(facts)
    }

    pub fn is_empty(&self) -> bool {
        self.flags.is_empty()
    }

    /// Whether `fact` holds for `state`. A state the table does not know has no facts.
    pub fn is(&self, state: &BlockState, fact: BlockFact) -> bool {
        self.flags.get(state).is_some_and(|bits| bits & fact as u32 != 0)
    }

    /// Whether the table lists `state` at all: a guard against writing a state the game lacks.
    pub fn knows(&self, state: &BlockState) -> bool {
        self.flags.contains_key(state)
    }

    /// The state the game gives a block when nothing sets its properties.
    pub fn default_state(&self, block: &ResourceLocation) -> Option<&BlockState> {
        self.defaults.get(block)
    }

    /// `state` with every property it leaves out filled from its block's default, which is how
    /// the game reads a partial state from data (`{"Name": "minecraft:oak_log"}` is `axis=y`).
    pub fn complete(&self, state: &BlockState) -> BlockState {
        let Some(default) = self.defaults.get(&state.name) else {
            return state.clone();
        };
        let mut full = default.clone();
        for (key, value) in &state.properties {
            if full.properties.contains_key(key) {
                full.properties.insert(key.clone(), value.clone());
            }
        }
        full
    }
}

/// Parses `ns:block[k=v,...]`, the form [`BlockState`] prints.
fn parse_state(text: &str) -> Option<BlockState> {
    let (name, properties) = match text.split_once('[') {
        Some((name, rest)) => (name, rest.strip_suffix(']')?),
        None => (text, ""),
    };
    let mut state = BlockState::new(ResourceLocation::from_str(name).ok()?);
    for pair in properties.split(',').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=')?;
        state.properties.insert(key.to_string(), value.to_string());
    }
    Some(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(path: &str) -> BlockState {
        BlockState::new(ResourceLocation::minecraft(path))
    }

    #[test]
    fn parses_flags_defaults_and_completes_partial_states() {
        let text = "minecraft:air\tDarc\n\
                    minecraft:oak_log[axis=x]\tsmoudf1234\n\
                    minecraft:oak_log[axis=y]\tDsmoudf1234\n\
                    minecraft:water[level=0]\tDlrcWS134\n";
        let facts = BlockFacts::parse(text, Path::new("test")).unwrap();

        assert!(facts.is(&block("air"), BlockFact::Air));
        assert!(!facts.is(&block("air"), BlockFact::HeightWorldSurface));
        let water = block("water").with_property("level", "0");
        assert!(facts.is(&water, BlockFact::Water));
        assert!(facts.is(&water, BlockFact::HeightMotionBlocking));
        assert!(!facts.is(&water, BlockFact::HeightOceanFloor));
        assert!(!facts.is(&block("unknown"), BlockFact::Solid));

        assert_eq!(
            facts.complete(&block("oak_log")),
            block("oak_log").with_property("axis", "y")
        );
        assert_eq!(
            facts.complete(&block("oak_log").with_property("axis", "x")),
            block("oak_log").with_property("axis", "x")
        );
    }

    /// The real 26.2 table, when the local reference tree has it (it is never committed).
    #[test]
    fn loads_the_reference_table_when_present() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../reference");
        let facts = BlockFacts::load(&root).unwrap();
        if facts.is_empty() {
            return;
        }
        assert!(facts.is(&block("stone"), BlockFact::Solid));
        assert!(facts.is(&block("short_grass"), BlockFact::Replaceable));
        let leaves = facts.default_state(&ResourceLocation::minecraft("oak_leaves")).unwrap();
        assert!(facts.is(leaves, BlockFact::HeightMotionBlocking));
        assert!(!facts.is(leaves, BlockFact::HeightMotionBlockingNoLeaves));
    }
}
