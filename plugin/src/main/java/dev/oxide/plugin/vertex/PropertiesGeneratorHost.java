package dev.oxide.plugin.vertex;

import dev.oxide.plugin.generator.GeneratorHost;

import java.io.IOException;
import java.io.InputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Properties;

/**
 * {@link GeneratorHost} for the Vertex Engine module.
 *
 * <p>A plain {@code .properties} file rather than the plugin's {@code config.yml}: modules are
 * constructed before the plugin system exists, so Bukkit's YAML configuration is not available
 * yet, and the same keys read fine out of {@link Properties} with nothing added to the jar.
 * Missing file means every key falls back to its default, which is a working configuration.
 */
final class PropertiesGeneratorHost implements GeneratorHost {

    static final String FILE_NAME = "oxide.properties";

    private final Path dataDirectory;
    private final Properties properties = new Properties();
    private final System.Logger logger;

    PropertiesGeneratorHost(Path dataDirectory, System.Logger logger) {
        this.dataDirectory = dataDirectory;
        this.logger = logger;
        Path file = dataDirectory.resolve(FILE_NAME);
        if (Files.isRegularFile(file)) {
            try (InputStream in = Files.newInputStream(file)) {
                properties.load(in);
            } catch (IOException e) {
                logger.log(System.Logger.Level.WARNING,
                        "could not read " + file + " -- using defaults", e);
            }
        }
    }

    @Override
    public String setting(String key, String fallback) {
        return properties.getProperty(key, fallback);
    }

    @Override
    public Path dataDirectory() {
        return dataDirectory;
    }

    @Override
    public void info(String message) {
        logger.log(System.Logger.Level.INFO, message);
    }

    @Override
    public void warn(String message) {
        logger.log(System.Logger.Level.WARNING, message);
    }
}
