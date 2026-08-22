package dev.oxide.plugin;

import dev.oxide.plugin.generator.GeneratorService;
import dev.oxide.plugin.provenance.ProvenanceLookup;
import dev.oxide.plugin.provenance.SidecarCache;
import io.papermc.paper.command.brigadier.Commands;
import io.papermc.paper.plugin.lifecycle.event.types.LifecycleEvents;
import org.bukkit.plugin.java.JavaPlugin;

import java.util.Arrays;

/**
 * Oxide chunk-provenance debug plugin, plus (wave 5) a manual
 * {@code /oxide createworld} hook into the real Rust generator over
 * {@code oxide-ffi}. The provenance overlay is read-only/display-only
 * diagnostic tooling; {@code createworld} is the first thing in this plugin
 * that actually generates terrain. See plugin/README.md and
 * {@code OxideChunkGenerator}'s class doc for what that does and doesn't
 * produce yet.
 */
public final class OxidePlugin extends JavaPlugin {
    private DebugState debugState;
    private GeneratorService generatorService;

    @Override
    public void onEnable() {
        saveDefaultConfig();
        debugState = new DebugState();
        SidecarCache sidecarCache = new SidecarCache(getLogger());
        ProvenanceLookup provenanceLookup = new ProvenanceLookup(sidecarCache);
        generatorService = new GeneratorService(this);

        getServer().getPluginManager().registerEvents(
                new ChunkTracker(this, debugState, provenanceLookup), this);

        OxideCommand command = new OxideCommand(this, debugState, provenanceLookup, generatorService);

        // Paper plugins (declared via paper-plugin.yml) do not support the legacy
        // plugin.yml/getCommand() runtime lookup path -- JavaPlugin#getCommand throws
        // UnsupportedOperationException during startup on Paper/Folia paper-plugin.yml
        // plugins. Commands must be registered through the lifecycle COMMANDS event
        // instead. The `commands:` block in paper-plugin.yml is retained for descriptor
        // metadata/help text only -- it does not wire up execution on its own.
        this.getLifecycleManager().registerEventHandler(LifecycleEvents.COMMANDS, event ->
                event.registrar().register(
                        Commands.literal("oxide")
                                .executes(ctx -> {
                                    String[] rawArgs = ctx.getInput().split("\\s+");
                                    String[] args = rawArgs.length > 1
                                            ? Arrays.copyOfRange(rawArgs, 1, rawArgs.length)
                                            : new String[0];
                                    boolean handled = command.onCommand(
                                            ctx.getSource().getSender(), null, "oxide", args);
                                    return handled
                                            ? com.mojang.brigadier.Command.SINGLE_SUCCESS
                                            : 0;
                                })
                                .build(),
                        "Oxide chunk-provenance debug tools and generator test commands."
                ));
    }

    @Override
    public void onDisable() {
        // SidecarCache holds only in-memory parsed bitmaps (no open file handles kept between
        // lookups) and this plugin schedules no repeating tasks to cancel -- the one real
        // resource to release is any oxide-ffi generator handle still open from
        // /oxide createworld, plus the native library itself.
        if (generatorService != null) {
            generatorService.closeAll();
        }
    }
}
