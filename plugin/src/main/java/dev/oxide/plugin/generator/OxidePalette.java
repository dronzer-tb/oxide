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
    private volatile BlockData[] resolved = new BlockData[512];
    private int count = 0;

    OxidePalette(OxideNative.Handle handle) {
        this.handle = handle;
    }

    /**
     * Lock-free read for interned BlockData states.
     */
    BlockData get(int index) {
        BlockData[] arr = this.resolved;
        if (index >= 0 && index < arr.length) {
            BlockData data = arr[index];
            if (data != null) {
                return data;
            }
        }
        return getSlow(index);
    }

    private synchronized BlockData getSlow(int index) {
        grow();
        BlockData[] arr = this.resolved;
        if (index >= 0 && index < arr.length) {
            BlockData data = arr[index];
            if (data != null) {
                return data;
            }
        }
        return Material.AIR.createBlockData();
    }

    private void grow() {
        int targetSize = handle.blockPaletteSize();
        if (targetSize <= count && count < resolved.length) {
            return;
        }
        int newCap = Math.max(targetSize + 128, resolved.length * 2);
        BlockData[] next = new BlockData[newCap];
        System.arraycopy(resolved, 0, next, 0, count);
        for (int i = count; i < targetSize; i++) {
            String state = handle.blockPaletteName(i);
            BlockData data;
            try {
                data = Bukkit.createBlockData(state);
            } catch (IllegalArgumentException e) {
                data = Material.AIR.createBlockData();
            }
            next[i] = data;
        }
        this.count = targetSize;
        this.resolved = next;
    }
}
