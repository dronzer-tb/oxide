// Asks the REAL Minecraft 26.2 `BlockState.canSurvive` for chosen block/neighbourhood pairs, so
// the Rust port in crates/oxide-chunkgen/src/decorator/survival.rs is verified against the game
// itself rather than trusted from any other implementation.
//
// Regenerate (output is Mojang-derived: it lives under the git-ignored reference/ tree):
//
//   MC=~/.gradle/caches/fabric-loom/26.2/minecraft-merged.jar
//   LIBS=$(find ~/.local/share/PolyMC/libraries -name '*.jar' | tr '\n' ':')
//   java -cp "$MC:$LIBS" scripts/BlockSurvivalOracle.java reference/block_survival.tsv reference/block_survival_classes.tsv
//
// block_survival_classes.tsv: "<block>\t<class chain>\t<class declaring canSurvive or ->" for
// every block that overrides canSurvive, which is the complete list of blocks whose rule is not
// the default "always true".
//
// block_survival.tsv: "<state>\t<ground>\t<env>\t<0|1>". The world is air except: the block
// under test's position P=(0,0,0) is air, the ground state sits at P-down, and the named
// environment adds blocks around P (the Rust test builds the identical worlds):
//   open          nothing else
//   sealed_sides  stone on the four horizontal neighbours of P
//   ceiling       stone above P
//   water_here    water source at P
//   water_above   water source above P
//   ground_water_sides  water source beside the ground block on all four horizontal sides
//   ground_ice_sides    frosted_ice beside the ground block on all four horizontal sides
//   lava_side     lava at the north neighbour of P
//   cactus_above  the block under test, default state, above P
// Block and fluid tags are bound from the vanilla data pack (see bindVanillaTags).
// The grounds are listed in GROUNDS plus the block under test itself (default state, and with
// half=lower when it has that property).
import java.lang.reflect.InvocationHandler;
import java.lang.reflect.Method;
import java.lang.reflect.Proxy;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.TreeMap;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.RegistryAccess;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.server.packs.PackType;
import net.minecraft.server.packs.repository.ServerPacksSource;
import net.minecraft.server.packs.resources.MultiPackResourceManager;
import net.minecraft.tags.TagLoader;
import net.minecraft.resources.Identifier;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.LevelReader;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockBehaviour;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.Property;

public class BlockSurvivalOracle {
    static final String[] GROUNDS = {
        "grass_block", "dirt", "coarse_dirt", "podzol", "rooted_dirt", "mud", "muddy_mangrove_roots",
        "moss_block", "pale_moss_block", "clay", "farmland", "sand", "red_sand", "suspicious_sand",
        "gravel", "stone", "netherrack", "soul_sand", "soul_soil", "crimson_nylium",
        "warped_nylium", "mycelium", "snow_block", "ice", "packed_ice", "water", "lava", "glass",
        "oak_leaves", "oak_log", "terracotta", "red_terracotta", "mangrove_roots", "cactus",
        "sugar_cane", "bedrock", "air", "magma_block", "sponge", "azalea_leaves", "oak_slab",
        "hopper", "cauldron", "oak_fence", "chain", "lantern", "spawner", "dripstone_block",
        "seagrass", "kelp", "kelp_plant", "big_dripleaf", "big_dripleaf_stem", "small_dripleaf",
        "snow", "moss_carpet", "wheat", "pale_moss_carpet", "bamboo", "composter", "soul_fire",
        "crimson_stem", "warped_wart_block", "basalt", "obsidian", "deepslate", "tuff",
    };
    static final String[] ENVS = {
        "open", "sealed_sides", "ceiling", "water_here", "water_above", "ground_water_sides",
        "ground_ice_sides", "lava_side", "cactus_above",
    };

    static String stateString(BlockState state) {
        StringBuilder out = new StringBuilder(BuiltInRegistries.BLOCK.getKey(state.getBlock()).toString());
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
        return out.toString();
    }

    static <T extends Comparable<T>> String name(Property<T> property, BlockState state) {
        return property.getName(state.getValue(property));
    }

    static LevelReader level(Map<BlockPos, BlockState> world) {
        InvocationHandler handler = (proxy, method, args) -> {
            switch (method.getName()) {
                case "getBlockState":
                    return world.getOrDefault((BlockPos) args[0], Blocks.AIR.defaultBlockState());
                case "getRawBrightness":
                case "getBrightness":
                    return 0;
                default:
                    if (method.isDefault()) return InvocationHandler.invokeDefault(proxy, method, args);
                    throw new UnsupportedOperationException(method.toString());
            }
        };
        return (LevelReader) Proxy.newProxyInstance(
            BlockSurvivalOracle.class.getClassLoader(), new Class<?>[] {LevelReader.class}, handler);
    }

