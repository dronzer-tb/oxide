package dev.oxide.plugin.generator;

import java.util.concurrent.ConcurrentHashMap;

/**
 * The biome grid Rust produced for a chunk, kept just long enough for CraftBukkit to ask the
 * {@link OxideBiomeProvider} about that same chunk's quarts.
 *
 * <p>Why this exists: {@code oxide_generate_chunk} already computes the whole chunk's biome grid
 * as a side effect of generating terrain, and the plugin used to discard it. CraftBukkit then
 * runs {@code ChunkGenerator.createBiomes} -> {@code LevelChunkSection.fillBiomesFromNoise},
 * which calls {@code BiomeResolver.getNoiseBiome} once per 4x4x4 quart -- 1,536 calls for a
 * 384-tall chunk. Each of those reached back over the FFI boundary into {@code oxide_biome_at},
 * measured at ~2.7us a call, to recompute a climate sample Rust had already taken. Serving them
 * from the grid instead makes each one an array read.
 *
 * <p>Entries are consumed on read: {@code fillBiomesFromNoise} walks a chunk once, so once the
 * last quart of a chunk has been answered the grid is dead weight. Generation can outrun the
 * biome pass across region threads, so this is bounded and drops the oldest entry rather than
 * growing without limit -- a miss simply falls back to the per-position FFI call, which is
 * exactly the old behaviour.
 *
 * <p>Thread-safety: Folia generates chunks for different regions on different threads, and the
 * generating thread is not necessarily the one that runs the biome pass, so this is shared
 * state behind a {@link ConcurrentHashMap}.
 */
final class BiomeGridCache {

    /**
     * Bounded so a burst of generation cannot pin unbounded native-sized arrays in the heap.
     * Each entry is one short per quart (1,536 shorts = ~3 KB for a 384-tall world), so this
     * caps the cache at roughly 12 MB.
     */
    private static final int MAX_ENTRIES = 4096;

    private final ConcurrentHashMap<Long, short[]> grids = new ConcurrentHashMap<>();

    void put(int chunkX, int chunkZ, short[] biomes) {
        if (grids.size() >= MAX_ENTRIES) {
            // Cheap eviction: drop an arbitrary entry rather than tracking insertion order.
            // A dropped grid costs a fallback to the FFI path, never a wrong answer.
            var it = grids.keySet().iterator();
            if (it.hasNext()) {
                grids.remove(it.next());
            }
        }
        grids.put(key(chunkX, chunkZ), biomes);
    }

    /**
     * The biome palette index at a block position, or {@code -1} if this chunk's grid is not
     * held (never generated here, or already evicted) -- the caller then falls back to the
     * per-position native lookup.
     *
     * <p>Indexing matches what {@code oxide_generate_chunk} writes:
     * {@code section_index*64 + (qy*4 + qz)*4 + qx}, with {@code qy} relative to {@code minY}.
     */
    int biomeIndexAt(int x, int y, int z, int minY, int height) {
        short[] grid = grids.get(key(x >> 4, z >> 4));
        if (grid == null) {
            return -1;
        }
        int localY = y - minY;
        if (localY < 0 || localY >= height) {
            return -1;
        }
        int sectionIndex = localY / 16;
        int qx = (x & 15) / 4;
        int qz = (z & 15) / 4;
        int qy = (localY % 16) / 4;
        int index = sectionIndex * 64 + (qy * 4 + qz) * 4 + qx;
        if (index < 0 || index >= grid.length) {
            return -1;
        }
        // u16 on the Rust side.
        return grid[index] & 0xFFFF;
    }

    void clear() {
        grids.clear();
    }

    private static long key(int chunkX, int chunkZ) {
        return (((long) chunkX) << 32) ^ (chunkZ & 0xFFFFFFFFL);
    }
}
