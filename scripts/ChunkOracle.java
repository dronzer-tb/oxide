// Runs the REAL Minecraft 26.2 noise chunk generator in-process (no server, no network port)
// and dumps what it produces, so Oxide's terrain and decoration can be measured against the
// game itself. Output is Mojang-derived: it goes under the git-ignored reference/ tree only.
//
// Run (JDK 25):
//
//   MC=~/.gradle/caches/fabric-loom/26.2/minecraft-merged.jar
//   LIBS=$(find ~/.local/share/PolyMC/libraries -name '*.jar' ! -name '*natives*' ! -path '*com/mojang/minecraft*' | tr '\n' ':')
//   java -cp "$MC:$LIBS" scripts/ChunkOracle.java reference/oracle <seed> overworld auto 16
//   java -cp "$MC:$LIBS" scripts/ChunkOracle.java reference/oracle <seed> the_nether 3,-7 10,2
//
// Arguments: <out root> <seed> <overworld|the_nether> then either `auto <n>` (pick n chunks
// covering distinct biomes, see pickChunks) or explicit `<cx>,<cz>` centre chunks.
//
// HOW THE GAME IS DRIVEN. Registries are loaded exactly as WorldLoader.load does (static tags,
// WORLDGEN_REGISTRIES, DIMENSION_REGISTRIES, then static tags bound), the dimension comes from
// the `minecraft:normal` world preset, and RandomState is created the way ChunkMap creates it.
// Structure generation is OFF (WorldOptions(seed, false, false)), so STRUCTURE_STARTS and
// STRUCTURE_REFERENCES leave nothing and the Beardifier is empty. For each centre chunk C:
//
//   1. Every chunk of the 3x3 around C is taken EMPTY -> CARVERS on its own ProtoChunk:
//      createBiomes, fillFromNoise, buildSurface, applyCarvers, each with Blender.empty() (a
//      fresh world has no old chunks, so Blender.of(region) is empty in the game too).
//   2. C alone is brought to FEATURES: Heightmap.primeHeightmaps then
//      ChunkGenerator.applyBiomeDecoration -- the game's own method -- against a WorldGenLevel
//      over the 3x3. Neighbours stay at CARVERS, which is exactly what the FEATURES step's
//      dependency pyramid gives the game.
//
// The one thing that is not the game's code is the region object. WorldGenRegion and the
// ChunkStatusTasks need a ServerLevel (and through it a MinecraftServer), so this file replaces
// them with Region below: a java.lang.reflect.Proxy implementing WorldGenLevel whose methods
// are line-for-line transcriptions of WorldGenRegion's (26.2, decompiled): the same dependency
// check in getChunk, the same write-radius check in ensureCanWrite, the same setBlock (DUMMY
// block-entity tags on proto chunks, post-processing marks), the same "worldgen_region_random"
// RandomSource, the same BiomeManager. Methods WorldGenRegion delegates to the ServerLevel and
// that worldgen never needs (getLevel, getServer, getChunkSource, light, difficulty, border)
// throw, so if any feature ever reached one the run fails loudly instead of diverging. Also
// transcribed rather than called: NoiseBasedChunkGenerator.buildSurface(WorldGenRegion,..)
// (calls its public overload with the same arguments) and applyCarvers (the loop is copied;
// ChunkAccess.carverBiome only memoises the supplier, which is called directly). Skipped:
// Blender.generateBorderTicks (ticks only, for old chunks), lighting, mob spawning.
// Known difference from a live server: none in blocks for a fresh world, as far as the
// decompiled code shows. A live server's neighbours may already be decorated by THEIR features
// when C decorates; this harness deliberately measures C's features alone, which is what Oxide
// computes per chunk.
//
// OUTPUT: <out root>/<seed>/<dimension>/<cx>_<cz>.txt.gz, gzip'd UTF-8 text, one per centre:
//
//   oxide-oracle 1
//   seed <seed>
//   dimension minecraft:<dim>
//   center <cx> <cz>
//   min_y <minY> height <height>
//   palette <n>              then n lines, one block state each: minecraft:id[k=v,...] with
//                            properties sorted by name (Oxide's BlockState Display form);
//                            line i is palette id i
//   biomes <n>               then n lines, one biome id each
//   blocks <phase> <cx> <cz> then ONE line: run-length "id*count" or "id" tokens, space
//                            separated, covering y=minY..maxY-1, then z 0..15, then x 0..15
//                            (index = ((y-minY)*16 + z)*16 + x). phase is `carvers` (all nine
//                            chunks, terrain before features) or `features` (all nine chunks
//                            after C's decoration).
//   biome <cx> <cz>          then ONE line: run-length biome ids over quart cells, quart y
//                            (from minY/4) then z 0..3 then x 0..3 (centre chunk only)
//   heightmap <phase> <TYPE> <cx> <cz>  then ONE line of 256 ints, z-major (z*16+x): the
//                            first free y above the column's top counted block, relative to
//                            minY (Oxide's Heightmap convention). carvers: WORLD_SURFACE_WG and
//                            OCEAN_FLOOR_WG; features: WORLD_SURFACE, OCEAN_FLOOR,
//                            MOTION_BLOCKING, MOTION_BLOCKING_NO_LEAVES (centre chunk only).
//   end
import java.io.OutputStreamWriter;
import java.io.PrintWriter;
import java.lang.reflect.InvocationHandler;
import java.lang.reflect.InvocationTargetException;
import java.lang.reflect.Method;
import java.lang.reflect.Proxy;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Collections;
import java.util.EnumSet;
import java.util.HashMap;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.TreeMap;
import java.util.concurrent.atomic.AtomicLong;
import java.util.function.Predicate;
import java.util.stream.Stream;
import java.util.zip.GZIPOutputStream;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Holder;
import net.minecraft.core.HolderLookup;
import net.minecraft.core.LayeredRegistryAccess;
import net.minecraft.core.QuartPos;
import net.minecraft.core.Registry;
import net.minecraft.core.RegistryAccess;
import net.minecraft.core.SectionPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.core.registries.Registries;
import net.minecraft.nbt.CompoundTag;
import net.minecraft.resources.Identifier;
import net.minecraft.resources.RegistryDataLoader;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.RegistryLayer;
import net.minecraft.server.packs.PackType;
import net.minecraft.server.packs.repository.ServerPacksSource;
import net.minecraft.server.packs.resources.MultiPackResourceManager;
import net.minecraft.tags.TagLoader;
import net.minecraft.util.RandomSource;
import net.minecraft.world.attribute.EnvironmentAttributeReader;
import net.minecraft.world.flag.FeatureFlags;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.LevelHeightAccessor;
import net.minecraft.world.level.StructureManager;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.biome.BiomeGenerationSettings;
import net.minecraft.world.level.biome.BiomeManager;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.EntityBlock;
import net.minecraft.world.level.block.entity.BlockEntity;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.Property;
import net.minecraft.world.level.chunk.CarvingMask;
import net.minecraft.world.level.chunk.ChunkAccess;
import net.minecraft.world.level.chunk.PalettedContainerFactory;
import net.minecraft.world.level.chunk.ProtoChunk;
import net.minecraft.world.level.chunk.UpgradeData;
import net.minecraft.world.level.chunk.status.ChunkPyramid;
import net.minecraft.world.level.chunk.status.ChunkStatus;
import net.minecraft.world.level.chunk.status.ChunkStep;
import net.minecraft.world.level.chunk.status.ChunkType;
import net.minecraft.world.level.dimension.DimensionType;
import net.minecraft.world.level.dimension.LevelStem;
import net.minecraft.world.level.levelgen.Aquifer;
import net.minecraft.world.level.levelgen.Heightmap;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.levelgen.NoiseBasedChunkGenerator;
import net.minecraft.world.level.levelgen.NoiseChunk;
import net.minecraft.world.level.levelgen.NoiseGeneratorSettings;
import net.minecraft.world.level.levelgen.RandomState;
import net.minecraft.world.level.levelgen.RandomSupport;
import net.minecraft.world.level.levelgen.WorldGenerationContext;
import net.minecraft.world.level.levelgen.WorldOptions;
import net.minecraft.world.level.levelgen.WorldgenRandom;
import net.minecraft.world.level.levelgen.blending.Blender;
import net.minecraft.world.level.levelgen.carver.CarvingContext;
import net.minecraft.world.level.levelgen.carver.ConfiguredWorldCarver;
import net.minecraft.world.level.levelgen.presets.WorldPresets;
import net.minecraft.world.level.material.FluidState;
import net.minecraft.world.ticks.WorldGenTickAccess;

