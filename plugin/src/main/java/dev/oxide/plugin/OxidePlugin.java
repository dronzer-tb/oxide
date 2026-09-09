package dev.oxide.plugin;

import com.mojang.brigadier.arguments.StringArgumentType;
import dev.oxide.plugin.generator.BukkitGeneratorHost;
import dev.oxide.plugin.generator.GeneratorService;
import dev.oxide.plugin.generator.OxideChunkGenerator;
import dev.oxide.plugin.pacside.PacsideCommand;
import dev.oxide.plugin.pacside.PacsideManager;
import dev.oxide.plugin.provenance.LiveProvenance;
import dev.oxide.plugin.provenance.ProvenanceLookup;
import dev.oxide.plugin.provenance.SidecarCache;
import io.papermc.paper.command.brigadier.Commands;
import io.papermc.paper.plugin.lifecycle.event.types.LifecycleEvents;
import org.bukkit.generator.ChunkGenerator;
import org.bukkit.plugin.java.JavaPlugin;
import org.jetbrains.annotations.NotNull;
import org.jetbrains.annotations.Nullable;

import java.util.List;

/**
 * Oxide chunk-provenance debug plugin, plus manual generator test hooks and
 * Pacside predictive chunk prefetcher and off-heap packet streamer.
 */
public final class OxidePlugin extends JavaPlugin {
    private DebugState debugState;
    private LiveProvenance liveProvenance;
    private GeneratorService generatorService;
    private PacsideManager pacsideManager;

    /**
     * Whether the Vertex Engine has already given terrain generation to a module -- this jar's
     * own module half, when it is installed in {@code vertex/modules/} as well as in
     * {@code plugins/}.
     */
    private static boolean vertexOwnsGeneration() {
        try {
            return dev.vertex.engine.api.Vertex.isChunkGenerationHooked();
        } catch (NoClassDefFoundError notVertex) {
            return false;
        }
    }

    @Override
    public void onEnable() {
        saveDefaultConfig();
        debugState = new DebugState();
        SidecarCache sidecarCache = new SidecarCache(getLogger());
        ProvenanceLookup provenanceLookup = new ProvenanceLookup(sidecarCache);
        liveProvenance = new LiveProvenance(this);
        generatorService = new GeneratorService(new BukkitGeneratorHost(this));
        pacsideManager = new PacsideManager(this);
        pacsideManager.enable();

        getServer().getPluginManager().registerEvents(
                new ChunkTracker(this, debugState, liveProvenance, pacsideManager), this);
        // Stamps the mark onto each chunk Oxide generated, as it loads.
        getServer().getPluginManager().registerEvents(liveProvenance, this);
        // Auto-equips joining players with Unbreakable Elytra & Infinite Rockets
        getServer().getPluginManager().registerEvents(
                new dev.oxide.plugin.flight.ElytraFlightListener(this), this);

        OxideCommand oxideCmd = new OxideCommand(
                this, debugState, provenanceLookup, liveProvenance, generatorService, pacsideManager);
        PacsideCommand pacsideCmd = new PacsideCommand(this, pacsideManager, debugState);

        this.getLifecycleManager().registerEventHandler(LifecycleEvents.COMMANDS, event -> {
            // Register /oxide
            event.registrar().register(
                    Commands.literal("oxide")
                            .executes(ctx -> dispatchOxide(oxideCmd, ctx.getSource().getSender(), new String[0]))
                            .then(Commands.argument("args", StringArgumentType.greedyString())
                                    .suggests((ctx, builder) -> {
                                        String remaining = builder.getRemaining();
                                        String[] typed = splitArgs(remaining);
                                        boolean atNewToken = remaining.isEmpty() || remaining.endsWith(" ");
                                        String[] forComplete = atNewToken ? append(typed, "") : typed;
                                        String prefix = forComplete[forComplete.length - 1];
                                        List<String> options = oxideCmd.onTabComplete(
                                                ctx.getSource().getSender(), null, "oxide", forComplete);
                                        var offsetBuilder = builder.createOffset(
                                                builder.getStart() + remaining.length() - prefix.length());
                                        if (options != null) {
                                            for (String option : options) {
                                                if (option.regionMatches(true, 0, prefix, 0, prefix.length())) {
                                                    offsetBuilder.suggest(option);
                                                }
                                            }
                                        }
                                        return offsetBuilder.buildFuture();
                                    })
                                    .executes(ctx -> dispatchOxide(oxideCmd, ctx.getSource().getSender(),
                                            splitArgs(StringArgumentType.getString(ctx, "args")))))
                            .build(),
                    "Oxide chunk-provenance debug tools and generator test commands."
            );

            // Register /pacside
            event.registrar().register(
                    Commands.literal("pacside")
                            .executes(ctx -> dispatchPacside(pacsideCmd, ctx.getSource().getSender(), new String[0]))
                            .then(Commands.argument("args", StringArgumentType.greedyString())
                                    .suggests((ctx, builder) -> {
                                        String remaining = builder.getRemaining();
                                        String[] typed = splitArgs(remaining);
                                        boolean atNewToken = remaining.isEmpty() || remaining.endsWith(" ");
                                        String[] forComplete = atNewToken ? append(typed, "") : typed;
                                        String prefix = forComplete[forComplete.length - 1];
                                        List<String> options = pacsideCmd.onTabComplete(
                                                ctx.getSource().getSender(), null, "pacside", forComplete);
                                        var offsetBuilder = builder.createOffset(
                                                builder.getStart() + remaining.length() - prefix.length());
                                        if (options != null) {
                                            for (String option : options) {
                                                if (option.regionMatches(true, 0, prefix, 0, prefix.length())) {
                                                    offsetBuilder.suggest(option);
                                                }
                                            }
                                        }
                                        return offsetBuilder.buildFuture();
                                    })
                                    .executes(ctx -> dispatchPacside(pacsideCmd, ctx.getSource().getSender(),
                                            splitArgs(StringArgumentType.getString(ctx, "args")))))
                            .build(),
                    "Pacside off-heap chunk cache and predictive trajectory prefetcher."
            );
        });
    }

    private static int dispatchOxide(OxideCommand command, org.bukkit.command.CommandSender sender, String[] args) {
        return command.onCommand(sender, null, "oxide", args)
                ? com.mojang.brigadier.Command.SINGLE_SUCCESS
                : 0;
    }

    private static int dispatchPacside(PacsideCommand command, org.bukkit.command.CommandSender sender, String[] args) {
        return command.onCommand(sender, null, "pacside", args)
                ? com.mojang.brigadier.Command.SINGLE_SUCCESS
                : 0;
    }

    private static String[] splitArgs(String raw) {
        String trimmed = raw.trim();
        return trimmed.isEmpty() ? new String[0] : trimmed.split("\\s+");
    }

    private static String[] append(String[] args, String extra) {
        String[] out = java.util.Arrays.copyOf(args, args.length + 1);
        out[args.length] = extra;
        return out;
    }

    @Override
    public @Nullable ChunkGenerator getDefaultWorldGenerator(
            @NotNull String worldName, @Nullable String id) {
        if (vertexOwnsGeneration()) {
            getLogger().info("Vertex Engine is active on this server; bukkit.yml generator: hook skipped -- "
                    + "terrain will generate through VertexChunkBridge (wave 6).");
            return null;
        }

        getLogger().info("registering OxideChunkGenerator for world '" + worldName
                + "' via bukkit.yml hook (seed will be populated per-world at startup)");
        return new OxideChunkGenerator(generatorService, liveProvenance, getLogger());
    }
}
