package dev.oxide.plugin.provenance;

import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.attribute.FileTime;
import java.util.logging.Logger;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Exercises the cache's file-presence / mtime-invalidation behaviour with
 * real temp-directory files. No Bukkit types involved, so this runs fine
 * without a server.
 */
class SidecarCacheTest {

    @TempDir
    Path tempDir;

    private static byte[] validFileWithBit(int bitIndex) {
        byte[] data = new byte[SidecarParser.FILE_LENGTH];
        data[0] = 'O';
        data[1] = 'X';
        data[2] = 'P';
        data[3] = 'V';
        data[4] = 1;
        data[SidecarParser.HEADER_LENGTH + bitIndex / 8] |= (byte) (1 << (bitIndex % 8));
        return data;
    }

    @Test
    void missingFileIsReportedAbsentNotError() {
        Path region = tempDir.resolve("r.0.0.mca");
        SidecarCache cache = new SidecarCache(Logger.getAnonymousLogger());

        SidecarCache.Lookup lookup = cache.get("world", 0, 0, region);

        assertFalse(lookup.present());
    }

    @Test
    void presentFileIsParsedAndBitReadable() throws IOException {
        Path region = tempDir.resolve("r.0.0.mca");
        Files.write(tempDir.resolve("r.0.0.mca.oxide"), validFileWithBit(5));
        SidecarCache cache = new SidecarCache(Logger.getAnonymousLogger());

        SidecarCache.Lookup lookup = cache.get("world", 0, 0, region);

        assertTrue(lookup.present());
        assertEquals(SidecarParseResult.Status.OK, lookup.result().status());
        assertTrue(SidecarParser.bit(lookup.result().bitmap(), 5));
    }

    @Test
    void staleCacheEntryIsRefreshedAfterMtimeChanges() throws IOException {
        Path region = tempDir.resolve("r.0.0.mca");
        Path sidecar = tempDir.resolve("r.0.0.mca.oxide");
        Files.write(sidecar, validFileWithBit(5));
        SidecarCache cache = new SidecarCache(Logger.getAnonymousLogger());

        SidecarCache.Lookup first = cache.get("world", 0, 0, region);
        assertTrue(SidecarParser.bit(first.result().bitmap(), 5));

        // Simulate a fresh pregen run rewriting the sidecar with a
        // different bit set, at a distinct (later) mtime.
        Files.write(sidecar, validFileWithBit(9));
        Files.setLastModifiedTime(sidecar,
                FileTime.fromMillis(Files.getLastModifiedTime(sidecar).toMillis() + 5000));

        SidecarCache.Lookup second = cache.get("world", 0, 0, region);
        assertFalse(SidecarParser.bit(second.result().bitmap(), 5));
        assertTrue(SidecarParser.bit(second.result().bitmap(), 9));
    }

    @Test
    void malformedFileIsReportedAsErrorNotAbsent() throws IOException {
        Path region = tempDir.resolve("r.0.0.mca");
        Files.write(tempDir.resolve("r.0.0.mca.oxide"), new byte[]{'X', 'X', 'X', 'X', 1, 0, 0, 0});
        SidecarCache cache = new SidecarCache(Logger.getAnonymousLogger());

        SidecarCache.Lookup lookup = cache.get("world", 0, 0, region);

        assertTrue(lookup.present());
        assertEquals(SidecarParseResult.Status.ERROR, lookup.result().status());
    }
}
