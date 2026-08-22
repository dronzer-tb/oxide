package dev.oxide.plugin;

import dev.oxide.plugin.generator.GeneratorService;
import dev.oxide.plugin.provenance.ProvenanceLookup;
import dev.oxide.plugin.provenance.SidecarCache;
import org.bukkit.command.PluginCommand;
import org.bukkit.plugin.java.JavaPlugin;

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
        PluginCommand pluginCommand = getCommand("oxide");
        if (pluginCommand != null) {
            pluginCommand.setExecutor(command);
            pluginCommand.setTabCompleter(command);
        } else {
            getLogger().severe("`oxide` command not registered -- check paper-plugin.yml");
        }
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
