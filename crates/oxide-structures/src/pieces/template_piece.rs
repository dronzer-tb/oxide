//! `TemplateStructurePiece`: a template placed with fixed `StructurePlaceSettings`, the base of
//! igloos, shipwrecks, ocean ruins, ruined portals, nether fossils and end cities.

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use oxide_chunkgen::decorator::{
    block_entity::with_loot_table, place_in_world, randomizable_container_id, BlockNbt,
    BoundingBox as Bounds, PlaceSettings, Processor, Template, TemplateWorld,
};
use oxide_core::rotation::{transform, Mirror, Rotation};
use oxide_core::{BlockPos, BlockState, RandomSource, ResourceLocation};
use oxide_datapack::{BlockFacts, Datapack, TagSet};

use super::PieceWorld;
use crate::BoundingBox;

/// `StructureTemplateManager.getOrCreate`: a missing template is an empty one.
pub trait PieceTemplates: Sync {
    fn get(&self, id: &ResourceLocation) -> Arc<Template>;
}

/// Templates read from a datapack's `structure/` files, each parsed once.
pub struct DatapackTemplates<'d> {
    datapack: &'d Datapack,
    facts: &'d BlockFacts,
    cache: Mutex<HashMap<ResourceLocation, Arc<Template>>>,
}

impl<'d> DatapackTemplates<'d> {
    pub fn new(datapack: &'d Datapack, facts: &'d BlockFacts) -> Self {
        Self {
            datapack,
            facts,
            cache: Mutex::new(HashMap::new()),
        }
    }
}

impl PieceTemplates for DatapackTemplates<'_> {
    fn get(&self, id: &ResourceLocation) -> Arc<Template> {
        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        cache
            .entry(id.clone())
            .or_insert_with(|| {
                // An unreadable file counts as a missing one, which is what vanilla's `tryLoad`
                // does after logging the error.
                let raw = self.datapack.structure_template(id).ok().flatten();
                Arc::new(raw.map_or_else(Template::default, |raw| {
                    Template::from_raw(&raw, self.facts)
                }))
            })
            .clone()
    }
}

/// What template structures are generated with besides [`super::GenerationContext`]: the
/// template manager, and the block tags their processors test the level against.
pub struct TemplateAssets<'a> {
    pub templates: &'a dyn PieceTemplates,
    pub block_tags: Arc<TagSet>,
}

/// What generation reads of the world beyond heights: `ChunkGenerator.getBaseColumn` (the noise
/// stage of a column, before surface rules, carvers or features) and the noise biome's climate.
pub trait BaseColumns: Sync {
    /// `getBaseColumn(x, z).getBlock(y)`.
    fn block(&self, x: i32, y: i32, z: i32) -> BlockState;
    /// `Biome.coldEnoughToSnow(pos, seaLevel)` for the noise biome at `pos`.
    fn cold_enough_to_snow(&self, pos: BlockPos) -> bool;
}

/// A template, where it goes, and how.
#[derive(Debug, Clone)]
pub struct TemplatePiece {
    pub id: ResourceLocation,
    pub template: Arc<Template>,
    /// `templatePosition`.
    pub position: BlockPos,
    pub rotation: Rotation,
    pub mirror: Mirror,
    pub pivot: BlockPos,
    pub processors: Vec<Processor>,
    pub ignore_entities: bool,
    /// `LiquidSettings.APPLY_WATERLOGGING`, the default.
    pub apply_waterlogging: bool,
    block_tags: Arc<TagSet>,
    seed: i64,
}

pub(crate) fn to_bounds(b: BoundingBox) -> Bounds {
    Bounds {
        min: BlockPos::new(b.min_x, b.min_y, b.min_z),
        max: BlockPos::new(b.max_x, b.max_y, b.max_z),
    }
}

pub(crate) fn offset(pos: BlockPos, dx: i32, dy: i32, dz: i32) -> BlockPos {
    BlockPos::new(pos.x + dx, pos.y + dy, pos.z + dz)
}

