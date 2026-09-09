// Runs the REAL Minecraft 26.2 WorldgenRandom, not a transcription of it: the merged
// Mojang-mapped jar Fabric Loom caches loads standalone, because LegacyRandomSource and
// WorldgenRandom pull in nothing that needs a running game.
//
// Regenerate (path is whatever local cache holds a 26.2 jar; the vectors it prints are what the
// Rust test actually reads, so this file only has to run when the vectors are rebuilt):
//
//   MC=~/.gradle/caches/fabric-loom/26.2/minecraft-merged.jar
//   java -cp "$MC" crates/oxide-structures/tests/data/FeatureSeedOracle.java \
//     > crates/oxide-structures/tests/data/feature_seed_vectors.txt
//
// Output: one line per case -- "<seed> <chunkX> <chunkZ>" then eight raw nextInt() draws from
// the seeded stream.
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.levelgen.WorldgenRandom;

public class FeatureSeedOracle {
    public static void main(String[] args) {
        long[] seeds = {0L, 1L, 1234L, -1L, 4489057056054590644L, -987654321098765432L};
        int[][] chunks = {
            {0, 0}, {1, 0}, {0, 1}, {-1, -1}, {10, -5}, {-100, 250},
            {5555, -9999}, {625, 625}, {1875000, 1875000}, {-1875000, 30}, {46, -46}, {3, 7},
        };

        for (long seed : seeds) {
            for (int[] chunk : chunks) {
                WorldgenRandom random = new WorldgenRandom(new LegacyRandomSource(0L));
                random.setLargeFeatureSeed(seed, chunk[0], chunk[1]);
                StringBuilder line = new StringBuilder();
                line.append(seed).append(' ').append(chunk[0]).append(' ').append(chunk[1]);
                for (int draw = 0; draw < 8; draw++) {
                    line.append(' ').append(random.nextInt());
                }
                System.out.println(line);
            }
        }
    }
}