    static Class<?> declaringClass(Block block) throws Exception {
        for (Class<?> c = block.getClass(); c != BlockBehaviour.class && c != null; c = c.getSuperclass()) {
            for (Method m : c.getDeclaredMethods()) {
                if (m.getName().equals("canSurvive") && m.getParameterCount() == 3) return c;
            }
        }
        return null;
    }

    static BlockState block(String path) {
        return BuiltInRegistries.BLOCK.getValue(Identifier.withDefaultNamespace(path)).defaultBlockState();
    }

    static Map<BlockPos, BlockState> world(String env, BlockState ground, BlockState self) {
        Map<BlockPos, BlockState> w = new HashMap<>();
        BlockPos p = BlockPos.ZERO;
        w.put(p.below(), ground);
        BlockState stone = block("stone");
        switch (env) {
            case "sealed_sides":
                for (Direction d : Direction.Plane.HORIZONTAL) w.put(p.relative(d), stone);
                break;
            case "ceiling":
                w.put(p.above(), stone);
                break;
            case "water_here":
                w.put(p, block("water"));
                break;
            case "water_above":
                w.put(p.above(), block("water"));
                break;
            case "ground_water_sides":
                for (Direction d : Direction.Plane.HORIZONTAL) w.put(p.below().relative(d), block("water"));
                break;
            case "ground_ice_sides":
                for (Direction d : Direction.Plane.HORIZONTAL) w.put(p.below().relative(d), block("frosted_ice"));
                break;
            case "lava_side":
                w.put(p.north(), block("lava"));
                break;
            case "cactus_above":
                w.put(p.above(), self.getBlock().defaultBlockState());
                break;
            default:
                break;
        }
        return w;
    }

    // Bootstrap leaves block and fluid tags unbound, so every `is(tag)` check (the 26.x
    // `supports_*` tags) would answer false. Bind the vanilla data pack's tags like a server does.
    static void bindVanillaTags() {
        MultiPackResourceManager resources = new MultiPackResourceManager(
            PackType.SERVER_DATA, List.of(ServerPacksSource.createVanillaPackSource()));
        for (var pending : TagLoader.loadTagsForExistingRegistries(
                resources, RegistryAccess.fromRegistryOfRegistries(BuiltInRegistries.REGISTRY))) {
            pending.apply();
        }
    }

    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        bindVanillaTags();
        StringBuilder classes = new StringBuilder();
        StringBuilder rows = new StringBuilder();
        for (Block block : BuiltInRegistries.BLOCK) {
            Class<?> declaring = declaringClass(block);
            if (declaring == null) continue;
            String id = BuiltInRegistries.BLOCK.getKey(block).toString();
            StringBuilder chain = new StringBuilder();
            for (Class<?> c = block.getClass(); c != BlockBehaviour.class && c != Object.class && c != null; c = c.getSuperclass()) {
                if (chain.length() > 0) chain.append('>');
                chain.append(c.getSimpleName());
            }
            classes.append(id).append('\t').append(chain).append('\t').append(declaring.getSimpleName()).append('\n');
            List<BlockState> grounds = new ArrayList<>();
            for (String g : GROUNDS) grounds.add(block(g));
            grounds.add(block.defaultBlockState());
            for (BlockState s : block.getStateDefinition().getPossibleStates()) {
                if (s.hasProperty(net.minecraft.world.level.block.state.properties.BlockStateProperties.DOUBLE_BLOCK_HALF)
                    && s.getValue(net.minecraft.world.level.block.state.properties.BlockStateProperties.DOUBLE_BLOCK_HALF)
                        == net.minecraft.world.level.block.state.properties.DoubleBlockHalf.LOWER) {
                    grounds.add(s);
                }
            }
            for (BlockState state : block.getStateDefinition().getPossibleStates()) {
                for (BlockState ground : grounds) {
                    for (String env : ENVS) {
                        boolean result;
                        try {
                            result = state.canSurvive(level(world(env, ground, state)), BlockPos.ZERO);
                        } catch (UnsupportedOperationException e) {
                            continue;
                        }
                        rows.append(stateString(state)).append('\t').append(stateString(ground)).append('\t')
                            .append(env).append('\t').append(result ? 1 : 0).append('\n');
                    }
                }
            }
        }
        Files.writeString(Path.of(args[0]), rows.toString());
        Files.writeString(Path.of(args[1]), classes.toString());
    }
}