pub(crate) fn minecraft(path: &str) -> ResourceLocation {
    ResourceLocation::minecraft(path)
}

impl TemplatePiece {
    pub fn new(
        assets: &TemplateAssets<'_>,
        seed: i64,
        id: ResourceLocation,
        position: BlockPos,
        rotation: Rotation,
        processors: Vec<Processor>,
    ) -> Self {
        Self {
            template: assets.templates.get(&id),
            id,
            position,
            rotation,
            mirror: Mirror::None,
            pivot: BlockPos::new(0, 0, 0),
            processors,
            ignore_entities: false,
            apply_waterlogging: true,
            block_tags: assets.block_tags.clone(),
            seed,
        }
    }

    /// `template.getBoundingBox(placeSettings, templatePosition)`.
    pub fn bounding_box(&self) -> BoundingBox {
        self.bounding_box_at(self.position)
    }

    pub fn bounding_box_at(&self, position: BlockPos) -> BoundingBox {
        let b = self
            .template
            .bounding_box_with(position, self.rotation, self.mirror, self.pivot);
        BoundingBox::new(b.min.x, b.min.y, b.min.z, b.max.x, b.max.y, b.max.z)
    }

    /// The template's size, unrotated (`template.getSize()`).
    pub fn size(&self) -> [i32; 3] {
        self.template.raw_size()
    }

    /// `StructureTemplate.calculateRelativePosition(settings, pos)`.
    pub fn relative(&self, pos: BlockPos) -> BlockPos {
        transform(pos, self.mirror, self.rotation, self.pivot)
    }

    fn settings(&self, chunk_box: &BoundingBox) -> PlaceSettings<'_> {
        PlaceSettings {
            rotation: self.rotation,
            mirror: self.mirror,
            pivot: self.pivot,
            bounding_box: Some(to_bounds(*chunk_box)),
            processors: &self.processors,
            positional_random: true,
            apply_waterlogging: self.apply_waterlogging,
            ignore_entities: self.ignore_entities,
        }
    }

    /// `TemplateStructurePiece.postProcess` with the template at `position`: place it, hand each
    /// data marker to `on_marker`, then turn each jigsaw block into its `final_state`.
    pub fn place(
        &self,
        world: &mut PieceWorld<'_, '_>,
        random: &mut dyn RandomSource,
        chunk_box: &BoundingBox,
        position: BlockPos,
        mut on_marker: impl FnMut(&mut PieceWorld<'_, '_>, &mut dyn RandomSource, &str, BlockPos),
    ) {
        let settings = self.settings(chunk_box);
        let facts = world.region.facts();
        // PARITY-CHECK: vanilla passes `placeInChunk`'s reference position as `placeInWorld`'s
        // second position, which only position-predicated processor rules read; every rule
        // these structures use has the default `always_true` predicate.
        let placed = place_in_world(
            &self.template,
            &self.id,
            &mut TemplateWorld {
                region: &mut *world.region,
                block_tags: &self.block_tags,
                facts,
                seed: self.seed,
                random: &mut *random,
                unsupported: None,
            },
            position,
            &settings,
        );
        if !placed {
            return;
        }
        let structure_block = minecraft("structure_block");
        for (pos, info) in self
            .template
            .filter_blocks(position, &settings, &structure_block)
        {
            if let Some(BlockNbt::Template(index)) = &info.nbt {
                if let Some(marker) = &self.template.nbt(*index).data_marker {
                    on_marker(world, random, marker, pos);
                }
            }
        }
        let jigsaw = minecraft("jigsaw");
        for (pos, info) in self.template.filter_blocks(position, &settings, &jigsaw) {
            let Some(BlockNbt::Template(index)) = &info.nbt else {
                continue;
            };
            if let Some(spec) = &self.template.nbt(*index).final_state {
                world.set_block(pos, parse_state(spec, facts));
            }
        }
    }
}