public class ChunkOracle {
    static RegistryAccess.Frozen registries;
    static NoiseBasedChunkGenerator generator;
    static RandomState randomState;
    static DimensionType dimensionType;
    static LevelHeightAccessor heightAccessor;
    static PalettedContainerFactory containers;
    static long seed;
    // ORACLE_TRACE=1: what each feature of the centre's FEATURES step changed, to stderr as
    // "TRACE <placed feature> x y z state" (3x3 diffed between features), to locate divergences.

    static final boolean TRACE = System.getenv("ORACLE_TRACE") != null;
    static final Identifier WORLDGEN_REGION_RANDOM = Identifier.withDefaultNamespace("worldgen_region_random");

    public static void main(String[] args) throws Exception {
        if (args.length < 4) {
            System.err.println("usage: ChunkOracle <out root> <seed> <overworld|the_nether> (auto <n> | <cx>,<cz>...)");
            System.exit(2);
        }
        Path outRoot = Path.of(args[0]);
        seed = Long.parseLong(args[1]);
        String dim = args[2];

        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        loadRegistries();

        var dims = WorldPresets.createNormalWorldDimensions(registries);
        var stemKey = switch (dim) {
            case "overworld" -> LevelStem.OVERWORLD;
            case "the_nether" -> LevelStem.NETHER;
            default -> throw new IllegalArgumentException("dimension must be overworld or the_nether");
        };
        LevelStem stem = dims.get(stemKey).orElseThrow();
        generator = (NoiseBasedChunkGenerator) stem.generator();
        dimensionType = stem.type().value();
        heightAccessor = LevelHeightAccessor.create(dimensionType.minY(), dimensionType.height());
        containers = PalettedContainerFactory.create(registries);
        // As ChunkMap does.
        randomState = RandomState.create(generator.generatorSettings().value(), registries.lookupOrThrow(Registries.NOISE), seed);
        generator.validate();

        List<ChunkPos> centres = new ArrayList<>();
        if (args[3].equals("auto")) {
            centres.addAll(pickChunks(dim, Integer.parseInt(args[4])));
        } else {
            for (int i = 3; i < args.length; i++) {
                String[] p = args[i].split(",");
                centres.add(new ChunkPos(Integer.parseInt(p[0]), Integer.parseInt(p[1])));
            }
        }
        Path outDir = outRoot.resolve(Long.toString(seed)).resolve(dim);
        Files.createDirectories(outDir);
        writeFeatureOrder(outDir.resolve("feature_order.txt"));
        for (ChunkPos c : centres) {
            long t0 = System.nanoTime();
            dumpCentre(c, outDir.resolve(c.x() + "_" + c.z() + ".txt.gz"), dim);
            System.out.printf("%s %d,%d  %s  %d ms%n", dim, c.x(), c.z(),
                surfaceBiome(c, dim), (System.nanoTime() - t0) / 1_000_000);
        }
        System.exit(0); // worldgen executors are non-daemon
    }

