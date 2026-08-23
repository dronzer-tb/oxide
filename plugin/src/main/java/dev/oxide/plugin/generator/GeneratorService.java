package dev.oxide.plugin.generator;

import dev.oxide.plugin.ffi.OxideNative;
import org.bukkit.plugin.java.JavaPlugin;

import java.io.IOException;
import java.io.InputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.util.LinkedHashMap;
import java.util.Map;

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
    /**
     * Seed -> handle. Opening one parses the datapack and builds the noise router, which takes
     * seconds, and handles are read-only once open -- so two worlds on the same seed, or a
     * /oxide createworld whose world then generates through the same seed, share one.
     */
    private final Map<Long, OxideNative.Handle> openHandles = new LinkedHashMap<>();
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
        OxideNative.Handle existing = openHandles.get(seed);
        if (existing != null) {
            return existing;
        }
        if (nativeLib == null) {
            nativeLib = new OxideNative(resolveLibraryPath());
        }
        Path datapack = resolveDatapackPath();
        String dimensionId = plugin.getConfig().getString("dimension-id", "minecraft:overworld");
        OxideNative.Handle handle = nativeLib.open(datapack.toString(), dimensionId, seed);
        openHandles.put(seed, handle);
        return handle;
    }

    /**
     * Resolves {@code datapack-path} by trying it against the server's working directory
     * first, then against this plugin's own data folder, taking whichever is a real directory.
     * Both because both conventions are in the wild here: the shipped default used to read
     * {@code plugins/Oxide/datapack} (server-relative) while the plugin's data folder is named
     * after the plugin, {@code plugins/OxideDebug/} -- so the default named a directory that
     * never exists, and any config written against either reading has to keep working. An
     * absolute path is used as given; an empty value means {@code datapack} in the data folder.
     *
     * <p>Validated here rather than left to the Rust loader so the failure names the missing
     * piece: a pack root needs a {@code version.json} (or a {@code pack.mcmeta} carrying a
     * DataVersion) and a {@code data/&lt;namespace&gt;/} tree. See docs/REFERENCE_DATA.md.
     */
    private Path resolveDatapackPath() {
        String configured = plugin.getConfig().getString("datapack-path", "");
        Path dataFolder = plugin.getDataFolder().toPath().toAbsolutePath();
        if (configured == null || configured.isBlank()) {
            configured = "datapack";
        }

        Path serverRelative = Path.of(configured).toAbsolutePath().normalize();
        Path pluginRelative = dataFolder.resolve(configured).normalize();
        Path datapack = Files.isDirectory(serverRelative) ? serverRelative : pluginRelative;

        if (!Files.isDirectory(datapack)) {
            throw new IllegalStateException(
                    "datapack-path names no directory: tried " + serverRelative
                            + " and " + pluginRelative
                            + " -- fix `datapack-path` in " + dataFolder.resolve("config.yml")
                            + ", or put an extracted worldgen datapack at "
                            + dataFolder.resolve("datapack")
                            + ". See docs/REFERENCE_DATA.md for how to produce one.");
        }
        if (!Files.isDirectory(datapack.resolve("data"))) {
            throw new IllegalStateException(
                    "not a datapack root: " + datapack + " has no `data/` directory."
                            + " You want the folder that directly contains data/<namespace>/worldgen,"
                            + " not the server's datapacks folder and not an archive around it.");
        }
        if (!Files.isRegularFile(datapack.resolve("version.json"))
                && !Files.isRegularFile(datapack.resolve("pack.mcmeta"))) {
            throw new IllegalStateException(
                    "datapack at " + datapack + " has neither version.json nor pack.mcmeta;"
                            + " one of them must supply the DataVersion (this project never"
                            + " hardcodes it). version.json comes from the vanilla data"
                            + " generator -- see docs/REFERENCE_DATA.md.");
        }
        plugin.getLogger().info("using datapack: " + datapack);
        return datapack;
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
        for (OxideNative.Handle handle : openHandles.values()) {
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