/// `BlockStateParser.parseForBlock` of a jigsaw's `final_state`, completed with the block's
/// defaults; an unparsable one is air, as vanilla logs and falls back to.
fn parse_state(spec: &str, facts: &BlockFacts) -> BlockState {
    let (name, properties) = match spec.split_once('[') {
        Some((name, rest)) => (name, rest.trim_end_matches(']')),
        None => (spec, ""),
    };
    let Ok(name) = ResourceLocation::from_str(name) else {
        return BlockState::new(minecraft("air"));
    };
    let mut state = BlockState::new(name);
    for pair in properties.split(',').filter(|pair| !pair.is_empty()) {
        if let Some((key, value)) = pair.split_once('=') {
            state = state.with_property(key.trim(), value.trim());
        }
    }
    facts.complete(&state)
}

/// `RandomizableContainer.setBlockEntityLootTable(level, random, pos, table)`: only a container
/// standing at `pos` takes the table, and only then is a seed drawn.
pub(crate) fn set_loot_table(
    world: &mut PieceWorld<'_, '_>,
    random: &mut dyn RandomSource,
    pos: BlockPos,
    table: &str,
) {
    let Some(id) = world
        .region
        .block(pos)
        .and_then(|state| randomizable_container_id(&state.name))
    else {
        return;
    };
    let seed = random.next_long();
    world.put_block_entity(pos, with_loot_table(id, pos, table, seed));
}