    // feature_order.txt: "<step> <index> <placed feature id>" for the generator's own
    // FeatureSorter result (ChunkGenerator.featuresPerStep), the ordering every
    // setFeatureSeed(decorationSeed, index, step) call depends on.
    static void writeFeatureOrder(Path out) throws Exception {
        java.lang.reflect.Field f = net.minecraft.world.level.chunk.ChunkGenerator.class.getDeclaredField("featuresPerStep");
        f.setAccessible(true);
        List<?> steps = (List<?>) ((java.util.function.Supplier<?>) f.get(generator)).get();
        var placed = registries.lookupOrThrow(Registries.PLACED_FEATURE);
        try (PrintWriter w = new PrintWriter(Files.newBufferedWriter(out))) {
            for (int step = 0; step < steps.size(); step++) {
                Object data = steps.get(step);
                List<?> features = (List<?>) data.getClass().getMethod("features").invoke(data);
                for (int i = 0; i < features.size(); i++) {
                    var id = placed.getKey((net.minecraft.world.level.levelgen.placement.PlacedFeature) features.get(i));
                    w.println(step + " " + i + " " + id);
                }
            }
        }
    }

    static void loadRegistries() {
        // WorldLoader.load, minus the server resources it builds afterwards.
        MultiPackResourceManager resources = new MultiPackResourceManager(
            PackType.SERVER_DATA, List.of(ServerPacksSource.createVanillaPackSource()));
        LayeredRegistryAccess<RegistryLayer> initial = RegistryLayer.createRegistryAccess();
        List<Registry.PendingTags<?>> staticTags = TagLoader.loadTagsForExistingRegistries(resources, initial.getLayer(RegistryLayer.STATIC));
        RegistryAccess.Frozen loadContext = initial.getAccessForLoading(RegistryLayer.WORLDGEN);
        List<HolderLookup.RegistryLookup<?>> worldgenContext = TagLoader.buildUpdatedLookups(loadContext, staticTags);
        RegistryAccess.Frozen worldgen = RegistryDataLoader.load(resources, worldgenContext, RegistryDataLoader.WORLDGEN_REGISTRIES, Runnable::run).join();
        List<HolderLookup.RegistryLookup<?>> dimensionContext = Stream.concat(worldgenContext.stream(), worldgen.listRegistries()).toList();
        RegistryAccess.Frozen dimensions = RegistryDataLoader.load(resources, dimensionContext, RegistryDataLoader.DIMENSION_REGISTRIES, Runnable::run).join();
        registries = initial.replaceFrom(RegistryLayer.WORLDGEN, worldgen, dimensions).compositeAccess();
        // ReloadableServerResources would bind these; block/fluid `is(tag)` checks need them.
        for (var pending : staticTags) pending.apply();
    }

    // ---------------------------------------------------------------- chunk choice

    static String biomeAt(int blockX, int blockY, int blockZ) {
        return generator.getBiomeSource()
            .getNoiseBiome(QuartPos.fromBlock(blockX), QuartPos.fromBlock(blockY), QuartPos.fromBlock(blockZ), randomState.sampler())
            .unwrapKey().orElseThrow().identifier().getPath();
    }

    static String surfaceBiome(ChunkPos c, String dim) {
        int x = c.getMinBlockX() + 8, z = c.getMinBlockZ() + 8;
        int y = dim.equals("overworld")
            ? generator.getBaseHeight(x, z, Heightmap.Types.WORLD_SURFACE_WG, heightAccessor, randomState)
            : 64;
        return biomeAt(x, y, z);
    }

    // Deterministic: spiral out from the origin in steps of 5 chunks, keep the first chunk whose
    // surface (or, for the cave group, y=-40) biome is in each still-unfilled group, then fill
    // up to n with the first chunks of biomes not seen yet.
    static List<ChunkPos> pickChunks(String dim, int n) {
        List<String[]> groups = new ArrayList<>();
        if (dim.equals("overworld")) {
            groups.add(new String[] {"plains", "sunflower_plains"});
            groups.add(new String[] {"forest", "birch_forest", "flower_forest"});
            groups.add(new String[] {"ocean", "deep_ocean", "lukewarm_ocean", "deep_lukewarm_ocean", "cold_ocean", "deep_cold_ocean", "warm_ocean"});
            groups.add(new String[] {"jagged_peaks", "frozen_peaks", "stony_peaks", "snowy_slopes", "grove", "windswept_hills"});
            groups.add(new String[] {"desert"});
            groups.add(new String[] {"CAVE:dripstone_caves", "CAVE:lush_caves", "CAVE:deep_dark"});
            groups.add(new String[] {"taiga", "old_growth_pine_taiga", "old_growth_spruce_taiga", "snowy_taiga"});
            groups.add(new String[] {"swamp", "mangrove_swamp"});
            groups.add(new String[] {"savanna", "savanna_plateau"});
            groups.add(new String[] {"badlands", "wooded_badlands", "eroded_badlands"});
            groups.add(new String[] {"jungle", "sparse_jungle", "bamboo_jungle"});
            groups.add(new String[] {"dark_forest", "pale_garden"});
            groups.add(new String[] {"river", "frozen_river", "beach"});
            groups.add(new String[] {"snowy_plains", "ice_spikes"});
            groups.add(new String[] {"meadow", "cherry_grove"});
        } else {
            for (String b : new String[] {"nether_wastes", "soul_sand_valley", "crimson_forest", "warped_forest", "basalt_deltas"}) {
                groups.add(new String[] {b});
            }
        }
        List<ChunkPos> picked = new ArrayList<>();
        Set<String> seen = new java.util.HashSet<>();
        boolean[] filled = new boolean[groups.size()];
        List<ChunkPos> spiral = new ArrayList<>();
        int r = 48;
        for (int ring = 0; ring <= r; ring++) {
            for (int dx = -ring; dx <= ring; dx++) for (int dz = -ring; dz <= ring; dz++) {
                if (Math.max(Math.abs(dx), Math.abs(dz)) == ring) spiral.add(new ChunkPos(dx * 5, dz * 5));
            }
        }
        Map<ChunkPos, String> surface = new HashMap<>();
        for (ChunkPos c : spiral) {
            if (picked.size() >= n) break;
            String s = surfaceBiome(c, dim);
            surface.put(c, s);
            String cave = dim.equals("overworld") ? biomeAt(c.getMinBlockX() + 8, -40, c.getMinBlockZ() + 8) : "";
            for (int g = 0; g < groups.size(); g++) {
                if (filled[g]) continue;
                for (String b : groups.get(g)) {
                    boolean hit = b.startsWith("CAVE:") ? cave.equals(b.substring(5)) : s.equals(b);
                    if (hit && !picked.contains(c)) {
                        filled[g] = true;
                        picked.add(c);
                        seen.add(s);
                    }
                }
            }
        }
        for (ChunkPos c : spiral) {
            if (picked.size() >= n) break;
            String s = surface.computeIfAbsent(c, k -> surfaceBiome(k, dim));
            if (seen.add(s)) picked.add(c);
        }
        return picked;
    }

