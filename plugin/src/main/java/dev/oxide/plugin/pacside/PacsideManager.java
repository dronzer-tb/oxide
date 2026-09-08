package dev.oxide.plugin.pacside;

import org.bukkit.plugin.java.JavaPlugin;

import java.util.logging.Logger;

/**
 * Manages the Pacside chunk packet cache and predictive streaming subsystem.
 */
public final class PacsideManager {

    private final JavaPlugin plugin;
    private final Logger logger;
    private final PacsidePrefetcher prefetcher;

    public PacsideManager(JavaPlugin plugin) {
        this.plugin = plugin;
        this.logger = plugin.getLogger();
        this.prefetcher = new PacsidePrefetcher(plugin);
    }

    public void enable() {
        plugin.getServer().getPluginManager().registerEvents(prefetcher, plugin);
        logger.info("[Pacside] Predictive trajectory chunk prefetcher and off-heap cache module enabled.");
    }

    public void disable() {
        PacsideNative.clear();
        logger.info("[Pacside] Off-heap chunk packet cache cleared.");
    }

    public PacsidePrefetcher getPrefetcher() {
        return prefetcher;
    }
}
