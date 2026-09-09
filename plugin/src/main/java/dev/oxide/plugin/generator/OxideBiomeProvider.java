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

/**
 * Answers Bukkit's per-position biome queries from the Rust multi-noise biome source,
 * with zero-allocation pre-resolved palette indexing.
 */
public final class OxideBiomeProvider extends BiomeProvider {

    private final OxideNative.Handle handle;
    private final Biome[] paletteBiomes;
    private final List<Biome> allBiomes;

    public OxideBiomeProvider(OxideNative.Handle handle) {
        this.handle = handle;
        int size = handle.biomePaletteSize();
        this.paletteBiomes = new Biome[Math.max(size, 512)];
        this.allBiomes = new ArrayList<>();

        for (int i = 0; i < size; i++) {
            String name = handle.biomePaletteName(i);
            Biome biome = lookup(name);
            this.paletteBiomes[i] = biome;
            if (!allBiomes.contains(biome)) {
                allBiomes.add(biome);
            }
        }
        if (allBiomes.isEmpty()) {
            allBiomes.add(Biome.PLAINS);
        }
    }

    @Override
    public @NotNull Biome getBiome(@NotNull WorldInfo worldInfo, int x, int y, int z) {
        int index = handle.biomeAt(x, y, z);
        if (index >= 0 && index < paletteBiomes.length) {
            Biome b = paletteBiomes[index];
            if (b != null) return b;
        }
        return Biome.PLAINS;
    }

    @Override
    public @NotNull List<Biome> getBiomes(@NotNull WorldInfo worldInfo) {
        return allBiomes;
    }

    /** Falls back to plains for an id this server has no biome for (a datapack-only biome). */
    private static Biome lookup(String id) {
        if (id == null || id.isBlank()) {
            return Biome.PLAINS;
        }
        NamespacedKey key = NamespacedKey.fromString(id);
        if (key == null) {
            return Biome.PLAINS;
        }
        Biome biome = Registry.BIOME.get(key);
        return biome != null ? biome : Biome.PLAINS;
    }
}