    // ---------------------------------------------------------------- the pipeline

    static long key(int x, int z) {
        return ((long) x << 32) ^ (z & 0xffffffffL);
    }

    static void dumpCentre(ChunkPos centre, Path out, String dim) throws Exception {
        // A fresh world per centre, so no other chunk's features are ever in it.
        Map<Long, ProtoChunk> world = new HashMap<>();
        for (int dz = -1; dz <= 1; dz++) for (int dx = -1; dx <= 1; dx++) {
            ensure(world, new ChunkPos(centre.x() + dx, centre.z() + dz), ChunkStatus.CARVERS);
        }
        // Snapshot of the nine terrain chunks before features touch them.
        Map<Long, BlockState[]> carvers = new LinkedHashMap<>();
        for (int dz = -1; dz <= 1; dz++) for (int dx = -1; dx <= 1; dx++) {
            long k = key(centre.x() + dx, centre.z() + dz);
            carvers.put(k, blocks(world.get(k)));
        }
        Map<String, int[]> carverHeights = new LinkedHashMap<>();
        ProtoChunk c = world.get(key(centre.x(), centre.z()));
        for (Heightmap.Types t : new Heightmap.Types[] {Heightmap.Types.WORLD_SURFACE_WG, Heightmap.Types.OCEAN_FLOOR_WG}) {
            carverHeights.put(t.name(), heights(c, t));
        }
        String[] biomes = biomeCells(c);

        ensure(world, centre, ChunkStatus.FEATURES);
        for (int dz = -1; dz <= 1; dz++) for (int dx = -1; dx <= 1; dx++) {
            ChunkStatus s = world.get(key(centre.x() + dx, centre.z() + dz)).getPersistedStatus();
            if ((dx != 0 || dz != 0) && s != ChunkStatus.CARVERS) throw new IllegalStateException("neighbour at " + s);
        }

        Map<Long, BlockState[]> features = new LinkedHashMap<>();
        for (long k : carvers.keySet()) features.put(k, blocks(world.get(k)));
        Map<String, int[]> featureHeights = new LinkedHashMap<>();
        for (Heightmap.Types t : new Heightmap.Types[] {Heightmap.Types.WORLD_SURFACE, Heightmap.Types.OCEAN_FLOOR,
                Heightmap.Types.MOTION_BLOCKING, Heightmap.Types.MOTION_BLOCKING_NO_LEAVES}) {
            featureHeights.put(t.name(), heights(c, t));
        }

        Map<BlockState, Integer> palette = new LinkedHashMap<>();
        Map<String, Integer> biomePalette = new LinkedHashMap<>();
        for (BlockState[] a : carvers.values()) for (BlockState s : a) palette.putIfAbsent(s, palette.size());
        for (BlockState[] a : features.values()) for (BlockState s : a) palette.putIfAbsent(s, palette.size());
        for (String b : biomes) biomePalette.putIfAbsent(b, biomePalette.size());

        try (PrintWriter w = new PrintWriter(new OutputStreamWriter(new GZIPOutputStream(Files.newOutputStream(out)), StandardCharsets.UTF_8))) {
            w.println("oxide-oracle 1");
            w.println("seed " + seed);
            w.println("dimension minecraft:" + dim);
            w.println("center " + centre.x() + " " + centre.z());
            w.println("min_y " + dimensionType.minY() + " height " + dimensionType.height());
            w.println("palette " + palette.size());
            for (BlockState s : palette.keySet()) w.println(stateString(s));
            w.println("biomes " + biomePalette.size());
            for (String b : biomePalette.keySet()) w.println(b);
            for (var e : carvers.entrySet()) writeBlocks(w, "carvers", e.getKey(), e.getValue(), palette);
            for (var e : features.entrySet()) writeBlocks(w, "features", e.getKey(), e.getValue(), palette);
            w.println("biome " + centre.x() + " " + centre.z());
            int[] ids = new int[biomes.length];
            for (int i = 0; i < ids.length; i++) ids[i] = biomePalette.get(biomes[i]);
            w.println(rle(ids));
            for (var e : carverHeights.entrySet()) writeHeights(w, "carvers", e.getKey(), centre, e.getValue());
            for (var e : featureHeights.entrySet()) writeHeights(w, "features", e.getKey(), centre, e.getValue());
            w.println("end");
        }
    }

    static StructureManager structureManager(WorldGenLevel level) {
        // ServerLevel.structureManager().forWorldGenRegion(region) with generate-structures off.
        return new StructureManager(level, new WorldOptions(seed, false, false), null);
    }

