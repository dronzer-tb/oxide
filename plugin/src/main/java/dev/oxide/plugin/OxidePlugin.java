package dev.oxide.plugin;

import com.mojang.brigadier.arguments.StringArgumentType;
import dev.oxide.plugin.generator.GeneratorService;
import dev.oxide.plugin.provenance.ProvenanceLookup;
import dev.oxide.plugin.provenance.SidecarCache;
import io.papermc.paper.command.brigadier.Commands;
import io.papermc.paper.plugin.lifecycle.event.types.LifecycleEvents;
import org.bukkit.plugin.java.JavaPlugin;

import java.util.List;

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
        //
        // The literal alone would only ever match a bare `/oxide`: brigadier fails the
        // parse for any trailing input a node can't consume, so the subcommands need a
        // greedy string child. The whole tail is handed to OxideCommand as a legacy
        // String[] rather than modelled as brigadier nodes -- keeps one argument-parsing
        // implementation (OxideCommand) instead of two that can disagree.
        this.getLifecycleManager().registerEventHandler(LifecycleEvents.COMMANDS, event ->
                event.registrar().register(
                        Commands.literal("oxide")
                                .executes(ctx -> dispatch(command, ctx.getSource().getSender(), new String[0]))
                                .then(Commands.argument("args", StringArgumentType.greedyString())
                                        .suggests((ctx, builder) -> {
                                            String remaining = builder.getRemaining();
                                            String[] typed = splitArgs(remaining);
                                            // A trailing space means the player has finished the
                                            // previous token and wants completions for the next one.
                                            boolean atNewToken = remaining.isEmpty() || remaining.endsWith(" ");
                                            String[] forComplete = atNewToken
                                                    ? append(typed, "")
                                                    : typed;
                                            String prefix = forComplete[forComplete.length - 1];
                                            List<String> options = command.onTabComplete(
                                                    ctx.getSource().getSender(), null, "oxide", forComplete);
                                            // Offset the suggestion range to the start of the token
                                            // being typed, so brigadier replaces just that token.
                                            var offsetBuilder = builder.createOffset(
                                                    builder.getStart() + remaining.length() - prefix.length());
                                            for (String option : options) {
                                                if (option.regionMatches(true, 0, prefix, 0, prefix.length())) {
                                                    offsetBuilder.suggest(option);
                                                }
                                            }
                                            return offsetBuilder.buildFuture();
                                        })
                                        .executes(ctx -> dispatch(command, ctx.getSource().getSender(),
                                                splitArgs(StringArgumentType.getString(ctx, "args")))))
                                .build(),
                        "Oxide chunk-provenance debug tools and generator test commands."
                ));
    }

    private static int dispatch(OxideCommand command, org.bukkit.command.CommandSender sender, String[] args) {
        return command.onCommand(sender, null, "oxide", args)
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
