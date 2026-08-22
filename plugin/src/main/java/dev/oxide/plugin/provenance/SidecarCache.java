package dev.oxide.plugin.provenance;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.attribute.FileTime;
import java.util.concurrent.ConcurrentHashMap;
import java.util.logging.Logger;

/**
 * In-memory cache of parsed sidecar files, keyed by world name + region
 * coordinates.
 *
 * Thread-safety: backed by {@link ConcurrentHashMap}. Folia runs many
 * region threads concurrently, and players standing in different regions
 * (or even the same region, briefly, around a cross-boundary move) can
 * trigger a lookup at the same instant. ConcurrentHashMap gives lock-free
 * reads and per-bucket-safe writes without a single global lock guarding
 * every cached region. Cache entries ({@link CacheEntry}) are immutable
 * records — a reader that gets one back from get() never observes a
 * partially-updated bitmap; a refresh always installs a brand new entry
 * rather than mutating one in place.
 *
 * Invalidation is by file mtime, checked on every lookup, deliberately NOT
 * "read once and cache forever": if a pregen run rewrites a region's
 * sidecar, the mtime changes, and the next lookup against that region sees
 * the new data. Two threads racing to refresh the same stale key both
 * re-read + re-parse (each installs an equivalent fresh entry) rather than
 * one blocking the other — reading/parsing 136 bytes twice is negligible
 * cost, and it avoids doing filesystem I/O inside a ConcurrentHashMap
 * compute() callback (which would hold that bucket's lock across the I/O).
 */
public final class SidecarCache {

    private final ConcurrentHashMap<RegionKey, CacheEntry> cache = new ConcurrentHashMap<>();
    private final Logger logger;

    public SidecarCache(Logger logger) {
        this.logger = logger;
    }

    public record RegionKey(String worldName, int regionX, int regionZ) {
    }

    /** present == false means "no sidecar file" — normal, not an error. */
    public record Lookup(boolean present, SidecarParseResult result) {
        static final Lookup ABSENT = new Lookup(false, null);
    }

    private record CacheEntry(FileTime mtime, Lookup lookup) {
    }

    /**
     * @param regionFile path to the .mca region file; the sidecar is
     *                   `<regionFile>.oxide` next to it.
     */
    public Lookup get(String worldName, int regionX, int regionZ, Path regionFile) {
        Path sidecarFile = regionFile.resolveSibling(regionFile.getFileName().toString() + ".oxide");
        RegionKey key = new RegionKey(worldName, regionX, regionZ);

        FileTime currentMtime;
        try {
            currentMtime = Files.getLastModifiedTime(sidecarFile);
        } catch (IOException e) {
            // Missing sidecar is the normal "no Oxide chunks in this
            // region" case — not an error, not logged.
            cache.remove(key);
            return Lookup.ABSENT;
        }

        CacheEntry cached = cache.get(key);
        if (cached != null && cached.mtime().equals(currentMtime)) {
            return cached.lookup();
        }

        Lookup fresh;
        try {
            byte[] data = Files.readAllBytes(sidecarFile);
            SidecarParseResult parsed = SidecarParser.parse(data);
            if (parsed.status() == SidecarParseResult.Status.ERROR) {
                // Logged once per (re-)read, i.e. once per mtime change —
                // not once per chunk step, since this branch is only
                // reached on a cache miss/refresh.
                logger.warning("Oxide sidecar " + sidecarFile + " is malformed: "
                        + parsed.errorMessage() + " -- chunks in this region will read as UNKNOWN");
            }
            fresh = new Lookup(true, parsed);
        } catch (IOException e) {
            // Vanished between the stat above and this read (e.g. a
            // concurrent pregen write) — treat as absent, not an error.
            cache.remove(key);
            return Lookup.ABSENT;
        }

        cache.put(key, new CacheEntry(currentMtime, fresh));
        return fresh;
    }
}
