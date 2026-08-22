package dev.oxide.plugin.generator;

import dev.oxide.plugin.ffi.OxideNative;
import org.bukkit.plugin.java.JavaPlugin;

import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;

/**
 * Owns the one {@link OxideNative} library load for this plugin's lifetime, and every
 * {@link OxideNative.Handle} opened from it (one per {@code /oxide createworld} call, since
 * each world may want its own seed). Handles outlive the command that opened them -- a world's
 * {@link OxideChunkGenerator} calls into its handle for the world's entire lifetime, not just
 * at creation -- so this class exists specifically to give them a place to be closed on plugin
 * disable rather than leaking native memory until the process exits.
 *
 * <p>Scope cut: handles are closed on plugin disable, not on world unload -- unloading a world
 * this plugin generated without disabling the plugin leaves its handle open and unused rather
 * than freed. A real fix needs a {@code WorldUnloadEvent} listener keyed off which handle backs
 * which world; not built here.
 *
 * <p>Config paths ({@code native-library-path}, {@code datapack-path}) are resolved relative to
 * the server's working directory (the JVM's, i.e. wherever {@code server.jar} was launched
 * from) -- the same convention the default {@code config.yml} values assume.
 */
public final class GeneratorService {

    private final JavaPlugin plugin;
    private final List<OxideNative.Handle> openHandles = new ArrayList<>();
    private OxideNative nativeLib;

    public GeneratorService(JavaPlugin plugin) {
        this.plugin = plugin;
    }

    /**
     * Opens a fresh generator handle for {@code seed}, using {@code config.yml}'s
     * {@code dimension-id}. Loads the native library on first call, not in the constructor, so
     * a server that never runs {@code /oxide createworld} never pays for it.
     *
     * @throws IllegalStateException if the native library or datapack fails to load/open --
     *         see {@link OxideNative#open} for what that wraps.
     */
    public synchronized OxideNative.Handle openHandle(long seed) {
        if (nativeLib == null) {
            Path libPath = Path.of(plugin.getConfig()
                    .getString("native-library-path", "plugins/Oxide/liboxide_ffi.so"))
                    .toAbsolutePath();
            nativeLib = new OxideNative(libPath);
        }
        String datapackPath = plugin.getConfig().getString("datapack-path", "plugins/Oxide/datapack");
        String dimensionId = plugin.getConfig().getString("dimension-id", "minecraft:overworld");
        OxideNative.Handle handle = nativeLib.open(
                Path.of(datapackPath).toAbsolutePath().toString(), dimensionId, seed);
        openHandles.add(handle);
        return handle;
    }

    /** Closes every handle opened through this service and unloads the native library. */
    public synchronized void closeAll() {
        for (OxideNative.Handle handle : openHandles) {
            try {
                handle.close();
            } catch (RuntimeException e) {
                plugin.getLogger().warning("failed to close an oxide-ffi handle: " + e.getMessage());
            }
        }
        openHandles.clear();
        if (nativeLib != null) {
            nativeLib.close();
            nativeLib = null;
        }
    }
}
