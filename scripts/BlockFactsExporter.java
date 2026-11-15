// Exports what every block state of the REAL Minecraft 26.2 *is*: air, solid, replaceable,
// fluid, which heightmaps count it. Decoration asks these questions at almost every block, and
// the answers are hardcoded in the game's Blocks class rather than in any data file -- so they
// are read out of the game itself, never written from memory.
//
// Regenerate (the output is Mojang-derived: it lives under the git-ignored reference/ tree):
//
//   MC=~/.gradle/caches/fabric-loom/26.2/minecraft-merged.jar
//   LIBS=$(find <a launcher's libraries dir> -name '*.jar' | tr '\n' ':')
//   java -cp "$MC:$LIBS" scripts/BlockFactsExporter.java > reference/block_facts.tsv
//
// Output: one line per block state, "<state>\t<flags>". <state> is "ns:block[k=v,...]" with the
// properties sorted by name, which is how oxide-core's BlockState prints. <flags> is a string of
// letters, each present when true:
//   D default state of its block     a isAir            s isSolid         m blocksMotion
//   l liquid                         r canBeReplaced    o isSolidRender   u sturdy top face
//   d sturdy bottom face             c no collision     f full-cube collision
//   W water fluid   L lava fluid     S fluid is a source
//   1 WORLD_SURFACE(_WG)   2 OCEAN_FLOOR(_WG)   3 MOTION_BLOCKING   4 MOTION_BLOCKING_NO_LEAVES
//   y hasDynamicShape      e hasBlockEntity
//   k updateShape never changes the state: the block's class inherits BlockBehaviour.updateShape,
//     or overrides it only to schedule ticks (LiquidBlock, FallingBlock,
//     WaterloggedTransparentBlock -- each read in the 26.2 decompile and returning super's state)
import java.util.Map;
import java.util.Set;
import java.util.TreeMap;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.EmptyBlockGetter;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.Property;
import net.minecraft.world.level.levelgen.Heightmap;
import net.minecraft.world.level.material.FluidState;

public class BlockFactsExporter {
    public static void main(String[] args) {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        EmptyBlockGetter level = EmptyBlockGetter.INSTANCE;
        BlockPos pos = BlockPos.ZERO;
        StringBuilder out = new StringBuilder();
        for (Block block : BuiltInRegistries.BLOCK) {
            String id = BuiltInRegistries.BLOCK.getKey(block).toString();
            for (BlockState state : block.getStateDefinition().getPossibleStates()) {
                out.append(id);
                TreeMap<String, String> properties = new TreeMap<>();
                for (Property<?> property : state.getProperties()) {
                    properties.put(property.getName(), name(property, state));
                }
                if (!properties.isEmpty()) {
                    out.append('[');
                    boolean first = true;
                    for (Map.Entry<String, String> e : properties.entrySet()) {
                        if (!first) out.append(',');
                        first = false;
                        out.append(e.getKey()).append('=').append(e.getValue());
                    }
                    out.append(']');
                }
                out.append('\t');
                if (state == block.defaultBlockState()) out.append('D');
                if (state.isAir()) out.append('a');
                if (state.isSolid()) out.append('s');
                if (state.blocksMotion()) out.append('m');
                if (state.liquid()) out.append('l');
                if (state.canBeReplaced()) out.append('r');
                if (state.isSolidRender()) out.append('o');
                if (state.isFaceSturdy(level, pos, Direction.UP)) out.append('u');
                if (state.isFaceSturdy(level, pos, Direction.DOWN)) out.append('d');
                if (state.getCollisionShape(level, pos).isEmpty()) out.append('c');
                if (state.isCollisionShapeFullBlock(level, pos)) out.append('f');
                FluidState fluid = state.getFluidState();
                if (!fluid.isEmpty()) {
                    String fluidId = BuiltInRegistries.FLUID.getKey(fluid.getType()).getPath();
                    if (fluidId.contains("water")) out.append('W');
                    if (fluidId.contains("lava")) out.append('L');
                    if (fluid.isSource()) out.append('S');
                }
                if (Heightmap.Types.WORLD_SURFACE.isOpaque().test(state)) out.append('1');
                if (Heightmap.Types.OCEAN_FLOOR.isOpaque().test(state)) out.append('2');
                if (Heightmap.Types.MOTION_BLOCKING.isOpaque().test(state)) out.append('3');
                if (Heightmap.Types.MOTION_BLOCKING_NO_LEAVES.isOpaque().test(state)) out.append('4');
                if (block.hasDynamicShape()) out.append('y');
                if (state.hasBlockEntity()) out.append('e');
                if (SHAPE_INERT_CLASSES.contains(updateShapeDeclarer(block))) out.append('k');
                out.append('\n');
            }
        }
        System.out.print(out);
    }

    private static final Set<String> SHAPE_INERT_CLASSES =
            Set.of("BlockBehaviour", "LiquidBlock", "FallingBlock", "WaterloggedTransparentBlock");

    /** The simple name of the nearest class up from the block's that declares updateShape. */
    private static String updateShapeDeclarer(Block block) {
        for (Class<?> c = block.getClass(); c != null; c = c.getSuperclass()) {
            for (java.lang.reflect.Method m : c.getDeclaredMethods()) {
                if (m.getName().equals("updateShape") && m.getParameterCount() == 8) {
                    return c.getSimpleName();
                }
            }
        }
        return "";
    }

    private static <T extends Comparable<T>> String name(Property<T> property, BlockState state) {
        return property.getName(state.getValue(property));
    }
}
