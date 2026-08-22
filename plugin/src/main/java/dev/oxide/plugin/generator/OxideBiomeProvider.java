package dev.oxide.plugin.generator;

import dev.oxide.plugin.ffi.OxideNative;
import org.bukkit.NamespacedKey;
import org.bukkit.Registry;
import org.bukkit.block.Biome;
import org.bukkit.generator.BiomeProvider;
import org.bukkit.generator.WorldInfo;
import org.jetbrains.annotations.NotNull;

import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;

/**
 * Answers Bukkit's per-position biome queries from the Rust multi-noise biome source, so the
 * biomes a world shows match the terrain {@link OxideChunkGenerator} generated rather than
 * whatever vanilla would have picked for the same coordinates.
 *
 * <p>Each query is a climate sample plus a search of the biome tree on the Rust side -- no
 * chunk generation -- because Bukkit calls this outside chunk generation and per position.
 */
public final class OxideBiomeProvider extends BiomeProvider {

    private final OxideNative.Handle handle;
    /** Palette index -> Bukkit biome. Indices are stable for the handle's life. */
    private final Map<Integer, Biome> resolved = new ConcurrentHashMap<>();

    public OxideBiomeProvider(OxideNative.Handle handle) {
        this.handle = handle;
    }

    @Override
    public @NotNull Biome getBiome(@NotNull WorldInfo worldInfo, int x, int y, int z) {
        int index = handle.biomeAt(x, y, z);
        return resolved.computeIfAbsent(index, i -> lookup(handle.biomePaletteName(i)));
    }

    /**
     * Every biome this provider may return. Bukkit requires a non-empty list here before any
     * chunk exists, which is why the Rust side interns the datapack's whole biome registry when
     * the handle opens rather than only as generation meets each one.
     */
    @Override
    public @NotNull List<Biome> getBiomes(@NotNull WorldInfo worldInfo) {
        List<Biome> biomes = new ArrayList<>();
        int size = handle.biomePaletteSize();
        for (int i = 0; i < size; i++) {
            Biome biome = lookup(handle.biomePaletteName(i));
            if (!biomes.contains(biome)) {
                biomes.add(biome);
            }
        }
        if (biomes.isEmpty()) {
            biomes.add(Biome.PLAINS);
        }
        return biomes;
    }

    /** Falls back to plains for an id this server has no biome for (a datapack-only biome). */
    private static Biome lookup(String id) {
        NamespacedKey key = NamespacedKey.fromString(id);
        if (key == null) {
            return Biome.PLAINS;
        }
        Biome biome = Registry.BIOME.get(key);
        return biome != null ? biome : Biome.PLAINS;
    }
}
