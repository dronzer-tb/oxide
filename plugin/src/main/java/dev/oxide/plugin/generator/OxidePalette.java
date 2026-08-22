package dev.oxide.plugin.generator;

import dev.oxide.plugin.ffi.OxideNative;
import org.bukkit.Bukkit;
import org.bukkit.Material;
import org.bukkit.block.data.BlockData;

import java.util.ArrayList;
import java.util.List;

/**
 * Caches the {@link BlockData} for each entry of a handle's block-state palette.
 *
 * <p>The Rust side hands back one {@code u16} index per block and grows its palette as
 * generation meets new states; those indices never change meaning, so each one is resolved
 * through {@link Bukkit#createBlockData(String)} exactly once here rather than per block. That
 * matters: a chunk is ~98k block positions, and parsing {@code minecraft:grass_block[snowy=false]}
 * that many times per chunk would dwarf the generation it is reporting.
 *
 * <p>Thread-safety: Folia generates chunks for different regions concurrently, so reads and
 * growth both happen under this object's monitor. The lock is held only while resolving newly
 * seen indices, never while placing blocks.
 */
final class OxidePalette {

    private final OxideNative.Handle handle;
    private final List<BlockData> resolved = new ArrayList<>();

    OxidePalette(OxideNative.Handle handle) {
        this.handle = handle;
    }

    /**
     * {@link BlockData} for a palette index, resolving any entries interned since the last
     * call. Falls back to air for a state this server has no block for -- a datapack-only
     * block, say -- rather than failing the whole chunk.
     */
    synchronized BlockData get(int index) {
        if (index >= resolved.size()) {
            grow();
        }
        if (index < 0 || index >= resolved.size()) {
            return Material.AIR.createBlockData();
        }
        return resolved.get(index);
    }

    private void grow() {
        int size = handle.blockPaletteSize();
        for (int i = resolved.size(); i < size; i++) {
            String state = handle.blockPaletteName(i);
            BlockData data;
            try {
                data = Bukkit.createBlockData(state);
            } catch (IllegalArgumentException e) {
                data = Material.AIR.createBlockData();
            }
            resolved.add(data);
        }
    }
}