/// Layouts checked against starts the real game generated: `scripts/StructureProbe.java` writes
/// each start's saved tag under the git-ignored `reference/oracle-structures/`.
#[cfg(test)]
pub(crate) mod oracle {
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, OnceLock};

    use oxide_core::rotation::Rotation;
    use oxide_core::{BiomeId, ChunkPos, HeightmapType};
    use oxide_datapack::{load_tags, BlockFacts, Datapack};

    use super::{DatapackTemplates, TemplateAssets, TemplatePiece};
    use crate::pieces::{GenerationContext, StructureTerrain};

    pub const SEED: i64 = 883502277;

    fn reference() -> Option<PathBuf> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../reference");
        root.join("oracle-structures").is_dir().then_some(root)
    }

    struct Loaded {
        datapack: Datapack,
        facts: BlockFacts,
        tags: Arc<oxide_datapack::TagSet>,
    }

    fn loaded(root: &Path) -> &'static Loaded {
        static LOADED: OnceLock<Loaded> = OnceLock::new();
        LOADED.get_or_init(|| {
            let datapack = oxide_datapack::load_datapack(root).expect("reference datapack");
            Loaded {
                facts: datapack.block_facts.clone(),
                tags: Arc::new(load_tags(&[root], "block").expect("block tags")),
                datapack,
            }
        })
    }

    /// Flat terrain: what the layouts tested here read of the world besides the random.
    pub struct Flat(pub i32);

    impl StructureTerrain for Flat {
        fn sea_level(&self) -> i32 {
            63
        }
        fn min_y(&self) -> i32 {
            -64
        }
        fn height(&self) -> i32 {
            384
        }
        fn base_height(&self, _x: i32, _z: i32, _kind: HeightmapType) -> i32 {
            self.0
        }
        fn noise_biome(&self, _x: i32, _y: i32, _z: i32) -> BiomeId {
            BiomeId::minecraft("plains")
        }
    }

    /// One piece as the game saved it.
    #[derive(Debug, PartialEq)]
    pub struct SavedPiece {
        pub template: String,
        pub position: [i32; 3],
        pub rotation: String,
        pub bounding_box: [i32; 6],
    }

    fn field<'a>(child: &'a str, key: &str) -> Option<&'a str> {
        let at = child.find(&format!("{key}:"))? + key.len() + 1;
        let rest = &child[at..];
        let end = if let Some(quoted) = rest.strip_prefix('"') {
            quoted.find('"')? + 2
        } else {
            rest.find([',', '}', ']']).unwrap_or(rest.len())
        };
        Some(rest[..end].trim_matches('"'))
    }

    fn parse_child(child: &str) -> SavedPiece {
        let int = |key| field(child, key).unwrap().parse::<i32>().unwrap();
        let bb = &child[child.find("BB:[I;").unwrap() + 6..];
        let bb: Vec<i32> = bb[..bb.find(']').unwrap()]
            .split(',')
            .map(|v| v.parse().unwrap())
            .collect();
        SavedPiece {
            template: field(child, "Template").unwrap().to_owned(),
            position: [int("TPX"), int("TPY"), int("TPZ")],
            rotation: field(child, "Rot").unwrap().to_owned(),
            bounding_box: bb.try_into().unwrap(),
        }
    }

    /// Every saved start of `structure`: its chunk and its pieces.
    pub fn starts(structure: &str) -> Vec<(ChunkPos, Vec<SavedPiece>)> {
        let Some(root) = reference() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for dim in ["overworld", "the_nether", "the_end"] {
            let path = root.join(format!("oracle-structures/{SEED}/{dim}.txt"));
            let Ok(text) = std::fs::read_to_string(path) else {
                continue;
            };
            for line in text.lines() {
                let mut parts = line.splitn(5, ' ');
                let (_, id) = (parts.next(), parts.next().unwrap());
                if id != structure {
                    continue;
                }
                let x = parts.next().unwrap().parse().unwrap();
                let z = parts.next().unwrap().parse().unwrap();
                let tag = parts.next().unwrap();
                let children = &tag[tag.find("Children:[").unwrap() + 10..];
                let pieces = children.split("},{").map(parse_child).collect();
                out.push((ChunkPos { x, z }, pieces));
            }
        }
        out
    }

    /// Generates every saved start of `structure` on flat terrain at `height` and compares each
    /// piece's template, position, rotation and box with the game's.
    pub fn check_layouts_at(
        structure: &str,
        height: i32,
        generate: impl Fn(&GenerationContext<'_>, &TemplateAssets<'_>) -> Option<Vec<TemplatePiece>>,
    ) {
        let Some(root) = reference() else { return };
        let starts = starts(structure);
        assert!(!starts.is_empty(), "no saved {structure} starts");
        let loaded = loaded(&root);
        let templates = DatapackTemplates::new(&loaded.datapack, &loaded.facts);
        let assets = TemplateAssets {
            templates: &templates,
            block_tags: loaded.tags.clone(),
        };
        let terrain = Flat(height);
        for (chunk, saved) in starts {
            let ctx = GenerationContext {
                seed: SEED,
                chunk,
                terrain: &terrain,
            };
            let generated = generate(&ctx, &assets).expect("vanilla kept this start");
            let ours: Vec<_> = generated.iter().map(TemplatePiece::saved).collect();
            assert_eq!(ours, saved, "{structure} at {chunk:?}");
        }
    }

    pub fn check_layouts(
        structure: &str,
        generate: impl Fn(&GenerationContext<'_>, &TemplateAssets<'_>) -> Option<Vec<TemplatePiece>>,
    ) {
        check_layouts_at(structure, 64, generate);
    }

    impl TemplatePiece {
        /// What the game saves of this piece.
        pub(crate) fn saved(&self) -> SavedPiece {
            let b = self.bounding_box();
            SavedPiece {
                template: self.id.to_string(),
                position: [self.position.x, self.position.y, self.position.z],
                rotation: match self.rotation {
                    Rotation::None => "NONE",
                    Rotation::Clockwise90 => "CLOCKWISE_90",
                    Rotation::Clockwise180 => "CLOCKWISE_180",
                    Rotation::Counterclockwise90 => "COUNTERCLOCKWISE_90",
                }
                .to_owned(),
                bounding_box: [b.min_x, b.min_y, b.min_z, b.max_x, b.max_y, b.max_z],
            }
        }
    }
}