    // Brings `pos` to `target` one status at a time, first bringing every chunk the step's
    // dependency pyramid names to the status it requires -- what the game's chunk scheduler
    // guarantees before it runs a step.
    static ProtoChunk ensure(Map<Long, ProtoChunk> world, ChunkPos pos, ChunkStatus target) {
        ProtoChunk chunk = world.computeIfAbsent(key(pos.x(), pos.z()),
            k -> new ProtoChunk(pos, UpgradeData.EMPTY, heightAccessor, containers, null));
        while (!chunk.getPersistedStatus().isOrAfter(target)) {
            ChunkStatus next = ChunkStatus.getStatusList().get(chunk.getPersistedStatus().getIndex() + 1);
            ChunkStep step = ChunkPyramid.GENERATION_PYRAMID.getStepTo(next);
            for (int d = 1; d < step.directDependencies().size(); d++) {
                ChunkStatus need = step.directDependencies().get(d);
                for (int dx = -d; dx <= d; dx++) for (int dz = -d; dz <= d; dz++) {
                    if (Math.max(Math.abs(dx), Math.abs(dz)) == d) ensure(world, new ChunkPos(pos.x() + dx, pos.z() + dz), need);
                }
            }
            runStep(world, chunk, step);
            chunk.setPersistedStatus(next);
        }
        return chunk;
    }

    // The generator calls ChunkStatusTasks makes for each status, with structures off.
    static void runStep(Map<Long, ProtoChunk> world, ProtoChunk chunk, ChunkStep step) {
        WorldGenLevel r = Region.create(world, chunk, step);
        ChunkStatus s = step.targetStatus();
        if (s == ChunkStatus.STRUCTURE_STARTS || s == ChunkStatus.STRUCTURE_REFERENCES) {
            // generateStructures=false: createStructures is skipped; createReferences finds no
            // starts in the neighbourhood and writes nothing.
        } else if (s == ChunkStatus.BIOMES) {
            generator.createBiomes(randomState, Blender.empty(), structureManager(r), chunk).join();
        } else if (s == ChunkStatus.NOISE) {
            generator.fillFromNoise(Blender.empty(), randomState, structureManager(r), chunk).join();
        } else if (s == ChunkStatus.SURFACE) {
            // NoiseBasedChunkGenerator.buildSurface(WorldGenRegion, ...) -> its public overload,
            // with the same context, biome manager and collectPossibleBiomes(region, 1).
            Set<Holder<Biome>> possible = new it.unimi.dsi.fastutil.objects.ReferenceOpenHashSet<>();
            for (int z = chunk.getPos().z() - 1; z <= chunk.getPos().z() + 1; z++)
                for (int x = chunk.getPos().x() - 1; x <= chunk.getPos().x() + 1; x++) r.getChunk(x, z).collectBiomesInPalette(possible);
            generator.buildSurface(chunk, new WorldGenerationContext(generator, r), randomState, structureManager(r),
                r.getBiomeManager(), Blender.empty(), possible);
        } else if (s == ChunkStatus.CARVERS) {
            applyCarvers(r, chunk, structureManager(r));
        } else if (s == ChunkStatus.FEATURES) {
            Heightmap.primeHeightmaps(chunk, EnumSet.of(Heightmap.Types.MOTION_BLOCKING, Heightmap.Types.MOTION_BLOCKING_NO_LEAVES,
                Heightmap.Types.OCEAN_FLOOR, Heightmap.Types.WORLD_SURFACE));
            generator.applyBiomeDecoration(r, chunk, structureManager(r));
        } else {
            throw new IllegalStateException("oracle does not run " + s);
        }
    }

    // NoiseBasedChunkGenerator.applyCarvers, transcribed (it takes the WorldGenRegion class).
    static void applyCarvers(WorldGenLevel region, ProtoChunk chunk, StructureManager structureManager) {
        var biomeSource = generator.getBiomeSource();
        BiomeManager biomeManager = new BiomeManager(region, BiomeManager.obfuscateSeed(seed));
        BiomeManager correctBiomeManager = biomeManager.withDifferentSource(
            (qx, qy, qz) -> biomeSource.getNoiseBiome(qx, qy, qz, randomState.sampler()));
        WorldgenRandom random = new WorldgenRandom(new LegacyRandomSource(RandomSupport.generateUniqueSeed()));
        ChunkPos pos = chunk.getPos();
        NoiseChunk noiseChunk = chunk.getOrCreateNoiseChunk(c -> {
            throw new IllegalStateException("noise chunk must exist after fillFromNoise");
        });
        Aquifer aquifer = noiseChunk.aquifer();
        CarvingContext context = new CarvingContext(generator, registries, chunk.getHeightAccessorForGeneration(), noiseChunk,
            randomState, generator.generatorSettings().value().surfaceRule());
        CarvingMask mask = chunk.getOrCreateCarvingMask();
        for (int dx = -8; dx <= 8; dx++) {
            for (int dz = -8; dz <= 8; dz++) {
                ChunkPos sourcePos = new ChunkPos(pos.x() + dx, pos.z() + dz);
                BiomeGenerationSettings settings = generator.getBiomeGenerationSettings(biomeSource.getNoiseBiome(
                    QuartPos.fromBlock(sourcePos.getMinBlockX()), 0, QuartPos.fromBlock(sourcePos.getMinBlockZ()), randomState.sampler()));
                int index = 0;
                for (Holder<ConfiguredWorldCarver<?>> holder : settings.getCarvers()) {
                    ConfiguredWorldCarver<?> carver = holder.value();
                    random.setLargeFeatureSeed(seed + index, sourcePos.x(), sourcePos.z());
                    if (carver.isStartChunk(random)) {
                        carver.carve(context, chunk, correctBiomeManager::getBiome, random, aquifer, sourcePos, mask);
                    }
                    index++;
                }
            }
        }
    }

