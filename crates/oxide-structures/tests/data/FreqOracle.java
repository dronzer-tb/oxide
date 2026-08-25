// Transcribed verbatim from the decompiled 26.1.2 sources:
//   java.util.Random (LegacyRandomSource) semantics via java.util.Random itself,
//   WorldgenRandom.setLargeFeatureWithSalt / setLargeFeatureSeed,
//   StructurePlacement's four FrequencyReductionMethod reducers.
// Standalone: runs on a plain JDK, no Minecraft build.
import java.util.Random;

public class FreqOracle {
    static final int HIGHLY_ARBITRARY_RANDOM_SALT = 10387320;

    static Random largeFeatureWithSalt(long seed, int x, int z, int salt) {
        return new Random(x * 341873128712L + z * 132897987541L + seed + salt);
    }

    static Random largeFeatureSeed(long seed, int x, int z) {
        Random r = new Random(seed);
        long xScale = r.nextLong();
        long zScale = r.nextLong();
        return new Random(x * xScale ^ z * zScale ^ seed);
    }

    static boolean defaultReducer(long seed, int salt, int sx, int sz, float p) {
        // vanilla: setLargeFeatureWithSalt(seed, salt, sourceX, sourceZ) against (seed,x,z,salt)
        return largeFeatureWithSalt(seed, salt, sx, sz).nextFloat() < p;
    }

    static boolean legacyType1(long seed, int sx, int sz, float p) {
        int cx = sx >> 4;
        int cz = sz >> 4;
        Random r = new Random(cx ^ cz << 4 ^ seed);
        r.nextInt();
        return r.nextInt((int) (1.0F / p)) == 0;
    }

    static boolean legacyType2(long seed, int sx, int sz, float p) {
        return largeFeatureWithSalt(seed, sx, sz, HIGHLY_ARBITRARY_RANDOM_SALT).nextFloat() < p;
    }

    static boolean legacyType3(long seed, int sx, int sz, float p) {
        return largeFeatureSeed(seed, sx, sz).nextDouble() < p;
    }

    public static void main(String[] args) {
        long[] seeds = {0L, 1L, -1L, 42L, 1234567890123L, Long.MIN_VALUE};
        int[] salts = {0, 165745296, 10387320, -1};
        int[][] pos = {{0,0},{1,-1},{16,32},{-64,208},{1875,-3011},{-1,-1}};
        float[] probs = {0.01f, 0.1f, 0.25f, 0.5f, 0.75f};
        for (long s : seeds)
            for (int salt : salts)
                for (int[] p : pos)
                    for (float pr : probs) {
                        System.out.println("def " + s + " " + salt + " " + p[0] + " " + p[1] + " "
                            + Float.floatToRawIntBits(pr) + " " + defaultReducer(s, salt, p[0], p[1], pr));
                        System.out.println("t1 " + s + " " + p[0] + " " + p[1] + " "
                            + Float.floatToRawIntBits(pr) + " " + legacyType1(s, p[0], p[1], pr));
                        System.out.println("t2 " + s + " " + p[0] + " " + p[1] + " "
                            + Float.floatToRawIntBits(pr) + " " + legacyType2(s, p[0], p[1], pr));
                        System.out.println("t3 " + s + " " + p[0] + " " + p[1] + " "
                            + Float.floatToRawIntBits(pr) + " " + legacyType3(s, p[0], p[1], pr));
                    }
    }
}
