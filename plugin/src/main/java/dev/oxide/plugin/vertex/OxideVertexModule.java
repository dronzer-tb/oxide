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
        // The same three stages the Bukkit half turns off through shouldGenerateSurface() and
        // shouldGenerateCaves(): oxide_generate_chunk returns terrain that is already surfaced
        // and carved, so vanilla running its own on top would apply both twice.
        context.registerChunkGeneration(new OxideChunkHook(this.service, logger),
                EnumSet.of(ChunkStage.SURFACE, ChunkStage.CARVERS));
    }
}
