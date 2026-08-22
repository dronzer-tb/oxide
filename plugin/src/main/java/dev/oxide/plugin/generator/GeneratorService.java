package dev.oxide.plugin.generator;

import dev.oxide.plugin.ffi.OxideNative;
import org.bukkit.plugin.java.JavaPlugin;

import java.io.IOException;
import java.io.InputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
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
 *
 * <p>{@code native-library-path} is an override, not a requirement: if it points at no
 * existing file, the library embedded in this jar at build time is extracted to the plugin's
 * data folder and loaded from there. See {@link #resolveLibraryPath()}.
 */
public final class GeneratorService {

    private static final String LIBRARY_FILE_NAME = "liboxide_ffi.so";

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
            nativeLib = new OxideNative(resolveLibraryPath());
        }
        String datapackPath = plugin.getConfig().getString("datapack-path", "plugins/Oxide/datapack");
        String dimensionId = plugin.getConfig().getString("dimension-id", "minecraft:overworld");
        OxideNative.Handle handle = nativeLib.open(
                Path.of(datapackPath).toAbsolutePath().toString(), dimensionId, seed);
        openHandles.add(handle);
        return handle;
    }

    /**
     * An explicit {@code native-library-path} that actually exists wins -- that's the escape
     * hatch for running a locally built .so without rebuilding the jar. Otherwise the copy
     * embedded in this jar by the build (see build.gradle.kts) is extracted into the plugin's
     * data folder and loaded from there: Panama's {@code SymbolLookup.libraryLookup} needs a
     * real filesystem path, it cannot dlopen a jar entry.
     *
     * <p>Extraction overwrites any previous copy on every server start, so a jar upgrade never
     * silently keeps loading a stale library. Safe to overwrite here because nothing has
     * dlopen'd it yet this run -- this method is called exactly once, immediately before the
     * single {@code new OxideNative(...)}.
     */
    private Path resolveLibraryPath() {
        String configured = plugin.getConfig().getString("native-library-path", "");
        if (configured != null && !configured.isBlank()) {
            Path explicit = Path.of(configured).toAbsolutePath();
            if (Files.isRegularFile(explicit)) {
                plugin.getLogger().info("loading oxide-ffi from configured path: " + explicit);
                return explicit;
            }
        }
        return extractBundledLibrary();
    }

    /** Linux x86_64 only -- matches what the build embeds. See OxideNative's javadoc. */
    private Path extractBundledLibrary() {
        String resource = "natives/linux-x86_64/" + LIBRARY_FILE_NAME;
        Path target = plugin.getDataFolder().toPath().resolve(LIBRARY_FILE_NAME).toAbsolutePath();
        try (InputStream in = plugin.getClass().getClassLoader().getResourceAsStream(resource)) {
            if (in == null) {
                throw new IllegalStateException(
                        "no oxide-ffi library found: `native-library-path` in config.yml does not point at"
                                + " an existing file, and this jar has no bundled " + resource
                                + " (was it built without `cargo build --release -p oxide-ffi` first?)");
            }
            Files.createDirectories(target.getParent());
            Files.copy(in, target, StandardCopyOption.REPLACE_EXISTING);
            target.toFile().setReadable(true, false);
        } catch (IOException e) {
            throw new IllegalStateException("failed to extract bundled oxide-ffi library to " + target, e);
        }
        plugin.getLogger().info("extracted bundled oxide-ffi to: " + target);
        return target;
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
