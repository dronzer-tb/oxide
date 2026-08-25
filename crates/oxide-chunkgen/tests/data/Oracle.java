// Transcribed verbatim from the decompiled 26.1.2 sources:
//   RandomSupport.mixStafford13 / upgradeSeedTo128bit
//   Xoroshiro128PlusPlus.nextLong
//   XoroshiroRandomSource.setSeed
//   WorldgenRandom.setDecorationSeed / setFeatureSeed / setLargeFeatureSeed / setLargeFeatureWithSalt
// Standalone so it can run on a plain JDK without building Minecraft.
public class Oracle {
    long seedLo, seedHi;

    static long mixStafford13(long z) {
        z = (z ^ z >>> 30) * -4658895280553007687L;
        z = (z ^ z >>> 27) * -7723592293110705685L;
        return z ^ z >>> 31;
    }

    void setSeed(long legacySeed) {
        long lowBits = legacySeed ^ 7640891576956012809L;
        long highBits = lowBits + -7046029254386353131L;
        this.seedLo = mixStafford13(lowBits);
        this.seedHi = mixStafford13(highBits);
    }

    long nextLong() {
        long s0 = this.seedLo;
        long s1 = this.seedHi;
        long result = Long.rotateLeft(s0 + s1, 17) + s0;
        s1 ^= s0;
        this.seedLo = Long.rotateLeft(s0, 49) ^ s1 ^ s1 << 21;
        this.seedHi = Long.rotateLeft(s1, 28);
        return result;
    }

    long setDecorationSeed(long seed, int chunkX, int chunkZ) {
        setSeed(seed);
        long xScale = nextLong() | 1L;
        long zScale = nextLong() | 1L;
        long result = chunkX * xScale + chunkZ * zScale ^ seed;
        setSeed(result);
        return result;
    }

    void setFeatureSeed(long seed, int index, int step) {
        setSeed(seed + index + 10000L * step);
    }

    void setLargeFeatureSeed(long seed, int chunkX, int chunkZ) {
        setSeed(seed);
        long xScale = nextLong();
        long zScale = nextLong();
        setSeed(chunkX * xScale ^ chunkZ * zScale ^ seed);
    }

    void setLargeFeatureWithSalt(long seed, int x, int z, int blend) {
        setSeed(x * 341873128712L + z * 132897987541L + seed + blend);
    }

    public static void main(String[] args) {
        long[] seeds = {0L, 1L, -1L, 42L, 1234567890123L, Long.MIN_VALUE, Long.MAX_VALUE};
        int[][] pos = {{0,0},{1,-1},{16,32},{-64,208},{1875,-3011}};
        Oracle o = new Oracle();
        for (long s : seeds) {
            for (int[] p : pos) {
                long dec = o.setDecorationSeed(s, p[0], p[1]);
                System.out.println("decoration " + s + " " + p[0] + " " + p[1] + " " + dec + " " + o.nextLong());
                for (int[] fi : new int[][]{{0,0},{3,2},{17,10},{-1,0}}) {
                    o.setFeatureSeed(dec, fi[0], fi[1]);
                    System.out.println("feature " + dec + " " + fi[0] + " " + fi[1] + " " + o.nextLong());
                }
                o.setLargeFeatureSeed(s, p[0], p[1]);
                System.out.println("large " + s + " " + p[0] + " " + p[1] + " " + o.nextLong());
                o.setLargeFeatureWithSalt(s, p[0], p[1], 987);
                System.out.println("salt " + s + " " + p[0] + " " + p[1] + " 987 " + o.nextLong());
            }
        }
    }
}
