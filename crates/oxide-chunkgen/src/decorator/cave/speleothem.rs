//! `speleothem`: one pointed formation (a stalactite or stalagmite, dripstone or sulfur) grown
//! out of a ceiling or a floor, plus the block classification and the column scan the cluster and
//! large-dripstone features share (vanilla's `SpeleothemUtils`).
//!
//! A formation needs a base block on one side of the origin. It first thickens that base into a
//! ragged patch, then writes a tip, or a frustum under a tip when the cell beyond is free.
//!
//! A pointed block whose state lacks `vertical_direction`, `thickness` or `waterlogged` makes the
//! feature unsupported at compile time; vanilla's codec would reject it, and the reference only
//! fails when it first places one.

use anyhow::Result;
use oxide_core::{BlockPos, BlockState, RandomSource};
use oxide_datapack::cave_feature::SpeleothemConfig;
use oxide_datapack::placement::{BlockList, BlockStateSpec};
use oxide_datapack::{BlockFact, BlockFacts};

use super::support::{has, is_block, offset, relative, state_has, Dir};
use crate::decorator::context::{block_in, FeatureContext};
use crate::decorator::predicate::IdSet;
use crate::decorator::state_provider::state_from_spec;

/// The ceiling and floor edges of a vertical scan, as y of the blocking block.
#[derive(Debug, Clone, Copy)]
pub(super) struct Column {
    pub floor: Option<i32>,
    pub ceiling: Option<i32>,
}

impl Column {
    /// `Column.getHeight`: the free cells between floor and ceiling.
    pub fn height(&self) -> Option<i32> {
        match (self.floor, self.ceiling) {
            (Some(floor), Some(ceiling)) => Some(ceiling - floor - 1),
            _ => None,
        }
    }
}

/// The base block and the blocks formations may replace.
#[derive(Debug)]
pub(super) struct Blocks {
    pub base: BlockState,
    replaceable: IdSet,
}

impl Blocks {
    pub fn compile(base: BlockState, replaceable: &BlockList) -> Result<Self> {
        Ok(Self {
            base,
            replaceable: IdSet::compile(replaceable)?,
        })
    }

    /// Air or the water block.
    pub fn empty_or_water(&self, cx: &FeatureContext<'_, '_, '_>, state: &BlockState) -> bool {
        state_has(cx, state, BlockFact::Air) || is_block(state, "water")
    }

    /// Air, water or lava.
    pub fn empty_water_or_lava(&self, cx: &FeatureContext<'_, '_, '_>, state: &BlockState) -> bool {
        self.empty_or_water(cx, state) || is_block(state, "lava")
    }

    /// `isBase`: the base block itself or something it may replace.
    pub fn is_base(&self, cx: &FeatureContext<'_, '_, '_>, state: &BlockState) -> bool {
        state.name == self.base.name || self.replaceable.contains_block(cx.env, &state.name)
    }

    pub fn is_replaceable(&self, cx: &FeatureContext<'_, '_, '_>, state: &BlockState) -> bool {
        self.replaceable.contains_block(cx.env, &state.name)
    }

    /// `placeBaseBlockIfPossible`: writes the base block over a replaceable block.
    pub fn place_base_if_possible(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        pos: BlockPos,
    ) -> bool {
        if self.is_replaceable(cx, block_in(cx.region, pos)) {
            cx.region.set_block(pos, self.base.clone());
            return true;
        }
        false
    }

    /// `Column.scan`: from the empty cell at `pos`, up to `range - 1` empty cells each way and then
    /// the cell that ends them; `edge` says whether that cell is a ceiling or floor.
    pub fn scan(
        &self,
        cx: &FeatureContext<'_, '_, '_>,
        pos: BlockPos,
        range: i32,
        edge: &dyn Fn(&BlockState) -> bool,
    ) -> Option<Column> {
        if !self.empty_or_water(cx, block_in(cx.region, pos)) {
            return None;
        }
        let direction = |step: i32| -> Option<i32> {
            // `pos` is known empty and the cell that ends the walk is read once.
            let (start, limit) = match range - 1 {
                ..=0 => (pos, 0),
                n => (offset(pos, 0, step, 0), n - 1),
            };
            let mut at = start;
            let mut stop = None;
            for _ in 0..limit {
                let state = block_in(cx.region, at);
                if !self.empty_or_water(cx, state) {
                    stop = Some(state);
                    break;
                }
                at = offset(at, 0, step, 0);
            }
            let ending = match stop {
                Some(state) => state,
                None => block_in(cx.region, BlockPos::new(pos.x, at.y, pos.z)),
            };
            edge(ending).then_some(at.y)
        };
        let ceiling = direction(1);
        let floor = direction(-1);
        Some(Column { floor, ceiling })
    }
}

/// The pointed block, with the three properties every formation state sets.
#[derive(Debug)]
pub(super) struct Pointed(BlockState);

impl Pointed {
    /// `None` when the block has no `vertical_direction`, `thickness` and `waterlogged`.
    pub fn compile(spec: &BlockStateSpec, facts: &BlockFacts) -> Result<Option<Self>> {
        let state = state_from_spec(spec, facts)?;
        let complete = ["vertical_direction", "thickness", "waterlogged"]
            .iter()
            .all(|key| state.properties.contains_key(*key));
        Ok(complete.then_some(Self(state)))
    }

