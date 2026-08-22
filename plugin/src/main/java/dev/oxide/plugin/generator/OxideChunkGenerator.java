package dev.oxide.plugin.generator;

import dev.oxide.plugin.ffi.OxideNative;
import org.bukkit.Material;
import org.bukkit.block.data.BlockData;
import org.bukkit.generator.BiomeProvider;
import org.bukkit.generator.ChunkGenerator;
import org.bukkit.generator.WorldInfo;
import org.jetbrains.annotations.NotNull;

import java.lang.foreign.Arena;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.ValueLayout;
import java.util.Random;

/**
 * Bridges Bukkit's world generation hook to {@code oxide-chunkgen} over
 * {@code oxide-ffi}. The Rust side runs noise fill *and* surface rules, so what
 * arrives here is real block variety -- grass, dirt, sand, gravel, bedrock --
 * not bare stone. Biomes come from {@link OxideBiomeProvider}, backed by the
 * same generator handle.
 *
 * <p>Still absent, and so still left at {@code ChunkGenerator}'s defaults:
 * carvers (caves/ravines), aquifers, ore veins. Structures and decoration are
 * deliberately left to vanilla -- {@code shouldGenerateStructures()} and
 * {@code shouldGenerateDecorations()} return true -- since those run on top of
 * finished terrain and vanilla's implementations work against it unchanged.
 *
 * <p>Coordinate convention for {@code ChunkData.setBlock}: {@code x}/{@code z}
 * are chunk-local (0-15), {@code y} is the world-absolute height -- the
 * long-stable Bukkit {@code ChunkGenerator.ChunkData} convention (unverified
 * against a javadoc for 26.2 specifically -- no sources jar was available to
 * check this session -- but confirmed by analogy to {@code ChunkData.getHeight}'s
 * explicit {@code @Range(0, 15)} on its x/z parameters in the same interface,
 * and otherwise unchanged since Bukkit's noise-generator API landed in 1.17).
 *
 * <p>Thread-safety: {@link #generateNoise} may be called concurrently for
 * chunks in different regions -- Folia parallelizes chunk generation across
 * regions. That's fine here: concurrent {@code oxide_generate_chunk} calls on
 * the same handle are safe (see {@link OxideNative.Handle#generateChunk}'s
 * doc) since the Rust generator is read-only after construction. What is
 * <em>not</em> handled by this class is closing the handle while a
 * generation call is still in flight -- that's the caller's (the plugin's)
 * responsibility, since only it knows when the world is actually unloaded.
 */
public final class OxideChunkGenerator extends ChunkGenerator {

    private final OxideNative.Handle handle;
    private final OxidePalette palette;

    public OxideChunkGenerator(OxideNative.Handle handle) {
        this.handle = handle;
        this.palette = new OxidePalette(handle);
    }


    @Override
    public boolean shouldGenerateNoise() {
        return true;
    }

    /** Surface rules run on the Rust side, inside the same call as the noise fill. */
    @Override
    public boolean shouldGenerateSurface() {
        return true;
    }

    /**
     * Bedrock is not a separate stage in modern vanilla -- it is a
     * {@code minecraft:vertical_gradient} rule inside the surface rule tree, which the Rust
     * side already evaluates. Letting Bukkit run its own bedrock pass on top would place a
     * second, differently-shaped bedrock layer.
     */
    @Override
    public boolean shouldGenerateBedrock() {
        return false;
    }

    /** Vanilla decorates and places structures on top of this terrain -- see the class doc. */
    @Override
    public boolean shouldGenerateDecorations() {
        return true;
    }

    @Override
    public boolean shouldGenerateStructures() {
        return true;
    }

    @Override
    public boolean shouldGenerateMobs() {
        return true;
    }

    /** Biomes come from the same handle that generated the terrain. */
    @Override
    public @NotNull BiomeProvider getDefaultBiomeProvider(@NotNull WorldInfo worldInfo) {
        return new OxideBiomeProvider(handle);
    }

    @Override
    public void generateNoise(@NotNull WorldInfo worldInfo, @NotNull Random random,
                               int chunkX, int chunkZ, @NotNull ChunkData chunkData) {
        int minY = handle.minY();
        int height = handle.height();
        long blockCount = 256L * height;
        long biomeCount = 64L * (height / 16);

        try (Arena arena = Arena.ofConfined()) {
            MemorySegment blocks = arena.allocate(blockCount * Short.BYTES);
            // Biomes cross the boundary in the same call but are consumed by
            // OxideBiomeProvider's per-position lookups, not here -- Bukkit gives a
            // ChunkGenerator no way to write the biome grid directly.
            MemorySegment biomes = arena.allocate(biomeCount * Short.BYTES);
            handle.generateChunk(chunkX, chunkZ, blocks, biomes);

            for (int z = 0; z < 16; z++) {
                for (int x = 0; x < 16; x++) {
                    placeColumn(chunkData, blocks, minY, height, x, z);
                }
            }
        }
    }

    /**
     * Writes one column as vertical runs rather than per block. A chunk is ~98k positions and
     * the great majority of them repeat the block below -- long stone runs, long air runs -- so
     * collapsing each run into a single {@code setRegion} call is what keeps this from being
     * the slowest part of generation by a wide margin.
     */
    private void placeColumn(ChunkData chunkData, MemorySegment blocks, int minY, int height,
                             int x, int z) {
        int runStart = 0;
        int runIndex = paletteIndex(blocks, 0, x, z);

        for (int localY = 1; localY <= height; localY++) {
            int index = localY < height ? paletteIndex(blocks, localY, x, z) : -1;
            if (index == runIndex) {
                continue;
            }
            BlockData data = palette.get(runIndex);
            // Air is ChunkData's default; skipping it avoids touching most of the column.
            if (data.getMaterial() != Material.AIR) {
                chunkData.setRegion(x, minY + runStart, z, x + 1, minY + localY, z + 1, data);
            }
            runStart = localY;
            runIndex = index;
        }
    }

    private static int paletteIndex(MemorySegment blocks, int localY, int x, int z) {
        long index = (long) (localY * 16 + z) * 16 + x;
        // u16 on the Rust side: mask so a high palette index does not read as negative.
        return blocks.get(ValueLayout.JAVA_SHORT, index * Short.BYTES) & 0xFFFF;
    }
}