    // ---------------------------------------------------------------- the region

    static final net.minecraft.world.level.storage.LevelData LEVEL_DATA = (net.minecraft.world.level.storage.LevelData) Proxy.newProxyInstance(
        ChunkOracle.class.getClassLoader(), new Class<?>[] {net.minecraft.world.level.storage.LevelData.class}, (p, m, a) -> {
            if (m.getName().equals("getGameTime")) return 0L; // a fresh world
            throw new UnsupportedOperationException("oracle level data does not provide " + m);
        });
    // The ServerLevel the real engine hangs off only lends it its height range; the region does too.
    static net.minecraft.world.level.lighting.LevelLightEngine lightEngine(net.minecraft.world.level.BlockGetter level) {
        return new net.minecraft.world.level.lighting.LevelLightEngine(new net.minecraft.world.level.chunk.LightChunkGetter() {
                public net.minecraft.world.level.chunk.LightChunk getChunkForLighting(int x, int z) { return null; }
                public net.minecraft.world.level.BlockGetter getLevel() { return level; }
            }, true, dimensionType.hasSkyLight());
    }

    // WorldGenRegion, transcribed onto a Proxy (see the file header).
    static final class Region implements InvocationHandler {
        final Map<Long, ProtoChunk> chunks;
        final ChunkAccess center;
        final ChunkStep step;
        final int cx, cz, writeRadius;
        final RandomSource random;
        BiomeManager biomeManager;
        WorldGenTickAccess<Block> blockTicks;
        WorldGenTickAccess<net.minecraft.world.level.material.Fluid> fluidTicks;
        final AtomicLong subTicks = new AtomicLong();
        WorldGenLevel self;
        net.minecraft.world.level.lighting.LevelLightEngine light;
        java.util.function.Supplier<String> generating;
        Map<Long, BlockState[]> traceSnapshot;

        // Called before each feature (and with null after the last): what the previous feature
        // changed, by diffing the 3x3 -- ores write through BulkSectionAccess, not setBlock.
        void traceFeature(java.util.function.Supplier<String> next) {
            Map<Long, BlockState[]> now = new HashMap<>();
            for (var e : chunks.entrySet()) if (center.getPos().getChessboardDistance(new ChunkPos((int) (e.getKey() >> 32), (int) (long) e.getKey())) <= 1) now.put(e.getKey(), blocks(e.getValue()));
            if (traceSnapshot != null) {
                String id = generating == null ? "-" : generating.get().replaceAll(".* / ", "").replace("]", "");
                for (var e : now.entrySet()) {
                    BlockState[] before = traceSnapshot.get(e.getKey()), after = e.getValue();
                    int bx = (int) (e.getKey() >> 32) * 16, bz = (int) (long) e.getKey() * 16;
                    for (int i = 0; i < after.length; i++) if (before[i] != after[i]) {
                        System.err.println("TRACE " + id + " " + (bx + (i & 15)) + " " + (dimensionType.minY() + (i >> 8)) + " " + (bz + ((i >> 4) & 15)) + " " + stateString(after[i]));
                    }
                }
            }
            traceSnapshot = now;
            generating = next;
        }

        Region(Map<Long, ProtoChunk> chunks, ChunkAccess center, ChunkStep step) {
            this.chunks = chunks;
            this.center = center;
            this.step = step;
            this.cx = center.getPos().x();
            this.cz = center.getPos().z();
            this.writeRadius = step.blockStateWriteRadius();
            this.random = randomState.getOrCreateRandomFactory(WORLDGEN_REGION_RANDOM).at(center.getPos().getWorldPosition());
        }

        static WorldGenLevel create(Map<Long, ProtoChunk> chunks, ChunkAccess center, ChunkStep step) {
            Region h = new Region(chunks, center, step);
            h.self = (WorldGenLevel) Proxy.newProxyInstance(ChunkOracle.class.getClassLoader(), new Class<?>[] {WorldGenLevel.class}, h);
            h.biomeManager = new BiomeManager(h.self, BiomeManager.obfuscateSeed(seed));
            h.blockTicks = new WorldGenTickAccess<>(p -> h.chunk(SectionPos.blockToSectionCoord(p.getX()), SectionPos.blockToSectionCoord(p.getZ()), ChunkStatus.EMPTY).getBlockTicks());
            h.fluidTicks = new WorldGenTickAccess<>(p -> h.chunk(SectionPos.blockToSectionCoord(p.getX()), SectionPos.blockToSectionCoord(p.getZ()), ChunkStatus.EMPTY).getFluidTicks());
            return h.self;
        }

        ChunkAccess chunk(int x, int z, ChunkStatus target) {
            int distance = center.getPos().getChessboardDistance(x, z);
            ChunkStatus max = distance >= step.directDependencies().size() ? null : step.directDependencies().get(distance);
            if (max != null && target.isOrBefore(max)) {
                ProtoChunk c = chunks.get(key(x, z));
                // getChunkIfPresentUnchecked(max): the chunk must have reached `max`.
                if (c != null && c.getPersistedStatus().isOrAfter(max)) return c;
            }
            throw new IllegalStateException("Requested chunk unavailable during world generation: " + x + "," + z
                + " status " + target + " max " + max + " step " + step.targetStatus());
        }

        ChunkAccess chunkAt(BlockPos pos) {
            return chunk(SectionPos.blockToSectionCoord(pos.getX()), SectionPos.blockToSectionCoord(pos.getZ()), ChunkStatus.EMPTY);
        }

        boolean withinWriteZone(int x, int z) {
            return Math.abs(cx - x) <= writeRadius && Math.abs(cz - z) <= writeRadius;
        }

