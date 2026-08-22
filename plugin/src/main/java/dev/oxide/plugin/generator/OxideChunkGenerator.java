package dev.oxide.plugin.generator;

import dev.oxide.plugin.ffi.OxideNative;
import org.bukkit.Material;
import org.bukkit.generator.ChunkGenerator;
import org.bukkit.generator.WorldInfo;
import org.jetbrains.annotations.NotNull;

import java.lang.foreign.Arena;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.ValueLayout;
import java.util.Random;

/**
 * Bridges Bukkit's world generation hook to {@code oxide-chunkgen} over
 * {@code oxide-ffi}. See {@code docs/ARCHITECTURE.md}'s wave 5 and
 * {@code oxide-ffi/src/lib.rs}'s module doc for exactly what this does and
 * doesn't produce yet: noise-shaped solid/fluid/air only -- no surface-rule
 * block variety, no biome coloring, no caves, no structures, no bedrock.
 * Every {@code shouldGenerate*} override is left at {@code ChunkGenerator}'s
 * default ({@code false}) except {@link #shouldGenerateNoise()} -- that's
 * an honest reflection of what's actually implemented, not an oversight.
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
    private final Material blockMaterial;
    private final Material fluidMaterial;

    public OxideChunkGenerator(OxideNative.Handle handle) {
        this.handle = handle;
        this.blockMaterial = resolveMaterial(handle.defaultBlockName());
        this.fluidMaterial = resolveMaterial(handle.defaultFluidName());
    }

    private static Material resolveMaterial(String namespacedId) {
        Material material = Material.matchMaterial(namespacedId);
        if (material == null) {
            throw new IllegalStateException(
                    "oxide-ffi reported an unknown block id: " + namespacedId
                            + " (Material.matchMaterial found nothing for it -- is this a"
                            + " modded/datapack-only block with no vanilla Material entry?)");
        }
        return material;
    }

    @Override
    public boolean shouldGenerateNoise() {
        return true;
    }

    @Override
    public void generateNoise(@NotNull WorldInfo worldInfo, @NotNull Random random,
                               int chunkX, int chunkZ, @NotNull ChunkData chunkData) {
        int minY = handle.minY();
        int height = handle.height();
        long bufferLen = 256L * height;

        try (Arena arena = Arena.ofConfined()) {
            MemorySegment buf = arena.allocate(bufferLen);
            handle.generateChunk(chunkX, chunkZ, buf);

            for (int localY = 0; localY < height; localY++) {
                int worldY = minY + localY;
                for (int z = 0; z < 16; z++) {
                    for (int x = 0; x < 16; x++) {
                        long index = (long) (localY * 16 + z) * 16 + x;
                        byte value = buf.get(ValueLayout.JAVA_BYTE, index);
                        if (value == 0) {
                            continue; // air is ChunkData's default -- nothing to set
                        }
                        chunkData.setBlock(x, worldY, z, value == 1 ? blockMaterial : fluidMaterial);
                    }
                }
            }
        }
    }
}
