//! Block-entity SNBT as vanilla 26.2 saves it, for the block entities features fill in.
//!
//! Each string is what `BlockEntity.saveWithFullMetadata` writes: `id`, `x`, `y`, `z` and the
//! fields of that type's `saveAdditional` that the feature set. Fields the feature leaves at
//! their defaults are written only where vanilla always writes them (the spawner's), so loading
//! the string gives the block entity vanilla's feature pass left.

use oxide_core::BlockPos;

/// `{id:"<id>",x:..,y:..,z:..` without the closing brace.
fn head(id: &str, pos: BlockPos) -> String {
    format!("{{id:\"{id}\",x:{},y:{},z:{}", pos.x, pos.y, pos.z)
}

/// A `RandomizableContainer` (chest) or `BrushableBlockEntity` after `setLootTable(table,
/// seed)`: `LootTable`, and `LootTableSeed` unless it is 0 (`trySaveLootTable`). Items are not
/// written while a loot table is set.
pub fn with_loot_table(id: &str, pos: BlockPos, table: &str, seed: i64) -> String {
    let mut snbt = head(id, pos);
    snbt.push_str(&format!(",LootTable:\"{table}\""));
    if seed != 0 {
        snbt.push_str(&format!(",LootTableSeed:{seed}L"));
    }
    snbt.push('}');
    snbt
}

/// `BlockPos.asLong`: the seed `DesertWellFeature` gives its suspicious sand.
pub fn pos_as_long(pos: BlockPos) -> i64 {
    ((i64::from(pos.x) & 0x3FF_FFFF) << 38)
        | (i64::from(pos.y) & 0xFFF)
        | ((i64::from(pos.z) & 0x3FF_FFFF) << 12)
}

/// A `SpawnerBlockEntity` after `setEntityId(entity, random)` on a fresh spawner, as
/// `BaseSpawner.save` writes it: the defaults, `SpawnData` holding only the entity id, and
/// `SpawnPotentials` emptied (Spigot's `setEntityId` clears it; omitting it would load as a
/// one-entry list).
pub fn spawner(pos: BlockPos, entity: &str) -> String {
    let mut snbt = head("minecraft:mob_spawner", pos);
    snbt.push_str(&format!(
        ",Delay:20s,MinSpawnDelay:200s,MaxSpawnDelay:800s,SpawnCount:4s,MaxNearbyEntities:6s,\
         RequiredPlayerRange:16s,SpawnRange:4s,SpawnData:{{entity:{{id:\"{entity}\"}}}},\
         SpawnPotentials:[]}}"
    ));
    snbt
}

/// A `BeehiveBlockEntity` holding one bee per entry of `ticks_in_hive`, each
/// `Occupant.create(ticks)`: an empty bee, `min_ticks_in_hive` 600.
pub fn beehive(pos: BlockPos, ticks_in_hive: &[i32]) -> String {
    let bees: Vec<String> = ticks_in_hive
        .iter()
        .map(|ticks| {
            format!(
                "{{entity_data:{{id:\"minecraft:bee\"}},ticks_in_hive:{ticks},min_ticks_in_hive:600}}"
            )
        })
        .collect();
    let mut snbt = head("minecraft:beehive", pos);
    snbt.push_str(&format!(",bees:[{}]}}", bees.join(",")));
    snbt
}

/// The placeholder `WorldGenRegion.setBlock` stores for any block-entity block written into a
/// proto chunk; the block entity itself is created from the block when the chunk is promoted.
pub fn dummy(pos: BlockPos) -> String {
    let mut snbt = head("DUMMY", pos);
    snbt.push('}');
    snbt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pos_as_long_packs_like_block_pos() {
        assert_eq!(
            pos_as_long(BlockPos::new(1, 2, 3)),
            (1 << 38) | (3 << 12) | 2
        );
        assert_eq!(pos_as_long(BlockPos::new(-1, -1, -1)), -1);
        assert_eq!(pos_as_long(BlockPos::new(0, -64, 0)), 4096 - 64);
    }

    #[test]
    fn snbt_matches_what_vanilla_saves() {
        let pos = BlockPos::new(-5, -40, 17);
        assert_eq!(dummy(pos), r#"{id:"DUMMY",x:-5,y:-40,z:17}"#);
        assert_eq!(
            with_loot_table(
                "minecraft:chest",
                pos,
                "minecraft:chests/simple_dungeon",
                -9
            ),
            r#"{id:"minecraft:chest",x:-5,y:-40,z:17,LootTable:"minecraft:chests/simple_dungeon",LootTableSeed:-9L}"#
        );
        // `trySaveLootTable` leaves a zero seed out.
        assert!(!with_loot_table("minecraft:chest", pos, "t", 0).contains("LootTableSeed"));
        assert_eq!(
            beehive(pos, &[3, 598]),
            "{id:\"minecraft:beehive\",x:-5,y:-40,z:17,bees:[\
             {entity_data:{id:\"minecraft:bee\"},ticks_in_hive:3,min_ticks_in_hive:600},\
             {entity_data:{id:\"minecraft:bee\"},ticks_in_hive:598,min_ticks_in_hive:600}]}"
        );
        assert_eq!(
            spawner(pos, "minecraft:zombie"),
            "{id:\"minecraft:mob_spawner\",x:-5,y:-40,z:17,Delay:20s,MinSpawnDelay:200s,\
             MaxSpawnDelay:800s,SpawnCount:4s,MaxNearbyEntities:6s,RequiredPlayerRange:16s,\
             SpawnRange:4s,SpawnData:{entity:{id:\"minecraft:zombie\"}},SpawnPotentials:[]}"
        );
    }
}
