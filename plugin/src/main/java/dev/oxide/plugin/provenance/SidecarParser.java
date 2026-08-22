package dev.oxide.plugin.provenance;

/**
 * Parses the Oxide sidecar file format in memory. Pure function over a byte
 * array — no filesystem access — so it is unit testable directly.
 *
 * Format (136 bytes, little-endian; only magic/version/bitmap are
 * meaningful today):
 *   bytes 0..4   magic ASCII "OXPV"
 *   byte  4      format version (u8), currently 1
 *   byte  5      flags (u8), currently 0
 *   bytes 6..8   reserved (u16)
 *   bytes 8..136 128-byte bitmap, 1024 bits, LSB-first within each byte
 */
public final class SidecarParser {

    public static final byte[] MAGIC = {'O', 'X', 'P', 'V'};
    public static final int SUPPORTED_VERSION = 1;
    public static final int HEADER_LENGTH = 8;
    public static final int BITMAP_LENGTH = 128;
    public static final int FILE_LENGTH = HEADER_LENGTH + BITMAP_LENGTH; // 136

    private SidecarParser() {
    }

    public static SidecarParseResult parse(byte[] data) {
        if (data.length != FILE_LENGTH) {
            return SidecarParseResult.error(
                    "bad length: expected " + FILE_LENGTH + " bytes, got " + data.length);
        }
        for (int i = 0; i < MAGIC.length; i++) {
            if (data[i] != MAGIC[i]) {
                return SidecarParseResult.error("bad magic");
            }
        }
        int version = data[4] & 0xFF;
        if (version != SUPPORTED_VERSION) {
            return SidecarParseResult.error("unsupported format version: " + version);
        }
        byte[] bitmap = new byte[BITMAP_LENGTH];
        System.arraycopy(data, HEADER_LENGTH, bitmap, 0, BITMAP_LENGTH);
        return SidecarParseResult.ok(bitmap);
    }

    /**
     * Reads bit `index` (0..1024) from a parsed bitmap. LSB-first within
     * each byte per the sidecar spec: bit i is bitmap[i/8] & (1 << (i%8)).
     */
    public static boolean bit(byte[] bitmap, int index) {
        int byteIndex = index / 8;
        int bitOffset = index % 8;
        return (bitmap[byteIndex] & (1 << bitOffset)) != 0;
    }
}
