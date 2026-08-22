package dev.oxide.plugin.provenance;

/**
 * Pure integer math mapping a chunk coordinate to its region file and to the
 * bit position inside that region's Oxide sidecar bitmap. No I/O, no Bukkit
 * types — kept separate from everything else so it is trivially unit
 * testable without a running server.
 */
public final class RegionCoords {

    /** Chunks per region file edge (Anvil regions are 32x32 chunks). */
    public static final int REGION_SIZE = 32;

    private RegionCoords() {
    }

    /**
     * Region coordinate containing the given chunk coordinate.
     *
     * Uses an arithmetic right shift (>>), NOT division, because Java's `/`
     * truncates toward zero and gives the wrong region for negative chunk
     * coordinates (e.g. -1 / 32 == 0, but chunk -1 belongs to region -1).
     * Arithmetic shift floors toward negative infinity, matching the
     * vanilla Anvil region layout.
     */
    public static int regionCoord(int chunkCoord) {
        return chunkCoord >> 5;
    }

    /**
     * Chunk coordinate local to its region, in [0, 32). Math.floorMod (not
     * the `%` operator) is required for the same negative-number reason as
     * regionCoord: -1 % 32 == -1 in Java, but the local coordinate must be
     * 31.
     */
    public static int localCoord(int chunkCoord) {
        return Math.floorMod(chunkCoord, REGION_SIZE);
    }

    /**
     * Bit index into the 1024-bit sidecar bitmap for a chunk's local
     * coordinates, per the Oxide sidecar format: index = localZ * 32 + localX.
     */
    public static int bitIndex(int localX, int localZ) {
        return localZ * REGION_SIZE + localX;
    }
}
