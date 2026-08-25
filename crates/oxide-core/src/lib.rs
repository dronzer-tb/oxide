//! oxide-core — shared types and RNG primitives for the Oxide worldgen engine.
//!
//! See `docs/ARCHITECTURE.md` for the crate graph and non-negotiables. This crate defines
//! positions, block/biome identifiers, paletted storage, the chunk data model, and every
//! RNG primitive that must be bit-exact with vanilla Java (`java.util.Random` and
//! `Xoroshiro128PlusPlus`), since every downstream parity property is built on top of it.

mod ident;
mod pos;
mod rng;
mod storage;

pub mod mth;

pub use ident::{BiomeId, BlockState, ResourceLocation, ResourceLocationParseError};
pub use pos::{BlockPos, ChunkPos, SectionPos};
pub use rng::{
    LegacyPositionalRandomFactory, LegacyRandom, PositionalRandomFactory, RandomSource,
    Xoroshiro128PlusPlus, XoroshiroPositionalRandomFactory,
};
pub use storage::{
    ChunkData, ChunkSection, ChunkStatus, Heightmap, HeightmapType, PalettedContainer,
};
