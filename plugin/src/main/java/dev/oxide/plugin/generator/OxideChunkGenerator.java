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
import java.util.Map;
import java.util.Random;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicLong;
import java.util.logging.Level;
import java.util.logging.Logger;

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

    /**
     * Cap on chunks generated-but-not-yet-placed. Each entry is one short per block --
     * ~196 KB for a 384-tall world -- so this bounds the pending set at roughly 12 MB.
     * Past it, a chunk is generated inline in {@link #generateNoise} instead of ahead of
     * time, giving up per-chunk fallback for that chunk rather than growing without limit.
     */
    private static final int MAX_PENDING_CHUNKS = 64;

    private final OxideNative.Handle handle;
    private final OxidePalette palette;
    private final Logger logger;
    /** Chunk key -> palette indices generated in shouldGenerateNoise, consumed by generateNoise. */
    private final Map<Long, short[]> pending = new ConcurrentHashMap<>();
    private final AtomicLong failures = new AtomicLong();

    public OxideChunkGenerator(OxideNative.Handle handle, Logger logger) {
        this.handle = handle;
        this.logger = logger;
        this.palette = new OxidePalette(handle);
    }

    /**
     * Generates the chunk here, one stage early, so that a failure can still be answered by
     * vanilla: this is the last point at which the server can be told to generate the chunk
     * itself. Returning {@code true} hands this one chunk to the vanilla generator
     * ("delegate to the Vanilla generator", per {@code ChunkGenerator}'s javadoc);
     * {@code false} means Oxide's data -- already computed and stashed in {@link #pending} --
     * is authoritative and vanilla should not waste the work.
     *
     * <p>This is fallback on <em>failure</em>, not on divergence. Verifying that Oxide's output
     * matches vanilla's would mean running vanilla's generator for every chunk to have
     * something to compare against, which costs more than it saves; that check belongs in the
     * offline harness, and at runtime only as sampling. See plugin/README.md.
     */
    @Override
    public boolean shouldGenerateNoise(@NotNull WorldInfo worldInfo, @NotNull Random random,
                                       int chunkX, int chunkZ) {
        if (pending.size() >= MAX_PENDING_CHUNKS) {
            // Generation is outrunning placement. Let generateNoise do the work inline.
            return false;
        }
        try {
            pending.put(chunkKey(chunkX, chunkZ), generateIndices(chunkX, chunkZ));
            return false;
        } catch (RuntimeException e) {
            reportFailure(chunkX, chunkZ, e);
            return true;
        }
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
        short[] blocks = pending.remove(chunkKey(chunkX, chunkZ));
        if (blocks == null) {
            // Either the pending cap was hit, or the server called the no-argument
            // shouldGenerateNoise() and never the per-chunk one. Generate inline: writing
            // nothing here would leave a hole in the world, which is worse than losing the
            // per-chunk fallback for this one chunk.
            try {
                blocks = generateIndices(chunkX, chunkZ);
            } catch (RuntimeException e) {
                reportFailure(chunkX, chunkZ, e);
                return;
            }
        }

        int minY = handle.minY();
        int height = handle.height();
        for (int z = 0; z < 16; z++) {
            for (int x = 0; x < 16; x++) {
                placeColumn(chunkData, blocks, minY, height, x, z);
            }
        }
    }

    /** Runs the Rust generator and copies its palette indices out of native memory. */
    private short[] generateIndices(int chunkX, int chunkZ) {
        int height = handle.height();
        int blockCount = 256 * height;
        int biomeCount = 64 * (height / 16);

        try (Arena arena = Arena.ofConfined()) {
            MemorySegment blocks = arena.allocate((long) blockCount * Short.BYTES);
            // Biomes cross the boundary in the same call but are consumed by
            // OxideBiomeProvider's per-position lookups, not here -- Bukkit gives a
            // ChunkGenerator no way to write the biome grid directly.
            MemorySegment biomes = arena.allocate((long) biomeCount * Short.BYTES);
            handle.generateChunk(chunkX, chunkZ, blocks, biomes);
            return blocks.toArray(ValueLayout.JAVA_SHORT);
        }
    }

    /**
     * Logs the first failure in full and then only every 100th, since a failure here is
     * usually systemic (a bad datapack, a closed handle) and would otherwise flood the log at
     * chunk-generation rate.
     */
    private void reportFailure(int chunkX, int chunkZ, RuntimeException e) {
        long count = failures.incrementAndGet();
        if (count == 1 || count % 100 == 0) {
            logger.log(Level.SEVERE, "oxide generation failed for chunk " + chunkX + ", " + chunkZ
                    + " (failure #" + count + "); this chunk falls back to vanilla generation", e);
        }
    }

    private static long chunkKey(int chunkX, int chunkZ) {
        return ((long) chunkX << 32) ^ (chunkZ & 0xFFFFFFFFL);
    }

    /**
     * Writes one column as vertical runs rather than per block. A chunk is ~98k positions and
     * the great majority of them repeat the block below -- long stone runs, long air runs -- so
     * collapsing each run into a single {@code setRegion} call is what keeps this from being
     * the slowest part of generation by a wide margin.
     */
    private void placeColumn(ChunkData chunkData, short[] blocks, int minY, int height,
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

    private static int paletteIndex(short[] blocks, int localY, int x, int z) {
        // u16 on the Rust side: mask so a high palette index does not read as negative.
        return blocks[(localY * 16 + z) * 16 + x] & 0xFFFF;
    }
}
