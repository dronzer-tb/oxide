// Runs the REAL Minecraft 26.2 classes that jigsaw placement is built on, so the Rust port is
// diffed against the game rather than against a second copy of the same guess:
//   StructureTemplate.transform  -- where a rotated/mirrored block lands
//   Rotation.getShuffled / getRandom, Util.shuffle -- the draw order that makes a seed reproduce
//
// Regenerate:
//   MC=~/.gradle/caches/fabric-loom/26.2/minecraft-merged.jar
//   LIBS=$(find ~/.gradle/caches/paperweight-userdev/v2/work/*/minecraftLibraries \
//     -name '*.jar' | tr '\n' ':')
//   java -cp "$MC:$LIBS" crates/oxide-structures/tests/data/JigsawMathOracle.java \
//     > crates/oxide-structures/tests/data/jigsaw_math_vectors.txt
import java.util.ArrayList;
import java.util.List;
import net.minecraft.core.BlockPos;
import net.minecraft.util.Util;
import net.minecraft.world.level.block.Mirror;
import net.minecraft.world.level.block.Rotation;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.levelgen.structure.templatesystem.StructureTemplate;

public class JigsawMathOracle {
    public static void main(String[] args) {
        int[][] positions = {
            {0, 0, 0}, {1, 0, 0}, {0, 0, 1}, {4, 2, 6}, {-3, 5, 7}, {15, -1, -9}, {2, 0, 2},
        };
        int[][] pivots = {{0, 0}, {2, 2}, {3, 5}, {-4, 1}};

        for (Mirror mirror : Mirror.values()) {
            for (Rotation rotation : Rotation.values()) {
                for (int[] pivot : pivots) {
                    for (int[] pos : positions) {
                        BlockPos result = StructureTemplate.transform(
                            new BlockPos(pos[0], pos[1], pos[2]),
                            mirror,
                            rotation,
                            new BlockPos(pivot[0], 0, pivot[1])
                        );
                        System.out.println(
                            "t " + mirror.ordinal() + " " + rotation.ordinal() + " " + pivot[0] + " " + pivot[1]
                            + " " + pos[0] + " " + pos[1] + " " + pos[2]
                            + " " + result.getX() + " " + result.getY() + " " + result.getZ()
                        );
                    }
                }
            }
        }

        long[] seeds = {0L, 1L, 1234L, -42L, 4489057056054590644L};
        for (long seed : seeds) {
            for (int size = 1; size <= 8; size++) {
                LegacyRandomSource random = new LegacyRandomSource(seed);
                List<Integer> list = new ArrayList<>();
                for (int i = 0; i < size; i++) {
                    list.add(i);
                }
                Util.shuffle(list, random);
                StringBuilder line = new StringBuilder("s " + seed + " " + size);
                for (int value : list) {
                    line.append(' ').append(value);
                }
                // Whatever the generator has left after the shuffle catches a wrong draw count.
                line.append(' ').append(random.nextInt());
                System.out.println(line);
            }

            LegacyRandomSource random = new LegacyRandomSource(seed);
            StringBuilder line = new StringBuilder("r " + seed);
            for (Rotation rotation : Rotation.getShuffled(random)) {
                line.append(' ').append(rotation.ordinal());
            }
            line.append(' ').append(random.nextInt());
            System.out.println(line);

            LegacyRandomSource pick = new LegacyRandomSource(seed);
            System.out.println("g " + seed + " " + Rotation.getRandom(pick).ordinal() + " " + pick.nextInt());
        }
    }
}