    pub fn block(&self) -> &BlockState {
        &self.0
    }

    /// The pointed block growing `up` or hanging down, at `thickness`.
    pub fn state(&self, up: bool, thickness: &str, water: bool) -> BlockState {
        self.0
            .clone()
            .with_property("vertical_direction", if up { "up" } else { "down" })
            .with_property("thickness", thickness)
            .with_property("waterlogged", water.to_string())
    }
}

/// A base-and-pointed pair: what `speleothem` and `speleothem_cluster` build with.
#[derive(Debug)]
pub(super) struct Speleothem {
    pub blocks: Blocks,
    pub pointed: Pointed,
}

impl Speleothem {
    pub fn compile(
        base: &BlockStateSpec,
        pointed: &BlockStateSpec,
        replaceable: &BlockList,
        facts: &BlockFacts,
    ) -> Result<Option<Self>> {
        let Some(pointed) = Pointed::compile(pointed, facts)? else {
            return Ok(None);
        };
        Ok(Some(Self {
            blocks: Blocks::compile(state_from_spec(base, facts)?, replaceable)?,
            pointed,
        }))
    }

    /// `SpeleothemUtils.growSpeleothem`: a column of pointed blocks from `start`, `height` tall,
    /// growing up or hanging down, provided the cell behind `start` is base.
    pub fn grow(
        &self,
        cx: &mut FeatureContext<'_, '_, '_>,
        start: BlockPos,
        up: bool,
        height: i32,
        merged: bool,
    ) {
        let step = if up { 1 } else { -1 };
        if !self
            .blocks
            .is_base(cx, block_in(cx.region, offset(start, 0, -step, 0)))
        {
            return;
        }
        let mut sequence: Vec<&str> = Vec::new();
        if height >= 3 {
            sequence.push("base");
            sequence.extend(std::iter::repeat_n("middle", (height - 3) as usize));
        }
        if height >= 2 {
            sequence.push("frustum");
        }
        if height >= 1 {
            sequence.push(if merged { "tip_merge" } else { "tip" });
        }
        let mut at = start;
        for thickness in sequence {
            let water = has(cx, at, BlockFact::Water);
            cx.region
                .set_block(at, self.pointed.state(up, thickness, water));
            at = offset(at, 0, step, 0);
        }
    }
}

/// `speleothem`.
#[derive(Debug)]
pub struct SpeleothemFeature {
    speleothem: Speleothem,
    /// Taller generation, directional spread, spread radius 2, spread radius 3.
    chances: [f32; 4],
}

impl SpeleothemFeature {
    pub fn compile(config: &SpeleothemConfig, facts: &BlockFacts) -> Result<Option<Self>> {
        let Some(speleothem) = Speleothem::compile(
            &config.base_block,
            &config.pointed_block,
            &config.replaceable_blocks,
            facts,
        )?
        else {
            return Ok(None);
        };
        Ok(Some(Self {
            speleothem,
            chances: [
                config.chance_of_taller_generation,
                config.chance_of_directional_spread,
                config.chance_of_spread_radius2,
                config.chance_of_spread_radius3,
            ],
        }))
    }

    /// `SpeleothemFeature.place`.
    pub fn place(&self, cx: &mut FeatureContext<'_, '_, '_>) -> bool {
        let blocks = &self.speleothem.blocks;
        let origin = cx.origin;
        let ceiling = blocks.is_base(cx, block_in(cx.region, offset(origin, 0, 1, 0)));
        let floor = blocks.is_base(cx, block_in(cx.region, offset(origin, 0, -1, 0)));
        let hanging = match (ceiling, floor) {
            (true, true) => cx.random.next_boolean(),
            (true, false) => true,
            (false, true) => false,
            (false, false) => return false,
        };
        let step = if hanging { Dir::Down } else { Dir::Up };
        let base = relative(origin, step.opposite(), 1);
        blocks.place_base_if_possible(cx, base);
        for (dx, dz) in [(0, -1), (1, 0), (0, 1), (-1, 0)] {
            if cx.random.next_float() > self.chances[1] {
                continue;
            }
            let mut at = offset(base, dx, 0, dz);
            blocks.place_base_if_possible(cx, at);
            for chance in &self.chances[2..] {
                if cx.random.next_float() > *chance {
                    break;
                }
                let direction = Dir::ALL[cx.random.next_int_bounded(6) as usize];
                at = relative(at, direction, 1);
                blocks.place_base_if_possible(cx, at);
            }
        }
        let beyond = block_in(cx.region, relative(origin, step, 1));
        let tall = cx.random.next_float() < self.chances[0] && blocks.empty_or_water(cx, beyond);
        if blocks.is_base(cx, block_in(cx.region, base)) {
            let mut at = origin;
            let thicknesses: &[&str] = if tall { &["frustum", "tip"] } else { &["tip"] };
            for thickness in thicknesses {
                let water = has(cx, at, BlockFact::Water);
                cx.region.set_block(
                    at,
                    self.speleothem.pointed.state(!hanging, thickness, water),
                );
                at = relative(at, step, 1);
            }
        }
        true
    }
}
