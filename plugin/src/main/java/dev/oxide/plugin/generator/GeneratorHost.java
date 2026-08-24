package dev.oxide.plugin.generator;

import java.nio.file.Path;

/**
 * Everything {@link GeneratorService} needs from whatever is hosting it.
 *
 * <p>The service is loaded two ways: as a Bukkit plugin, where settings come from
 * {@code config.yml} and logging goes through the plugin logger, and as a Vertex Engine module,
 * which is constructed before the plugin system exists and so has neither. Only these four
 * operations differ between them, so they are the whole seam.
 */
public interface GeneratorHost {

    /** A configuration value, or {@code fallback} when unset. */
    String setting(String key, String fallback);

    /** Where this instance keeps its datapack, extracted native library and other state. */
    Path dataDirectory();

    void info(String message);

    void warn(String message);
}