        boolean ensureCanWrite(BlockPos pos) {
            if (!withinWriteZone(SectionPos.blockToSectionCoord(pos.getX()), SectionPos.blockToSectionCoord(pos.getZ()))) {
                return false; // the game logs "Detected setBlock in a far chunk" and refuses
            }
            if (center.isUpgrading()) {
                return !center.getHeightAccessorForGeneration().isOutsideBuildHeight(pos.getY());
            }
            return true;
        }

        boolean setBlock(BlockPos pos, BlockState blockState, int updateFlags) {
            if (!ensureCanWrite(pos)) return false;
            ChunkAccess chunk = chunkAt(pos);
            BlockState oldState = chunk.setBlockState(pos, blockState, updateFlags);
            // updatePOIOnBlockStateChange: POI bookkeeping only, no block effect.
            if (blockState.hasBlockEntity()) {
                if (chunk.getPersistedStatus().getChunkType() == ChunkType.LEVELCHUNK) {
                    BlockEntity be = ((EntityBlock) blockState.getBlock()).newBlockEntity(pos, blockState);
                    if (be != null) chunk.setBlockEntity(be); else chunk.removeBlockEntity(pos);
                } else {
                    CompoundTag tag = new CompoundTag();
                    tag.putInt("x", pos.getX());
                    tag.putInt("y", pos.getY());
                    tag.putInt("z", pos.getZ());
                    tag.putString("id", "DUMMY");
                    chunk.setBlockEntityNbt(tag);
                }
            } else if (oldState != null && oldState.hasBlockEntity()) {
                chunk.removeBlockEntity(pos);
            }
            if ((updateFlags & 16) == 0) {
                BlockPos post = blockState.getPostProcessPos(self, pos);
                if (post != null) chunkAt(post).markPosForPostProcessing(post);
            }
            return true;
        }

        BlockEntity getBlockEntity(BlockPos pos) {
            ChunkAccess chunk = chunkAt(pos);
            BlockEntity be = chunk.getBlockEntity(pos);
            if (be != null) return be;
            CompoundTag tag = chunk.getBlockEntityNbt(pos);
            BlockState state = chunk.getBlockState(pos);
            if (tag != null) {
                if ("DUMMY".equals(tag.getStringOr("id", ""))) {
                    if (!state.hasBlockEntity()) return null;
                    be = ((EntityBlock) state.getBlock()).newBlockEntity(pos, state);
                } else {
                    be = BlockEntity.loadStatic(pos, state, tag, registries);
                }
                if (be != null) {
                    chunk.setBlockEntity(be);
                    return be;
                }
            }
            return null;
        }

        @Override
        @SuppressWarnings("unchecked")
        public Object invoke(Object proxy, Method m, Object[] a) throws Throwable {
            String n = m.getName();
            int argc = a == null ? 0 : a.length;
            switch (n) {
                case "getChunk":
                    if (argc == 2 && a[0] instanceof Integer) return chunk((Integer) a[0], (Integer) a[1], ChunkStatus.EMPTY);
                    if (argc == 4) return chunk((Integer) a[0], (Integer) a[1], (ChunkStatus) a[2]);
                    break;
                case "hasChunk": {
                    int d = center.getPos().getChessboardDistance((Integer) a[0], (Integer) a[1]);
                    return d < step.directDependencies().size();
                }
                case "getBlockState": return chunkAt((BlockPos) a[0]).getBlockState((BlockPos) a[0]);
                case "getFluidState": return chunkAt((BlockPos) a[0]).getFluidState((BlockPos) a[0]);
                case "isStateAtPosition": return ((Predicate<BlockState>) a[1]).test(chunkAt((BlockPos) a[0]).getBlockState((BlockPos) a[0]));
                case "isFluidAtPosition": return ((Predicate<FluidState>) a[1]).test(chunkAt((BlockPos) a[0]).getFluidState((BlockPos) a[0]));
                case "getBlockEntity": if (argc == 1) return getBlockEntity((BlockPos) a[0]); break;
                case "setBlock": if (argc == 4) return setBlock((BlockPos) a[0], (BlockState) a[1], (Integer) a[2]); break;
                case "destroyBlock":
                    if (argc == 4) {
                        BlockState s = chunkAt((BlockPos) a[0]).getBlockState((BlockPos) a[0]);
                        return s.isAir() ? false : setBlock((BlockPos) a[0], Blocks.AIR.defaultBlockState(), 3);
                    }
                    break;
                case "removeBlock": return setBlock((BlockPos) a[0], Blocks.AIR.defaultBlockState(), 3);
                case "ensureCanWrite": return ensureCanWrite((BlockPos) a[0]);
                case "isWithinWriteZone": {
                    BlockPos p = (BlockPos) a[0];
                    return withinWriteZone(SectionPos.blockToSectionCoord(p.getX()), SectionPos.blockToSectionCoord(p.getZ()));
                }
                case "getHeight":
                    if (argc == 0) return dimensionType.height();
                    if (argc == 3) {
                        int x = (Integer) a[1], z = (Integer) a[2];
                        return chunk(SectionPos.blockToSectionCoord(x), SectionPos.blockToSectionCoord(z), ChunkStatus.EMPTY)
                            .getHeight((Heightmap.Types) a[0], x & 15, z & 15) + 1;
                    }
                    break;
                case "getMinY": return dimensionType.minY();
                case "getSeaLevel": return generator.getSeaLevel();
                case "getSeed": return seed;
                case "getRandom": return random;
                case "getBiomeManager": return biomeManager;
                case "getUncachedNoiseBiome":
                    return generator.getBiomeSource().getNoiseBiome((Integer) a[0], (Integer) a[1], (Integer) a[2], randomState.sampler());
                case "registryAccess": return registries;
                case "enabledFeatures": return FeatureFlags.DEFAULT_FLAGS;
                case "dimensionType": return dimensionType;
                case "getBlockTicks": return blockTicks;
                case "getFluidTicks": return fluidTicks;
                case "nextSubTickCount": return subTicks.getAndIncrement();
                case "getCenter": return center.getPos();
                case "setCurrentlyGenerating":
                    if (TRACE && step.targetStatus() == ChunkStatus.FEATURES) traceFeature((java.util.function.Supplier<String>) a[0]);
                    return null;
                case "isOldChunkAround": return false;
                case "isClientSide": return false;
                case "getSkyDarken": return 0;
                case "getNearestPlayer": if (argc == 5) return null; break;
                case "players": return Collections.emptyList();
                case "getEntities": if (argc == 3) return Collections.emptyList(); break;
                case "addFreshEntity": {
                    var e = (net.minecraft.world.entity.Entity) a[0];
                    chunk(SectionPos.blockToSectionCoord(e.getBlockX()), SectionPos.blockToSectionCoord(e.getBlockZ()), ChunkStatus.EMPTY).addEntity(e);
                    return true;
                }
                case "playSound": case "addParticle": case "levelEvent": case "gameEvent": return null;
                case "environmentAttributes": return EnvironmentAttributeReader.EMPTY;
                // Read by scheduleTick (tick time only; ticks never change a dumped block).
                case "getLevelData": return LEVEL_DATA;
                // Mushrooms' canSurvive reads raw brightness. Features run before the LIGHT
                // steps, so in a fresh world no chunk has been handed to the light engine:
                // an engine with no chunks registered answers exactly that.
                case "getLightEngine": if (light == null) light = lightEngine(self); return light;
                case "toString": return "OracleRegion" + center.getPos();
                case "hashCode": return System.identityHashCode(proxy);
                case "equals": return proxy == a[0];
                default: break;
            }
            if (m.isDefault()) {
                try {
                    return InvocationHandler.invokeDefault(proxy, m, a);
                } catch (InvocationTargetException e) {
                    throw e.getCause();
                }
            }
            throw new UnsupportedOperationException("oracle region does not provide " + m);
        }
    }

