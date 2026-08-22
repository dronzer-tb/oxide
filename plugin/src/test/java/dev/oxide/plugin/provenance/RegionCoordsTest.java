package dev.oxide.plugin.provenance;

import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertEquals;

class RegionCoordsTest {

    @Test
    void regionCoordForPositiveChunk() {
        assertEquals(0, RegionCoords.regionCoord(0));
        assertEquals(0, RegionCoords.regionCoord(31));
        assertEquals(1, RegionCoords.regionCoord(32));
        assertEquals(3, RegionCoords.regionCoord(100));
    }

    @Test
    void regionCoordForNegativeChunkUsesFloorDivisionNotTruncation() {
        // -1 must fall in region -1, not region 0 (which `/` would give).
        assertEquals(-1, RegionCoords.regionCoord(-1));
        assertEquals(-1, RegionCoords.regionCoord(-32));
        assertEquals(-2, RegionCoords.regionCoord(-33));
    }

    @Test
    void localCoordForPositiveChunk() {
        assertEquals(0, RegionCoords.localCoord(0));
        assertEquals(31, RegionCoords.localCoord(31));
        assertEquals(0, RegionCoords.localCoord(32));
        assertEquals(4, RegionCoords.localCoord(100));
    }

    @Test
    void localCoordForNegativeChunkUsesFloorModNotRemainder() {
        // -1 % 32 == -1 in Java; the local coordinate must be 31.
        assertEquals(31, RegionCoords.localCoord(-1));
        assertEquals(0, RegionCoords.localCoord(-32));
        assertEquals(31, RegionCoords.localCoord(-33));
    }

    @Test
    void bitIndexIsLocalZTimes32PlusLocalX() {
        assertEquals(0, RegionCoords.bitIndex(0, 0));
        assertEquals(31, RegionCoords.bitIndex(31, 0));
        assertEquals(32, RegionCoords.bitIndex(0, 1));
        assertEquals(1023, RegionCoords.bitIndex(31, 31));
    }
}
