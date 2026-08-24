package dev.oxide.plugin.generator;

import org.bukkit.plugin.java.JavaPlugin;

import java.nio.file.Path;

/** {@link GeneratorHost} backed by the Bukkit plugin: config.yml, the data folder, the plugin logger. */
public final class BukkitGeneratorHost implements GeneratorHost {

    private final JavaPlugin plugin;

    public BukkitGeneratorHost(JavaPlugin plugin) {
        this.plugin = plugin;
    }

    @Override
    public String setting(String key, String fallback) {
        return plugin.getConfig().getString(key, fallback);
    }

    @Override
    public Path dataDirectory() {
        return plugin.getDataFolder().toPath();
    }

    @Override
    public void info(String message) {
        plugin.getLogger().info(message);
    }

    @Override
    public void warn(String message) {
        plugin.getLogger().warning(message);
    }
}
