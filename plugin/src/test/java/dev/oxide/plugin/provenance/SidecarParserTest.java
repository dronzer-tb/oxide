package dev.oxide.plugin.provenance;

import org.junit.jupiter.api.Test;

import java.util.Arrays;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertTrue;

class SidecarParserTest {

    private static byte[] validFile(byte[] bitmap) {
        byte[] data = new byte[SidecarParser.FILE_LENGTH];
        data[0] = 'O';
        data[1] = 'X';
        data[2] = 'P';
        data[3] = 'V';
        data[4] = 1; // version
        data[5] = 0; // flags
        // bytes 6..8 reserved, left zero
        System.arraycopy(bitmap, 0, data, SidecarParser.HEADER_LENGTH, bitmap.length);
        return data;
    }

    @Test
    void parsesValidFileAndReadsBits() {
        byte[] bitmap = new byte[SidecarParser.BITMAP_LENGTH];
        bitmap[0] = (byte) 0b0000_0101; // bits 0 and 2 set
        SidecarParseResult result = SidecarParser.parse(validFile(bitmap));

        assertEquals(SidecarParseResult.Status.OK, result.status());
        assertTrue(SidecarParser.bit(result.bitmap(), 0));
        assertFalse(SidecarParser.bit(result.bitmap(), 1));
        assertTrue(SidecarParser.bit(result.bitmap(), 2));
        assertFalse(SidecarParser.bit(result.bitmap(), 3));
    }

    @Test
    void bitsAreLsbFirstWithinByte() {
        byte[] bitmap = new byte[SidecarParser.BITMAP_LENGTH];
        bitmap[1] = (byte) 0b1000_0000; // bit index 15 == byte 1, offset 7
        SidecarParseResult result = SidecarParser.parse(validFile(bitmap));

        assertTrue(SidecarParser.bit(result.bitmap(), 15));
        assertFalse(SidecarParser.bit(result.bitmap(), 14));
        assertFalse(SidecarParser.bit(result.bitmap(), 8)); // byte 1, offset 0
    }

    @Test
    void rejectsBadMagic() {
        byte[] data = validFile(new byte[SidecarParser.BITMAP_LENGTH]);
        data[0] = 'X';

        SidecarParseResult result = SidecarParser.parse(data);

        assertEquals(SidecarParseResult.Status.ERROR, result.status());
        assertNotNull(result.errorMessage());
    }

    @Test
    void rejectsUnknownVersion() {
        byte[] data = validFile(new byte[SidecarParser.BITMAP_LENGTH]);
        data[4] = 2;

        SidecarParseResult result = SidecarParser.parse(data);

        assertEquals(SidecarParseResult.Status.ERROR, result.status());
    }

    @Test
    void rejectsTruncatedFile() {
        byte[] data = Arrays.copyOf(validFile(new byte[SidecarParser.BITMAP_LENGTH]), 100);

        SidecarParseResult result = SidecarParser.parse(data);

        assertEquals(SidecarParseResult.Status.ERROR, result.status());
    }

    @Test
    void rejectsOverlongFile() {
        byte[] data = Arrays.copyOf(
                validFile(new byte[SidecarParser.BITMAP_LENGTH]), SidecarParser.FILE_LENGTH + 1);

        SidecarParseResult result = SidecarParser.parse(data);

        assertEquals(SidecarParseResult.Status.ERROR, result.status());
    }
}