    // ---------------------------------------------------------------- output

    static BlockState[] blocks(ChunkAccess c) {
        int minY = dimensionType.minY(), h = dimensionType.height();
        BlockState[] out = new BlockState[h * 256];
        BlockPos.MutableBlockPos p = new BlockPos.MutableBlockPos();
        int bx = c.getPos().getMinBlockX(), bz = c.getPos().getMinBlockZ();
        int i = 0;
        for (int y = minY; y < minY + h; y++) for (int z = 0; z < 16; z++) for (int x = 0; x < 16; x++) {
            out[i++] = c.getBlockState(p.set(bx + x, y, bz + z));
        }
        return out;
    }

    static String[] biomeCells(ChunkAccess c) {
        int minQy = QuartPos.fromBlock(dimensionType.minY()), hq = dimensionType.height() / 4;
        int qx0 = QuartPos.fromBlock(c.getPos().getMinBlockX()), qz0 = QuartPos.fromBlock(c.getPos().getMinBlockZ());
        String[] out = new String[hq * 16];
        int i = 0;
        for (int qy = 0; qy < hq; qy++) for (int qz = 0; qz < 4; qz++) for (int qx = 0; qx < 4; qx++) {
            Holder<Biome> b = c.getNoiseBiome(qx0 + qx, minQy + qy, qz0 + qz);
            out[i++] = b.unwrapKey().orElseThrow().identifier().toString();
        }
        return out;
    }

    static int[] heights(ChunkAccess c, Heightmap.Types t) {
        int[] out = new int[256];
        for (int z = 0; z < 16; z++) for (int x = 0; x < 16; x++) {
            out[z * 16 + x] = c.getHeight(t, x, z) + 1 - dimensionType.minY();
        }
        return out;
    }

    static String stateString(BlockState s) {
        StringBuilder b = new StringBuilder(BuiltInRegistries.BLOCK.getKey(s.getBlock()).toString());
        TreeMap<String, String> props = new TreeMap<>();
        for (Property<?> p : s.getProperties()) props.put(p.getName(), valueName(s, p));
        if (!props.isEmpty()) {
            b.append('[');
            boolean first = true;
            for (var e : props.entrySet()) {
                if (!first) b.append(',');
                first = false;
                b.append(e.getKey()).append('=').append(e.getValue());
            }
            b.append(']');
        }
        return b.toString();
    }

    static <T extends Comparable<T>> String valueName(BlockState s, Property<T> p) {
        return p.getName(s.getValue(p));
    }

    static void writeBlocks(PrintWriter w, String phase, long k, BlockState[] a, Map<BlockState, Integer> palette) {
        int x = (int) (k >> 32), z = (int) k;
        w.println("blocks " + phase + " " + x + " " + z);
        int[] ids = new int[a.length];
        for (int i = 0; i < a.length; i++) ids[i] = palette.get(a[i]);
        w.println(rle(ids));
    }

    static void writeHeights(PrintWriter w, String phase, String type, ChunkPos c, int[] h) {
        w.println("heightmap " + phase + " " + type + " " + c.x() + " " + c.z());
        StringBuilder b = new StringBuilder();
        for (int i = 0; i < h.length; i++) {
            if (i > 0) b.append(' ');
            b.append(h[i]);
        }
        w.println(b);
    }

    static String rle(int[] ids) {
        StringBuilder b = new StringBuilder();
        for (int i = 0; i < ids.length; ) {
            int j = i;
            while (j < ids.length && ids[j] == ids[i]) j++;
            if (b.length() > 0) b.append(' ');
            b.append(ids[i]);
            if (j - i > 1) b.append('*').append(j - i);
            i = j;
        }
        return b.toString();
    }
}
