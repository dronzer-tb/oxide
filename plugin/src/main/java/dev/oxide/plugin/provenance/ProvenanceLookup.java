package dev.oxide.plugin.provenance;

import java.io.File;
import java.nio.file.Path;

/**
 * Glue between a world's on-disk region layout and the pure sidecar-cache /
 * region-math logic. This is the only class in this package that assumes
 * anything about how a Bukkit World is laid out on disk.
 */
public final class ProvenanceLookup {

    private final SidecarCache cache;

    public ProvenanceLookup(SidecarCache cache) {
        this.cache = cache;
    }

    public record Result(
            int chunkX,
            int chunkZ,
            int regionX,
            int regionZ,
            Path regionFile,
            boolean sidecarPresent,
            boolean sidecarParseError,
            boolean bitSet,
            ChunkProvenance provenance) {
    }

    /**
     * @param worldFolder the world's own folder, i.e. {@code World#getWorldFolder()} —
     *                     already dimension-specific for nether/end on Paper/Folia,
     *                     so no special-casing needed here.
     */
    public Result lookup(String worldName, File worldFolder, int chunkX, int chunkZ) {
        int regionX = RegionCoords.regionCoord(chunkX);
        int regionZ = RegionCoords.regionCoord(chunkZ);
        Path regionFile = worldFolder.toPath()
                .resolve("region")
                .resolve("r." + regionX + "." + regionZ + ".mca");

        SidecarCache.Lookup lookup = cache.get(worldName, regionX, regionZ, regionFile);

        if (!lookup.present()) {
            return new Result(chunkX, chunkZ, regionX, regionZ, regionFile,
                    false, false, false, ChunkProvenance.JAVA);
        }

        SidecarParseResult parsed = lookup.result();
        if (parsed.status() == SidecarParseResult.Status.ERROR) {
            return new Result(chunkX, chunkZ, regionX, regionZ, regionFile,
                    true, true, false, ChunkProvenance.UNKNOWN);
        }

        int localX = RegionCoords.localCoord(chunkX);
        int localZ = RegionCoords.localCoord(chunkZ);
        int bit = RegionCoords.bitIndex(localX, localZ);
        boolean set = SidecarParser.bit(parsed.bitmap(), bit);
        return new Result(chunkX, chunkZ, regionX, regionZ, regionFile,
                true, false, set, set ? ChunkProvenance.OXIDE : ChunkProvenance.JAVA);
    }
}
