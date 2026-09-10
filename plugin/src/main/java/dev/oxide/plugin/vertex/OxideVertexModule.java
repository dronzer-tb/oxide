package dev.oxide.plugin.vertex;

import dev.oxide.plugin.generator.GeneratorService;
import dev.vertex.engine.api.ChunkStage;
import dev.vertex.engine.api.ModuleContext;
import dev.vertex.engine.api.VertexModule;

import java.util.EnumSet;

/**
 * Oxide as a Vertex Engine module: the same Rust generator the Bukkit plugin drives, wired in
 * one stage lower, where it writes blocks and biomes into the chunk directly.
 *
 * <p>Shipped in the same jar as the plugin. Which half runs is decided by where the jar is
 * installed -- {@code vertex/modules/} loads this through {@link java.util.ServiceLoader},
 * {@code plugins/} loads {@code OxidePlugin} -- and the plugin stands down when
 * {@code Vertex.isChunkGenerationHooked()} says terrain is already taken, so a jar dropped in
 * both places still generates once.
 */
public final class OxideVertexModule implements VertexModule {

    private GeneratorService service;

    @Override
    public String id() {
        return "oxide";
    }

    @Override
    public String version() {
        return "0.1.0";
    }

    @Override
    public void onEnable(ModuleContext context) {
        System.Logger logger = System.getLogger("oxide");
        // The native library is not loaded here: GeneratorService loads it on the first chunk,
        // so a server that never generates a chunk in an Oxide world never pays for it, and a
        // missing .so surfaces as chunks falling back rather than as a boot failure.
        this.service = new GeneratorService(new PropertiesGeneratorHost(context.dataDirectory(), logger));
        // SURFACE and CARVERS for the same reason the Bukkit half turns them off through
        // shouldGenerateSurface()/shouldGenerateCaves(): oxide_generate_chunk returns terrain
        // that is already surfaced and carved, so vanilla running its own on top applies both
        // twice.
        //
        // BIOMES because the Rust generator resolves the whole biome grid in that same pass and
        // the hook writes it through ChunkTarget.setBiomes. Without claiming it the server still
        // runs createBiomes and recomputes every climate sample -- 37% of generation time on a
        // heavy datapack, and unavoidable on the Bukkit path because CraftBukkit's
        // CustomWorldChunkManager samples the vanilla router before consulting any provider.
        context.registerChunkGeneration(new OxideChunkHook(this.service, logger),
                EnumSet.of(ChunkStage.BIOMES, ChunkStage.SURFACE, ChunkStage.CARVERS));
    }
}
